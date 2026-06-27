//! GQEP 聚集器 — 并发异步操作的聚集汇聚核心
//!
//! 基于 `FuturesUnordered` 流式处理(对应 A.2 设计决策),
//! 集成 QEEP `OrphanDetector` 实现零孤儿调用保证。
//!
//! # 设计决策
//! - **FuturesUnordered vs join_all**:FuturesUnordered 支持流式处理
//!   (完成一个处理一个),内存占用更低,首个完成可立即处理。
//!   `join_all` 在 1000 个 Future 同时聚集时内存峰值高。
//! - **QEEP 集成**:每个 future 经 `QeepProtocol::entangle` 包裹,
//!   利用 `OrphanGuard` 在 future drop 时检测孤儿调用。
//!   WHY entangle:QEEP 内部维护 `pending_calls` 与 `OrphanDetector`,
//!   future 正常完成标记 `completed=true`,异常 drop 则报告孤儿。

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use futures::stream::{FuturesUnordered, StreamExt};
use qeep_protocol::{QeepError as QeepErr, QeepProtocol};
use tracing::warn;

use crate::config::GqepConfig;
use crate::error::GqepError;
use crate::types::{GatherResult, GqepFuture};

/// GQEP 执行器 — 并发异步操作的聚集汇聚核心
///
/// 基于 `FuturesUnordered` 流式处理,集成 QEEP `OrphanDetector`
/// 实现零孤儿调用保证。
///
/// # 线程安全
/// `GqepExecutor` 内部所有字段均为线程安全(`EventBus` 与 `QeepProtocol`
/// 均基于 `Arc` 实现 `Clone` 廉价)。但 `gather` 方法为 `&self`,
/// 保证多次聚集调用共享同一 `QeepProtocol` 实例(统一孤儿检测)。
pub struct GqepExecutor {
    /// 执行配置(超时、并发度、原子性开关)
    pub(crate) config: GqepConfig,
    /// 事件总线,用于发布聚集完成/超时/孤儿事件
    pub(crate) event_bus: EventBus,
    /// QEEP 协议实例,提供孤儿调用检测(entangle 包裹 + OrphanGuard)
    pub(crate) qeep: QeepProtocol,
}

impl GqepExecutor {
    /// 创建新的 GQEP 执行器
    ///
    /// # 参数
    /// - `config`:执行配置(超时、并发度、原子性)
    /// - `event_bus`:事件总线,用于发布 `GatherCompleted`/`OrphanCallDetected` 事件
    ///
    /// # 内部初始化
    /// 创建 `QeepProtocol` 实例,默认超时取自 `config.default_timeout_ms`。
    /// 该超时作用于 `entangle` 包裹的每个 future(单操作超时)。
    pub fn new(config: GqepConfig, event_bus: EventBus) -> Self {
        let default_timeout = std::time::Duration::from_millis(config.default_timeout_ms);
        let qeep = QeepProtocol::new(default_timeout);
        Self {
            config,
            event_bus,
            qeep,
        }
    }

    /// 聚集执行多个异步操作
    ///
    /// 使用 `FuturesUnordered` 流式处理,每个 future 经 QEEP `entangle` 包裹。
    /// 聚集完成后发布 `GatherCompleted` 事件,并检查孤儿调用报告。
    ///
    /// # 流程
    /// 1. 记录开始时间
    /// 2. 每个 future 经 `entangle` 包裹(实现孤儿检测 + 单操作超时)
    /// 3. `FuturesUnordered` 流式处理:完成一个处理一个
    /// 4. 检查 `orphan_reports`,发布 `OrphanCallDetected` 事件(Critical)
    /// 5. 发布 `GatherCompleted` 事件
    ///
    /// # 参数
    /// - `futures`:待聚集的异步操作列表
    ///
    /// # 返回
    /// 聚集结果统计(总数/成功数/失败数/延迟/错误列表)
    pub async fn gather(&self, futures: Vec<GqepFuture<String>>) -> GatherResult {
        let total = futures.len() as u32;
        let start = Instant::now();

        let mut succeeded: u32 = 0;
        let mut failed: u32 = 0;
        let mut errors: Vec<GqepError> = Vec::new();

        // 将每个 future 经 QEEP entangle 包裹后放入 FuturesUnordered
        // WHY entangle:利用 OrphanGuard 在 future drop 时检测孤儿调用,
        // 同时 entangle 内部用 tokio::time::timeout 提供单操作超时保护
        let mut stream: FuturesUnordered<GqepFuture<String>> = FuturesUnordered::new();
        for future in futures {
            let qeep = self.qeep.clone();
            // 将 GqepFuture<String> 经 entangle 包裹,转换为 GqepFuture<String>
            // 内部做 GqepError <-> QeepError 错误映射(entangle 要求 QeepError)
            let entangled: GqepFuture<String> = Box::pin(async move {
                // 将 GqepFuture 转换为 entangle 要求的 Future<Output=Result<String, QeepError>>
                let mapped: Pin<Box<dyn Future<Output = Result<String, QeepErr>> + Send>> =
                    Box::pin(async move { future.await.map_err(map_gqep_to_qeep) });
                // entangle 提供孤儿检测 + 单操作超时
                qeep.entangle(mapped).await.map_err(map_qeep_to_gqep)
            });
            stream.push(entangled);
        }

        // 流式处理:完成一个处理一个(对应 A.2 设计决策)
        while let Some(result) = stream.next().await {
            match result {
                Ok(_value) => succeeded += 1,
                Err(e) => {
                    failed += 1;
                    errors.push(e);
                }
            }
        }

        let latency_ms = start.elapsed().as_secs_f32() * 1000.0;

        // SubTask 24.5:检查孤儿调用并发布 Critical 事件
        // WHY 在 gather 中检查:孤儿可能由 future 内部 spawn 子任务且不 await 引起,
        // QEEP OrphanGuard 会在子任务 drop 时报告。gather 聚集后统一发布事件。
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
            warn!(error = %e, "发布聚集完成事件失败");
        }

        result
    }

    /// 发布所有已检测到的孤儿调用事件
    ///
    /// 从 QEEP `OrphanDetector` 获取孤儿报告,逐个发布 `OrphanCallDetected` 事件。
    /// 事件为 Critical 级别(对应 Claude Code 尸检 5.4% 孤儿调用教训),
    /// 发布失败仅记录 warn 日志,不阻塞聚集流程。
    pub(crate) async fn publish_orphan_events(&self) {
        let orphan_reports = self.qeep.orphan_reports();
        for report in &orphan_reports {
            let event = NexusEvent::OrphanCallDetected {
                metadata: EventMetadata::new("gqep-executor"),
                operation_id: report.call_id.0.to_string(),
                spawn_location: report.reason.clone(),
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "发布孤儿调用事件失败");
            }
        }
    }

    /// 获取当前待处理(pending)操作数量
    ///
    /// 反映当前正在执行(未完成)的纠缠调用数。
    pub fn pending_count(&self) -> usize {
        self.qeep.pending_count()
    }

    /// 获取已完成操作数量(成功 + 失败 + 超时,不含孤儿)
    pub fn completed_count(&self) -> usize {
        self.qeep.completed_count()
    }

    /// 获取孤儿调用数量
    ///
    /// 对应 Claude Code 尸检 5.4% 孤儿调用指标,用于监控告警。
    pub fn orphan_count(&self) -> usize {
        self.qeep.orphan_count()
    }

    /// 创建纠缠后台任务(委托 QEEP entangle_spawn)
    ///
    /// WHY 公开此方法:集成测试需通过 entangle_spawn + abort 验证孤儿检测,
    /// 内部字段 qeep 为 pub(crate),外部测试无法直接访问
    pub fn entangle_spawn<F, T>(&self, future: F) -> tokio::task::JoinHandle<Result<T, QeepErr>>
    where
        F: std::future::Future<Output = Result<T, QeepErr>> + Send + 'static,
        T: Send + 'static,
    {
        self.qeep.entangle_spawn(future)
    }

    /// 获取配置引用
    pub fn config(&self) -> &GqepConfig {
        &self.config
    }
}

/// GqepError → QeepError 映射
///
/// WHY:QEEP `entangle` 要求 future 输出 `Result<T, QeepError>`,
/// 需将 GQEP 错误转换为 QEEP 错误以接入纠缠协议。
/// 映射保留语义(超时→超时,孤儿→孤儿),其他错误映射到 `SerializationError`
/// (复用 QEEP 的通用错误变体,携带原始原因字符串)。
pub(crate) fn map_gqep_to_qeep(e: GqepError) -> QeepErr {
    match e {
        GqepError::OperationTimeout { .. } => QeepErr::Timeout,
        GqepError::OrphanCallDetected { .. } => QeepErr::Orphaned,
        GqepError::OperationFailed { reason, .. } => QeepErr::SerializationError(reason),
        GqepError::BatchAtomicFailure { reason, .. } => QeepErr::SerializationError(reason),
    }
}

/// QeepError → GqepError 映射
///
/// WHY:`entangle` 返回 `Result<T, QeepError>`,需转回 GQEP 错误以保持 API 一致性。
/// QEEP 的 `Timeout`/`Orphaned` 语义明确,直接映射;
/// 其他错误映射到 `OperationFailed`(携带 QEEP 错误描述)。
pub(crate) fn map_qeep_to_gqep(e: QeepErr) -> GqepError {
    match e {
        QeepErr::Timeout => GqepError::OperationTimeout {
            operation_id: String::new(),
            timeout_ms: 0,
        },
        QeepErr::Orphaned => GqepError::OrphanCallDetected {
            operation_id: String::new(),
            spawn_location: String::new(),
        },
        QeepErr::Cancelled => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "调用被取消".to_string(),
        },
        QeepErr::AlreadyCompleted => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "调用已完成,不能重复操作".to_string(),
        },
        QeepErr::AckMissing => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "缺少确认(Ack)".to_string(),
        },
        QeepErr::ReceiptMissing => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: "缺少回执(Receipt)".to_string(),
        },
        QeepErr::SerializationError(s) => GqepError::OperationFailed {
            operation_id: String::new(),
            reason: s,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 创建立即成功的 future
    fn make_success_future(value: &str) -> GqepFuture<String> {
        let value = value.to_string();
        Box::pin(async move { Ok(value) })
    }

    /// 创建立即失败的 future
    fn make_failure_future(reason: &str) -> GqepFuture<String> {
        let reason = reason.to_string();
        Box::pin(async move {
            Err(GqepError::OperationFailed {
                operation_id: String::new(),
                reason,
            })
        })
    }

    #[tokio::test]
    async fn test_gather_all_success() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_success_future("b"),
            make_success_future("c"),
        ];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert!(result.errors.is_empty());
        assert!(result.is_all_success());
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_partial_failure() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_failure_future("fail-1"),
            make_success_future("c"),
        ];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(!result.is_all_success());
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_all_failure() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![make_failure_future("fail-1"), make_failure_future("fail-2")];
        let result = executor.gather(futures).await;

        assert_eq!(result.total, 2);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 2);
        assert_eq!(result.errors.len(), 2);
        assert_eq!(executor.orphan_count(), 0);
    }

    #[tokio::test]
    async fn test_gather_empty() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let result = executor.gather(vec![]).await;

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn test_gather_publishes_completed_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let executor = GqepExecutor::new(GqepConfig::default(), bus.clone());

        let futures = vec![make_success_future("a"), make_success_future("b")];
        let _ = executor.gather(futures).await;

        // 应收到 GatherCompleted 事件
        let event = rx.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok(), "应收到事件");
        let event = event.unwrap();
        assert!(
            matches!(
                event,
                NexusEvent::GatherCompleted {
                    total: 2,
                    succeeded: 2,
                    failed: 0,
                    ..
                }
            ),
            "应为 GatherCompleted 事件,实际: {:?}",
            event
        );
    }

    #[tokio::test]
    async fn test_gather_with_slow_operations() {
        // 验证 FuturesUnordered 流式处理:慢操作不阻塞快操作的结果收集
        let executor = GqepExecutor::new(
            GqepConfig {
                default_timeout_ms: 5000,
                ..Default::default()
            },
            EventBus::new(),
        );

        let futures = vec![
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok("slow".to_string())
            }) as GqepFuture<String>,
            Box::pin(async { Ok("fast".to_string()) }) as GqepFuture<String>,
        ];

        let start = Instant::now();
        let result = executor.gather(futures).await;
        let elapsed = start.elapsed();

        assert_eq!(result.succeeded, 2);
        // 流式处理:快操作立即完成,慢操作 50ms,总耗时约 50ms(并发)
        assert!(
            elapsed < Duration::from_millis(200),
            "并发聚集应快于串行,实际耗时: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_orphan_detection_via_entangle_spawn_abort() {
        // SubTask 24.5:验证孤儿调用检测
        // 方法:用 entangle_spawn 创建后台任务,abort 后触发 OrphanGuard drop
        let bus = EventBus::new();
        let executor = GqepExecutor::new(GqepConfig::default(), bus);

        // 创建一个长时间运行的纠缠调用,然后 abort
        let handle = executor.qeep.entangle_spawn(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });

        // 等待任务启动(被 poll,OrphanGuard 已创建)
        tokio::time::sleep(Duration::from_millis(50)).await;

        // abort 任务,触发 future drop,OrphanGuard 检测到未完成
        handle.abort();
        // 等待 abort 生效与 Drop 执行
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 应检测到孤儿调用
        assert!(
            executor.orphan_count() > 0,
            "abort 后应检测到孤儿调用,实际: {}",
            executor.orphan_count()
        );
    }

    #[tokio::test]
    async fn test_orphan_event_published_on_gather() {
        // SubTask 24.5:验证孤儿调用事件发布
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let executor = GqepExecutor::new(GqepConfig::default(), bus.clone());

        // 创建孤儿:entangle_spawn + abort
        let handle = executor.qeep.entangle_spawn(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok("done".to_string())
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(executor.orphan_count() > 0, "应已检测到孤儿");

        // 调用 gather,触发孤儿事件发布
        let _ = executor
            .gather(vec![Box::pin(async { Ok("ok".to_string()) })])
            .await;

        // 接收事件:应有 OrphanCallDetected(Critical)和 GatherCompleted
        let mut found_orphan = false;
        for _ in 0..5 {
            match rx.recv_timeout(Duration::from_millis(100)).await {
                Ok(NexusEvent::OrphanCallDetected { .. }) => {
                    found_orphan = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(found_orphan, "应发布 OrphanCallDetected 事件(Critical)");
    }

    #[tokio::test]
    async fn test_no_orphan_on_normal_completion() {
        // 验证正常完成的操作不会产生孤儿
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        let futures = vec![
            make_success_future("a"),
            make_success_future("b"),
            make_failure_future("c"),
        ];
        let _ = executor.gather(futures).await;

        assert_eq!(
            executor.orphan_count(),
            0,
            "正常完成(含失败)的操作不应产生孤儿"
        );
    }

    #[test]
    fn test_error_mapping_roundtrip() {
        // 验证 GqepError <-> QeepError 映射
        let gqep_err = GqepError::OperationTimeout {
            operation_id: "op-1".into(),
            timeout_ms: 1000,
        };
        let qeep_err = map_gqep_to_qeep(gqep_err);
        assert!(matches!(qeep_err, QeepErr::Timeout));

        let gqep_back = map_qeep_to_gqep(qeep_err);
        assert!(matches!(gqep_back, GqepError::OperationTimeout { .. }));
    }

    #[tokio::test]
    async fn test_pending_and_completed_count() {
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(executor.completed_count(), 0);

        let futures = vec![make_success_future("a"), make_success_future("b")];
        let _ = executor.gather(futures).await;

        // 聚集完成后:pending=0, completed=2
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(executor.completed_count(), 2);
    }
}
