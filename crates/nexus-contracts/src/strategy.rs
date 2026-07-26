//! 记忆策略契约 — S2 接缝（记忆策略选择）类型定义
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P4-W14.1.1**（S2 接缝上下文/臂/奖励定义）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 六接缝表 S2
//!
//! # 核心职责
//!
//! 承载 mlc-engine 召回路径的"记忆策略"参数，将其从硬编码默认值
//! （`recall()` 全量召回）升级为可注入策略，为 P4 `omega-learner` Bandit
//! 异步下发学习值奠基。对应三重悖论中"记忆悖论"的修复路径——
//! 任务阶段切换时通过 LinUCB 学习最优策略，避免"幽灵记忆"。
//!
//! # 五种记忆策略（臂集）
//!
//! | 策略 | 语义 | 召回范围 | 适用阶段 |
//! |------|------|---------|---------|
//! | `MinimalRecall` | 最小检索 | L0 only, k=1 | 初期快速响应 |
//! | `StandardTopK` | 标准 TopK | L0-L2, k=10 | 通用默认 |
//! | `QueryReformulation` | 查询重构 | 多查询融合 k=10 | 卡壳需多角度 |
//! | `AggressivePruning` | 激进剪枝 | 高相似度阈值 k=5 | 长跑抑制噪声 |
//! | `TimeFocused` | 时间聚焦 | TemporalMeta 加权 | 时序敏感任务 |
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `MemoryStrategyPolicy::default()` 返回 `Static(MemoryStrategy::StandardTopK)`，
//! `StandardTopK` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `MemoryStrategyPolicy::Static`，**无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 与 SelectorPolicy / DensityPolicy 的对称设计
//!
//! 三接缝（S1/S2/S4）共用相同骨架：
//! - 枚举 + `Static`/`Learned` 双变体策略
//! - `default()` = `Static(默认常量)`（C4 合规）
//! - `fallback()` = `Static(默认常量)`（学习熔断入口）
//! - `version()` 用于 A/B 测试与回滚
//!
//! # 示例
//!
//! ## 静态策略（默认 fallback）
//!
//! ```
//! use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
//!
//! let policy = MemoryStrategyPolicy::default();
//! assert!(policy.is_static());
//! assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
//! assert_eq!(policy.version(), None);
//! ```
//!
//! ## 学习策略（omega-learner 异步下发）
//!
//! ```
//! use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
//!
//! let learned = MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused);
//! assert!(learned.is_learned());
//! assert_eq!(learned.version(), Some(42));
//! assert_eq!(learned.strategy(), MemoryStrategy::TimeFocused);
//! ```
//!
//! ## learner panic 时本地 fallback
//!
//! ```
//! use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
//!
//! let learned = MemoryStrategyPolicy::learned(1, MemoryStrategy::AggressivePruning);
//! let fallback = MemoryStrategyPolicy::fallback();
//! assert!(fallback.is_static());
//! assert_eq!(fallback.strategy(), MemoryStrategy::StandardTopK);
//! ```

use serde::{Deserialize, Serialize};

/// 记忆策略 — 五种召回策略（S2 接缝臂集）
///
/// 决定 mlc-engine 召回路径的行为：
/// - 召回范围（L0 only / L0-L2 / L0-L3）
/// - Top-K 大小（1 / 5 / 10）
/// - 过滤策略（相似度阈值 / TemporalMeta 加权 / 多查询融合）
///
/// # 设计决策（WHY）
/// - **枚举而非字符串**: 5 种策略有限且固定，枚举提供编译期穷尽性检查
/// - **Copy + Clone**: 枚举为单元变体，Copy 语义零成本
/// - **`StandardTopK` 默认**: 对应既有 `recall_by_clv(top_k=10)` 行为，向后兼容
/// - **不暴露具体 k 值**: 策略语义而非数值参数，避免 bandit 探索到无效 k
///
/// # 与三重悖论"记忆悖论"的修复关系
///
/// 三重悖论病理：静态稀疏掩码无法替代 MemCon 式自适应记忆控制，固定 top-k
/// 召回在任务阶段切换时会产生"幽灵记忆"（新旧事实共存无法区分时间有效性）。
/// S2 接缝通过 LinUCB 学习任务阶段（Initial/Stuck/LongRun）→ 策略映射，
/// 使记忆策略随任务阶段自适应（MinimalRecall → StandardTopK →
/// QueryReformulation → AggressivePruning）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MemoryStrategy {
    /// 最小检索 — L0 only, k=1
    ///
    /// WHY 适用初期: 任务初期阶段目标尚未明确，召回少而精，避免噪声污染。
    /// 仅查 L0 WorkingMemory（容量 64，LRU），返回最相关 1 条目。
    MinimalRecall = 1,

    /// 标准 TopK — L0-L2, k=10（默认 fallback）
    ///
    /// WHY 通用默认: 平衡召回率与延迟，适用于大多数任务阶段。
    /// 跨 L0/L1/L2 三层召回，按相似度 Top-10 返回。
    StandardTopK = 2,

    /// 查询重构 — 多查询融合 k=10
    ///
    /// WHY 适用卡壳: 任务卡壳阶段单一查询召回不足，通过多查询
    /// （如同义词扩展、CLV 邻居向量）融合召回，提升召回率。
    QueryReformulation = 3,

    /// 激进剪枝 — 高相似度阈值 k=5
    ///
    /// WHY 适用长跑: 长期任务上下文积累过多，需高阈值剪枝抑制噪声。
    /// 相似度阈值从默认 0.0 提升至 0.5，仅返回强相关条目。
    AggressivePruning = 4,

    /// 时间聚焦 — TemporalMeta 加权
    ///
    /// WHY 时序敏感: 时序敏感任务需优先召回 Current 状态条目，
    /// 通过 TemporalMeta.transition_type 加权（Current ×1.0，
    /// Transition ×0.5，Historical ×0.0）。
    TimeFocused = 5,
}

impl MemoryStrategy {
    /// 所有记忆策略（按枚举值升序，便于遍历初始化 LinUCB 臂集）
    pub const ALL: [Self; 5] = [
        Self::MinimalRecall,
        Self::StandardTopK,
        Self::QueryReformulation,
        Self::AggressivePruning,
        Self::TimeFocused,
    ];

    /// 返回臂数（5 个策略对应 LinUCB 5 臂）
    pub const ARM_COUNT: usize = 5;

    /// 返回策略简称（用于日志/调试与 ArmId 构造）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::MinimalRecall => "minimal",
            Self::StandardTopK => "standard",
            Self::QueryReformulation => "reformulation",
            Self::AggressivePruning => "pruning",
            Self::TimeFocused => "time-focused",
        }
    }

    /// 返回 Top-K 默认值（用于 recall 路径参数化）
    ///
    /// 不同策略对应不同 k 值，调用方按策略选择 k。
    /// `TimeFocused` 与 `QueryReformulation` 复用 StandardTopK 的 k=10，
    /// 差异在过滤逻辑而非 k 值。
    pub const fn default_top_k(self) -> usize {
        match self {
            Self::MinimalRecall => 1,
            Self::StandardTopK => 10,
            Self::QueryReformulation => 10,
            Self::AggressivePruning => 5,
            Self::TimeFocused => 10,
        }
    }

    /// 返回相似度阈值（用于 AggressivePruning 高阈值剪枝）
    ///
    /// 默认 0.0（无过滤），AggressivePruning 提升至 0.5 抑制噪声。
    pub const fn similarity_threshold(self) -> f32 {
        match self {
            Self::MinimalRecall => 0.0,
            Self::StandardTopK => 0.0,
            Self::QueryReformulation => 0.0,
            Self::AggressivePruning => 0.5,
            Self::TimeFocused => 0.0,
        }
    }

    /// 是否仅查 L0（MinimalRecall 优化路径）
    pub const fn l0_only(self) -> bool {
        matches!(self, Self::MinimalRecall)
    }

    /// 是否启用 TemporalMeta 加权（TimeFocused 专用）
    pub const fn temporal_weighted(self) -> bool {
        matches!(self, Self::TimeFocused)
    }
}

impl Default for MemoryStrategy {
    /// 默认策略 = `StandardTopK`（向后兼容 `recall_by_clv(top_k=10)` 行为）
    ///
    /// WHY(C4 合规): 默认值 = 既有 mlc-engine 行为，fallback 编译进二进制。
    /// `omega-learner` 未下发学习策略时，行为与 D1 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::StandardTopK
    }
}

impl std::fmt::Display for MemoryStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// 记忆策略策略 — 静态常量或学习版本（S2 接缝）
///
/// 承载 mlc-engine 召回路径的策略，将其从硬编码默认值
/// 升级为可注入策略，为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](MemoryStrategyPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](MemoryStrategyPolicy::Learned): `omega-learner` 异步下发的版本化策略
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(MemoryStrategy::StandardTopK)`，
/// `StandardTopK` 是 `const` 常量。`omega-learner` panic/超时时，
/// 调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量
/// - **Copy 语义**: `MemoryStrategy` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚
/// - **与 SelectorPolicy / DensityPolicy 对称设计**: 复用相同模式降低学习成本
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
/// ```
/// use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
///
/// let policy = MemoryStrategyPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
/// ```
/// use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
///
/// let policy = MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused);
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MemoryStrategyPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `MemoryStrategy` 常量，`default()` 返回
    /// `Static(MemoryStrategy::StandardTopK)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(MemoryStrategy),

    /// 学习策略 — `omega-learner` 异步下发的版本化策略
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `strategy` 承载学习到的策略值，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的记忆策略
        strategy: MemoryStrategy,
    },
}

impl MemoryStrategyPolicy {
    /// 创建静态策略（便捷构造函数）
    pub const fn static_policy(strategy: MemoryStrategy) -> Self {
        Self::Static(strategy)
    }

    /// 创建学习策略（便捷构造函数）
    pub const fn learned(version: u64, strategy: MemoryStrategy) -> Self {
        Self::Learned { version, strategy }
    }

    /// 返回 fallback 策略 — `Static(MemoryStrategy::StandardTopK)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `StandardTopK` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(MemoryStrategy::StandardTopK)
    }

    /// 返回当前策略的记忆策略值（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取策略。
    pub const fn strategy(&self) -> MemoryStrategy {
        match self {
            Self::Static(s) => *s,
            Self::Learned { strategy, .. } => *strategy,
        }
    }

    /// 返回学习版本号（Static 返回 None，Learned 返回 Some(version)）
    ///
    /// WHY: 用于 A/B 测试与回滚 — `omega-learner` 每次下发递增版本号，
    /// 调用方可记录使用的版本号用于效果追踪。
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::Static(_) => None,
            Self::Learned { version, .. } => Some(*version),
        }
    }

    /// 是否为学习策略
    pub const fn is_learned(&self) -> bool {
        matches!(self, Self::Learned { .. })
    }

    /// 是否为静态策略
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }
}

impl Default for MemoryStrategyPolicy {
    /// 默认策略 = `Static(MemoryStrategy::StandardTopK)`
    ///
    /// WHY(C4 合规): 默认值 = 既有 mlc-engine 行为（recall_by_clv top_k=10），
    /// fallback 编译进二进制。调用方未注入策略时行为与 D1 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Static(MemoryStrategy::StandardTopK)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // MemoryStrategy 单元测试
    // ============================================================

    #[test]
    fn test_strategy_default_is_standard_topk() {
        let strategy = MemoryStrategy::default();
        assert_eq!(strategy, MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_strategy_all_returns_five() {
        let all = MemoryStrategy::ALL;
        assert_eq!(all.len(), 5);
        assert_eq!(MemoryStrategy::ARM_COUNT, 5);
    }

    #[test]
    fn test_strategy_all_unique() {
        let all = MemoryStrategy::ALL;
        let mut seen = std::collections::HashSet::new();
        for strategy in all.iter() {
            assert!(seen.insert(*strategy), "duplicate strategy: {strategy:?}");
        }
    }

    #[test]
    fn test_strategy_short_name() {
        assert_eq!(MemoryStrategy::MinimalRecall.short_name(), "minimal");
        assert_eq!(MemoryStrategy::StandardTopK.short_name(), "standard");
        assert_eq!(
            MemoryStrategy::QueryReformulation.short_name(),
            "reformulation"
        );
        assert_eq!(MemoryStrategy::AggressivePruning.short_name(), "pruning");
        assert_eq!(MemoryStrategy::TimeFocused.short_name(), "time-focused");
    }

    #[test]
    fn test_strategy_display() {
        assert_eq!(format!("{}", MemoryStrategy::MinimalRecall), "minimal");
        assert_eq!(format!("{}", MemoryStrategy::StandardTopK), "standard");
    }

    #[test]
    fn test_strategy_default_top_k() {
        assert_eq!(MemoryStrategy::MinimalRecall.default_top_k(), 1);
        assert_eq!(MemoryStrategy::StandardTopK.default_top_k(), 10);
        assert_eq!(MemoryStrategy::QueryReformulation.default_top_k(), 10);
        assert_eq!(MemoryStrategy::AggressivePruning.default_top_k(), 5);
        assert_eq!(MemoryStrategy::TimeFocused.default_top_k(), 10);
    }

    #[test]
    fn test_strategy_similarity_threshold() {
        // 默认 0.0（无过滤）
        assert!((MemoryStrategy::MinimalRecall.similarity_threshold() - 0.0).abs() < 1e-6);
        assert!((MemoryStrategy::StandardTopK.similarity_threshold() - 0.0).abs() < 1e-6);
        // AggressivePruning 提升至 0.5
        assert!(
            (MemoryStrategy::AggressivePruning.similarity_threshold() - 0.5).abs() < 1e-6,
            "AggressivePruning 阈值应为 0.5"
        );
    }

    #[test]
    fn test_strategy_l0_only() {
        assert!(MemoryStrategy::MinimalRecall.l0_only());
        assert!(!MemoryStrategy::StandardTopK.l0_only());
        assert!(!MemoryStrategy::TimeFocused.l0_only());
    }

    #[test]
    fn test_strategy_temporal_weighted() {
        assert!(MemoryStrategy::TimeFocused.temporal_weighted());
        assert!(!MemoryStrategy::StandardTopK.temporal_weighted());
        assert!(!MemoryStrategy::AggressivePruning.temporal_weighted());
    }

    #[test]
    fn test_strategy_copy_semantics() {
        let s1 = MemoryStrategy::StandardTopK;
        let s2 = s1; // Copy
        assert_eq!(s1, s2); // s1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_strategy_equality() {
        assert_eq!(MemoryStrategy::StandardTopK, MemoryStrategy::StandardTopK);
        assert_ne!(MemoryStrategy::MinimalRecall, MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_strategy_serialize_json() {
        let strategy = MemoryStrategy::TimeFocused;
        let json = serde_json::to_string(&strategy).unwrap();
        let deserialized: MemoryStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, deserialized);
    }

    #[test]
    fn test_strategy_serialize_yaml() {
        let strategy = MemoryStrategy::AggressivePruning;
        let yaml = serde_yaml::to_string(&strategy).unwrap();
        let deserialized: MemoryStrategy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(strategy, deserialized);
    }

    // ============================================================
    // MemoryStrategyPolicy 单元测试
    // ============================================================

    #[test]
    fn test_policy_default_is_static_standard() {
        let policy = MemoryStrategyPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
        assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_policy_default_version_none() {
        let policy = MemoryStrategyPolicy::default();
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_policy_fallback_equals_default() {
        assert_eq!(
            MemoryStrategyPolicy::fallback(),
            MemoryStrategyPolicy::default()
        );
    }

    #[test]
    fn test_policy_fallback_is_static() {
        let fallback = MemoryStrategyPolicy::fallback();
        assert!(fallback.is_static());
        assert!(!fallback.is_learned());
        assert_eq!(fallback.strategy(), MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_policy_static_constructor() {
        let policy = MemoryStrategyPolicy::static_policy(MemoryStrategy::MinimalRecall);
        assert!(policy.is_static());
        assert_eq!(policy.version(), None);
        assert_eq!(policy.strategy(), MemoryStrategy::MinimalRecall);
    }

    #[test]
    fn test_policy_learned_constructor() {
        let policy = MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), MemoryStrategy::TimeFocused);
    }

    #[test]
    fn test_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let policy = MemoryStrategyPolicy::learned(0, MemoryStrategy::MinimalRecall);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_policy_strategy_static() {
        let policy = MemoryStrategyPolicy::Static(MemoryStrategy::AggressivePruning);
        assert_eq!(policy.strategy(), MemoryStrategy::AggressivePruning);
    }

    #[test]
    fn test_policy_strategy_learned() {
        let policy = MemoryStrategyPolicy::Learned {
            version: 1,
            strategy: MemoryStrategy::QueryReformulation,
        };
        assert_eq!(policy.strategy(), MemoryStrategy::QueryReformulation);
    }

    #[test]
    fn test_policy_equality_static() {
        let p1 = MemoryStrategyPolicy::static_policy(MemoryStrategy::StandardTopK);
        let p2 = MemoryStrategyPolicy::static_policy(MemoryStrategy::StandardTopK);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_equality_learned() {
        let p1 = MemoryStrategyPolicy::learned(1, MemoryStrategy::TimeFocused);
        let p2 = MemoryStrategyPolicy::learned(1, MemoryStrategy::TimeFocused);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_version() {
        let p1 = MemoryStrategyPolicy::learned(1, MemoryStrategy::StandardTopK);
        let p2 = MemoryStrategyPolicy::learned(2, MemoryStrategy::StandardTopK);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_strategy() {
        let p1 = MemoryStrategyPolicy::learned(1, MemoryStrategy::MinimalRecall);
        let p2 = MemoryStrategyPolicy::learned(1, MemoryStrategy::StandardTopK);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_static_vs_learned() {
        let p1 = MemoryStrategyPolicy::static_policy(MemoryStrategy::StandardTopK);
        let p2 = MemoryStrategyPolicy::learned(1, MemoryStrategy::StandardTopK);
        assert_ne!(p1, p2); // 不同变体
    }

    #[test]
    fn test_policy_copy_semantics() {
        let policy = MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused);
        let copied = policy; // Copy
        assert_eq!(policy, copied); // policy 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_policy_serialize_json_static() {
        let policy = MemoryStrategyPolicy::static_policy(MemoryStrategy::AggressivePruning);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: MemoryStrategyPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_json_learned() {
        let policy = MemoryStrategyPolicy::learned(42, MemoryStrategy::TimeFocused);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: MemoryStrategyPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_static() {
        let policy = MemoryStrategyPolicy::static_policy(MemoryStrategy::QueryReformulation);
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: MemoryStrategyPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_learned() {
        let policy = MemoryStrategyPolicy::learned(7, MemoryStrategy::MinimalRecall);
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: MemoryStrategyPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    // ============================================================
    // D1 修复场景测试（S2 接缝 — 记忆策略外置）
    // ============================================================

    #[test]
    fn test_d1_scenario_static_fallback_compiled_into_binary() {
        // spec.md "默认静态值 = 当前常量，fallback 编译进同一二进制"
        let policy = MemoryStrategyPolicy::default();
        assert!(policy.is_static());
        // 验证 fallback 值 = 既有 mlc-engine 行为（StandardTopK, top_k=10）
        assert_eq!(policy.strategy(), MemoryStrategy::StandardTopK);
        assert_eq!(policy.strategy().default_top_k(), 10);
    }

    #[test]
    fn test_d1_scenario_learner_panic_local_fallback() {
        // 模拟: omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        let learned = MemoryStrategyPolicy::learned(1, MemoryStrategy::TimeFocused);
        assert!(learned.is_learned());

        // learner panic → 本地 fallback
        let fallback = MemoryStrategyPolicy::fallback();
        assert!(fallback.is_static());
        assert_ne!(fallback.strategy(), learned.strategy());
    }

    #[test]
    fn test_d1_scenario_no_cross_crate_flag() {
        // spec.md "无跨 crate 旗标"
        // MemoryStrategyPolicy 通过值注入（Copy），不依赖全局 static 或 feature flag
        let policy = MemoryStrategyPolicy::default();
        let strategy = policy.strategy();
        // 策略值直接从 const 常量获取，无运行时旗标查询
        assert_eq!(strategy, MemoryStrategy::StandardTopK);
    }

    #[test]
    fn test_d1_scenario_learned_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let v1 = MemoryStrategyPolicy::learned(1, MemoryStrategy::StandardTopK);
        let v2 = MemoryStrategyPolicy::learned(2, MemoryStrategy::AggressivePruning);
        assert_ne!(v1.version(), v2.version());
        assert_ne!(v1.strategy(), v2.strategy());
    }
}
