//! 批量入口方法簇 — 池派发、饱和三态分流与 Inline 降级执行。
//!
//! 从 `bridge` 单文件按方法簇拆出:本文件承载唯一带错误面的公开入口
//! [`ComputeBridge::spawn_compute_batch`](super::ComputeBridge::spawn_compute_batch)
//! 与其共用的 Inline 实现,方法体逐字平移。

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{ComputeBridge, ComputeError};
use crate::calibration::{self, CalibrationMode, ExecPath};
use crate::dispatch::TaskKind;
use crate::telemetry::SaturationMode;

impl ComputeBridge {
    /// L-a 批量入口 — CPU 卸载批量任务（任意数量闭包,分块并行）
    ///
    /// WHY `rayon::scope` 而非 `rayon::join`:`join` 只接收 **2 个**闭包（§8.3 修正 1/C13）,
    /// 批量必须用 scope 动态 spawn 任意数量 FnOnce 任务到本桥专用池。
    /// 结果按输入顺序对应（槽位写入索引与输入枚举一致）,结果数恒等于输入数。
    ///
    /// 单个任务 panic 不影响同批其他任务（逐个 catch_unwind 隔离）。
    ///
    /// `kind` 用于按类型归因遥测（ADR-197,[`KindCounters`]）。
    ///
    /// # 饱和行为（ADR-197 D2）
    /// 达阈值时按策略分流：`Observe`（默认）仍走池只记观测；`Degrade` 在当前线程逐个
    /// Inline 执行（结果序仍 = 输入序,逐元素等价）；`Strict` 整批不执行并逐项返回
    /// [`ComputeError::Saturated`]。
    #[must_use]
    pub fn spawn_compute_batch<F, T, I>(
        &self,
        kind: TaskKind,
        items: I,
    ) -> Vec<Result<T, ComputeError>>
    where
        I: IntoIterator<Item = F>,
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.counters.record_spawn(kind);
        let items: Vec<F> = items.into_iter().collect();
        let n = items.len();
        if n == 0 {
            return Vec::new();
        }

        // B1/ADR-198：校准探测（默认 off ⇒ 不进本块，零开销、零行为变更）。
        // 入参是 FnOnce 闭包——同一个闭包不能跑两遍，故成对样本只能靠
        // “探测式分流”获得：被采样的调用按奇偶决定本次走池还是走 Inline。
        // 两桶统一按 µs/项 记账，避免不同 n_items 的调用直接配对时把
        // “任务更大”误判为“池更慢”。
        let mut probe_start: Option<Instant> = None;
        let mut probe_inline = false;
        if calibration::mode() != CalibrationMode::Off {
            let c = self.cal_counter.fetch_add(1, Ordering::Relaxed);
            if calibration::should_sample(c) {
                probe_start = Some(Instant::now());
                probe_inline = calibration::explores_inline(c);
            }
        }

        // A1/ADR-197：超阈处置按策略分流；Observe 不 return（避免排队堆积把尾延
        // 拉成队头阻塞这件事交给 Degrade，需显式开启）
        if self.is_saturated() {
            self.counters.record_saturation();
            match self.saturation_mode() {
                SaturationMode::Strict => {
                    self.counters.record_rejection();
                    return (0..n).map(|_| Err(ComputeError::Saturated)).collect();
                }
                SaturationMode::Degrade => {
                    self.counters.record_degradation();
                    // T2.2 独立项粒度遥测:降级批的 n 项计入 degraded_items_total,
                    // 与 active.enter 解耦(不致饱和自屏蔽);批粒度 degradations 仍 +1。
                    self.counters.record_degraded_items(n as u64);
                    let out = self.run_inline_all(kind, items);
                    if let Some(t0) = probe_start {
                        calibration::observe(kind, ExecPath::Inline, n, t0.elapsed());
                    }
                    return out;
                }
                SaturationMode::Observe => {}
            }
        }

        // 探测位：本次改走 Inline（结果逐元素等价，仅延迟特征不同）
        if probe_inline {
            let out = self.run_inline_all(kind, items);
            if let Some(t0) = probe_start {
                calibration::observe(kind, ExecPath::Inline, n, t0.elapsed());
            }
            return out;
        }

        // 槽位共享:每任务写唯一索引槽(结果序 = 输入序);
        // WHY Mutex 而非原子/无锁:骨架阶段正确性优先,8 线程 × 槽位锁开销可忽略,
        // 后续可换 PerCpuPadded 计数器(ADR-125)优化
        let slots: Mutex<Vec<Option<Result<T, ComputeError>>>> =
            Mutex::new((0..n).map(|_| None).collect());

        // WI-34 探针:每任务闭包内守卫配对（执行即计,完成/panic 均归还）
        let active = Arc::clone(&self.active);
        // WHY 取实例计数器的借用而非闭包内 `bridge()`:失败归因必须落在**发生 panic
        // 的那个实例**上;走全局单例会把非单例实例（测试/多池场景）的失败错记到全局
        // 桥,统计失真（L1-H-1）。
        let counters = &self.counters;

        // self.pool.scope 把任务调度到本桥专用池(而非 rayon 全局池,纪律④ 桥接唯一)
        self.pool.scope(|s| {
            for (idx, f) in items.into_iter().enumerate() {
                let slots = &slots;
                let active = Arc::clone(&active);
                s.spawn(move |_| {
                    let _guard = active.enter();
                    let r =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| {
                            counters.record_failure(kind);
                            ComputeError::Panicked
                        });
                    // 任务内全部 catch_unwind,锁不 poison;unwrap_or_else 兜底防御
                    let mut guard = slots
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard[idx] = Some(r);
                });
            }
        });

        // 理论不可达分支:scope 同步阻塞至全部任务完成,槽位必然已填;
        // None 兜底用 Cancelled(取消语义)而非 panic,保持库代码零 panic 红线
        let results: Vec<Result<T, ComputeError>> = slots
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_iter()
            .map(|slot| slot.unwrap_or(Err(ComputeError::Cancelled)))
            .collect();
        if let Some(t0) = probe_start {
            calibration::observe(kind, ExecPath::Pool, n, t0.elapsed());
        }
        results
    }

    /// 在当前线程逐个执行整批闭包（Degrade 降级与 B1 探测共用同一实现）
    ///
    /// WHY 抽函数:两条路径必须**逐元素等价**，各写一份就有静默分叉风险
    /// （本仓曾因 publish/publish_blocking 双份实现踩过同类问题，现以宏收敛）。
    /// 活跃计数守卫同样适用:Inline 也是占一份计算力的执行。
    fn run_inline_all<F, T>(&self, kind: TaskKind, items: Vec<F>) -> Vec<Result<T, ComputeError>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let _guard = self.active.enter();
        items
            .into_iter()
            .map(|f| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| {
                    self.counters.record_failure(kind);
                    ComputeError::Panicked
                })
            })
            .collect()
    }
}
