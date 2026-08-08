//! RewardSpec 契约测试（Milestone C-1，奖励函数统一框架）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P3 / §6 C-1 / §7.1）：
//! 设计 §17 八维度权重表 → L0 RewardSpec 契约 1:1 映射 + 各层 Reward 类型统一
//! + EventBus 奖励信号流（R1 数据面先接入，R2 训练面解冻后激活）。

#![forbid(unsafe_code)]

use nexus_contracts::reward::{
    reward_layer_weight, security_event_reward, RewardLayer, RewardSignal, RewardSpec,
    SecuritySeverity,
};

/// 权重表 1:1 映射（§17）：L5 1.2x / L10 1.0x / L9·L8 0.9x / L7·L6 0.8x /
/// L2 0.7x / L0 0.6x / L4·L3 0.5x / L1 0.3x
#[test]
fn layer_weights_match_spec_section17() {
    assert_eq!(reward_layer_weight(RewardLayer::L5), 1.2);
    assert_eq!(reward_layer_weight(RewardLayer::L10), 1.0);
    assert_eq!(reward_layer_weight(RewardLayer::L9), 0.9);
    assert_eq!(reward_layer_weight(RewardLayer::L8), 0.9);
    assert_eq!(reward_layer_weight(RewardLayer::L7), 0.8);
    assert_eq!(reward_layer_weight(RewardLayer::L6), 0.8);
    assert_eq!(reward_layer_weight(RewardLayer::L2), 0.7);
    assert_eq!(reward_layer_weight(RewardLayer::L0), 0.6);
    assert_eq!(reward_layer_weight(RewardLayer::L4), 0.5);
    assert_eq!(reward_layer_weight(RewardLayer::L3), 0.5);
    assert_eq!(reward_layer_weight(RewardLayer::L1), 0.3);
}

/// 全部 11 层均有权重（无缺失）
#[test]
fn all_layers_have_weights() {
    for layer in RewardLayer::ALL {
        let w = reward_layer_weight(layer);
        assert!(w > 0.0, "层 {layer:?} 权重应为正");
    }
}

/// RewardSpec::apply：raw × weight × scale
#[test]
fn apply_multiplies_weight_and_scale() {
    let spec = RewardSpec::new("rs-s9", RewardLayer::L6, "s9_route", 1.0);
    assert_eq!(spec.apply(10.0), 8.0); // L6 权重 0.8 × 10

    let spec2 = RewardSpec::new("rs-s9x2", RewardLayer::L6, "s9_route", 2.0);
    assert_eq!(spec2.apply(10.0), 16.0); // 0.8 × 2.0 × 10
}

/// L4 安全事件奖励映射（§8.1，仅观测）：Critical -10 / High -5 / Medium -2 /
/// Low -0.5 / Info +0.1（五档安全严重度，与 EventSeverity 三档投递语义不同域）
#[test]
fn security_event_reward_matches_spec_section81() {
    assert_eq!(security_event_reward(SecuritySeverity::Critical), -10.0);
    assert_eq!(security_event_reward(SecuritySeverity::High), -5.0);
    assert_eq!(security_event_reward(SecuritySeverity::Medium), -2.0);
    assert_eq!(security_event_reward(SecuritySeverity::Low), -0.5);
    assert_eq!(security_event_reward(SecuritySeverity::Info), 0.1);
}

/// RewardSignal 构造 + 加权奖励计算
#[test]
fn signal_carries_weighted_reward() {
    let spec = RewardSpec::new("rs-l5", RewardLayer::L5, "skill_reuse", 1.0);
    let signal = RewardSignal::new(&spec, 0.8, 1_000);
    assert_eq!(signal.raw_reward, 0.8);
    // f32 精度陷阱（项目记忆）：0.8 × 1.2 = 0.95999998，用近似比较
    assert!(
        (signal.weighted_reward - 0.96).abs() < 1e-6,
        "加权奖励应 ≈0.96（L5 权重 1.2），实际 {}",
        signal.weighted_reward
    );
    assert!(!signal.is_security_observation);
}

/// L4 观测信号标记（不参与训练）
#[test]
fn security_observation_flag_is_marked() {
    let spec = RewardSpec::new("rs-l4", RewardLayer::L4, "security", 1.0);
    let signal = RewardSignal::security_observation(&spec, -5.0, 1_000);
    assert!(signal.is_security_observation);
    assert_eq!(signal.weighted_reward, -5.0); // L4 观测不乘权重（仅观测语义）
}

/// RewardSignal serde 序列化往返（EventBus 奖励信号流）
#[test]
fn signal_serde_roundtrip() {
    let spec = RewardSpec::new("rs-rt", RewardLayer::L2, "mem_pi", 1.0);
    let signal = RewardSignal::new(&spec, 0.5, 42);
    let json = serde_json::to_string(&signal).expect("序列化应成功");
    let back: RewardSignal = serde_json::from_str(&json).expect("反序列化应成功");
    assert_eq!(back, signal);
}
