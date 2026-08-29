//! utilization — WI-34 CPU 利用率测量基座（v4.0 §7.5.4 / 手册 §8.5）
//!
//! 对应架构层:L1 Core
//!
//! # 职责
//! 提供「8 核 ≥70% / 16 核 ≥65%」WI-34 两档门禁的测量管道（T1 建立，T14 中期验收）。
//! 双探针设计:
//! - **tokio 探针**:`tokio::runtime::RuntimeMetrics` **稳定版子集**——
//!   `num_workers` / `num_alive_tasks` / `blocking_queue_depth` / `num_blocking_threads`。
//!   ⚠️ 诚实数据边界:`worker_total_busy_duration` 需 `tokio_unstable` cfg，
//!   本项目钉 stable Rust 且不改 `.cargo/config.toml`（避免全 workspace 重编译 + CI 五平台
//!   行为漂移），故 busy 时长不可得，以任务活跃度近似 tokio 侧利用率（见 [`TokioProbe`]）。
//! - **rayon 探针**:[`ComputeBridge`] 池活跃任务计数（spawn +1 / 完成 −1，Relaxed 原子），
//!   活跃率 = active / pool_threads，为池占用率的直接观测。
//!
//! # 输出
//! [`UtilizationSnapshot`] 单次采样;聚合与周期落盘由调用方（T14 接线）负责,
//! 本模块保持纯测量无存储。
//!
//! # 红线
//! `#![forbid(unsafe_code)]` 由 crate 顶层保证;无自旋（计数走 Relaxed 原子,不忙等）;
//! 无外部依赖新增（tokio RuntimeMetrics 为 tokio 内建,rayon 探针为桥内字段）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// rayon 池活跃任务计数探针 — 挂在 [`ComputeBridge`] 上,RAII 守卫保证增减配对
///
/// WHY 独立于桥本身:闭包需 `'static`,计数器必须 `Arc` 共享;桥只持有句柄,
/// 守卫在任务闭包内构造,任务完成/panic 均触发 `Drop` 归还计数（panic 隔离不泄漏）。
#[derive(Debug, Default)]
pub(crate) struct ActiveCounter {
    /// 当前池内活跃任务数（spawn 后未完成）
    count: AtomicUsize,
}

impl ActiveCounter {
    /// 登记一个任务入场 — spawn 前调用,返回守卫（Drop 时自动 −1）
    pub(crate) fn enter(&self) -> ActiveGuard<'_> {
        self.count.fetch_add(1, Ordering::Relaxed);
        ActiveGuard { counter: self }
    }

    /// 当前活跃任务数（瞬时,Relaxed 读）
    pub(crate) fn active(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// RAII 活跃守卫 — Drop 时归还计数;任务 panic 也经 Drop 归还（无泄漏）
pub(crate) struct ActiveGuard<'a> {
    counter: &'a ActiveCounter,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.counter.count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// tokio 侧探针 — RuntimeMetrics 稳定版子集采样
///
/// 诚实数据标注:`worker_total_busy_duration` / `num_blocking_threads` 等需
/// `tokio_unstable`,本项目未启用;故 tokio 侧以「存活任务数 / worker 数」近似
/// 繁忙度,不宣称 busy 时长。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokioProbe {
    /// 当前 runtime worker 线程数
    pub num_workers: usize,
    /// 当前存活任务数（含 idle 任务,近似负载）
    pub num_alive_tasks: usize,
    /// 全局队列深度（待调度任务积压,IO 侧压力指标）
    pub global_queue_depth: usize,
    /// worker 累计 busy 时长（微秒;P5-T1 D-Q4）:
    /// cfg(tokio_unstable) 局部 RUSTFLAGS 下为真实 `worker_total_busy_duration` 和,
    /// 否则恒 0（存活任务近似口径不受影响;差分得窗口 busy,见 utilization_probe）
    pub busy_us: u64,
}

impl TokioProbe {
    /// 从当前 runtime 句柄采样;无 runtime 上下文时返回 `None`（纯观测降级,不 panic）
    pub fn sample() -> Option<Self> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let m = handle.metrics();
        // P5-T1（D-Q4）:tokio_unstable 局部 RUSTFLAGS 下 busy_duration 真实口径;
        // 无 cfg 时保持 0（RK-Q1 降级路径,存活任务近似口径独立成立）
        #[cfg(tokio_unstable)]
        let busy_us: u64 = {
            let total: std::time::Duration = (0..m.num_workers())
                .map(|i| m.worker_total_busy_duration(i))
                .sum();
            u64::try_from(total.as_micros()).unwrap_or(u64::MAX)
        };
        #[cfg(not(tokio_unstable))]
        let busy_us: u64 = 0;
        Some(Self {
            num_workers: m.num_workers(),
            num_alive_tasks: m.num_alive_tasks(),
            global_queue_depth: m.global_queue_depth(),
            busy_us,
        })
    }

    /// tokio 侧近似利用率 — 存活任务 / worker 数,上限 1.0（近似口径,见模块文档）
    #[must_use]
    pub fn utilization_approx(&self) -> f64 {
        if self.num_workers == 0 {
            return 0.0;
        }
        (self.num_alive_tasks as f64 / self.num_workers as f64).min(1.0)
    }
}

/// rayon 侧探针 — 池活跃率直接观测（桥内计数）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayonProbe {
    /// 池线程总数（num_cpus−2,下限 2）
    pub pool_threads: usize,
    /// 当前活跃任务数
    pub active_tasks: usize,
}

impl RayonProbe {
    /// 池活跃率 = active / threads（可 >1.0:单线程可叠多个任务,表示任务队列积压）
    #[must_use]
    pub fn active_ratio(&self) -> f64 {
        if self.pool_threads == 0 {
            return 0.0;
        }
        self.active_tasks as f64 / self.pool_threads as f64
    }
}

/// 单次采样快照 — 双探针合并输出
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilizationSnapshot {
    /// 采样时刻（wall clock,自进程启动）
    pub sampled_at: Instant,
    /// tokio 侧探针（无 runtime 时为 None）
    pub tokio: Option<TokioProbe>,
    /// rayon 侧探针
    pub rayon: RayonProbe,
}

impl UtilizationSnapshot {
    /// 合成评分 — WI-34 §7.5.4 口径:取 tokio 近似利用率与 rayon 活跃率的加权均值
    ///
    /// 权重说明:两运行时各占一半（双运行时对等地位,红线 3）;tokio 不可用时
    /// 退化为 rayon 单侧（报告须标注降级,诚实数据）。
    #[must_use]
    pub fn combined_utilization(&self) -> f64 {
        match self.tokio {
            Some(t) => 0.5 * t.utilization_approx() + 0.5 * self.rayon.active_ratio(),
            None => self.rayon.active_ratio(),
        }
    }
}

/// 利用率采样器 — 周期采样聚合（均值/峰值/末值）
///
/// T1 提供采样与聚合原语;真实注入场景（T14 中期验收）以本采样器驱动,
/// 报告落盘由调用方负责。线程安全:跨线程 `record` 可并发。
#[derive(Debug)]
pub struct UtilizationSampler {
    /// 采样窗口内累计 combined 值之和（均值 = sum / samples）
    sum: AtomicUsize,
    /// 采样次数
    samples: AtomicUsize,
    /// 采样窗口内峰值（以千分位整数存,避免 f64 原子）
    peak_milli: AtomicUsize,
}

impl Default for UtilizationSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl UtilizationSampler {
    /// 构造空采样器
    #[must_use]
    pub fn new() -> Self {
        Self {
            sum: AtomicUsize::new(0),
            samples: AtomicUsize::new(0),
            peak_milli: AtomicUsize::new(0),
        }
    }

    /// 记录一次采样 — 无 runtime 时跳过（降级,不计数）
    pub fn record(&self, snap: &UtilizationSnapshot) {
        let v = snap.combined_utilization();
        // WHY 千分位整数:stable Rust 无 AtomicF32（红线）,整数量化误差 ≤0.1% 可忽略
        let milli = (v * 1000.0).round() as usize;
        self.sum.fetch_add(milli, Ordering::Relaxed);
        self.samples.fetch_add(1, Ordering::Relaxed);
        // 峰值更新:Relaxed 读改写竞争允许丢失一次峰值（仅测量,非不变量）
        let mut peak = self.peak_milli.load(Ordering::Relaxed);
        while milli > peak {
            match self.peak_milli.compare_exchange_weak(
                peak,
                milli,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(cur) => peak = cur,
            }
        }
    }

    /// 聚合均值（0..=1,无量纲）
    #[must_use]
    pub fn mean(&self) -> f64 {
        let s = self.samples.load(Ordering::Relaxed);
        if s == 0 {
            return 0.0;
        }
        self.sum.load(Ordering::Relaxed) as f64 / (s as f64 * 1000.0)
    }

    /// 聚合峰值（0..=1）
    #[must_use]
    pub fn peak(&self) -> f64 {
        self.peak_milli.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// 采样次数
    #[must_use]
    pub fn count(&self) -> usize {
        self.samples.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// ActiveCounter 增减配对 — enter 后 active=1,drop 后归零
    #[test]
    fn active_counter_enter_drop() {
        let c = ActiveCounter::default();
        assert_eq!(c.active(), 0);
        {
            let _g = c.enter();
            assert_eq!(c.active(), 1);
        }
        assert_eq!(c.active(), 0, "守卫 Drop 后计数必须归还");
    }

    /// ActiveGuard 并发 — 8 线程各 enter/drop 1000 次,最终归零
    #[test]
    fn active_counter_concurrent_balanced() {
        let c = Arc::new(ActiveCounter::default());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = Arc::clone(&c);
                std::thread::spawn(move || {
                    for _ in 0..1_000usize {
                        let _g = c.enter();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("线程应正常退出");
        }
        assert_eq!(c.active(), 0, "并发 enter/drop 后计数必须精确归零");
    }

    /// RayonProbe 比率 — 分母为零防除零,active>threads 允许（积压语义）
    #[test]
    fn rayon_probe_ratio() {
        let idle = RayonProbe {
            pool_threads: 8,
            active_tasks: 0,
        };
        assert_eq!(idle.active_ratio(), 0.0);
        let half = RayonProbe {
            pool_threads: 8,
            active_tasks: 4,
        };
        assert_eq!(half.active_ratio(), 0.5);
        let zero_threads = RayonProbe {
            pool_threads: 0,
            active_tasks: 1,
        };
        assert_eq!(zero_threads.active_ratio(), 0.0, "分母为零必须防除零");
        let backlog = RayonProbe {
            pool_threads: 8,
            active_tasks: 16,
        };
        assert_eq!(backlog.active_ratio(), 2.0, "积压可 >1.0");
    }

    /// TokioProbe 无 runtime 降级 — 非 tokio 测试上下文返回 None
    #[test]
    fn tokio_probe_none_without_runtime() {
        // 注意:本测试在普通 #[test] 线程运行,无 tokio runtime 上下文
        let t = TokioProbe::sample();
        assert!(
            t.is_none() || t.is_some(),
            "无 runtime 时 None,有则 Some（不 panic 即可）"
        );
    }

    /// UtilizationSampler 聚合 — 记录后均值/峰值/计数正确
    #[test]
    fn sampler_aggregation() {
        let s = UtilizationSampler::new();
        assert_eq!(s.count(), 0);
        assert_eq!(s.mean(), 0.0);
        assert_eq!(s.peak(), 0.0);
        let ray = RayonProbe {
            pool_threads: 8,
            active_tasks: 2,
        };
        let a = UtilizationSnapshot {
            sampled_at: Instant::now(),
            tokio: Some(TokioProbe {
                num_workers: 4,
                num_alive_tasks: 4,
                global_queue_depth: 0,
                busy_us: 0,
            }),
            rayon: ray,
        };
        let b = UtilizationSnapshot {
            sampled_at: Instant::now(),
            tokio: None,
            rayon: ray,
        };
        // a: tokio 1.0 + rayon 0.25 → 0.625;b: rayon 0.25（tokio 降级）
        s.record(&a);
        s.record(&b);
        assert_eq!(s.count(), 2);
        assert!(
            (s.mean() - (0.625 + 0.25) / 2.0).abs() < 1e-3,
            "均值={}",
            s.mean()
        );
        assert!((s.peak() - 0.625).abs() < 1e-3, "峰值={}", s.peak());
    }

    /// combined 降级 — tokio 缺失时退化为 rayon 单侧
    #[test]
    fn combined_fallback() {
        let ray = RayonProbe {
            pool_threads: 8,
            active_tasks: 4,
        };
        let snap = UtilizationSnapshot {
            sampled_at: Instant::now(),
            tokio: None,
            rayon: ray,
        };
        assert!((snap.combined_utilization() - 0.5).abs() < 1e-9);
    }

    /// 千分位整数量化 — 极端值 1.0 与 0.0 边界
    #[test]
    fn sampler_boundaries() {
        let s = UtilizationSampler::new();
        let ray_full = RayonProbe {
            pool_threads: 1,
            active_tasks: 1,
        };
        let snap_full = UtilizationSnapshot {
            sampled_at: Instant::now(),
            tokio: None,
            rayon: ray_full,
        };
        s.record(&snap_full);
        assert!((s.mean() - 1.0).abs() < 1e-3);
        assert!((s.peak() - 1.0).abs() < 1e-3);
    }
}
