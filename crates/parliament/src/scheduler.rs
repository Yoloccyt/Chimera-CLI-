//! 工作窃取调度器 — 跨线程任务窃取提高 CPU 核心利用率
//!
//! 对应架构层:L8 Parliament
//!
//! # 设计原则
//! - 轻量级:基于 crossbeam-deque 实现，零依赖轻量级调度
//! - 回退:当 work-stealing 不可用时自动回退到 FuturesUnordered
//! - 线程安全:所有操作 Send + Sync

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam::deque::Injector;

/// 工作窃取调度器 — 分发任务到工作线程，支持窃取
///
/// 基于 crossbeam-deque 的全局注入器实现。任务通过 `submit` 推入
/// 全局队列，工作线程通过 `injector.steal()` 窃取任务。
/// 与 `collect_opinions_filtered` 的 `FuturesUnordered` 后备协同:
/// 当启用时，使用 `spawn_blocking` + 窃取分配任务；当禁用时，
/// 回退到既有 `FuturesUnordered` 路径。
///
/// # 线程安全
/// 内部 `Injector` 是 `Send + Sync`，`AtomicBool` 是 `Send + Sync`，
/// `usize` 是 `Send + Sync`。整个结构体无条件 `Send + Sync`。
pub struct WorkStealingScheduler {
    /// 全局任务队列(注入器)
    injector: Arc<Injector<Box<dyn FnOnce() + Send>>>,
    /// 工作线程数量
    worker_count: usize,
    /// 是否启用工作窃取
    enabled: AtomicBool,
}

impl WorkStealingScheduler {
    /// 创建新的工作窃取调度器
    ///
    /// # 参数
    /// - `worker_count`: 工作线程数量，默认 = 可用并行度
    pub fn new(worker_count: Option<usize>) -> Self {
        let count = worker_count.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
        Self {
            injector: Arc::new(Injector::new()),
            worker_count: count,
            enabled: AtomicBool::new(true),
        }
    }

    /// 提交任务到调度器
    pub fn submit<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.injector.push(Box::new(task));
    }

    /// 检查工作窃取是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 设置启用状态
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// 获取工作线程数量
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// 获取全局注入器引用(供工作线程窃取)
    pub fn injector(&self) -> &Arc<Injector<Box<dyn FnOnce() + Send>>> {
        &self.injector
    }
}

// 线程安全:内部所有字段均为 Send + Sync(无需 unsafe impl,`Arc<Injector<...>>`/`AtomicBool`/`usize` 均为自动 Send + Sync)
// 此结构体满足 Send + Sync 的自动推导条件。

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_scheduler_default_worker_count() {
        let scheduler = WorkStealingScheduler::new(None);
        assert!(scheduler.worker_count() > 0, "工作线程数应 > 0");
        assert!(scheduler.is_enabled(), "默认应启用");
    }

    #[test]
    fn test_scheduler_custom_worker_count() {
        let scheduler = WorkStealingScheduler::new(Some(2));
        assert_eq!(scheduler.worker_count(), 2);
    }

    #[test]
    fn test_scheduler_submit_and_execute() {
        let scheduler = WorkStealingScheduler::new(Some(1));
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        scheduler.submit(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        // 从 injector 窃取并执行任务
        match scheduler.injector.steal() {
            crossbeam::deque::Steal::Success(task) => {
                task();
            }
            _ => panic!("应有可窃取的任务"),
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1, "任务应被执行");
    }

    #[test]
    fn test_scheduler_enable_disable_toggle() {
        let scheduler = WorkStealingScheduler::new(None);
        assert!(scheduler.is_enabled());

        scheduler.set_enabled(false);
        assert!(!scheduler.is_enabled());

        scheduler.set_enabled(true);
        assert!(scheduler.is_enabled());
    }

    #[test]
    fn test_scheduler_multiple_tasks() {
        let scheduler = WorkStealingScheduler::new(Some(2));
        let counter = Arc::new(AtomicUsize::new(0));

        for i in 0..5 {
            let counter = Arc::clone(&counter);
            scheduler.submit(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = i; // 确保 i 被使用
            });
        }

        // 执行所有任务
        let mut executed = 0;
        loop {
            match scheduler.injector.steal() {
                crossbeam::deque::Steal::Success(task) => {
                    task();
                    executed += 1;
                }
                crossbeam::deque::Steal::Empty => break,
                crossbeam::deque::Steal::Retry => continue,
            }
        }

        assert_eq!(executed, 5, "应执行全部 5 个任务");
        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }
}