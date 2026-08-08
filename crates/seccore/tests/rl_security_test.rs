//! RLSecurityPolicy 可学习安全策略测试（Milestone D-2b，设计 §8.1 目标形态）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 D-2）：
//! RLSecurityPolicy —— 可学习安全策略（审计频率/沙箱严格度微调）。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决 + 安全绝对红线（§8.3）：
//! - RL 只能微调**审计频率**（非拦截决策），不能降低沙箱底线
//! - 奖励映射（§8.1 五档）仅作观测信号（RewardSignal::security_observation）
//! - 微调因子 clamp [0.5, 2.0]——任何情况下审计不会关闭/失控

#![forbid(unsafe_code)]

use nexus_contracts::reward::{security_event_reward, SecuritySeverity};
use seccore::rl_security::{
    BaseSecurityPolicy, RLSecurityPolicy, SandboxLevel, DEFAULT_ADJUSTMENT_LOWER_BOUND,
    DEFAULT_ADJUSTMENT_UPPER_BOUND,
};
use std::collections::HashMap;

/// 默认基础策略（规则式）
fn base_policy() -> BaseSecurityPolicy {
    BaseSecurityPolicy::default()
}

/// 带微调的安全策略：audit 任务 +0.5（宽松 1.5×）、recovery 任务 -0.5（收紧 0.5×）
fn adjusted_policy() -> RLSecurityPolicy {
    let mut adjustments = HashMap::new();
    adjustments.insert("audit".to_string(), 0.5);
    adjustments.insert("recovery".to_string(), -0.5);
    RLSecurityPolicy::new(base_policy(), adjustments)
}

/// 五档奖励映射精确值（§8.1）
#[test]
fn five_level_reward_mapping_exact() {
    assert!((security_event_reward(SecuritySeverity::Critical) - (-10.0)).abs() < 1e-6);
    assert!((security_event_reward(SecuritySeverity::High) - (-5.0)).abs() < 1e-6);
    assert!((security_event_reward(SecuritySeverity::Medium) - (-2.0)).abs() < 1e-6);
    assert!((security_event_reward(SecuritySeverity::Low) - (-0.5)).abs() < 1e-6);
    assert!((security_event_reward(SecuritySeverity::Info) - 0.1).abs() < 1e-6);
}

/// 审计频率随风险单调递减（风险越高 → 间隔越小 → 审计越频繁）
#[test]
fn audit_frequency_decreases_with_risk() {
    let policy = base_policy();
    let low = policy.audit_frequency_ms(0.1);
    let mid = policy.audit_frequency_ms(0.5);
    let high = policy.audit_frequency_ms(0.9);
    assert!(low > mid, "低风险间隔应大于中风险（{low} vs {mid}）");
    assert!(mid > high, "中风险间隔应大于高风险（{mid} vs {high}）");
}

/// 微调因子生效：+0.5 → 间隔 ×1.5；-0.5 → ×0.5
#[test]
fn adjustment_factor_scales_frequency() {
    let policy = adjusted_policy();
    let base = base_policy().audit_frequency_ms(0.5);
    let relaxed = policy.audit_frequency_ms(0.5, "audit");
    let tightened = policy.audit_frequency_ms(0.5, "recovery");
    assert!(
        (relaxed as f32 - base as f32 * 1.5).abs() < 1.0,
        "宽松任务间隔 ×1.5（{relaxed} vs {}）",
        base as f32 * 1.5
    );
    assert!(
        (tightened as f32 - base as f32 * 0.5).abs() < 1.0,
        "收紧任务间隔 ×0.5（{tightened} vs {}）",
        base as f32 * 0.5
    );
}

/// clamp：极端微调不越界（+3.0 → 2.0×；-3.0 → 0.5×）
#[test]
fn adjustment_clamped_to_bounds() {
    let mut adjustments = HashMap::new();
    adjustments.insert("fast".to_string(), 3.0);
    adjustments.insert("slow".to_string(), -3.0);
    let policy = RLSecurityPolicy::new(base_policy(), adjustments);
    let base = base_policy().audit_frequency_ms(0.5);
    let fast = policy.audit_frequency_ms(0.5, "fast");
    let slow = policy.audit_frequency_ms(0.5, "slow");
    assert!(
        (fast as f32 - base as f32 * DEFAULT_ADJUSTMENT_UPPER_BOUND).abs() < 1.0,
        "上限 clamp 2.0×"
    );
    assert!(
        (slow as f32 - base as f32 * DEFAULT_ADJUSTMENT_LOWER_BOUND).abs() < 1.0,
        "下限 clamp 0.5×（审计不会关闭）"
    );
}

/// 未知任务类型 → 无调整（1.0×）
#[test]
fn unknown_task_type_has_no_adjustment() {
    let policy = adjusted_policy();
    let base = base_policy().audit_frequency_ms(0.5);
    let unknown = policy.audit_frequency_ms(0.5, "no-such-task");
    assert_eq!(unknown, base, "未知任务类型不应用微调");
}

/// 沙箱严格度阈值映射
#[test]
fn sandbox_strictness_thresholds() {
    let policy = base_policy();
    assert_eq!(policy.sandbox_strictness(0.95), SandboxLevel::Maximum);
    assert_eq!(policy.sandbox_strictness(0.6), SandboxLevel::High);
    assert_eq!(policy.sandbox_strictness(0.3), SandboxLevel::Medium);
    assert_eq!(policy.sandbox_strictness(0.05), SandboxLevel::Low);
}

/// 安全底线不可降：RL 微调不影响沙箱严格度（Maximum/High 不可降低）
#[test]
fn sandbox_strictness_immune_to_rl_adjustment() {
    let mut adjustments = HashMap::new();
    adjustments.insert("any".to_string(), 3.0); // 极端宽松微调
    let policy = RLSecurityPolicy::new(base_policy(), adjustments);
    assert_eq!(
        policy.sandbox_strictness(0.95),
        SandboxLevel::Maximum,
        "Maximum 不可因 RL 微调降低"
    );
    assert_eq!(
        policy.sandbox_strictness(0.6),
        SandboxLevel::High,
        "High 不可因 RL 微调降低"
    );
}

/// 严格度排序（Maximum > High > Medium > Low）
#[test]
fn strictness_ordering() {
    assert!(SandboxLevel::Maximum.rank() > SandboxLevel::High.rank());
    assert!(SandboxLevel::High.rank() > SandboxLevel::Medium.rank());
    assert!(SandboxLevel::Medium.rank() > SandboxLevel::Low.rank());
}

/// 八维度奖励接入（D-2e）：安全奖励 × L4 权重 0.5（§17；仅观测语义）
#[test]
fn security_reward_scaled_by_layer_weight() {
    use nexus_contracts::reward::{reward_layer_weight, RewardLayer};
    assert!(
        (reward_layer_weight(RewardLayer::L4) - 0.5).abs() < 1e-6,
        "L4 权重 0.5"
    );
    let raw = security_event_reward(SecuritySeverity::Critical);
    let scaled = raw * reward_layer_weight(RewardLayer::L4);
    assert!((scaled - (-5.0)).abs() < 1e-6, "L4 权重应用，实际 {scaled}");
}
