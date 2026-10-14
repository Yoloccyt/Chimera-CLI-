//! 三态派发计划与任务类型登记（手册 §8.3 L-f 路由器 / ADR-127）
//!
//! 对应架构层:L1 Core
//!
//! # 设计来源
//! - `DispatchPlan` 三态合并自 S8 RuntimeSwitcher（ADR-127 裁决）:
//!   S8 的 50-200μs 运行时切换器被裁决并入 L-f route() 的纳秒级查表,
//!   本文件只承载类型与静态阈值表,查表逻辑在 [`super::bridge`]。
//! - 十类任务登记对应手册 §8.4 HTS-CPU 阈值表;阈值来源标注
//!   "S9 离线测定,W1 复测"（诚实数据红线:W1 复测前为预填初值,不作已校准结论）。
//!
//! # 契约（Ω₃）
//! 公开类型均派生 Debug/Clone/Copy/PartialEq/Eq,保证可比较、可拷贝、
//! 可嵌入热路径（route 每调用一次仅读取,无堆分配）。

/// 三态派发计划 — L-f 路由输出（ADR-127 合并 S8 RuntimeSwitcher 后的终版形态）
///
/// 判定顺序（见 [`super::bridge::ComputeBridge::route`]）:
/// ① IO 密集任务直接 [`Async`](DispatchPlan::Async)（进 tokio L-b 面）;
/// ② 条目数 < 阈值走 [`Inline`](DispatchPlan::Inline)（调用线程直跑,零调度开销）;
/// ③ 否则走 [`Rayon`](DispatchPlan::Rayon)（进 L-a 全局计算池）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchPlan {
    /// 任务量 < 阈值:调用线程直跑,零调度开销
    Inline,
    /// CPU 密集且条目数 >= 阈值:进 L-a 全局池(ComputeBridge 独立 rayon 池)
    Rayon,
    /// IO 密集:进 L-b tokio 结构化并发(JoinSet/FuturesUnordered,不进 rayon 池)
    Async,
}

/// 计算任务类型登记 — 对应手册 §8.4 HTS-CPU 阈值表的十类
///
/// 语义约定:`is_io_bound() == true` 的任务**永不**进入 rayon 池
/// （红线:IO 不上 rayon,§7.5.3 纪律⑥;v4.0 §7.5.1 分类表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// CLV 向量相似度批量计算（阈值 1,000）
    ClvSimilarity,
    /// 编辑距离掩码计算（阈值 100）
    OsaMask,
    /// K 近邻检索（阈值 5,000）
    KnnSearch,
    /// GSOE 变体适应度批量评估（阈值 500,离线通道,R2 约束不动）
    GsoeEvaluate,
    /// CSC 四级压缩评分（阈值 200）
    CscCollapseScore,
    /// 未登记任务（保守默认阈值 10,000）
    Generic,
    /// HCW 条目重要性评分批量计算（阈值 200）— ADR-196 C1 从 `CscCollapseScore` 拆位
    /// 消费者:`crates/hcw-window/src/parallel.rs`(compress 评分段)
    /// 阈值来源:S9 离线测定预填(measured_at=0,未 W 复测不作已校准结论)
    /// 为何不用 `CscCollapseScore`:该位语义为「CSC 四级压缩折叠评分」,与本负载
    /// (分层窗口条目评分)不同名同源,共用会使 SPRT 校准历史无法归属
    HcwEntryScore,
    /// NMC 多模态批量编码(阈值 1,000)— ADR-196 C1 从 `ClvSimilarity` 拆位
    /// 消费者:`crates/nmc-encoder/src/parallel.rs`;与 `ClvSimilarity`(相似度配对语义)
    /// 解耦,产出恰为 CLV 不代表输入语义同类
    NmcEncode,
    /// Faae 批量专家匹配评分(阈值 10,000,保守默认)— ADR-196 C1 从 `Generic` 拆出
    /// 消费者:`crates/faae-router/src/parallel.rs`;使 decay 的阈值校准不再连带影响本负载
    ExpertScore,
    /// 能力批量衰减(快照语义)计算(阈值 10,000,保守默认)— ADR-196 C1 从 `Generic` 拆出
    /// 消费者:`crates/decay-engine/src/parallel.rs`(与 `ExpertScore` 反向理由)
    DecayBatch,
}

impl TaskKind {
    /// 全部已登记任务 — 供测试与遍历(十类,手册 §8.4 + ADR-196 C1 拆位)
    pub const ALL: [TaskKind; 10] = [
        TaskKind::ClvSimilarity,
        TaskKind::OsaMask,
        TaskKind::KnnSearch,
        TaskKind::GsoeEvaluate,
        TaskKind::CscCollapseScore,
        TaskKind::Generic,
        TaskKind::HcwEntryScore,
        TaskKind::NmcEncode,
        TaskKind::ExpertScore,
        TaskKind::DecayBatch,
    ];

    /// 登记类数 — 数组长度/表项数的**唯一定义源**(E1 单源收敛)
    ///
    /// WHY: `HtsTable::entries` 的数组长度以本常量表达(非硬编码数字),
    /// 使「表项数 = 登记类数」成为类型层约束——新增变体时 `ALL` 长度变化,
    /// `ALL.map(default_entry)` 的赋值即编译错(编译期断言),不依赖人工核对。
    pub const COUNT: usize = Self::ALL.len();

    /// IO 密集判定 — `true` 时 [`super::bridge::ComputeBridge::route`] 直投 [`DispatchPlan::Async`]
    ///
    /// # 当前登记（手册 §8.4 事实）
    /// 十类均为 CPU 计算型,返回 `false`;IO 类任务（LLM 网络调用 / 网络检索等）
    /// 按 v4.0 §7.5.1 分类应归 L-b tokio 面,登记时经此处返回 `true`,
    /// 由 HTS 阈值表（T9 序贯检验）统一驱动——本骨架不预登记 IO 类。
    #[must_use]
    pub const fn is_io_bound(self) -> bool {
        false
    }

    /// 任务阈值（items）— 手册 §8.4 HTS-CPU 阈值表的**参照投影**
    ///
    /// 来源标注:七类为 **S9 离线测定初值,W1 复测**（诚实数据:未复测前仅作预填）;
    /// [`Generic`](TaskKind::Generic) 与 ExpertScore/DecayBatch 三类为保守默认值（10,000）。
    ///
    /// # T9/E1 迁移说明
    /// 本方法已被 [`super::hts::HtsTable`] 动态表取代（route 读 arc-swap 动态表）。
    /// E1 单源收敛后本方法**委托** [`super::hts::HtsTable::default_entry`]——值的唯一定义点
    /// 在 `default_entry`,本方法仅保留「静态参照面」供本文件路由测试取边界值;
    /// 运行期阈值一律经 [`super::hts::HtsTable`] 查询。值快照由
    /// `dispatch::tests::thresholds_match_manual_s8_4`（独立 expect 数组）与
    /// `hts::tests::default_table_matches_dispatch_static`（双路径对照）共同锁定。
    #[allow(dead_code)] // T9 迁移后仅测试引用（见上方迁移说明）
    #[must_use]
    pub(crate) const fn threshold(self) -> usize {
        super::hts::HtsTable::default_entry(self).threshold
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use crate::compute::bridge::bridge;

    /// route() 三态判定 — Inline 分支:条目数 < 阈值
    #[test]
    fn route_inline_below_threshold() {
        for kind in TaskKind::ALL {
            let t = kind.threshold();
            // n_items 严格小于阈值 → Inline;0 条目也合法(空任务就地返回)
            assert_eq!(
                bridge().route(kind, t.saturating_sub(1)),
                DispatchPlan::Inline
            );
            assert_eq!(bridge().route(kind, 0), DispatchPlan::Inline);
        }
    }

    /// route() 三态判定 — Rayon 分支:条目数 >= 阈值(含边界 n == threshold)
    #[test]
    fn route_rayon_at_and_above_threshold() {
        for kind in TaskKind::ALL {
            let t = kind.threshold();
            assert_eq!(bridge().route(kind, t), DispatchPlan::Rayon);
            assert_eq!(bridge().route(kind, t + 10_000), DispatchPlan::Rayon);
        }
    }

    /// route() 三态判定 — Async 分支:IO 密集任务直投 tokio 面
    ///
    /// 当前十类登记均非 IO 型(手册 §8.4),故 Async 分支在公开 API 上不可达;
    /// 分支逻辑经纯函数 [`crate::compute::bridge::decide`] 直接覆盖,
    /// 保证 IO 类任务登记(T9)时不破坏分支语义。
    #[test]
    fn route_async_for_io_bound() {
        let plan = crate::compute::bridge::decide(TaskKind::Generic, true, 10_000, 0);
        assert_eq!(plan, DispatchPlan::Async);
        // 当前登记事实:十类均非 IO 型 → route 对全部类型永不返回 Async
        for kind in TaskKind::ALL {
            assert!(!kind.is_io_bound(), "{kind:?} 应为 CPU 计算型(手册 §8.4)");
            assert_ne!(bridge().route(kind, usize::MAX), DispatchPlan::Async);
        }
    }

    /// is_io_bound 契约 — 当前十类登记全部为 CPU 计算型
    #[test]
    fn is_io_bound_false_for_all_registered() {
        for kind in TaskKind::ALL {
            assert!(!kind.is_io_bound());
        }
    }

    /// 阈值表 — 与手册 §8.4 预填值逐项核对(来源:S9 离线测定,W1 复测)
    #[test]
    fn thresholds_match_manual_s8_4() {
        let expect = [
            (TaskKind::ClvSimilarity, 1_000),
            (TaskKind::OsaMask, 100),
            (TaskKind::KnnSearch, 5_000),
            (TaskKind::GsoeEvaluate, 500),
            (TaskKind::CscCollapseScore, 200),
            (TaskKind::Generic, 10_000),
            (TaskKind::HcwEntryScore, 200),
            (TaskKind::NmcEncode, 1_000),
            (TaskKind::ExpertScore, 10_000),
            (TaskKind::DecayBatch, 10_000),
        ];
        for (kind, want) in expect {
            assert_eq!(kind.threshold(), want, "{kind:?} 阈值偏离手册 §8.4");
        }
    }

    // ===== proptest 属性测试（mandate 要求；三态派发是路由正确性之根，
    //       B2 新加 4 位需与旧 6 位共享同一派发契约）=====
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// 三态完备且互斥：decide 恒等于 {io⇒Async; n<thr⇒Inline; else Rayon}
        #[test]
        fn prop_decide_tristate(
            kind_idx in 0..TaskKind::ALL.len(),
            io in any::<bool>(),
            thr in 0usize..65_536,
            n in 0usize..131_072,
        ) {
            let kind = TaskKind::ALL[kind_idx];
            let plan = crate::compute::bridge::decide(kind, io, thr, n);
            let expected = if io {
                DispatchPlan::Async
            } else if n < thr {
                DispatchPlan::Inline
            } else {
                DispatchPlan::Rayon
            };
            prop_assert_eq!(plan, expected);
        }

        /// 阈值是干净的切割点：恰 n-1⇒Inline、n⇒Rayon（io=false，thr=n）。
        /// 捕捉 `<`/`<=` 的 off-by-one（spec B2 依赖此边界）。
        #[test]
        fn prop_threshold_is_clean_cut(
            kind_idx in 0..TaskKind::ALL.len(),
            n in 1usize..65_536,
        ) {
            let kind = TaskKind::ALL[kind_idx];
            let thr = n;
            prop_assert_eq!(
                crate::compute::bridge::decide(kind, false, thr, n - 1),
                DispatchPlan::Inline,
                "n-1 < thr 应判 Inline"
            );
            prop_assert_eq!(
                crate::compute::bridge::decide(kind, false, thr, n),
                DispatchPlan::Rayon,
                "n >= thr 应判 Rayon"
            );
        }

        /// io 支配性：io_bound=true ⇒ Async，与阈值/条目数无关
        #[test]
        fn prop_io_bound_dominates(
            kind_idx in 0..TaskKind::ALL.len(),
            thr in any::<usize>(),
            n in any::<usize>(),
        ) {
            let kind = TaskKind::ALL[kind_idx];
            prop_assert_eq!(
                crate::compute::bridge::decide(kind, true, thr, n),
                DispatchPlan::Async
            );
        }
    }
}
