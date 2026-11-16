//! HTS-CPU 混合阈值调度 — 动态阈值表 + 序贯检验 + cgroup 核数校正（P1-T9，手册 W4）
//!
//! 对应架构层:L1 Core
//!
//! # 设计来源
//! - 阈值"三重来源法"（手册 §8.4 / ADR-103）:第一重 **W1 离线测定**（由 T1 观测基线
//!   提供,本波次以 dispatch.rs 静态初值入表并保留更新接口）;第二重 **运行期序贯检验**
//!   （本文件 `sequential_test`,Inline vs Rayon 双桶 A/B 对照）;第三重 **cgroup 核数校正**
//!   （本文件 `cgroup`,容器内 `available_parallelism` 不可信的修正,ADR-103）。
//! - §4.1 推演 15:调度阈值不可拍脑袋——阈值必须可溯源（诚实数据红线）。
//!
//! # 性能契约（ADR-128 / 手册 §8.4）
//! - [`HtsTable`] 由 `ArcSwap` 承载（workspace 已有 arc-swap = "1.7"）:读路径
//!   `load()` 拿无锁快照 Guard（~5ns,RCU 语义）,`get(kind)` 为固定数组索引,**零分配**;
//!   写路径（T1 测定灌入 / T14 序贯校准）低频,走 RCU store 全表替换。
//! - `route()` 决策门禁:P99 < 1µs（见 `benches/hts_bench.rs`）。
//!
//! # 红线
//! 本文件全 safe（`forbid(unsafe_code)` 由 crate 根保证,arc-swap 为纯 safe 封装,
//! unsafe 在依赖内不传播）;禁自旋（无等待逻辑）;库代码禁 unwrap/expect;
//! 错误经 [`sequential_test::HtsError`]（thiserror）或 `None` 传播。

use super::dispatch::TaskKind;

/// HTS 阈值表条目 — 手册 §8.4 单行记录
///
/// WHY 全 Copy:热路径 `route()` 每调用一次仅读取本结构的一个快照副本,
/// 无堆分配、无借用逃逸（`[Entry; TaskKind::COUNT]` 数组索引拷贝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 并行调度阈值（items）:条目数 >= 阈值时进 L-a rayon 池
    pub threshold: usize,
    /// 并行块大小（chunk）:分块粒度；当前无并行接线消费者（ADR-196 D2-ter /
    /// C3：[`calibration`](super::calibration) 写表时回读现值即唯一读点）
    pub chunk: usize,
    /// 阈值来源 — 诚实数据红线:阈值必须可溯源（ADR-103 三重来源）
    pub source: ThresholdSource,
}

/// 阈值来源标记 — ADR-103 三重来源 + 诚实数据红线
///
/// 语义约定:
/// - [`OfflineMeasured`](ThresholdSource::OfflineMeasured):离线测定值。
///   `measured_at = 0` 表示 **S9 离线测定预填初值,尚未 W1 复测**——预填不等于已校准
///   （手册 §8.4 标注"W1 复测"）;T1 测定灌入时携带真实时间戳。
/// - [`SequentialTest`](ThresholdSource::SequentialTest):序贯检验 Promote 后的校准值
///   （T14 运行时接线后产生）。
/// - [`ConservativeDefault`](ThresholdSource::ConservativeDefault):保守默认
///   （如 [`TaskKind::Generic`] 的 10,000,未测定前的兜底）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdSource {
    /// 离线测定（W1 复测前 `measured_at = 0` 为预填占位）
    OfflineMeasured {
        /// 测定时间戳（unix 秒）;0 = 预填占位（未复测,不作已校准结论）
        measured_at: u64,
    },
    /// 运行期序贯检验校准（T14 接线后产生）
    SequentialTest {
        /// Promote 时间戳（unix 秒）
        promoted_at: u64,
        /// 判定时已积累的样本数（检验证据量）
        samples: u32,
    },
    /// 保守默认 — 未测定前的兜底值（不宣称任何测定证据）
    ConservativeDefault,
}

/// HTS 动态阈值表 — 按 [`TaskKind`] 十类登记阈值与块大小（手册 §8.4 + ADR-196 C1 拆位）
///
/// # 存储
/// `[Entry; TaskKind::COUNT]` 定长数组（长度由 [`TaskKind::ALL`] 单源派生,非硬编码）:
/// - 读:`get(kind)` 经 `kind_index` 数组索引,**零分配、零锁**（在 ArcSwap 快照内）;
/// - 写:`update` 改快照副本,由持有方（`ComputeBridge`）RCU store 全表替换。
///
/// # 生命周期
/// `ComputeBridge` 以 `ArcSwap<HtsTable>` 持有本表;`update` 为运行期更新接口
/// （T1 测定值灌入 + T14 序贯检验校准用）。
///
/// # 初始值来源（E1 单源收敛）
/// 每类初始值唯一登记在 [`HtsTable::default_entry`];[`Default`] 经
/// `TaskKind::ALL.map(default_entry)` 派生（数量/顺序与枚举清单编译期绑定）。
/// 值:ClvSimilarity=1000/64、OsaMask=100/16、KnnSearch=5000/256、GsoeEvaluate=500/8、
/// CscCollapseScore=200/32、Generic=10000/64、HcwEntryScore=200/32、NmcEncode=1000/64、
/// ExpertScore=10000/64、DecayBatch=10000/64（手册 §8.4,标注"S9 离线测定,W1 复测"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtsTable {
    /// 全登记类表项,索引与 [`kind_index`] 一致（Clv=0, Osa=1, Knn=2, Gsoe=3, Csc=4, Gen=5,
    /// Hcw=6, Nmc=7, Expert=8, Decay=9）— 长度由 `TaskKind::COUNT` 单源绑定
    entries: [Entry; TaskKind::COUNT],
}

impl Default for HtsTable {
    /// 初始表 — E1 单源派生自 [`HtsTable::default_entry`]（与旧手写表逐项一致）:
    /// 条目序列 = `TaskKind::ALL.map(default_entry)`——数量与顺序由枚举清单锁定,
    /// `ALL` 长度变化而派生未同步时赋值即编译错（编译期断言）;
    /// 顺序恒等另由 `tests::kind_index_bijective` 运行期复核。
    ///
    /// 七类标注 [`OfflineMeasured`](ThresholdSource::OfflineMeasured)（measured_at=0 预填占位）,
    /// Generic/ExpertScore/DecayBatch 三类为 [`ConservativeDefault`](ThresholdSource::ConservativeDefault)
    /// （10,000,未测定兜底,来源标注见手册 §8.4）。
    fn default() -> Self {
        Self {
            entries: TaskKind::ALL.map(Self::default_entry),
        }
    }
}

impl HtsTable {
    /// 单类默认表项 — 全登记类初始阈值/chunk/来源的**唯一登记点**（E1 单源收敛）
    ///
    /// WHY 收敛到单点:此前 `Default` 手写十条目 + `dispatch::threshold()` 各自维护
    /// 同名值表（两处人工同步,靠测试跨锁）;现在值的唯一登记点在本函数——`Default` 经
    /// `TaskKind::ALL.map` 派生、`TaskKind::threshold()` 委托投影。新增登记类时只需改
    /// 本函数（match 穷举,漏臂即编译错）+ `TaskKind::ALL`。
    ///
    /// 来源标注（诚实数据红线,ADR-103）:七类为 S9 离线测定预填
    /// （`measured_at = 0`,未 W1 复测不作已校准结论）;Generic/ExpertScore/DecayBatch
    /// 三类为保守默认（10,000,未测定兜底）。
    #[must_use]
    pub(crate) const fn default_entry(kind: TaskKind) -> Entry {
        match kind {
            // ClvSimilarity:S9 离线测定初值,W1 复测（chunk=64 并行分块）
            TaskKind::ClvSimilarity => Entry {
                threshold: 1_000,
                chunk: 64,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // OsaMask:S9 离线测定初值,W1 复测（chunk=16）
            TaskKind::OsaMask => Entry {
                threshold: 100,
                chunk: 16,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // KnnSearch:S9 离线测定初值,W1 复测（chunk=256）
            TaskKind::KnnSearch => Entry {
                threshold: 5_000,
                chunk: 256,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // GsoeEvaluate:S9 离线测定初值,W1 复测（chunk=8,离线通道,R2 约束不动）
            TaskKind::GsoeEvaluate => Entry {
                threshold: 500,
                chunk: 8,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // CscCollapseScore:S9 离线测定初值,W1 复测（chunk=32）
            TaskKind::CscCollapseScore => Entry {
                threshold: 200,
                chunk: 32,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // Generic:保守默认兜底（未测定,不作已校准结论）
            TaskKind::Generic => Entry {
                threshold: 10_000,
                chunk: 64,
                source: ThresholdSource::ConservativeDefault,
            },
            // HcwEntryScore（ADR-196 C1 拆自 CscCollapseScore）:200/32 原值迁移,阈值零漂移
            TaskKind::HcwEntryScore => Entry {
                threshold: 200,
                chunk: 32,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // NmcEncode（拆自 ClvSimilarity）:1000/64 原值迁移
            TaskKind::NmcEncode => Entry {
                threshold: 1_000,
                chunk: 64,
                source: ThresholdSource::OfflineMeasured { measured_at: 0 },
            },
            // ExpertScore（从 Generic 拆出 faae）:10000/64 保守默认
            TaskKind::ExpertScore => Entry {
                threshold: 10_000,
                chunk: 64,
                source: ThresholdSource::ConservativeDefault,
            },
            // DecayBatch（从 Generic 拆出 decay）:10000/64 保守默认
            TaskKind::DecayBatch => Entry {
                threshold: 10_000,
                chunk: 64,
                source: ThresholdSource::ConservativeDefault,
            },
        }
    }

    /// 查询单条目 — 热路径核心读操作
    ///
    /// WHY 返回 `Entry` 副本而非引用:`Entry` 全 Copy（阈值/块大小/来源都是值语义）,
    /// 副本零分配且不产生借用逃逸,`ArcSwap` 快照 Guard 可在语句末尾立即释放,
    /// 配合 `route()` 每调用一次仅读取的语义（ADR-128 读无锁）。
    #[must_use]
    pub fn get(&self, kind: TaskKind) -> Entry {
        self.entries[kind_index(kind)]
    }

    /// 运行期更新 — T1 测定值灌入 + T14 序贯检验校准
    ///
    /// # 签名说明（相对任务原型扩展一个参数）
    /// 任务原型为 `update(kind, threshold, chunk)`;但诚实数据红线要求阈值必须可溯源
    /// （ADR-103）,来源不能由调用方隐式推断——故 `source` 为**强制显式参数**,
    /// 防止"静默标错来源"污染溯源链。T1 灌入用 [`OfflineMeasured`](ThresholdSource::OfflineMeasured),
    /// T14 校准用 [`SequentialTest`](ThresholdSource::SequentialTest)。
    pub fn update(
        &mut self,
        kind: TaskKind,
        threshold: usize,
        chunk: usize,
        source: ThresholdSource,
    ) {
        self.entries[kind_index(kind)] = Entry {
            threshold,
            chunk,
            source,
        };
    }
}

/// [`TaskKind`] → 数组索引 — 与 [`HtsTable`] 的 `entries` 布局唯一对应
///
/// WHY 不用 HashMap:十类登记可穷举 match,索引 O(1) 且零哈希开销;
/// match 穷举性在 [`TaskKind`] 新增变体时编译报错（新增登记位漏改由编译期兜底）。
#[must_use]
pub(crate) const fn kind_index(kind: TaskKind) -> usize {
    match kind {
        TaskKind::ClvSimilarity => 0,
        TaskKind::OsaMask => 1,
        TaskKind::KnnSearch => 2,
        TaskKind::GsoeEvaluate => 3,
        TaskKind::CscCollapseScore => 4,
        TaskKind::Generic => 5,
        TaskKind::HcwEntryScore => 6,
        TaskKind::NmcEncode => 7,
        TaskKind::ExpertScore => 8,
        TaskKind::DecayBatch => 9,
    }
}

/// 序贯检验模块（第二重阈值来源）— ADR-314 W7 / BC-04 拆分自原 `hts.rs` 的内联模块。
///
/// WHY 仍为 `pub mod`：公开路径 `nexus_compute::hts::sequential_test::*` 逐字不变。
pub mod sequential_test;

/// cgroup 核数校正模块（第三重阈值来源）— ADR-314 W7 / BC-04 拆分自原 `hts.rs` 的内联模块。
///
/// WHY 仍为 `pub mod`：公开路径 `nexus_compute::hts::cgroup::*` 逐字不变。
pub mod cgroup;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;

    use super::cgroup::{parse_cpu_max, CgroupProbe};
    use super::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision};
    use super::*;
    use crate::bridge::decide;
    use crate::dispatch::DispatchPlan;

    /// §8.4 初始值核对 — 阈值与 chunk 逐项锁定（来源:S9 离线测定,W1 复测）
    #[test]
    fn initial_values_match_manual_s8_4() {
        let table = HtsTable::default();
        let expect = [
            (TaskKind::ClvSimilarity, 1_000, 64),
            (TaskKind::OsaMask, 100, 16),
            (TaskKind::KnnSearch, 5_000, 256),
            (TaskKind::GsoeEvaluate, 500, 8),
            (TaskKind::CscCollapseScore, 200, 32),
            (TaskKind::Generic, 10_000, 64),
            (TaskKind::HcwEntryScore, 200, 32),
            (TaskKind::NmcEncode, 1_000, 64),
            (TaskKind::ExpertScore, 10_000, 64),
            (TaskKind::DecayBatch, 10_000, 64),
        ];
        for (kind, want_threshold, want_chunk) in expect {
            let e = table.get(kind);
            assert_eq!(e.threshold, want_threshold, "{kind:?} 阈值偏离手册 §8.4");
            assert_eq!(e.chunk, want_chunk, "{kind:?} chunk 偏离手册 §8.4");
        }
    }

    /// 动态表初值与 dispatch 静态参照一致 — T9/E1 迁移等价性
    ///
    /// E1 后 `Default` 由 `TaskKind::ALL.map(default_entry)` 派生、`threshold()` 委托
    /// `default_entry`——本测试从**双路径**验证同一值：
    /// ① `get(kind)`（经 kind_index 槽位）= `default_entry(kind)`（直取）——槽位错位在此被抓
    /// （对值互异的 kind 对；值相同的 kind 对由 `kind_index_bijective` 恒等断言兜捕）;
    /// ② `get(kind).threshold` = `threshold()`——静态参照面（route 测试边界值来源）不漂。
    #[test]
    fn default_table_matches_dispatch_static() {
        let table = HtsTable::default();
        for kind in TaskKind::ALL {
            assert_eq!(
                table.get(kind),
                HtsTable::default_entry(kind),
                "{kind:?} 表槽位值与 default_entry 不一致（槽位/派生错位）"
            );
            assert_eq!(
                table.get(kind).threshold,
                kind.threshold(),
                "{kind:?} 动态表初值须与 dispatch 静态参照一致（迁移等价性）"
            );
        }
    }

    /// 初始来源标记 — 七类 OfflineMeasured 预填 + 三类保守默认(Generic/ExpertScore/DecayBatch)（诚实数据红线）
    ///
    /// E1 收敛后扩展为**全十类**逐项校验（旧版仅查 5+1 类,Hcw/Nmc/Expert/Decay 的
    /// 来源标注未被覆盖）——每类的 source 必须与 default_entry 单点登记一致。
    #[test]
    fn initial_sources_are_honest() {
        let table = HtsTable::default();
        for kind in [
            TaskKind::ClvSimilarity,
            TaskKind::OsaMask,
            TaskKind::KnnSearch,
            TaskKind::GsoeEvaluate,
            TaskKind::CscCollapseScore,
            TaskKind::HcwEntryScore,
            TaskKind::NmcEncode,
        ] {
            assert_eq!(
                table.get(kind).source,
                ThresholdSource::OfflineMeasured { measured_at: 0 },
                "{kind:?} 应为 S9 预填占位（measured_at=0,未复测不作已校准结论）"
            );
        }
        for kind in [
            TaskKind::Generic,
            TaskKind::ExpertScore,
            TaskKind::DecayBatch,
        ] {
            assert_eq!(
                table.get(kind).source,
                ThresholdSource::ConservativeDefault,
                "{kind:?} 应为保守默认（未测定兜底,不得标为已测定）"
            );
        }
    }

    /// update 后 get 反映新值 — 运行期更新接口基础语义
    #[test]
    fn update_changes_entry() {
        let mut table = HtsTable::default();
        table.update(
            TaskKind::OsaMask,
            42,
            7,
            ThresholdSource::SequentialTest {
                promoted_at: 1_700_000_000,
                samples: 30,
            },
        );
        let e = table.get(TaskKind::OsaMask);
        assert_eq!(e.threshold, 42);
        assert_eq!(e.chunk, 7);
        assert_eq!(
            e.source,
            ThresholdSource::SequentialTest {
                promoted_at: 1_700_000_000,
                samples: 30,
            }
        );
        // 未更新的类别不受影响
        assert_eq!(table.get(TaskKind::ClvSimilarity).threshold, 1_000);
    }

    /// update 后 route 行为变化 — 阈值 1000→10 后 20 项从 Inline 转 Rayon
    ///
    /// WHY 经 `decide` 纯函数而非全局 `bridge()`:全局单例表会被并行测试共享,
    /// 修改会与其他测试（如 T8 `route_concurrent_reads` 的静态阈值断言）竞争;
    /// 真实 `ComputeBridge::route` 联动在 bridge.rs 测试中用独立实例锁定。
    #[test]
    fn update_changes_route_behavior() {
        let mut table = HtsTable::default();
        let kind = TaskKind::ClvSimilarity;
        // 初值阈值 1000:20 项 < 1000 → Inline
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 20),
            DispatchPlan::Inline
        );
        // 运行期校准阈值降至 10:20 项 >= 10 → Rayon
        table.update(kind, 10, 64, ThresholdSource::ConservativeDefault);
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 20),
            DispatchPlan::Rayon
        );
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 9),
            DispatchPlan::Inline
        );
    }

    /// arc-swap 并发读写 — 独立 ArcSwap<HtsTable>,8 写 × 8 读,无 panic 无数据竞争
    ///
    /// WHY 而非 loom（同 T8 理由）：原注称「Windows GNU 工具链 loom 受限」，
    /// **已被 R2 C-102 实测推翻**（loom 0.7.2 在本工具链编过且模型检验有牙）。
    /// 不用它的真实理由是：arc-swap 的 RCU 语义靠类型系统 + ArcSwap 自身保证，
    /// loom 不建模 arc-swap 的发布/读取协议，只能验证更底层的原子组合；
    /// 本测试直接锁该协议的行为结果——读线程任意时刻看到完整快照（要么旧表要么新表，永不撕裂）。
    #[test]
    fn arc_swap_concurrent_read_write() {
        // Arc<ArcSwap<..>> 包装:线程间共享同一 RCU 实例（ArcSwapAny 自身无 Clone）
        let table = Arc::new(ArcSwap::from_pointee(HtsTable::default()));
        let writers: Vec<_> = (0..8)
            .map(|w| {
                let table = Arc::clone(&table);
                std::thread::spawn(move || {
                    for i in 0..1_000usize {
                        let kind = TaskKind::ALL[w % TaskKind::ALL.len()];
                        let t = (i % 20_000) + 1;
                        let src = ThresholdSource::ConservativeDefault;
                        let cur = table.load_full();
                        let mut next = (*cur).clone();
                        next.update(kind, t, 64, src);
                        table.store(Arc::new(next));
                    }
                })
            })
            .collect();
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let table = Arc::clone(&table);
                std::thread::spawn(move || {
                    for _ in 0..10_000usize {
                        let guard = table.load();
                        for kind in TaskKind::ALL {
                            let e = guard.get(kind);
                            // 快照一致性:阈值恒 >= 1（写线程从未写 0）
                            assert!(e.threshold >= 1, "读线程观测到非法快照");
                        }
                    }
                })
            })
            .collect();
        for h in writers.into_iter().chain(readers) {
            h.join().expect("并发读写线程应正常退出");
        }
    }

    /// 序贯检验:小样本强制 Continue（即使数据效果极显著,n < min_samples 不判定）
    #[test]
    fn sequential_small_sample_continues() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        for i in 0..29 {
            // 强效果数据（rayon 快 90%）
            let d = t.record(100.0, 10.0 + (i % 3) as f64);
            assert_eq!(
                d,
                TestDecision::Continue,
                "第 {} 次记录（n<30）必须 Continue",
                i + 1
            );
        }
        assert_eq!(t.sample_count(), 29);
        assert_eq!(t.spent_alpha(), 0.0, "小样本阶段不消耗 alpha 预算");
    }

    /// 序贯检验:效果量达标 + SPRT 跨上界 → Promote（回传候选阈值）
    #[test]
    fn sequential_promote_when_effect_reached() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        t.set_candidate_threshold(256);
        let mut decisions = Vec::new();
        for i in 0..35 {
            // 强效果:rayon 快 90%,带小抖动（方差非零）
            decisions.push(t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64));
        }
        assert_eq!(
            decisions[29],
            TestDecision::Promote { threshold: 256 },
            "n=30 首个判定点应 Promote"
        );
        // 终局后实例关闭:后续记录强制 Reject（预算耗尽,永不二次 Promote）
        assert_eq!(
            t.record(100.0, 10.0),
            TestDecision::Reject,
            "终局后 record 一律 Reject"
        );
        assert_eq!(t.sample_count(), 30, "终局后不再积累样本");
        assert_eq!(t.spent_alpha(), 0.05, "终局判定耗尽 alpha 预算");
        assert_eq!(t.decision(), Some(TestDecision::Promote { threshold: 256 }));
    }

    /// 序贯检验:SPRT 跨上界但效果量未达标（15% < 20%）→ Reject（Promote 仅在效果量达到时发出）
    #[test]
    fn sequential_reject_when_effect_not_reached() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        let mut last = TestDecision::Continue;
        for i in 0..35 {
            // 只快 10%,未达 effect_size=20%;零方差 → SPRT 确定性退化
            last = t.record(100.0 + (i % 2) as f64, 90.0 + (i % 2) as f64);
        }
        assert_eq!(
            last,
            TestDecision::Reject,
            "效果量未达标必须 Reject,不 Promote"
        );
        assert_eq!(t.spent_alpha(), 0.05, "Reject 同样是终局,耗尽预算");
    }

    /// 序贯检验:alpha spending 耗尽后强制 Reject（预算墙——防 p-hacking 门禁核心）
    ///
    /// 语义:一次检验只能有一次终局判定;终局后实例预算耗尽,继续喂数据
    /// 只能 Reject,永不二次 Promote。
    #[test]
    fn sequential_alpha_spending_exhausted_forces_reject() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        // 阶段 1:强效果 → Promote（消耗 alpha 预算）
        for i in 0..30 {
            let _ = t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64);
        }
        assert_eq!(t.spent_alpha(), 0.05, "Promote 后预算必须耗尽");
        // 阶段 2:预算已耗尽,即使更强的效果数据也强制 Reject（不 Promote）
        for i in 0..8 {
            let d = t.record(100.0 + (i % 3) as f64, 1.0 + (i % 3) as f64);
            assert_eq!(
                d,
                TestDecision::Reject,
                "alpha 预算耗尽后强制 Reject,永不 Promote"
            );
        }
        assert_eq!(t.spent_alpha(), 0.05, "耗尽后 spent_alpha 保持不变");
    }

    /// 防 p-hacking 门禁:同分布数据重复检验不会误 Promote
    ///
    /// Inline == Rayon（无真实效果）,喂满 max_samples:
    /// 全程无 Promote;首个判定点（n=30）跨 SPRT 下界 → Reject 并冻结。
    #[test]
    fn sequential_no_false_promote_on_equal_distributions() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        for i in 0..500usize {
            let j = i as f64;
            let d = t.record(100.0 + (j % 5.0), 100.0 + ((j * 7.0) % 5.0));
            assert_ne!(
                d,
                TestDecision::Promote { threshold: 0 },
                "第 {} 次记录:同分布数据严禁误 Promote（p-hacking 红线）",
                i + 1
            );
        }
        // 同分布下 n=30 跨下界 Reject 并冻结,后续幂等
        assert_eq!(t.decision(), Some(TestDecision::Reject));
        assert_eq!(t.spent_alpha(), 0.05);
    }

    /// 确定性（Ω₂）:同输入序列两次检验结果完全一致
    #[test]
    fn sequential_deterministic_same_input_same_decision() {
        // 混合序列:先无差异（29 条）→ 强效果（31 条）
        let seq: Vec<(f64, f64)> = (0..60)
            .map(|i| {
                if i < 29 {
                    (100.0 + (i % 5) as f64, 100.0 + ((i * 3) % 5) as f64)
                } else {
                    (100.0 + (i % 5) as f64, 10.0 + (i % 5) as f64)
                }
            })
            .collect();
        let mut a = SequentialTest::new(SequentialTestConfig::default());
        let mut b = SequentialTest::new(SequentialTestConfig::default());
        let (mut da, mut db) = (Vec::new(), Vec::new());
        for (x, y) in &seq {
            da.push(a.record(*x, *y));
            db.push(b.record(*x, *y));
        }
        assert_eq!(da, db, "Ω₂:同输入序列必须同决策序列");
        assert_eq!(a.sample_count(), b.sample_count());
        assert_eq!(a.spent_alpha(), b.spent_alpha());
    }

    /// 硬停止:max_samples 是终局上限 — 弱效果数据最迟在 max_samples 处给出终局
    #[test]
    fn sequential_max_samples_hard_stop() {
        let cfg = SequentialTestConfig {
            min_samples: 5,
            max_samples: 20,
            ..SequentialTestConfig::default()
        };
        // 同分布:首个判定点（n=5）跨下界 Reject;断言终局在 <= max_samples 内发生
        let mut t = SequentialTest::new(cfg);
        for _ in 0..20 {
            let _ = t.record(100.0, 100.0);
        }
        assert_eq!(
            t.decision(),
            Some(TestDecision::Reject),
            "同分布必在 max_samples 内终局"
        );
        assert!(t.sample_count() <= 20, "样本量不得超过 max_samples");
    }

    /// 配置校验 — 非法配置 try_new 拒绝,new panic;合法配置通过
    #[allow(clippy::field_reassign_with_default)] // 逐字段置非法值以覆盖每个校验分支
    #[test]
    fn sequential_config_validation() {
        use super::sequential_test::HtsError;
        // 合法默认
        assert!(SequentialTestConfig::default().validate().is_ok());
        // alpha 越界（含 NaN）与 power 越界
        let mut cfg = SequentialTestConfig::default();
        cfg.alpha = 1.0;
        assert_eq!(cfg.validate(), Err(HtsError::AlphaOutOfRange(1.0)));
        cfg.alpha = f64::NAN;
        assert!(matches!(cfg.validate(), Err(HtsError::AlphaOutOfRange(_))));
        cfg = SequentialTestConfig::default();
        cfg.power = 0.0;
        assert_eq!(cfg.validate(), Err(HtsError::PowerOutOfRange(0.0)));
        // 样本窗口倒挂
        cfg = SequentialTestConfig::default();
        cfg.min_samples = 600;
        assert_eq!(cfg.validate(), Err(HtsError::SampleRange(600, 500)));
        // effect_size 非正
        cfg = SequentialTestConfig::default();
        cfg.effect_size = 0.0;
        assert_eq!(cfg.validate(), Err(HtsError::EffectSizeNonPositive(0.0)));
        // try_new 拒绝非法;new 非法 panic（构造前提,编程错误）
        cfg = SequentialTestConfig::default();
        cfg.effect_size = -1.0;
        assert!(SequentialTest::try_new(cfg).is_err());
    }

    /// cgroup:Linux 路径解析 — "80000 100000" → 0.8
    #[test]
    fn cgroup_parse_quota_period() {
        assert_eq!(parse_cpu_max("80000 100000"), Some(0.8));
        assert_eq!(parse_cpu_max("80000 100000\n"), Some(0.8), "容忍尾部换行");
        assert_eq!(parse_cpu_max("400000 100000"), Some(4.0));
        assert_eq!(parse_cpu_max("1 1000"), Some(0.001));
    }

    /// cgroup:"max 100000"（未限制）→ None
    #[test]
    fn cgroup_parse_max_unlimited() {
        assert_eq!(parse_cpu_max("max 100000"), None);
        assert_eq!(parse_cpu_max("max max"), None);
    }

    /// cgroup:无效输入 → None（字段缺失/多余/非数值/非正/NaN）
    #[test]
    fn cgroup_parse_invalid_inputs() {
        assert_eq!(parse_cpu_max(""), None, "空内容");
        assert_eq!(parse_cpu_max("   "), None, "纯空白");
        assert_eq!(parse_cpu_max("80000"), None, "缺 period");
        assert_eq!(parse_cpu_max("80000 100000 123"), None, "多余字段");
        assert_eq!(parse_cpu_max("abc 100000"), None, "quota 非数值");
        assert_eq!(parse_cpu_max("80000 xyz"), None, "period 非数值");
        assert_eq!(parse_cpu_max("0 100000"), None, "quota 为零");
        assert_eq!(parse_cpu_max("80000 0"), None, "period 为零");
        assert_eq!(parse_cpu_max("-1 100000"), None, "quota 为负");
        assert_eq!(parse_cpu_max("NaN 100000"), None, "quota 为 NaN");
    }

    /// cgroup:文件注入 — 临时文件路径可配置（Ω₇ 缝合）
    #[test]
    fn cgroup_probe_reads_file() {
        let dir = std::env::temp_dir().join(format!("hts_cgroup_t9_{}", std::process::id()));
        let path = dir.join("cpu.max");
        std::fs::create_dir_all(&dir).expect("临时目录创建");
        std::fs::write(&path, "80000 100000").expect("临时文件写入");
        let probe = CgroupProbe::new(path.clone());
        assert_eq!(probe.probe(), Some(0.8), "注入路径应解析出 0.8 核");
        // 文件不存在 → None
        let missing = CgroupProbe::new(dir.join("nope.max"));
        assert_eq!(missing.probe(), None, "文件缺失返回 None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// cgroup:默认探测点在非 Linux 平台恒 None（占位语义）
    #[test]
    fn cgroup_effective_cores_platform() {
        #[cfg(target_os = "linux")]
        {
            // Linux 上:无 cgroup 限制或探测失败 → None;有限制 → 正数
            let v = crate::hts::cgroup::effective_cores();
            if let Some(n) = v {
                assert!(n > 0.0, "有效核数必须为正");
            }
            // None: 未限制/无权限 → 无校正信息,合法,不校验
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(
                crate::hts::cgroup::effective_cores(),
                None,
                "非 Linux 平台 cgroup 校正恒 None"
            );
        }
    }

    /// kind_index — 与 `TaskKind::ALL` 顺序恒等且全覆盖（布局一致性防御,E1 强化）
    ///
    /// 强化自 B2 的单射断言:**逐臂交换不破坏单射**（两值仍互异）,只有恒等断言
    /// （`kind_index(ALL[i]) == i`）能抓槽位错位——而槽位错位会让 `get()` 命中
    /// 错误表项且不报错。数组长度绑定由 `[Entry; TaskKind::COUNT]` 编译期锁定,
    /// 本测试复核顺序与覆盖（运行期断言面）。
    #[test]
    fn kind_index_bijective() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (i, kind) in TaskKind::ALL.iter().enumerate() {
            let idx = kind_index(*kind);
            assert_eq!(
                idx, i,
                "kind_index(TaskKind::ALL[{i}]) 必须为 {i}（槽位错位）"
            );
            assert!(idx < TaskKind::COUNT, "索引越界（须 < TaskKind::COUNT）");
            assert!(seen.insert(idx), "索引重复:kind_index 非单射");
        }
        assert_eq!(
            seen.len(),
            TaskKind::COUNT,
            "全部登记类必须映射到唯一条目（覆盖完整性）"
        );
    }

    /// B2 价值证明(spec §7-3):faae(`ExpertScore`) 与 decay(`DecayBatch`) 从共享
    /// `Generic` 解耦为**独立表项** —— 更新其一的阈值绝不影响另一的路由决策。
    /// 这正是四维审计 §1.1 / ADR-196 要消除的「SPRT 连带升降」缺陷的回归锁。
    #[test]
    fn expert_score_and_decay_batch_entries_are_decoupled() {
        // 解耦前提:两者落在不同 kind_index(共享 Generic 时会是同一索引)。
        assert_ne!(
            kind_index(TaskKind::ExpertScore),
            kind_index(TaskKind::DecayBatch),
            "ExpertScore 与 DecayBatch 必须是不同表项(解耦)"
        );
        let mut table = HtsTable::default();
        assert_eq!(table.get(TaskKind::ExpertScore).threshold, 10_000);
        assert_eq!(table.get(TaskKind::DecayBatch).threshold, 10_000);
        // 只下调 ExpertScore 阈值 → DecayBatch 表项纹丝不动。
        table.update(
            TaskKind::ExpertScore,
            5_000,
            64,
            ThresholdSource::ConservativeDefault,
        );
        assert_eq!(
            table.get(TaskKind::ExpertScore).threshold,
            5_000,
            "ExpertScore 应被更新"
        );
        assert_eq!(
            table.get(TaskKind::DecayBatch).threshold,
            10_000,
            "DecayBatch 不得被 ExpertScore 的写入连带影响(解耦核心断言)"
        );
        // 路由因此可独立标定:n=7_000 一侧 Rayon、另一侧仍 Inline。
        assert_eq!(
            decide(TaskKind::ExpertScore, false, 5_000, 7_000),
            DispatchPlan::Rayon,
            "ExpertScore 降阈值后 n=7000 应入 Rayon"
        );
        assert_eq!(
            decide(
                TaskKind::DecayBatch,
                false,
                table.get(TaskKind::DecayBatch).threshold,
                7_000
            ),
            DispatchPlan::Inline,
            "DecayBatch 保持 10_000,n=7000 应仍 Inline"
        );
    }
}
