//! 按 `TaskKind` 归因的计算面计数器 — 有状态遥测真值源（ADR-197 / ADR-222 D3）
//!
//! 对应架构层:L0-B Core Foundation(机器层号 1)
//!
//! # 为什么单独成模块（ADR-222 D3）
//!
//! 纯判定（千分比、阈值、去抖）、`SaturationSource` 契约与上报循环都在
//! `nexus-telemetry`；本模块只留**必须依赖 `TaskKind` / `hts::kind_index`** 的
//! 有状态部分。二者分离后依赖方向才是单向的:
//! `nexus-core -> nexus-telemetry`；若计数器留在 `telemetry.rs` 且被 L0-B 契约
//! 引用，就会与 `TaskKind` 形成 crate 环。
//!
//! # 数值纪律
//!
//! 定长原子数组、`Ordering::Relaxed`、零锁零堆分配——热路径每次卸载只做 1 次
//! `fetch_add`。批粒度 `degradations` 与项粒度 `degraded_items` 是两个口径，
//! 不可互推（各自的 WHY 见字段注释）。

use std::sync::atomic::{AtomicU64, Ordering};

use super::dispatch::TaskKind;
use super::hts::kind_index;

/// 数组槽位数 —— 与 [`TaskKind::ALL`] 唯一对应
///
/// WHY 用 `ALL.len()` 而非硬编码:ADR-196 的 TaskKind 扩容（+四登记位）会改变变体数,
/// 此处自动跟随,不留需要同步的魔数。
const KIND_SLOTS: usize = TaskKind::ALL.len();

/// 单个 `TaskKind` 的累计计数快照（供导出与测试断言）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindStat {
    /// 任务类型
    pub kind: TaskKind,
    /// 卸载次数（含降级执行的那些）
    pub spawns: u64,
    /// 池内 panic 次数
    pub failures: u64,
}

/// 按 `TaskKind` 归因的计数器 —— 定长原子数组,零分配、零锁
///
/// WHY 不用 DashMap/Vec:热路径每次卸载只做 1 次 `fetch_add`,索引由
/// [`kind_index`] 在编译期确定的数组上取,避免任何堆分配与哈希开销。
#[derive(Debug)]
pub struct KindCounters {
    spawns: [AtomicU64; KIND_SLOTS],
    failures: [AtomicU64; KIND_SLOTS],
    /// 累计"因池饱和而降级为 Inline 执行"的卸载次数
    ///
    /// WHY 与 `saturations` 分开：默认 Observe 策略下“超阈”发生了但“降级”没发生，
    /// 合用一个计数器会让事件谎报行为变更次数。
    degradations: AtomicU64,
    /// 累计降级为 Inline 的**项数**(第三轮 T2.2):`degradations` 是批粒度
    /// (一批只 +1),本计数是项粒度(一批降级 n 项则 +n)。二者口径不同不可互推。
    /// WHY 独立计数而非逐项 `active.enter`:逐项 enter 会把降级本身计入并发
    /// 占用→高负载下触发饱和自屏蔽(降级越多越饱和)的行为变更(T2.2 明确拒绝);
    /// 纯遥测增量,不改变 `active` 语义。
    degraded_items: AtomicU64,
    /// 累计超阈观测次数（三种策略下均递增，上报去抖基线）
    saturations: AtomicU64,
    /// 累计"严格模式下直接返回 `Saturated`"的拒绝次数
    rejections: AtomicU64,
}

impl Default for KindCounters {
    fn default() -> Self {
        Self {
            spawns: std::array::from_fn(|_| AtomicU64::new(0)),
            failures: std::array::from_fn(|_| AtomicU64::new(0)),
            degradations: AtomicU64::new(0),
            degraded_items: AtomicU64::new(0),
            saturations: AtomicU64::new(0),
            rejections: AtomicU64::new(0),
        }
    }
}

impl KindCounters {
    /// 记录一次卸载提交（降级路径同样计入 —— 语义是"任务被受理"）
    pub fn record_spawn(&self, kind: TaskKind) {
        self.spawns[kind_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次池内 panic（`ComputeError::Panicked`）
    pub fn record_failure(&self, kind: TaskKind) {
        self.failures[kind_index(kind)].fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次因饱和触发的 Inline 降级
    pub fn record_degradation(&self) {
        self.degradations.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一批降级为 Inline 的**项数**(第三轮 T2.2 独立项粒度遥测)
    ///
    /// WHY 传 `n` 而非逐项调用:降级项数由调用方(`spawn_compute_batch`)一次传入,
    /// 与 `active.enter`(并发占用)解耦——避免降级被计入饱和判据导致自屏蔽行为变更。
    pub fn record_degraded_items(&self, n: u64) {
        self.degraded_items.fetch_add(n, Ordering::Relaxed);
    }

    /// 记录一次超阈观测（三种策略下均记，用于事件去抖基线）
    pub fn record_saturation(&self) {
        self.saturations.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计超阈观测次数
    #[must_use]
    pub fn saturations(&self) -> u64 {
        self.saturations.load(Ordering::Relaxed)
    }

    /// 记录一次严格模式下的 `Saturated` 拒绝
    pub fn record_rejection(&self) {
        self.rejections.fetch_add(1, Ordering::Relaxed);
    }

    /// 累计降级次数（单调递增,供上报器去抖判据）
    #[must_use]
    pub fn degradations(&self) -> u64 {
        self.degradations.load(Ordering::Relaxed)
    }

    /// 累计降级为 Inline 的项数(第三轮 T2.2;项粒度,≠ `degradations` 批粒度)
    #[must_use]
    pub fn degraded_items_total(&self) -> u64 {
        self.degraded_items.load(Ordering::Relaxed)
    }

    /// 累计严格拒绝次数
    #[must_use]
    pub fn rejections(&self) -> u64 {
        self.rejections.load(Ordering::Relaxed)
    }

    /// 导出各类型计数快照（按 [`TaskKind::ALL`] 顺序,确定性输出）
    #[must_use]
    pub fn snapshot(&self) -> Vec<KindStat> {
        TaskKind::ALL
            .iter()
            .enumerate()
            .map(|(idx, kind)| KindStat {
                kind: *kind,
                spawns: self.spawns[idx].load(Ordering::Relaxed),
                failures: self.failures[idx].load(Ordering::Relaxed),
            })
            .collect()
    }

    /// 卸载计数最高的类型名（用于事件归因字段）
    ///
    /// WHY 决胜规则确定化:计数相同时取 [`TaskKind::ALL`] 中靠前者 —— 与
    /// `variant_pool::best_by_perf` 同类"确定性决胜"纪律,避免归因抖动。
    #[must_use]
    pub fn hottest_kind(&self) -> String {
        let snap = self.snapshot();
        let mut best: Option<KindStat> = None;
        for stat in snap {
            match best {
                Some(cur) if stat.spawns > cur.spawns => best = Some(stat),
                None => best = Some(stat),
                _ => {}
            }
        }
        // 全零（尚无采样）时输出 "Unknown",不伪造归因
        match best {
            Some(stat) if stat.spawns > 0 => format!("{:?}", stat.kind),
            _ => "Unknown".to_string(),
        }
    }

    /// 某一类型的卸载次数（避开 `HashMap<TaskKind, _>` —— `TaskKind` 未 derive Hash）
    #[must_use]
    pub fn spawns_of(&self, kind: TaskKind) -> u64 {
        self.spawns[kind_index(kind)].load(Ordering::Relaxed)
    }

    /// 某一类型的池内 panic 次数
    #[must_use]
    pub fn failures_of(&self, kind: TaskKind) -> u64 {
        self.failures[kind_index(kind)].load(Ordering::Relaxed)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    #[test]
    fn counters_track_per_kind_and_hottest_is_deterministic() {
        let counters = KindCounters::default();
        counters.record_spawn(TaskKind::Generic);
        counters.record_spawn(TaskKind::Generic);
        counters.record_spawn(TaskKind::OsaMask);
        counters.record_failure(TaskKind::OsaMask);
        counters.record_degradation();
        counters.record_saturation();
        counters.record_rejection();

        let map_ok = (
            counters.spawns_of(TaskKind::Generic),
            counters.spawns_of(TaskKind::OsaMask),
            counters.spawns_of(TaskKind::ClvSimilarity),
        );
        assert_eq!(map_ok, (2, 1, 0));
        assert_eq!(counters.failures_of(TaskKind::OsaMask), 1);
        assert_eq!(counters.degradations(), 1);
        assert_eq!(counters.rejections(), 1);
        // Generic 计数更高 → 归因给它
        assert_eq!(counters.hottest_kind(), "Generic");

        let snap = counters.snapshot();
        assert_eq!(snap.len(), KIND_SLOTS);
        let osa = snap.iter().find(|s| s.kind == TaskKind::OsaMask).unwrap();
        assert_eq!(osa.failures, 1);
    }

    #[test]
    fn hottest_kind_unknown_when_no_samples() {
        let counters = KindCounters::default();
        assert_eq!(counters.hottest_kind(), "Unknown");
    }

    #[test]
    fn snapshot_covers_every_kind_exactly_once() {
        let counters = KindCounters::default();
        let snap = counters.snapshot();
        assert_eq!(snap.len(), KIND_SLOTS);
        for kind in TaskKind::ALL {
            assert_eq!(
                snap.iter().filter(|s| s.kind == kind).count(),
                1,
                "{kind:?} 必须且只能出现一次"
            );
        }
    }
}
