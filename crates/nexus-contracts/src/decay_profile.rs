//! 衰减参数策略契约 — S6 接缝（decay-engine DecayProfile）类型定义
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应任务: **P4-W14.4.1**（S6 接缝上下文/臂/奖励定义）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 六接缝表 S6
//!
//! # 核心职责
//!
//! 承载 decay-engine 衰减路径的"衰减参数档位"，将其从硬编码默认值
//! （`DecayConfig::default()` 单一固定配置）升级为可注入策略，为 P4 `omega-learner`
//! Bandit 异步下发学习值奠基。对应三重悖论中"进化悖论"的修复路径——
//! 通过 LinUCB 学习上下文特征 → 衰减参数档位映射，避免验证器层级 L3（执行反馈）
//! 被游戏化（误拦率 vs 漏拦率的加权奖励驱动参数自适应）。
//!
//! # 四种衰减档位（臂集）
//!
//! | 档位 | 语义 | time_decay_rate | event_decay_penalty | freeze_threshold | 适用场景 |
//! |------|------|------------------|---------------------|-------------------|---------|
//! | `Lenient` | 宽松衰减 | 0.0005 | 0.05 | 0.02 | 低风险只读操作 |
//! | `Standard` | 标准衰减（默认 fallback） | 0.001 | 0.1 | 0.05 | 通用默认 |
//! | `Strict` | 严格衰减 | 0.005 | 0.15 | 0.10 | 高风险写操作 |
//! | `Aggressive` | 激进衰减 | 0.01 | 0.2 | 0.15 | 密集违规场景 |
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `DecayPolicy::default()` 返回 `Static(DecayProfile::Standard)`，
//! `Standard` 是 `const` 常量编译进二进制。`omega-learner` panic/超时时，
//! 调用方本地 fallback 到 `DecayPolicy::Static`，**无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 与 PrefetchPolicy / ParliamentPolicy / MemoryStrategyPolicy 的对称设计
//!
//! 五接缝（S1/S2/S3/S5/S6）共用相同骨架：
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
//! use nexus_contracts::{DecayPolicy, DecayProfile};
//!
//! let policy = DecayPolicy::default();
//! assert!(policy.is_static());
//! assert_eq!(policy.profile(), DecayProfile::Standard);
//! assert_eq!(policy.version(), None);
//! ```
//!
//! ## 学习策略（omega-learner 异步下发）
//!
//! ```
//! use nexus_contracts::{DecayPolicy, DecayProfile};
//!
//! let learned = DecayPolicy::learned(42, DecayProfile::Strict);
//! assert!(learned.is_learned());
//! assert_eq!(learned.version(), Some(42));
//! assert_eq!(learned.profile(), DecayProfile::Strict);
//! ```
//!
//! ## learner panic 时本地 fallback
//!
//! ```
//! use nexus_contracts::{DecayPolicy, DecayProfile};
//!
//! let learned = DecayPolicy::learned(1, DecayProfile::Aggressive);
//! let fallback = DecayPolicy::fallback();
//! assert!(fallback.is_static());
//! assert_eq!(fallback.profile(), DecayProfile::Standard);
//! ```

use serde::{Deserialize, Serialize};

/// 衰减参数档位 — 四种衰减行为（S6 接缝臂集）
///
/// 决定 decay-engine `decay()` 路径的具体参数：
/// - `time_decay_rate`: 时间驱动衰减速率（每秒衰减比例）
/// - `event_decay_penalty`: 违规事件惩罚基数
/// - `freeze_threshold`: 自动冻结阈值
/// - `restore_rate`: 恢复速率
///
/// # 设计决策（WHY）
///
/// - **枚举而非字符串**: 4 种档位有限且固定，枚举提供编译期穷尽性检查
/// - **Copy + Clone**: 枚举为单元变体，Copy 语义零成本
/// - **`Standard` 默认**: 对应既有 `DecayConfig::default()` 行为，向后兼容
/// - **不暴露具体参数**: 调用方通过 `to_config()` 获取 `DecayConfig`，
///   避免调用方手动拼装参数导致漂移
///
/// # 与三重悖论"进化悖论"的修复关系
///
/// 三重悖论病理：验证器层级 L3（执行反馈）的"测试通过/失败"信号可被游戏化，
/// decay-engine 的"误拦率 vs 漏拦率"权衡是典型的"验证器边界"权衡：
/// - 误拦（false_block）：合法操作被错误冻结（生产力损失）
/// - 漏拦（false_pass）：违规操作未被冻结（安全风险）
///
/// S6 接缝通过 LinUCB 学习上下文特征 → 档位映射，使衰减强度随场景自适应，
/// 避免 Lenient 时违规操作漏拦（安全风险）与 Aggressive 时合法操作误拦
/// （生产力损失）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum DecayProfile {
    /// 宽松衰减 — time_decay_rate=0.0005, event_decay_penalty=0.05, freeze_threshold=0.02
    ///
    /// WHY 适用低风险只读: 只读操作误拦代价高（生产力损失），
    /// 阈值低（0.02）允许权限较低时仍可操作，慢衰减避免权限快速流失。
    Lenient = 1,

    /// 标准衰减 — time_decay_rate=0.001, event_decay_penalty=0.1, freeze_threshold=0.05（默认 fallback）
    ///
    /// WHY 通用默认: 平衡误拦与漏拦，对应既有 decay-engine 行为。
    Standard = 2,

    /// 严格衰减 — time_decay_rate=0.005, event_decay_penalty=0.15, freeze_threshold=0.10
    ///
    /// WHY 适用高风险写: 写操作误拦代价低（重新尝试），
    /// 阈值高（0.10）确保权限下降到危险水平时立即冻结。
    Strict = 3,

    /// 激进衰减 — time_decay_rate=0.01, event_decay_penalty=0.2, freeze_threshold=0.15
    ///
    /// WHY 适用密集违规: 违规频发时需要快速收敛权限，
    /// 高 time_decay_rate + 高 event_decay_penalty + 高 freeze_threshold 联合作用。
    Aggressive = 4,
}

impl DecayProfile {
    /// 所有衰减档位（按枚举值升序，便于遍历初始化 LinUCB 臂集）
    pub const ALL: [Self; 4] = [
        Self::Lenient,
        Self::Standard,
        Self::Strict,
        Self::Aggressive,
    ];

    /// 返回臂数（4 个档位对应 LinUCB 4 臂）
    pub const ARM_COUNT: usize = 4;

    /// 返回档位简称（用于日志/调试与 ArmId 构造）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Lenient => "lenient",
            Self::Standard => "standard",
            Self::Strict => "strict",
            Self::Aggressive => "aggressive",
        }
    }

    /// 返回时间驱动衰减速率（每秒衰减比例）
    ///
    /// - `Lenient` = 0.0005: 慢衰减（5 分钟衰减约 13.5%）
    /// - `Standard` = 0.001: 默认（5 分钟衰减约 26%）
    /// - `Strict` = 0.005: 快衰减（5 分钟衰减约 78%）
    /// - `Aggressive` = 0.01: 极快（5 分钟衰减约 95%）
    pub const fn time_decay_rate(self) -> f32 {
        match self {
            Self::Lenient => 0.0005,
            Self::Standard => 0.001,
            Self::Strict => 0.005,
            Self::Aggressive => 0.01,
        }
    }

    /// 返回违规事件衰减惩罚基数
    ///
    /// 实际惩罚 = penalty × severity
    pub const fn event_decay_penalty(self) -> f32 {
        match self {
            Self::Lenient => 0.05,
            Self::Standard => 0.1,
            Self::Strict => 0.15,
            Self::Aggressive => 0.2,
        }
    }

    /// 返回自动冻结阈值（低于此值自动冻结）
    pub const fn freeze_threshold(self) -> f32 {
        match self {
            Self::Lenient => 0.02,
            Self::Standard => 0.05,
            Self::Strict => 0.10,
            Self::Aggressive => 0.15,
        }
    }

    /// 返回恢复速率（每秒恢复比例）
    ///
    /// WHY 所有档位相同: 恢复速率主要影响解冻后回到正常水平的速度，
    /// 与档位的"严格度"无强关联，统一 0.01（5 分钟恢复约 26%）。
    pub const fn restore_rate(self) -> f32 {
        0.01
    }

    /// 返回最低权限下限（衰减不会低于此值，除非冻结）
    ///
    /// WHY 所有档位为 0.0: 与既有 `DecayConfig::default()` 一致，
    /// 允许衰减到 0（但 freeze_threshold 会先触发自动冻结）。
    pub const fn min_level(self) -> f32 {
        0.0
    }
}

impl Default for DecayProfile {
    /// 默认档位 = `Standard`（向后兼容 `DecayConfig::default()` 行为）
    ///
    /// WHY(C4 合规): 默认值 = 既有 decay-engine 行为，fallback 编译进二进制。
    /// `omega-learner` 未下发学习策略时，行为与 P4 修复前完全一致（向后兼容）。
    fn default() -> Self {
        Self::Standard
    }
}

impl std::fmt::Display for DecayProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// 衰减参数策略 — 静态常量或学习版本（S6 接缝）
///
/// 承载 decay-engine 衰减路径的策略，将其从硬编码默认值
/// （`DecayConfig::default()` 单一固定配置）升级为可注入策略，
/// 为 `omega-learner` Bandit 异步下发学习值奠基。
///
/// # 变体
/// - [`Static`](DecayPolicy::Static): 编译进二进制的常量（fallback，C4 合规）
/// - [`Learned`](DecayPolicy::Learned): `omega-learner` 异步下发的版本化策略
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// `default()` 返回 `Static(DecayProfile::Standard)`，
/// `Standard` 是 `const` 常量。`omega-learner` panic/超时时，
/// 调用方本地 fallback 到 `Static`，无跨 crate 旗标传播。
///
/// # 设计决策（WHY）
///
/// - **枚举而非 trait**: 策略变体有限（Static/Learned），枚举比 trait object 更轻量
/// - **Copy 语义**: `DecayProfile` 为 Copy，枚举整体 Copy，注入时零成本
/// - **版本号 `Option<u64>`**: Learned 携带版本号用于 A/B 测试与回滚
/// - **与 PrefetchPolicy / ParliamentPolicy 对称设计**: 复用相同模式降低学习成本
///
/// # 示例
///
/// ## 静态策略（默认 fallback）
///
/// ```
/// use nexus_contracts::{DecayPolicy, DecayProfile};
///
/// let policy = DecayPolicy::default();
/// assert!(policy.is_static());
/// assert_eq!(policy.profile(), DecayProfile::Standard);
/// ```
///
/// ## 学习策略（omega-learner 异步下发）
///
/// ```
/// use nexus_contracts::{DecayPolicy, DecayProfile};
///
/// let policy = DecayPolicy::learned(42, DecayProfile::Strict);
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DecayPolicy {
    /// 静态策略 — 编译进二进制的常量（fallback，C4 合规）
    ///
    /// 承载 `DecayProfile` 常量，`default()` 返回
    /// `Static(DecayProfile::Standard)`。
    /// `omega-learner` panic/超时时调用方本地 fallback 到此变体。
    Static(DecayProfile),

    /// 学习策略 — `omega-learner` 异步下发的版本化策略
    ///
    /// `version` 单调递增，用于 A/B 测试与回滚（P4 `SpecRegistry` 版本化）。
    /// `profile` 承载学习到的档位，由 `omega-learner` Bandit 算法计算。
    Learned {
        /// 学习版本号（单调递增，用于 A/B 测试与回滚）
        version: u64,
        /// 学习到的衰减档位
        profile: DecayProfile,
    },
}

impl DecayPolicy {
    /// 创建静态策略（便捷构造函数）
    pub const fn static_policy(profile: DecayProfile) -> Self {
        Self::Static(profile)
    }

    /// 创建学习策略（便捷构造函数）
    pub const fn learned(version: u64, profile: DecayProfile) -> Self {
        Self::Learned { version, profile }
    }

    /// 返回 fallback 策略 — `Static(DecayProfile::Standard)`
    ///
    /// WHY(C4 合规): `omega-learner` panic/超时时调用方本地 fallback 到此值。
    /// `Standard` 是 `const` 常量编译进二进制，非运行时 feature flag。
    pub const fn fallback() -> Self {
        Self::Static(DecayProfile::Standard)
    }

    /// 返回当前策略的衰减档位（无论 Static 还是 Learned）
    ///
    /// 统一访问方法，调用方无需 match 策略变体即可获取档位。
    pub const fn profile(&self) -> DecayProfile {
        match self {
            Self::Static(p) => *p,
            Self::Learned { profile, .. } => *profile,
        }
    }

    /// 返回学习版本号（Static 返回 None，Learned 返回 Some(version)）
    ///
    /// 便于上层编排器记录使用的版本号用于效果追踪与 A/B 测试。
    pub const fn version(&self) -> Option<u64> {
        match self {
            Self::Static(_) => None,
            Self::Learned { version, .. } => Some(*version),
        }
    }

    /// 返回是否为静态策略（fallback）
    pub const fn is_static(&self) -> bool {
        matches!(self, Self::Static(_))
    }

    /// 返回是否为学习策略
    pub const fn is_learned(&self) -> bool {
        matches!(self, Self::Learned { .. })
    }
}

impl Default for DecayPolicy {
    /// 默认策略 = `Static(DecayProfile::Standard)`（fallback，C4 合规）
    ///
    /// WHY 与 `DecayProfile::default()` 一致: 保持向后兼容，
    /// `omega-learner` 未注入时行为与 P4 修复前完全一致。
    fn default() -> Self {
        Self::Static(DecayProfile::Standard)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // DecayProfile 测试
    // ============================================================

    #[test]
    fn test_decay_profile_all_count() {
        assert_eq!(DecayProfile::ALL.len(), 4);
        assert_eq!(DecayProfile::ARM_COUNT, 4);
    }

    #[test]
    fn test_decay_profile_short_names() {
        assert_eq!(DecayProfile::Lenient.short_name(), "lenient");
        assert_eq!(DecayProfile::Standard.short_name(), "standard");
        assert_eq!(DecayProfile::Strict.short_name(), "strict");
        assert_eq!(DecayProfile::Aggressive.short_name(), "aggressive");
    }

    #[test]
    fn test_decay_profile_display() {
        assert_eq!(format!("{}", DecayProfile::Lenient), "lenient");
        assert_eq!(format!("{}", DecayProfile::Standard), "standard");
        assert_eq!(format!("{}", DecayProfile::Strict), "strict");
        assert_eq!(format!("{}", DecayProfile::Aggressive), "aggressive");
    }

    #[test]
    fn test_decay_profile_default_is_standard() {
        // C4 合规: 默认 = Standard = 既有 decay-engine 行为
        assert_eq!(DecayProfile::default(), DecayProfile::Standard);
    }

    // ============================================================
    // DecayProfile 参数表测试
    // ============================================================

    #[test]
    fn test_time_decay_rate_ordering() {
        // 严格递增: Lenient < Standard < Strict < Aggressive
        let lenient = DecayProfile::Lenient.time_decay_rate();
        let standard = DecayProfile::Standard.time_decay_rate();
        let strict = DecayProfile::Strict.time_decay_rate();
        let aggressive = DecayProfile::Aggressive.time_decay_rate();

        assert!(lenient < standard, "Lenient 应比 Standard 慢");
        assert!(standard < strict, "Standard 应比 Strict 慢");
        assert!(strict < aggressive, "Strict 应比 Aggressive 慢");
    }

    #[test]
    fn test_event_decay_penalty_ordering() {
        // 严格递增
        assert!(
            DecayProfile::Lenient.event_decay_penalty()
                < DecayProfile::Standard.event_decay_penalty()
        );
        assert!(
            DecayProfile::Standard.event_decay_penalty()
                < DecayProfile::Strict.event_decay_penalty()
        );
        assert!(
            DecayProfile::Strict.event_decay_penalty()
                < DecayProfile::Aggressive.event_decay_penalty()
        );
    }

    #[test]
    fn test_freeze_threshold_ordering() {
        // 严格递增
        assert!(
            DecayProfile::Lenient.freeze_threshold() < DecayProfile::Standard.freeze_threshold()
        );
        assert!(
            DecayProfile::Standard.freeze_threshold() < DecayProfile::Strict.freeze_threshold()
        );
        assert!(
            DecayProfile::Strict.freeze_threshold() < DecayProfile::Aggressive.freeze_threshold()
        );
    }

    #[test]
    fn test_standard_matches_default_config() {
        // C4 合规: Standard 档位参数必须与既有 DecayConfig::default() 一致
        // 避免引入 P4 修复前的行为漂移
        assert!((DecayProfile::Standard.time_decay_rate() - 0.001).abs() < 1e-6);
        assert!((DecayProfile::Standard.event_decay_penalty() - 0.1).abs() < 1e-6);
        assert!((DecayProfile::Standard.freeze_threshold() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_restore_rate_constant_across_profiles() {
        // 恢复速率在所有档位一致
        let lenient = DecayProfile::Lenient.restore_rate();
        let standard = DecayProfile::Standard.restore_rate();
        let strict = DecayProfile::Strict.restore_rate();
        let aggressive = DecayProfile::Aggressive.restore_rate();

        assert!((lenient - standard).abs() < 1e-6);
        assert!((standard - strict).abs() < 1e-6);
        assert!((strict - aggressive).abs() < 1e-6);
        // 与既有 DecayConfig::default() 一致
        assert!((standard - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_min_level_zero_for_all() {
        for profile in DecayProfile::ALL.iter() {
            assert!(
                profile.min_level().abs() < 1e-6,
                "{:?} min_level 应为 0",
                profile
            );
        }
    }

    // ============================================================
    // DecayPolicy 测试
    // ============================================================

    #[test]
    fn test_decay_policy_default_is_static_standard() {
        // C4 合规: default = Static(Standard)
        let policy = DecayPolicy::default();
        assert!(policy.is_static());
        assert!(!policy.is_learned());
        assert_eq!(policy.profile(), DecayProfile::Standard);
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_decay_policy_fallback_equals_default() {
        // fallback 与 default 行为一致
        assert_eq!(DecayPolicy::fallback(), DecayPolicy::default());
    }

    #[test]
    fn test_decay_policy_static_policy() {
        let policy = DecayPolicy::static_policy(DecayProfile::Strict);
        assert!(policy.is_static());
        assert_eq!(policy.profile(), DecayProfile::Strict);
        assert_eq!(policy.version(), None);
    }

    #[test]
    fn test_decay_policy_learned() {
        let policy = DecayPolicy::learned(42, DecayProfile::Aggressive);
        assert!(policy.is_learned());
        assert!(!policy.is_static());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.profile(), DecayProfile::Aggressive);
    }

    #[test]
    fn test_decay_policy_learned_version_zero() {
        // 版本号 0 是合法值（首个学习版本）
        let policy = DecayPolicy::learned(0, DecayProfile::Lenient);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_decay_policy_equality() {
        // 静态策略相等性
        assert_eq!(
            DecayPolicy::static_policy(DecayProfile::Standard),
            DecayPolicy::static_policy(DecayProfile::Standard)
        );
        assert_ne!(
            DecayPolicy::static_policy(DecayProfile::Standard),
            DecayPolicy::static_policy(DecayProfile::Strict)
        );

        // 学习策略相等性
        assert_eq!(
            DecayPolicy::learned(1, DecayProfile::Standard),
            DecayPolicy::learned(1, DecayProfile::Standard)
        );
        assert_ne!(
            DecayPolicy::learned(1, DecayProfile::Standard),
            DecayPolicy::learned(2, DecayProfile::Standard)
        );
        assert_ne!(
            DecayPolicy::learned(1, DecayProfile::Standard),
            DecayPolicy::learned(1, DecayProfile::Strict)
        );
    }

    #[test]
    fn test_decay_policy_c4_compliance_no_runtime_flag() {
        // C4 合规: 策略通过值注入（Copy），不依赖全局 static 或 feature flag
        let policy = DecayPolicy::default();
        let profile = policy.profile();
        // 直接从 const 常量获取参数，无运行时旗标查询
        assert_eq!(profile, DecayProfile::Standard);
    }

    #[test]
    fn test_decay_policy_c4_local_fallback() {
        // C4 合规: learner panic 时调用方本地 fallback 到 Static(Standard)
        let learned = DecayPolicy::learned(1, DecayProfile::Aggressive);
        assert!(learned.is_learned());

        // 模拟 learner panic: 调用 fallback
        let fallback = DecayPolicy::fallback();
        assert!(fallback.is_static());
        assert_eq!(fallback.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_decay_policy_versioned_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let v1 = DecayPolicy::learned(1, DecayProfile::Strict);
        let v2 = DecayPolicy::learned(2, DecayProfile::Aggressive);
        assert_ne!(v1.version(), v2.version());
        assert_ne!(v1.profile(), v2.profile());
    }

    #[test]
    fn test_all_profiles_covered_in_arm_set() {
        // S6 臂集 = DecayProfile::ALL（4 臂）
        for profile in DecayProfile::ALL.iter() {
            // 验证每个档位都能构造对应策略
            let policy = DecayPolicy::static_policy(*profile);
            assert_eq!(policy.profile(), *profile);
        }
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_scenario_lifecycle_learned_to_fallback() {
        // 模拟完整生命周期: Static → Learned → 熔断 → Static
        let mut policy = DecayPolicy::default();
        assert!(!policy.is_learned());

        // 1. omega-learner 下发 Learned(v=1, Strict)
        policy = DecayPolicy::learned(1, DecayProfile::Strict);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(1));
        assert_eq!(policy.profile(), DecayProfile::Strict);

        // 2. 继续学习 v=2, 切到 Aggressive
        policy = DecayPolicy::learned(2, DecayProfile::Aggressive);
        assert_eq!(policy.version(), Some(2));
        assert_eq!(policy.profile(), DecayProfile::Aggressive);

        // 3. 灰度指标不达标，触发熔断
        policy = DecayPolicy::fallback();
        assert!(!policy.is_learned());
        assert_eq!(policy.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_scenario_alternating_static_and_learned() {
        // 交替更新 Static 与 Learned
        let policies = [
            DecayPolicy::static_policy(DecayProfile::Standard),
            DecayPolicy::learned(1, DecayProfile::Strict),
            DecayPolicy::static_policy(DecayProfile::Lenient),
            DecayPolicy::learned(2, DecayProfile::Aggressive),
            DecayPolicy::fallback(),
        ];

        let mut last_version = None;
        for policy in policies.iter() {
            if policy.is_learned() {
                let v = policy.version().unwrap();
                if let Some(prev) = last_version {
                    assert!(v > prev, "学习版本号应单调递增");
                }
                last_version = Some(v);
            }
        }
    }
}
