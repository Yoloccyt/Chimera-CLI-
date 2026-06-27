//! 批量原子性保证 — 任一失败触发回滚
//!
//! 对应架构红线:批量操作的一致性保证。
//!
//! # 机制
//! `gather_atomic` 顺序执行批量操作,任一失败时:
//! 1. 停止后续操作(未执行的操作不计入结果)
//! 2. 触发回滚回调(回滚已成功的操作)
//! 3. 回滚操作本身也经 GQEP 聚集(避免孤儿调用)
//!
//! # 对应尸检教训
//! Claude Code 中批量操作无原子性保证,部分失败导致状态不一致。
//! GQEP 通过顺序执行 + 回滚回调,保证批量操作的"全有或全无"语义。

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use event_bus::{EventMetadata, NexusEvent};
use tracing::warn;

use crate::error::GqepError;
use crate::gatherer::{map_gqep_to_qeep, map_qeep_to_gqep, GqepExecutor};
use crate::types::{GatherResult, GqepFuture};

/// 回滚回调类型
///
/// 返回一个异步 future,执行回滚逻辑。
///
/// WHY `Box<dyn Fn>`:回滚逻辑由调用者提供,GQEP 不关心具体实现,
/// 只需在失败时触发。`Send + Sync` 保证可跨线程调用。
///
/// WHY 返回 `Pin<Box<dyn Future<Output = ()> + Send>>`:
/// 回滚通常是异步操作(如数据库回滚、API 调用),
/// 返回 future 使回滚可经 GQEP 聚集(避免孤儿调用)。
pub type RollbackFn = Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

impl GqepExecutor {
    /// 批量原子性聚集执行
    ///
    /// 顺序执行批量操作,任一操作失败时:
    /// 1. 停止后续操作(未执行的操作不计入结果)
    /// 2. 触发回滚回调(回滚已成功的操作)
    /// 3. 回滚操作本身也经 GQEP 聚集(避免孤儿调用)
    ///
    /// # 顺序执行 vs 并发执行
    /// WHY 顺序执行:批量原子性要求"任一失败时,后续操作不执行"。
    /// 并发执行(`FuturesUnordered`)无法保证此语义(所有 future 同时启动)。
    /// 顺序执行虽牺牲并发性,但保证原子性语义清晰。
    ///
    /// # 参数
    /// - `futures`:批量操作列表(按顺序执行)
    /// - `rollback`:回滚回调,失败时调用(回滚已成功的操作)
    ///
    /// # 返回
    /// 聚集结果:
    /// - `total`:传入的 futures 总数
    /// - `succeeded`:成功执行的操作数(失败前的)
    /// - `failed`:失败操作数(1,触发回滚的那个)
    /// - `errors`:失败错误列表(含 `BatchAtomicFailure`)
    ///
    /// # 示例
    /// ```no_run
    /// # use gqep_executor::{GqepExecutor, GqepConfig, GqepFuture, GqepError, RollbackFn};
    /// # use event_bus::EventBus;
    /// # use std::sync::atomic::{AtomicBool, Ordering};
    /// # use std::sync::Arc;
    /// # async fn example() {
    /// let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    /// let rolled_back = Arc::new(AtomicBool::new(false));
    /// let rolled_back_clone = rolled_back.clone();
    /// let rollback: RollbackFn = Box::new(move || {
    ///     let flag = rolled_back_clone.clone();
    ///     Box::pin(async move { flag.store(true, Ordering::SeqCst); })
    /// });
    /// let futures: Vec<GqepFuture<String>> = vec![
    ///     Box::pin(async { Ok("op-1".to_string()) }),
    ///     Box::pin(async { Err(GqepError::OperationFailed { operation_id: "op-2".into(), reason: "fail".into() }) }),
    /// ];
    /// let result = executor.gather_atomic(futures, rollback).await;
    /// assert_eq!(result.succeeded, 1);
    /// assert!(rolled_back.load(Ordering::SeqCst));
    /// # }
    /// ```
    pub async fn gather_atomic(
        &self,
        futures: Vec<GqepFuture<String>>,
        rollback: RollbackFn,
    ) -> GatherResult {
        let total = futures.len() as u32;
        let start = Instant::now();

        let mut succeeded: u32 = 0;
        let mut failed: u32 = 0;
        let mut errors: Vec<GqepError> = Vec::new();

        // 顺序执行:任一失败则停止后续操作
        // WHY 顺序:批量原子性要求"失败后不执行后续操作",
        // 并发执行无法保证此语义
        for (idx, future) in futures.into_iter().enumerate() {
            let qeep = self.qeep.clone();
            // 经 QEEP entangle 包裹(孤儿检测 + 单操作超时)
            let entangled: GqepFuture<String> = Box::pin(async move {
                let mapped: Pin<
                    Box<dyn Future<Output = Result<String, qeep_protocol::QeepError>> + Send>,
                > = Box::pin(async move { future.await.map_err(map_gqep_to_qeep) });
                qeep.entangle(mapped).await.map_err(map_qeep_to_gqep)
            });

            match entangled.await {
                Ok(_value) => {
                    succeeded += 1;
                }
                Err(e) => {
                    // 失败:记录错误(包装为 BatchAtomicFailure),停止后续操作
                    failed += 1;
                    errors.push(GqepError::BatchAtomicFailure {
                        failed_index: idx,
                        reason: e.to_string(),
                    });
                    break;
                }
            }
        }

        // 任一失败触发回滚(若配置启用)
        if failed > 0 && self.config.batch_atomic_enabled {
            self.execute_rollback(rollback).await;
        }

        let latency_ms = start.elapsed().as_secs_f32() * 1000.0;

        // 检查孤儿调用并发布事件
        self.publish_orphan_events().await;

        let result = GatherResult {
            total,
            succeeded,
            failed,
            latency_ms,
            errors,
        };

        // 发布聚集完成事件
        let event = NexusEvent::GatherCompleted {
            metadata: EventMetadata::new("gqep-executor"),
            total: result.total,
            succeeded: result.succeeded,
            failed: result.failed,
            latency_ms: result.latency_ms,
        };
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "发布批量聚集完成事件失败");
        }

        result
    }

    /// 执行回滚操作
    ///
    /// 回滚操作本身也经 GQEP 聚集(避免孤儿调用)。
    /// 回滚失败仅记录 warn 日志,不阻塞主流程(回滚失败可能需要人工介入)。
    async fn execute_rollback(&self, rollback: RollbackFn) {
        // 将回滚回调包装为 GqepFuture,经 gather 聚集
        // WHY 经 gather:回滚本身也是异步操作,必须经 GQEP 聚集以避免孤儿调用
        let rollback_future: GqepFuture<String> = Box::pin(async move {
            rollback().await;
            Ok("rollback completed".to_string())
        });

        let rollback_result = self.gather(vec![rollback_future]).await;
        if rollback_result.failed > 0 {
            warn!(
                failed = rollback_result.failed,
                "回滚操作失败,可能存在状态不一致,需人工介入"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use event_bus::EventBus;

    use crate::GqepConfig;

    /// 创建计数器,记录操作执行情况
    fn make_counted_future(counter: Arc<AtomicU32>, should_fail: bool) -> GqepFuture<String> {
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            if should_fail {
                Err(GqepError::OperationFailed {
                    operation_id: String::new(),
                    reason: "intentional failure".to_string(),
                })
            } else {
                Ok("success".to_string())
            }
        })
    }

    #[tokio::test]
    async fn test_gather_atomic_all_success() {
        // 全部成功:不触发回滚
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let counter = Arc::new(AtomicU32::new(0));
        let rollback_called = Arc::new(AtomicU32::new(0));

        let futures: Vec<GqepFuture<String>> = (0..5)
            .map(|_| make_counted_future(counter.clone(), false))
            .collect();

        let rollback_called_clone = rollback_called.clone();
        let rollback: RollbackFn = Box::new(move || {
            let flag = rollback_called_clone.clone();
            Box::pin(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            })
        });

        let result = executor.gather_atomic(futures, rollback).await;

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 5);
        assert_eq!(result.failed, 0);
        assert_eq!(counter.load(Ordering::SeqCst), 5, "应执行全部 5 个操作");
        assert_eq!(
            rollback_called.load(Ordering::SeqCst),
            0,
            "全部成功不应触发回滚"
        );
    }

    #[tokio::test]
    async fn test_gather_atomic_rollback_on_5th_failure() {
        // SubTask 24.4 核心测试:10 操作中第 5 个失败,验证前 4 个回滚、后 5 个不执行
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let counter = Arc::new(AtomicU32::new(0));
        let rollback_called = Arc::new(AtomicU32::new(0));

        // 10 个操作,第 5 个(index=4)失败
        let futures: Vec<GqepFuture<String>> = (0..10)
            .map(|i| make_counted_future(counter.clone(), i == 4))
            .collect();

        let rollback_called_clone = rollback_called.clone();
        let rollback: RollbackFn = Box::new(move || {
            let flag = rollback_called_clone.clone();
            Box::pin(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            })
        });

        let result = executor.gather_atomic(futures, rollback).await;

        // 验证结果
        assert_eq!(result.total, 10, "total 应为传入的 10");
        assert_eq!(result.succeeded, 4, "前 4 个成功");
        assert_eq!(result.failed, 1, "第 5 个失败");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            5,
            "应只执行前 5 个(4 成功 + 1 失败),后 5 个不执行"
        );
        assert_eq!(rollback_called.load(Ordering::SeqCst), 1, "应触发回滚");
        // 验证错误为 BatchAtomicFailure
        assert_eq!(result.errors.len(), 1);
        assert!(
            matches!(
                &result.errors[0],
                GqepError::BatchAtomicFailure {
                    failed_index: 4,
                    ..
                }
            ),
            "错误应为 BatchAtomicFailure,failed_index=4"
        );
    }

    #[tokio::test]
    async fn test_gather_atomic_first_failure() {
        // 第一个操作失败:0 个成功,1 个失败,触发回滚
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let counter = Arc::new(AtomicU32::new(0));
        let rollback_called = Arc::new(AtomicU32::new(0));

        let futures: Vec<GqepFuture<String>> = (0..5)
            .map(|i| make_counted_future(counter.clone(), i == 0))
            .collect();

        let rollback_called_clone = rollback_called.clone();
        let rollback: RollbackFn = Box::new(move || {
            let flag = rollback_called_clone.clone();
            Box::pin(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            })
        });

        let result = executor.gather_atomic(futures, rollback).await;

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1, "只执行第一个(失败)");
        assert_eq!(rollback_called.load(Ordering::SeqCst), 1, "应触发回滚");
    }

    #[tokio::test]
    async fn test_gather_atomic_disabled_no_rollback() {
        // batch_atomic_enabled=false:失败不触发回滚
        let executor = GqepExecutor::new(
            GqepConfig {
                batch_atomic_enabled: false,
                ..Default::default()
            },
            EventBus::new(),
        );
        let counter = Arc::new(AtomicU32::new(0));
        let rollback_called = Arc::new(AtomicU32::new(0));

        let futures: Vec<GqepFuture<String>> = (0..5)
            .map(|i| make_counted_future(counter.clone(), i == 2))
            .collect();

        let rollback_called_clone = rollback_called.clone();
        let rollback: RollbackFn = Box::new(move || {
            let flag = rollback_called_clone.clone();
            Box::pin(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            })
        });

        let result = executor.gather_atomic(futures, rollback).await;

        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(
            rollback_called.load(Ordering::SeqCst),
            0,
            "batch_atomic_enabled=false 不应触发回滚"
        );
    }

    #[tokio::test]
    async fn test_gather_atomic_empty() {
        // 空列表:不执行任何操作,不触发回滚
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let rollback_called = Arc::new(AtomicU32::new(0));

        let rollback_called_clone = rollback_called.clone();
        let rollback: RollbackFn = Box::new(move || {
            let flag = rollback_called_clone.clone();
            Box::pin(async move {
                flag.fetch_add(1, Ordering::SeqCst);
            })
        });

        let result = executor.gather_atomic(vec![], rollback).await;

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(rollback_called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_gather_atomic_no_orphan() {
        // 验证批量原子执行不产生孤儿
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let counter = Arc::new(AtomicU32::new(0));

        let futures: Vec<GqepFuture<String>> = (0..5)
            .map(|i| make_counted_future(counter.clone(), i == 3))
            .collect();

        let rollback: RollbackFn = Box::new(|| Box::pin(async {}));

        let _ = executor.gather_atomic(futures, rollback).await;

        assert_eq!(executor.orphan_count(), 0, "批量原子执行不应产生孤儿");
    }
}
