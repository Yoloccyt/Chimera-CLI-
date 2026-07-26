//! Parliament 激活策略契约 — S5 接缝（Parliament 激活）类型定义
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P4-W14.3.1**（S5 接缝上下文/臂/奖励定义）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 六接缝表 S5
//!
//! # 核心职责
//!
//! 承载 Parliament 辩论激活路径的"激活策略"参数，将其从硬编码默认值
//! （`deliberate()` 总是走完整辩论）升级为可注入策略，为 P4 `omega-learner`
//! Bandit 异步下发学习值奠基。对应三重悖论中"推理悖论"的修复路径——
//! 通过 LinUCB 学习上下文特征 → 激活策略映射，避免辩论成本超过推理增益。
//!
//! # 三种激活策略（臂集）
//!
//! | 策略 | 语义 | 辩论成本 | 适用场景 |
//! |------|------|---------|---------|
//! | `FastPath` | 跳过辩论 | 0.0 | 低风险 + 只读操作 + 历史推翻率低 |
//! | `Simplified` | 精简辩论 | 0.3 | 中等风险或不确定场景 |
//! | `Full` | 完整辩论 | 1.0 | 高风险 + 写操作 + 历史推翻率高 |
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `ParliamentPolicy::default()` 返回 `Static(ActivationStrategy::Full)`，
//! `Full` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `ParliamentPolicy::Static`，**无跨 crate 旗标传播**
//! （spec.md:334）。
//!
//! # 与 PrefetchPolicy / MemoryStrategyPolicy / DensityPolicy 的对称设计
//!
//! 四接缝（S1/S2/S3/S5）共用相同骨架：
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
//! use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
//!
//! let policy = ParliamentPolicy::default();
//! assert!(policy.is_static());
//! assert_eq!(policy.strategy(), ActivationStrategy::Full);
//! assert_eq!(policy.version(), None);
//! ```
//!
//! ## 学习策略（omega-learner 异步下发）
//!
//! ```
//! use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
//!
//! let learned = ParliamentPolicy::learned(42, ActivationStrategy::FastPath);
//! assert!(learned.is_learned());
//! assert_eq!(learned.version(), Some(42));
//! assert_eq!(learned.strategy(), ActivationStrategy::FastPath);
//! ```
//!
//! ## learner panic 时本地 fallback
//!
//! ```
//! use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
//!
//! let learned = ParliamentPolicy::learned(1, ActivationStrategy::FastPath);
//! let fallback = ParliamentPolicy::fallback();
//! assert!(fallback.is_static());
//! assert_eq!(fallback.strategy(), ActivationStrategy::Full);
//! ```

use serde::{Deserialize, Serialize};

/// Parliament 激活策略 — 三种辩论强度（S5 接缝臂集）
///
/// 决定 Parliament `deliberate` 路径的行为：
/// - `FastPath`: 跳过辩论，直接返回共识（低风险场景）
/// - `Simplified`: 精简辩论，仅关键角色投票（中等风险场景）
/// - `Full`: 完整辩论，所有 5 角色投票 + Skeptic 否决权（高风险场景）
///
/// # 设计决策（WHY）
/// - **枚举而非字符串**: 3 种策略有限且固定，枚举提供编译期穷尽性检查
/// - **Copy + Clone**: 枚举为单元变体，Copy 语义零成本
/// - **`Full` 默认**: 对应既有 `deliberate()` 行为（5 角色完整辩论 + 否决），向后兼容
/// - **辩论成本常量**: 每个策略携带归一化成本（0.0/0.3/1.0），用于奖励函数
///
/// # 与三重悖论"推理悖论"的修复关系
///
/// 三重悖论病理：10 层架构的跨层协调成本存在阈值，当协调成本超过推理增益时
/// 多 Agent 反而不如单 Agent。Parliament 辩论是典型的"协调成本 vs 推理增益"权衡：
/// - 辩论消耗 CPU/内存/时间资源（协调成本）
/// - 辩论可避免高风险提案通过（推理增益）
///
/// S5 接缝通过 LinUCB 学习上下文特征 → 策略映射，使辩论强度随场景自适应，
/// 避免 FastPath 时高风险提案误通过（推理增益损失）与 Full 时
/// 辩论成本过大（协调成本超载）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActivationStrategy {
    /// 跳过辩论 — cost=0.0（直接返回共识，无角色投票）
    ///
    /// WHY 适用低风险只读: 风险低 + 只读操作不会产生副作用，
    /// 辩论的推理增益小于其协调成本，直接返回 FastPath 共识更经济。
    FastPath = 1,

    /// 精简辩论 — cost=0.3（仅关键角色投票：Architect + Skeptic + Optimizer）
    ///
    /// WHY 适用中等风险: 风险中等时无需 5 角色完整辩论，
    /// 仅需架构师（架构合理性）+ 怀疑者（红队风险）+ 优化者（性能）三关键角色
    /// 投票即可达成共识，省去 Librarian + Bard 的开销。
    Simplified = 2,

    /// 完整辩论 — cost=1.0（5 角色完整辩论 + Skeptic 否决权，默认 fallback）
    ///
    /// WHY 适用高风险: 风险高时需要所有 5 角色视角全面审议，
    /// 任意角色（特别是 Skeptic）可否决提案，确保高风险提案经过充分审查。
    Full = 3,
}

impl ActivationStrategy {
    /// 所有激活策略（按枚举值升序，便于遍历初始化 LinUCB 臂集）
    pub const ALL: [Self; 3] = [Self::FastPath, Self::Simplified, Self::Full];

    /// 返回臂数（3 个策略对应 LinUCB 3 臂）
    pub const ARM_COUNT: usize = 3;

    /// 返回策略简称（用于日志/调试与 ArmId 构造）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::FastPath => "fast-path",
            Self::Simplified => "simplified",
            Self::Full => "full",
        }
    }

    /// 返回归一化辩论成本 ∈ [0.0, 1.0]
    ///
    /// 用于 S5 奖励函数 `reward = correctness_score − λ × cost`。
    /// - `FastPath` = 0.0：无辩论成本
    /// - `Simplified` = 0.3：3 角色投票成本
    /// - `Full` = 1.0：5 角色完整辩论成本
    pub const fn debate_cost(self) -> f32 {
        match self {
            Self::FastPath => 0.0,
            Self::Simplified => 0.3,
            Self::Full => 1.0,
        }
    }

    /// 返回参与投票的角色数（用于辩论成本细粒度计算）
    ///
    /// - `FastPath` = 0：无角色投票
    /// - `Simplified` = 3：Architect + Skeptic + Optimizer
    /// - `Full` = 5：全部 5 角色
    pub const fn voter_count(self) -> usize {
        match self {
            Self::FastPath => 0,
            Self::Simplified => 3,
            Self::Full => 5,
        }
    }

    /// 是否跳过辩论（FastPath 专用）
    pub const fn skipped(self) -> bool {
        matches!(self, Self::FastPath)
    }
}

impl Default for ActivationStrategy {
    /// 默认策略 = `Full`（向后兼容 `deliberate()` 5 角色完整辩论行为）
    ///
    /// WHY(C4 合规): 默认值 = 既有 Parliament 行为，fallback 编译进二进制。
    /// `omega-learner` 未下发学习策略时，行为与 P4 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Full
    }
}

impl std::fmt::Display for ActivationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// Parliament 激活策略策略 — 静态常量或学习版本（S5 接缝）
///
/// 承载 Parliament 辩论路径的策略，将其从硬编码默认值（完整辩论）
/// 升级为可注入策略，为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](ParliamentPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](ParliamentPolicy::Learned): `omega-learner` 异步下发的版本化策略
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(ActivationStrategy::Full)`，
/// `Full` 是 `const` 常量。`omega-learner` panic/超时时，
/// 调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量
/// - **Copy 语义**: `ActivationStrategy` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚
/// - **与 PrefetchPolicy / MemoryStrategyPolicy / DensityPolicy 对称设计**:
///   复用相同模式降低学习成本
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
/// ```
/// use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
///
/// let policy = ParliamentPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.strategy(), ActivationStrategy::Full);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
/// ```
/// use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
///
/// let policy = ParliamentPolicy::learned(42, ActivationStrategy::FastPath);
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ParliamentPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `ActivationStrategy` 常量，`default()` 返回
    /// `Static(ActivationStrategy::Full)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(ActivationStrategy),

    /// 学习策略 — `omega-learner` 异步下发的版本化策略
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `strategy` 承载学习到的策略值，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的激活策略
        strategy: ActivationStrategy,
    },
}

impl ParliamentPolicy {
    /// 创建静态策略（便捷构造函数）
    pub const fn static_policy(strategy: ActivationStrategy) -> Self {
        Self::Static(strategy)
    }

    /// 创建学习策略（便捷构造函数）
    pub const fn learned(version: u64, strategy: ActivationStrategy) -> Self {
        Self::Learned { version, strategy }
    }

    /// 返回 fallback 策略 — `Static(ActivationStrategy::Full)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `Full` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(ActivationStrategy::Full)
    }

    /// 返回当前策略的激活策略值（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取策略。
    pub const fn strategy(&self) -> ActivationStrategy {
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

impl Default for ParliamentPolicy {
    /// 默认策略 = `Static(ActivationStrategy::Full)`
    ///
    /// WHY(C4 合规): 默认值 = 既有 Parliament 行为（5 角色完整辩论），
    /// fallback 编译进二进制。调用方未注入策略时行为与 P4 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Static(ActivationStrategy::Full)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // ActivationStrategy 单元测试
    // ============================================================

    #[test]
    fn test_strategy_default_is_full() {
        let strategy = ActivationStrategy::default();
        assert_eq!(strategy, ActivationStrategy::Full);
    }

    #[test]
    fn test_strategy_all_returns_three() {
        let all = ActivationStrategy::ALL;
        assert_eq!(all.len(), 3);
        assert_eq!(ActivationStrategy::ARM_COUNT, 3);
    }

    #[test]
    fn test_strategy_all_unique() {
        let all = ActivationStrategy::ALL;
        let mut seen = std::collections::HashSet::new();
        for strategy in all.iter() {
            assert!(seen.insert(*strategy), "duplicate strategy: {strategy:?}");
        }
    }

    #[test]
    fn test_strategy_short_name() {
        assert_eq!(ActivationStrategy::FastPath.short_name(), "fast-path");
        assert_eq!(ActivationStrategy::Simplified.short_name(), "simplified");
        assert_eq!(ActivationStrategy::Full.short_name(), "full");
    }

    #[test]
    fn test_strategy_display() {
        assert_eq!(format!("{}", ActivationStrategy::FastPath), "fast-path");
        assert_eq!(format!("{}", ActivationStrategy::Full), "full");
    }

    #[test]
    fn test_strategy_debate_cost() {
        // FastPath 成本 0.0
        assert!(ActivationStrategy::FastPath.debate_cost().abs() < 1e-6);
        // Simplified 成本 0.3
        assert!((ActivationStrategy::Simplified.debate_cost() - 0.3).abs() < 1e-6);
        // Full 成本 1.0
        assert!((ActivationStrategy::Full.debate_cost() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_strategy_voter_count() {
        assert_eq!(ActivationStrategy::FastPath.voter_count(), 0);
        assert_eq!(ActivationStrategy::Simplified.voter_count(), 3);
        assert_eq!(ActivationStrategy::Full.voter_count(), 5);
    }

    #[test]
    fn test_strategy_skipped() {
        assert!(ActivationStrategy::FastPath.skipped());
        assert!(!ActivationStrategy::Simplified.skipped());
        assert!(!ActivationStrategy::Full.skipped());
    }

    #[test]
    fn test_strategy_copy_semantics() {
        let s1 = ActivationStrategy::Full;
        let s2 = s1; // Copy
        assert_eq!(s1, s2); // s1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_strategy_equality() {
        assert_eq!(ActivationStrategy::Full, ActivationStrategy::Full);
        assert_ne!(ActivationStrategy::FastPath, ActivationStrategy::Full);
    }

    #[test]
    fn test_strategy_serialize_json() {
        let strategy = ActivationStrategy::Simplified;
        let json = serde_json::to_string(&strategy).unwrap();
        let deserialized: ActivationStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, deserialized);
    }

    #[test]
    fn test_strategy_debate_cost_monotonic() {
        // FastPath (0.0) < Simplified (0.3) < Full (1.0)
        assert!(
            ActivationStrategy::FastPath.debate_cost()
                < ActivationStrategy::Simplified.debate_cost()
        );
        assert!(
            ActivationStrategy::Simplified.debate_cost() < ActivationStrategy::Full.debate_cost()
        );
    }

    // ============================================================
    // ParliamentPolicy 单元测试
    // ============================================================

    #[test]
    fn test_policy_default_is_static_full() {
        let policy = ParliamentPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
        assert_eq!(policy.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_policy_default_version_none() {
        let policy = ParliamentPolicy::default();
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_policy_fallback_equals_default() {
        assert_eq!(ParliamentPolicy::fallback(), ParliamentPolicy::default());
    }

    #[test]
    fn test_policy_fallback_is_static() {
        let fallback = ParliamentPolicy::fallback();
        assert!(fallback.is_static());
        assert!(!fallback.is_learned());
        assert_eq!(fallback.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_policy_static_constructor() {
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);
        assert!(policy.is_static());
        assert_eq!(policy.version(), None);
        assert_eq!(policy.strategy(), ActivationStrategy::FastPath);
    }

    #[test]
    fn test_policy_learned_constructor() {
        let policy = ParliamentPolicy::learned(42, ActivationStrategy::Simplified);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), ActivationStrategy::Simplified);
    }

    #[test]
    fn test_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let policy = ParliamentPolicy::learned(0, ActivationStrategy::FastPath);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_policy_strategy_static() {
        let policy = ParliamentPolicy::Static(ActivationStrategy::Simplified);
        assert_eq!(policy.strategy(), ActivationStrategy::Simplified);
    }

    #[test]
    fn test_policy_strategy_learned() {
        let policy = ParliamentPolicy::Learned {
            version: 1,
            strategy: ActivationStrategy::FastPath,
        };
        assert_eq!(policy.strategy(), ActivationStrategy::FastPath);
    }

    #[test]
    fn test_policy_equality_static() {
        let p1 = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        let p2 = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_equality_learned() {
        let p1 = ParliamentPolicy::learned(1, ActivationStrategy::Simplified);
        let p2 = ParliamentPolicy::learned(1, ActivationStrategy::Simplified);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_version() {
        let p1 = ParliamentPolicy::learned(1, ActivationStrategy::Full);
        let p2 = ParliamentPolicy::learned(2, ActivationStrategy::Full);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_different_strategy() {
        let p1 = ParliamentPolicy::learned(1, ActivationStrategy::FastPath);
        let p2 = ParliamentPolicy::learned(1, ActivationStrategy::Full);
        assert_ne!(p1, p2);
    }

    #[test]
    fn test_policy_inequality_static_vs_learned() {
        let p1 = ParliamentPolicy::static_policy(ActivationStrategy::Full);
        let p2 = ParliamentPolicy::learned(1, ActivationStrategy::Full);
        assert_ne!(p1, p2); // 不同变体
    }

    #[test]
    fn test_policy_copy_semantics() {
        let policy = ParliamentPolicy::learned(42, ActivationStrategy::Simplified);
        let copied = policy; // Copy
        assert_eq!(policy, copied); // policy 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_policy_serialize_json_static() {
        let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: ParliamentPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    #[test]
    fn test_policy_serialize_json_learned() {
        let policy = ParliamentPolicy::learned(42, ActivationStrategy::FastPath);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: ParliamentPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(policy, deserialized);
    }

    // ============================================================
    // D1 修复场景测试（S5 接缝 — 激活策略外置）
    // ============================================================

    #[test]
    fn test_d1_scenario_static_fallback_compiled_into_binary() {
        // spec.md "默认静态值 = 当前常量，fallback 编译进同一二进制"
        let policy = ParliamentPolicy::default();
        assert!(policy.is_static());
        // 验证 fallback 值 = 既有 Parliament 行为（Full, 5 角色完整辩论）
        assert_eq!(policy.strategy(), ActivationStrategy::Full);
        assert!((policy.strategy().debate_cost() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_d1_scenario_learner_panic_local_fallback() {
        // 模拟: omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        let learned = ParliamentPolicy::learned(1, ActivationStrategy::FastPath);
        assert!(learned.is_learned());

        // learner panic → 本地 fallback
        let fallback = ParliamentPolicy::fallback();
        assert!(fallback.is_static());
        assert_ne!(fallback.strategy(), learned.strategy());
    }

    #[test]
    fn test_d1_scenario_no_cross_crate_flag() {
        // spec.md "无跨 crate 旗标"
        // ParliamentPolicy 通过值注入（Copy），不依赖全局 static 或 feature flag
        let policy = ParliamentPolicy::default();
        let strategy = policy.strategy();
        // 策略值直接从 const 常量获取，无运行时旗标查询
        assert_eq!(strategy, ActivationStrategy::Full);
    }

    #[test]
    fn test_d1_scenario_learned_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let v1 = ParliamentPolicy::learned(1, ActivationStrategy::Full);
        let v2 = ParliamentPolicy::learned(2, ActivationStrategy::FastPath);
        assert_ne!(v1.version(), v2.version());
        assert_ne!(v1.strategy(), v2.strategy());
    }
}
