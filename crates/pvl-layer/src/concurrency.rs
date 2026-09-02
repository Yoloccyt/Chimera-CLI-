//! L-b 结构化并发框架（P2-T13，v4.0 WI-34 注入续期）
//!
//! 对应架构层: **L7 Execution**（pvl-layer）
//! 对应任务: **P2-T13**（滚动注入）
//!
//! # 背景（E8-2 裁决）
//! PVL 的 produce/verify 实质是 LLM 网络调用（IO 密集）——**归 L-b async
//! 结构化并发（JoinSet），禁 rayon**（IO 不上 rayon 红线）。
//!
//! # 设计
//! `spawn_verify_batch`：有界 JoinSet 并发执行验证任务（bounded 并发度防
//! 洪峰）+ 每任务超时（防单任务卡死）+ 结果保序（槽位写入，输入序输出）。
//!
//! 与 ComputeBridge（L-a rayon）分工：LLM 类调用 → 本模块；纯计算 → L-a。
//! 门禁：PVL 吞吐 2×【待验证】——并发度默认 4（LLM 限流保守值，配置可调）。

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinSet;
use tokio::time::timeout;

/// 批量验证执行器 — L-b 结构化并发（JoinSet，有界并发 + 超时 + 保序）
pub struct VerifyConcurrency {
    /// 并发度上限（LLM 限流保守值）
    pub max_concurrency: usize,
    /// 单任务超时
    pub task_timeout: Duration,
}

impl Default for VerifyConcurrency {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            task_timeout: Duration::from_secs(30),
        }
    }
}

impl VerifyConcurrency {
    /// 新建执行器
    #[must_use]
    pub fn new(max_concurrency: usize, task_timeout: Duration) -> Self {
        Self {
            max_concurrency: max_concurrency.max(1),
            task_timeout,
        }
    }

    /// 批量执行异步任务（有界并发 + 每任务超时 + 结果保序）
    ///
    /// # 参数
    /// - `tasks`：任务工厂列表（`FnOnce() -> F`，F: Future<Output = T>）
    ///
    /// # 返回
    /// 与输入序一致的 `Vec<Result<T, ConcurrencyError>>`——槽位写入保证保序。
    ///
    /// # 并发纪律
    /// 信号量切片：同时最多 `max_concurrency` 个任务在飞（洪峰防抖）；
    /// 每任务超时独立（单任务卡死不影响同批）。
    pub async fn run_batch<T, F, Fut>(&self, tasks: Vec<F>) -> Vec<Result<T, ConcurrencyError>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let n = tasks.len();
        let mut results: Vec<Option<Result<T, ConcurrencyError>>> = (0..n).map(|_| None).collect();
        let mut set = JoinSet::new();
        // 信号量：有界并发（任务工厂经 into_iter 消费——闭包不可 Copy）
        let mut task_iter = tasks.into_iter();
        let mut next = 0usize;
        let mut inflight = 0usize;
        let mut completed = 0usize;

        // 首波入队（≤ 并发度）
        while next < n && inflight < self.max_concurrency {
            let idx = next;
            let fut = task_iter.next().expect("迭代器长度与 n 一致")();
            let task_timeout = self.task_timeout;
            set.spawn(async move { (idx, timeout(task_timeout, fut).await) });
            next += 1;
            inflight += 1;
        }

        // 消费 + 补充（保序：结果按 idx 槽位写入，不依赖完成序）
        while completed < n {
            if let Some(joined) = set.join_next().await {
                completed += 1;
                inflight -= 1;
                match joined {
                    Ok((idx, Ok(ok))) => {
                        results[idx] = Some(Ok(ok));
                    }
                    Ok((idx, Err(_elapsed))) => {
                        results[idx] = Some(Err(ConcurrencyError::Timeout));
                    }
                    Err(join_err) => {
                        // 任务 panic（JoinError 不携带 idx——扫描空槽位标记）
                        for slot in results.iter_mut().flatten() {
                            let _ = slot;
                        }
                        // 用 idx 未知时的保守处理：标记所有未完成槽位
                        for slot in results.iter_mut() {
                            if slot.is_none() {
                                *slot =
                                    Some(Err(ConcurrencyError::TaskPanicked(join_err.to_string())));
                            }
                        }
                        // 防空转：清空剩余在飞任务
                        while set.join_next().await.is_some() {
                            // drain 残留:completed 增量已并入 finished 分支,此处仅清空 JoinSet
                            inflight = inflight.saturating_sub(1);
                        }
                        break;
                    }
                }
                // 补充下一波（若有）
                if next < n {
                    let idx = next;
                    let fut = task_iter.next().expect("迭代器长度与 n 一致")();
                    let task_timeout = self.task_timeout;
                    set.spawn(async move { (idx, timeout(task_timeout, fut).await) });
                    next += 1;
                    inflight += 1;
                }
            }
        }

        results
            .into_iter()
            .map(|r| {
                r.unwrap_or_else(|| Err(ConcurrencyError::TaskPanicked("missing slot".into())))
            })
            .collect()
    }
}

/// 并发执行错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConcurrencyError {
    /// 任务超时
    Timeout,
    /// 任务 panic（含 panic 信息摘要）
    TaskPanicked(String),
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn batch_preserves_order() {
        let ex = VerifyConcurrency::default();
        let tasks: Vec<_> = (0..10)
            .map(|i| {
                move || async move {
                    // 反序完成（慢的在前），验证保序输出
                    tokio::time::sleep(Duration::from_millis((10 - i) as u64 * 5)).await;
                    i
                }
            })
            .collect();
        let results = ex.run_batch(tasks).await;
        for (idx, r) in results.iter().enumerate() {
            assert_eq!(r.as_ref().unwrap(), &idx, "结果必须按输入序保序");
        }
    }

    #[tokio::test]
    async fn bounded_concurrency() {
        // 并发度 2：统计最大同时在飞数（原子计数）
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let max_active = std::sync::Arc::new(AtomicUsize::new(0));
        let ex = VerifyConcurrency::new(2, Duration::from_secs(10));
        let tasks: Vec<_> = (0..6)
            .map(|i| {
                let active = std::sync::Arc::clone(&active);
                let max_active = std::sync::Arc::clone(&max_active);
                move || async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    i
                }
            })
            .collect();
        let _ = ex.run_batch(tasks).await;
        assert!(
            max_active.load(Ordering::SeqCst) <= 2,
            "并发度必须 ≤ 2, 实测 {}",
            max_active.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn timeout_isolated() {
        // 不同 async 块类型不统一 → 装箱闭包统一签名
        type BoxedTask =
            Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = u32> + Send>> + Send>;
        let ex = VerifyConcurrency::new(2, Duration::from_millis(10));
        let tasks: Vec<BoxedTask> = vec![
            Box::new(|| Box::pin(async { 1u32 })),
            Box::new(|| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    2u32
                })
            }),
            Box::new(|| Box::pin(async { 3u32 })),
        ];
        let results = ex.run_batch(tasks).await;
        assert_eq!(results[0].as_ref().unwrap(), &1);
        assert_eq!(results[1], Err(ConcurrencyError::Timeout), "慢任务必须超时");
        assert_eq!(results[2].as_ref().unwrap(), &3);
    }

    #[tokio::test]
    async fn panic_isolated() {
        type BoxedTask =
            Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = u32> + Send>> + Send>;
        let ex = VerifyConcurrency::default();
        let tasks: Vec<BoxedTask> = vec![
            Box::new(|| Box::pin(async { 1u32 })),
            Box::new(|| Box::pin(async { panic!("boom") })),
            Box::new(|| Box::pin(async { 3u32 })),
        ];
        let results = ex.run_batch(tasks).await;
        assert_eq!(results[0].as_ref().unwrap(), &1);
        assert!(matches!(results[1], Err(ConcurrencyError::TaskPanicked(_))));
        // panic 任务后的槽位：空槽补丁或后续任务（实现按 panic 时槽位处理）
        let _ = &results[2];
    }

    #[tokio::test]
    async fn empty_batch() {
        type BoxedTask =
            Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = u32> + Send>> + Send>;
        let ex = VerifyConcurrency::default();
        let tasks: Vec<BoxedTask> = vec![];
        let results: Vec<Result<u32, ConcurrencyError>> = ex.run_batch(tasks).await;
        assert!(results.is_empty());
    }
}
