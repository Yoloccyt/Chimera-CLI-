//! 预取策略契约 — S3 接缝（SCC 预取）类型定义
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P4-W14.2.1**（S3 接缝上下文/臂/奖励定义）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 六接缝表 S3
//!
//! # 核心职责
//!
//! 承载 scc-cache 预取路径的"预取策略"参数，将其从硬编码默认值
//! （`prefetch_threshold = 0.6`）升级为可注入策略，为 P4 `omega-learner` Bandit
//! 异步下发学习值奠基。对应三重悖论中"推理悖论"的修复路径——
//! 通过 LinUCB 学习上下文特征 → 预取策略映射，避免预取消耗超过推理增益。
//!
//! # 五种预取策略（臂集）
//!
//! | 策略 | 语义 | 预取阈值 | Top-K | 适用场景 |
//! |------|------|---------|-------|---------|
//! | `NoPrefetch` | 不预取 | 1.1（永不触发） | 0 | 编辑历史稀疏 |
//! | `Conservative` | 保守预取 | 0.8 | 5 | 高置信度场景 |
//! | `Standard` | 标准预取 | 0.6（默认 fallback） | 10 | 通用默认 |
//! | `Aggressive` | 激进预取 | 0.3 | 20 | 编辑历史密集 |
//! | `TopK3` | 固定 Top-3 预取 | 0.0（无阈值） | 3 | 紧凑缓存场景 |
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `PrefetchPolicy::default()` 返回 `Static(PrefetchStrategy::Standard)`，
//! `Standard` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `PrefetchPolicy::Static`，**无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 与 MemoryStrategyPolicy / DensityPolicy 的对称设计
//!
//! 三接缝（S1/S2/S3）共用相同骨架：
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
//! use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
//!
//! let policy = PrefetchPolicy::default();
//! assert!(policy.is_static());
//! assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
//! assert_eq!(policy.version(), None);
//! ```
//!
//! ## 学习策略（omega-learner 异步下发）
//!
//! ```
//! use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
//!
//! let learned = PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive);
//! assert!(learned.is_learned());
//! assert_eq!(learned.version(), Some(42));
//! assert_eq!(learned.strategy(), PrefetchStrategy::Aggressive);
//! ```
//!
//! ## learner panic 时本地 fallback
//!
//! ```
//! use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
//!
//! let learned = PrefetchPolicy::learned(1, PrefetchStrategy::TopK3);
//! let fallback = PrefetchPolicy::fallback();
//! assert!(fallback.is_static());
//! assert_eq!(fallback.strategy(), PrefetchStrategy::Standard);
//! ```

use serde::{Deserialize, Serialize};

/// 预取策略 — 五种预取行为（S3 接缝臂集）
///
/// 决定 scc-cache `AccessPatternLearner::prefetch` 路径的行为：
/// - 预取阈值（何种概率的上下文会被预取）
/// - Top-K 限制（最多预取多少个候选）
///
/// # 设计决策（WHY）
/// - **枚举而非字符串**: 5 种策略有限且固定，枚举提供编译期穷尽性检查
/// - **Copy + Clone**: 枚举为单元变体，Copy 语义零成本
/// - **`Standard` 默认**: 对应既有 `prefetch_threshold=0.6` 行为，向后兼容
/// - **不暴露具体阈值数值**: 策略语义而非数值参数，避免 bandit 探索到无效组合
///
/// # 与三重悖论"推理悖论"的修复关系
///
/// 三重悖论病理：10 层架构的跨层协调成本存在阈值，当协调成本超过推理增益时
/// 多 Agent 反而不如单 Agent。预取是典型的"协调成本 vs 推理增益"权衡：
/// - 预取消耗 CPU/内存/IO 资源（协调成本）
/// - 预取命中可避免缓存未命中延迟（推理增益）
///
/// S3 接缝通过 LinUCB 学习上下文特征 → 策略映射，使预取强度随场景自适应，
/// 避免 NoPrefetch 时缓存未命中率高（推理增益损失）与 Aggressive 时
/// 预取消耗过大（协调成本超载）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PrefetchStrategy {
    /// 不预取 — threshold=1.1（永不触发），top_k=0
    ///
    /// WHY 适用稀疏编辑: 编辑历史稀疏时马尔可夫链预测置信度低，
    /// 预取命中率低且消耗资源，禁用预取更经济。
    NoPrefetch = 1,

    /// 保守预取 — threshold=0.8，top_k=5
    ///
    /// WHY 适用高置信度: 仅对高概率（≥0.8）的转移预取，
    /// 避免预取低相关上下文占用缓存空间。
    Conservative = 2,

    /// 标准预取 — threshold=0.6（默认 fallback），top_k=10
    ///
    /// WHY 通用默认: 平衡预取命中率与资源消耗，对应既有 scc-cache 行为。
    Standard = 3,

    /// 激进预取 — threshold=0.3，top_k=20
    ///
    /// WHY 适用密集编辑: 编辑历史密集时局部性强，预取命中率高，
    /// 降低阈值扩大预取范围可显著减少缓存未命中。
    Aggressive = 4,

    /// 固定 Top-3 预取 — threshold=0.0（无阈值），top_k=3
    ///
    /// WHY 适用紧凑缓存: 缓存容量受限时，固定取 Top-3 概率上下文，
    /// 避免预取过多条目挤占热点条目空间。
    TopK3 = 5,
}

impl PrefetchStrategy {
    /// 所有预取策略（按枚举值升序，便于遍历初始化 LinUCB 臂集）
    pub const ALL: [Self; 5] = [
        Self::NoPrefetch,
        Self::Conservative,
        Self::Standard,
        Self::Aggressive,
        Self::TopK3,
    ];

    /// 返回臂数（5 个策略对应 LinUCB 5 臂）
    pub const ARM_COUNT: usize = 5;

    /// 返回策略简称（用于日志/调试与 ArmId 构造）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::NoPrefetch => "no-prefetch",
            Self::Conservative => "conservative",
            Self::Standard => "standard",
            Self::Aggressive => "aggressive",
            Self::TopK3 => "topk3",
        }
    }

    /// 返回预取概率阈值（0.0-1.1）
    ///
    /// - `NoPrefetch` 返回 1.1：永不满足 `prob >= threshold`，等价于禁用预取
    /// - `TopK3` 返回 0.0：所有预测都满足阈值，仅受 top_k 限制
    /// - 其他策略按语义设定阈值
    pub const fn prefetch_threshold(self) -> f32 {
        match self {
            Self::NoPrefetch => 1.1,
            Self::Conservative => 0.8,
            Self::Standard => 0.6,
            Self::Aggressive => 0.3,
            Self::TopK3 => 0.0,
        }
    }

    /// 返回最大预取候选数（Top-K）
    ///
    /// `NoPrefetch` 返回 0：预取列表为空，等价于禁用预取。
    /// `TopK3` 返回 3：固定取 Top-3。
    pub const fn top_k(self) -> usize {
        match self {
            Self::NoPrefetch => 0,
            Self::Conservative => 5,
            Self::Standard => 10,
            Self::Aggressive => 20,
            Self::TopK3 => 3,
        }
    }

    /// 是否禁用预取（NoPrefetch 专用）
    pub const fn disabled(self) -> bool {
        matches!(self, Self::NoPrefetch)
    }
}

impl Default for PrefetchStrategy {
    /// 默认策略 = `Standard`（向后兼容 `prefetch_threshold=0.6` 行为）
    ///
    /// WHY(C4 合规): 默认值 = 既有 scc-cache 行为，fallback 编译进二进制。
    /// `omega-learner` 未下发学习策略时，行为与 P4 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Standard
    }
}

impl std::fmt::Display for PrefetchStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// 预取策略策略 — 静态常量或学习版本（S3 接缝）
///
/// 承载 scc-cache 预取路径的策略，将其从硬编码默认值（threshold=0.6）
/// 升级为可注入策略，为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](PrefetchPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](PrefetchPolicy::Learned): `omega-learner` 异步下发的版本化策略
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(PrefetchStrategy::Standard)`，
/// `Standard` 是 `const` 常量。`omega-learner` panic/超时时，
/// 调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量
/// - **Copy 语义**: `PrefetchStrategy` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚
/// - **与 MemoryStrategyPolicy / DensityPolicy 对称设计**: 复用相同模式降低学习成本
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
/// ```
/// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
///
/// let policy = PrefetchPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
/// ```
/// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
///
/// let policy = PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive);
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PrefetchPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `PrefetchStrategy` 常量，`default()` 返回
    /// `Static(PrefetchStrategy::Standard)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(PrefetchStrategy),

    /// 学习策略 — `omega-learner` 异步下发的版本化策略
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `strategy` 承载学习到的策略值，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的预取策略
        strategy: PrefetchStrategy,
    },
}

impl PrefetchPolicy {
    /// 创建静态策略（便捷构造函数）
    pub const fn static_policy(strategy: PrefetchStrategy) -> Self {
        Self::Static(strategy)
    }

    /// 创建学习策略（便捷构造函数）
    pub const fn learned(version: u64, strategy: PrefetchStrategy) -> Self {
        Self::Learned { version, strategy }
    }

    /// 返回 fallback 策略 — `Static(PrefetchStrategy::Standard)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `Standard` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(PrefetchStrategy::Standard)
    }

    /// 返回当前策略的预取策略值（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取策略。
    pub const fn strategy(&self) -> PrefetchStrategy {
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

impl Default for PrefetchPolicy {
    /// 默认策略 = `Static(PrefetchStrategy::Standard)`
    ///
    /// WHY(C4 合规): 默认值 = 既有 scc-cache 行为（prefetch_threshold=0.6），
    /// fallback 编译进二进制。调用方未注入策略时行为与 P4 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Static(PrefetchStrategy::Standard)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // PrefetchStrategy 单元测试
    // ============================================================

    #[test]
    fn test_strategy_default_is_standard() {
        let strategy = PrefetchStrategy::default();
        assert_eq!(strategy, PrefetchStrategy::Standard);
    }

    #[test]
    fn test_strategy_all_returns_five() {
        let all = PrefetchStrategy::ALL;
        assert_eq!(all.len(), 5);
        assert_eq!(PrefetchStrategy::ARM_COUNT, 5);
    }

    #[test]
    fn test_strategy_all_unique() {
        let all = PrefetchStrategy::ALL;
        let mut seen = std::collections::HashSet::new();
        for strategy in all.iter() {
            assert!(seen.insert(*strategy), "duplicate strategy: {strategy:?}");
        }
    }

    #[test]
    fn test_strategy_short_name() {
        assert_eq!(PrefetchStrategy::NoPrefetch.short_name(), "no-prefetch");
        assert_eq!(PrefetchStrategy::Conservative.short_name(), "conservative");
        assert_eq!(PrefetchStrategy::Standard.short_name(), "standard");
        assert_eq!(PrefetchStrategy::Aggressive.short_name(), "aggressive");
        assert_eq!(PrefetchStrategy::TopK3.short_name(), "topk3");
    }

    #[test]
    fn test_strategy_display() {
        assert_eq!(format!("{}", PrefetchStrategy::NoPrefetch), "no-prefetch");
        assert_eq!(format!("{}", PrefetchStrategy::Standard), "standard");
    }

    #[test]
    fn test_strategy_prefetch_threshold() {
        // NoPrefetch 阈值 > 1.0，永不触发
        assert!(PrefetchStrategy::NoPrefetch.prefetch_threshold() > 1.0);
        // Conservative 阈值 0.8
        assert!((PrefetchStrategy::Conservative.prefetch_threshold() - 0.8).abs() < 1e-6);
        // Standard 阈值 0.6（默认）
        assert!((PrefetchStrategy::Standard.prefetch_threshold() - 0.6).abs() < 1e-6);
        // Aggressive 阈值 0.3
        assert!((PrefetchStrategy::Aggressive.prefetch_threshold() - 0.3).abs() < 1e-6);
        // TopK3 阈值 0.0（无阈值）
        assert!((PrefetchStrategy::TopK3.prefetch_threshold() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_strategy_top_k() {
        assert_eq!(PrefetchStrategy::NoPrefetch.top_k(), 0);
        assert_eq!(PrefetchStrategy::Conservative.top_k(), 5);
        assert_eq!(PrefetchStrategy::Standard.top_k(), 10);
        assert_eq!(PrefetchStrategy::Aggressive.top_k(), 20);
        assert_eq!(PrefetchStrategy::TopK3.top_k(), 3);
    }

    #[test]
    fn test_strategy_disabled() {
        assert!(PrefetchStrategy::NoPrefetch.disabled());
        assert!(!PrefetchStrategy::Standard.disabled());
        assert!(!PrefetchStrategy::TopK3.disabled());
    }

    #[test]
    fn test_strategy_copy_semantics() {
        let s1 = PrefetchStrategy::Standard;
        let s2 = s1; // Copy
        assert_eq!(s1, s2); // s1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_strategy_equality() {
        assert_eq!(PrefetchStrategy::Standard, PrefetchStrategy::Standard);
        assert_ne!(PrefetchStrategy::NoPrefetch, PrefetchStrategy::Standard);
    }

    #[test]
    fn test_strategy_serialize_json() {
        let strategy = PrefetchStrategy::TopK3;
        let json = serde_json::to_string(&strategy).unwrap();
        let deserialized: PrefetchStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, deserialized);
    }

    #[test]
    fn test_strategy_serialize_yaml() {
        let strategy = PrefetchStrategy::Aggressive;
        let yaml = serde_yaml::to_string(&strategy).unwrap();
        let deserialized: PrefetchStrategy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(strategy, deserialized);
    }

    // ============================================================
    // PrefetchPolicy 单元测试
    // ============================================================

    #[test]
    fn test_policy_default_is_static_standard() {
        let policy = PrefetchPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
        assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
    }

    #[test]
    fn test_policy_default_version_none() {
        let policy = PrefetchPolicy::default();
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_policy_fallback_equals_default() {
        assert_eq!(PrefetchPolicy::fallback(), PrefetchPolicy::default());
    }

    #[test]
    fn test_policy_fallback_is_static() {
        let fallback = PrefetchPolicy::fallback();
        assert!(fallback.is_static());
        assert!(!fallback.is_learned());
        assert_eq!(fallback.strategy(), PrefetchStrategy::Standard);
    }

    #[test]
    fn test_policy_static_constructor() {
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::NoPrefetch);
        assert!(policy.is_static());
        assert_eq!(policy.version(), None);
        assert_eq!(policy.strategy(), PrefetchStrategy::NoPrefetch);
    }

    #[test]
    fn test_policy_learned_constructor() {
        let policy = PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), PrefetchStrategy::Aggressive);
    }

    #[test]
    fn test_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let policy = PrefetchPolicy::learned(0, PrefetchStrategy::NoPrefetch);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_policy_strategy_static() {
        let policy = PrefetchPolicy::Static(PrefetchStrategy::TopK3);
        assert_eq!(policy.strategy(), PrefetchStrategy::TopK3);
    }

    #[test]
    fn test_policy_strategy_learned() {
        let policy = PrefetchPolicy::Learned {
            version: 1,
            strategy: PrefetchStrategy::Conservative,
        };
        assert_eq!(policy.strategy(), PrefetchStrategy::Conservative);
    }

    #[test]
    fn test_policy_equality_static() {
        let p1 = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
        let p2 = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_equality_learned() {
        let p1 = PrefetchPolicy::learned(1, PrefetchStrategy::Aggressive);
        let p2 = PrefetchPolicy::learned(1, PrefetchStrategy::Aggressive);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_version() {
        let p1 = PrefetchPolicy::learned(1, PrefetchStrategy::Standard);
        let p2 = PrefetchPolicy::learned(2, PrefetchStrategy::Standard);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_strategy() {
        let p1 = PrefetchPolicy::learned(1, PrefetchStrategy::NoPrefetch);
        let p2 = PrefetchPolicy::learned(1, PrefetchStrategy::Standard);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_static_vs_learned() {
        let p1 = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
        let p2 = PrefetchPolicy::learned(1, PrefetchStrategy::Standard);
        assert_ne!(p1, p2); // 不同变体
    }

    #[test]
    fn test_policy_copy_semantics() {
        let policy = PrefetchPolicy::learned(42, PrefetchStrategy::TopK3);
        let copied = policy; // Copy
        assert_eq!(policy, copied); // policy 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_policy_serialize_json_static() {
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Aggressive);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: PrefetchPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_json_learned() {
        let policy = PrefetchPolicy::learned(42, PrefetchStrategy::TopK3);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: PrefetchPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_static() {
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Conservative);
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: PrefetchPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_yaml_learned() {
        let policy = PrefetchPolicy::learned(7, PrefetchStrategy::NoPrefetch);
        let yaml = serde_yaml::to_string(&policy).unwrap();
        let deserialized: PrefetchPolicy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(policy, deserialized);
    }

    // ============================================================
    // D1 修复场景测试（S3 接缝 — 预取策略外置）
    // ============================================================

    #[test]
    fn test_d1_scenario_static_fallback_compiled_into_binary() {
        // spec.md "默认静态值 = 当前常量，fallback 编译进同一二进制"
        let policy = PrefetchPolicy::default();
        assert!(policy.is_static());
        // 验证 fallback 值 = 既有 scc-cache 行为（Standard, threshold=0.6）
        assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
        assert!((policy.strategy().prefetch_threshold() - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_d1_scenario_learner_panic_local_fallback() {
        // 模拟: omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        let learned = PrefetchPolicy::learned(1, PrefetchStrategy::TopK3);
        assert!(learned.is_learned());

        // learner panic → 本地 fallback
        let fallback = PrefetchPolicy::fallback();
        assert!(fallback.is_static());
        assert_ne!(fallback.strategy(), learned.strategy());
    }

    #[test]
    fn test_d1_scenario_no_cross_crate_flag() {
        // spec.md "无跨 crate 旗标"
        // PrefetchPolicy 通过值注入（Copy），不依赖全局 static 或 feature flag
        let policy = PrefetchPolicy::default();
        let strategy = policy.strategy();
        // 策略值直接从 const 常量获取，无运行时旗标查询
        assert_eq!(strategy, PrefetchStrategy::Standard);
    }

    #[test]
    fn test_d1_scenario_learned_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let v1 = PrefetchPolicy::learned(1, PrefetchStrategy::Standard);
        let v2 = PrefetchPolicy::learned(2, PrefetchStrategy::Aggressive);
        assert_ne!(v1.version(), v2.version());
        assert_ne!(v1.strategy(), v2.strategy());
    }

    // ============================================================
    // 策略参数语义测试（验证策略 → 行为映射正确性）
    // ============================================================

    #[test]
    fn test_strategy_no_prefetch_never_triggers() {
        // NoPrefetch 阈值 > 1.0，任何概率（≤1.0）都不满足
        let strategy = PrefetchStrategy::NoPrefetch;
        let threshold = strategy.prefetch_threshold();
        let top_k = strategy.top_k();

        assert!(threshold > 1.0, "NoPrefetch 阈值应 > 1.0");
        assert_eq!(top_k, 0, "NoPrefetch top_k 应为 0");

        // 概率 1.0（最大可能）也不应满足阈值
        assert!(1.0f32 < threshold);
        // 即便阈值满足，top_k=0 也意味着无预取
        assert_eq!(top_k, 0);
    }

    #[test]
    fn test_strategy_topk3_no_threshold_limit() {
        // TopK3 阈值 0.0，所有概率都满足，仅受 top_k=3 限制
        let strategy = PrefetchStrategy::TopK3;
        let threshold = strategy.prefetch_threshold();
        let top_k = strategy.top_k();

        assert_eq!(threshold, 0.0);
        assert_eq!(top_k, 3);

        // 概率 0.01 也满足阈值
        assert!(0.01f32 >= threshold);
    }

    #[test]
    fn test_strategy_aggressive_low_threshold() {
        // Aggressive 阈值 0.3，允许较低概率预取
        let strategy = PrefetchStrategy::Aggressive;
        assert!((strategy.prefetch_threshold() - 0.3).abs() < 1e-6);
        assert_eq!(strategy.top_k(), 20);
    }

    #[test]
    fn test_strategy_conservative_high_threshold() {
        // Conservative 阈值 0.8，仅高概率预取
        let strategy = PrefetchStrategy::Conservative;
        assert!((strategy.prefetch_threshold() - 0.8).abs() < 1e-6);
        assert_eq!(strategy.top_k(), 5);
    }

    #[test]
    fn test_all_strategies_threshold_monotonic_except_topk3() {
        // NoPrefetch > Conservative > Standard > Aggressive > TopK3
        // (TopK3 = 0.0 是特殊情况，无阈值过滤)
        let strategies = [
            PrefetchStrategy::NoPrefetch,
            PrefetchStrategy::Conservative,
            PrefetchStrategy::Standard,
            PrefetchStrategy::Aggressive,
            PrefetchStrategy::TopK3,
        ];
        for window in strategies.windows(2) {
            let a = window[0].prefetch_threshold();
            let b = window[1].prefetch_threshold();
            assert!(
                a > b,
                "阈值应单调递减: {} ({}) > {} ({})",
                window[0].short_name(),
                a,
                window[1].short_name(),
                b
            );
        }
    }
}
