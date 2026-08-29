//! 三态派发计划与任务类型登记（手册 §8.3 L-f 路由器 / ADR-127）
//!
//! 对应架构层:L1 Core
//!
//! # 设计来源
//! - `DispatchPlan` 三态合并自 S8 RuntimeSwitcher（ADR-127 裁决）:
//!   S8 的 50-200μs 运行时切换器被裁决并入 L-f route() 的纳秒级查表,
//!   本文件只承载类型与静态阈值表,查表逻辑在 [`super::bridge`]。
//! - 六类任务登记对应手册 §8.4 HTS-CPU 阈值表;阈值来源标注
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

/// 计算任务类型登记 — 对应手册 §8.4 HTS-CPU 阈值表的六类
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
}

impl TaskKind {
    /// 全部已登记任务 — 供测试与遍历（当前六类,手册 §8.4）
    pub const ALL: [TaskKind; 6] = [
        TaskKind::ClvSimilarity,
        TaskKind::OsaMask,
        TaskKind::KnnSearch,
        TaskKind::GsoeEvaluate,
        TaskKind::CscCollapseScore,
        TaskKind::Generic,
    ];

    /// IO 密集判定 — `true` 时 [`super::bridge::ComputeBridge::route`] 直投 [`DispatchPlan::Async`]
    ///
    /// # 当前登记（手册 §8.4 事实）
    /// 六类均为 CPU 计算型,返回 `false`;IO 类任务（LLM 网络调用 / 网络检索等）
    /// 按 v4.0 §7.5.1 分类应归 L-b tokio 面,登记时经此处返回 `true`,
    /// 由 HTS 阈值表（T9 序贯检验）统一驱动——本骨架不预登记 IO 类。
    #[must_use]
    pub const fn is_io_bound(self) -> bool {
        false
    }

    /// 任务阈值（items）— 手册 §8.4 HTS-CPU 阈值表
    ///
    /// 来源标注:五类为 **S9 离线测定初值,W1 复测**（诚实数据:未复测前仅作预填）;
    /// [`Generic`](TaskKind::Generic) 为保守默认值（10,000）。
    ///
    /// # T9 迁移说明
    /// 本方法已被 [`super::hts::HtsTable`] 动态表取代（route 读 arc-swap 动态表）,
    /// 此处保留仅为本文件测试提供初始值参照（迁移等价性由
    /// `hts::tests::default_table_matches_dispatch_static` 锁定）;运行期阈值一律经
    /// [`super::hts::HtsTable`] 查询。
    #[allow(dead_code)] // T9 迁移后仅测试引用（见上方迁移说明）
    #[must_use]
    pub(crate) const fn threshold(self) -> usize {
        match self {
            TaskKind::ClvSimilarity => 1_000,
            TaskKind::OsaMask => 100,
            TaskKind::KnnSearch => 5_000,
            TaskKind::GsoeEvaluate => 500,
            TaskKind::CscCollapseScore => 200,
            TaskKind::Generic => 10_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::bridge::bridge;

    /// route() 三态判定 — Inline 分支:条目数 < 阈值
    #[test]
    fn route_inline_below_threshold() {
        for kind in TaskKind::ALL {
            let t = kind.threshold();
            // n_items 严格小于阈值 → Inline;0 条目也合法(空任务就地返回)
            assert_eq!(bridge().route(kind, t.saturating_sub(1)), DispatchPlan::Inline);
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
    /// 当前六类登记均非 IO 型(手册 §8.4),故 Async 分支在公开 API 上不可达;
    /// 分支逻辑经纯函数 [`crate::compute::bridge::decide`] 直接覆盖,
    /// 保证 IO 类任务登记(T9)时不破坏分支语义。
    #[test]
    fn route_async_for_io_bound() {
        let plan = crate::compute::bridge::decide(TaskKind::Generic, true, 10_000, 0);
        assert_eq!(plan, DispatchPlan::Async);
        // 当前登记事实:六类均非 IO 型 → route 对全部类型永不返回 Async
        for kind in TaskKind::ALL {
            assert!(!kind.is_io_bound(), "{kind:?} 应为 CPU 计算型(手册 §8.4)");
            assert_ne!(bridge().route(kind, usize::MAX), DispatchPlan::Async);
        }
    }

    /// is_io_bound 契约 — 当前六类登记全部为 CPU 计算型
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
        ];
        for (kind, want) in expect {
            assert_eq!(kind.threshold(), want, "{kind:?} 阈值偏离手册 §8.4");
        }
    }
}
