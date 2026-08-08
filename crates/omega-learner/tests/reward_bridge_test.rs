//! S9 接缝奖励 → RewardSignal 桥接测试（Milestone C-1，各层 Reward 类型统一）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 C-1 / §7.1）：
//! 各层实现为 Reward 类型 + EventBus 奖励信号流——S9 路由接缝的 S9Reward
//! 经桥接转换为 L0 RewardSignal（统一载荷），R1 数据面先接入。

#![forbid(unsafe_code)]

use event_bus::EventBus;
use event_bus::NexusEvent;
use nexus_contracts::reward::{RewardLayer, RewardSpec};
use omega_learner::reward_bridge::{reward_signal_event, s9_reward_to_signal};
use omega_learner::s9_route::S9Reward;

/// S9Reward → RewardSignal 桥接：raw = success 门控 × 质量 − 成本 − 延迟
#[test]
fn s9_reward_bridges_to_signal() {
    let reward = S9Reward {
        success: true,
        quality: 1.0,
        normalized_cost: 0.1,
        normalized_latency: 0.2,
    };
    let spec = RewardSpec::new("rs-s9_route", RewardLayer::L6, "s9_route", 1.0);
    let signal = s9_reward_to_signal(&spec, &reward, 1_000);

    assert_eq!(signal.spec_id, "rs-s9_route");
    assert!(!signal.is_security_observation);
    // raw = 1.0 × 1.0 − 0.1 − 0.2 = 0.7；weighted = 0.7 × 0.8（L6）= 0.56
    assert!((signal.raw_reward - 0.7).abs() < 1e-6);
    assert!((signal.weighted_reward - 0.56).abs() < 1e-6);
}

/// 失败任务：success 门控 → 奖励为 0
#[test]
fn failed_task_yields_zero_reward() {
    let reward = S9Reward {
        success: false,
        quality: 0.9,
        normalized_cost: 0.0,
        normalized_latency: 0.0,
    };
    let spec = RewardSpec::new("rs-s9_route", RewardLayer::L6, "s9_route", 1.0);
    let signal = s9_reward_to_signal(&spec, &reward, 1_000);
    assert_eq!(signal.raw_reward, 0.0);
    assert_eq!(signal.weighted_reward, 0.0);
}

/// RewardSignal → EventBus 奖励信号流事件（R1 数据面接入）
#[tokio::test]
async fn signal_publishes_as_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let reward = S9Reward {
        success: true,
        quality: 0.8,
        normalized_cost: 0.0,
        normalized_latency: 0.0,
    };
    let spec = RewardSpec::new("rs-s9_route", RewardLayer::L6, "s9_route", 1.0);
    let signal = s9_reward_to_signal(&spec, &reward, 1_000);

    bus.publish(reward_signal_event(signal.clone()))
        .await
        .unwrap();

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("应收到奖励信号")
        .expect("recv 不应失败");
    match event {
        NexusEvent::RewardSignalReported { signal: s, .. } => {
            assert_eq!(s, signal);
            assert_eq!(s.spec_id, "rs-s9_route");
        }
        other => panic!("应收到 RewardSignalReported: {other:?}"),
    }
}
