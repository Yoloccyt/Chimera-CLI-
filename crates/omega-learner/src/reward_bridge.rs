//! 接缝奖励 → RewardSignal 桥接（Milestone C-1，各层 Reward 类型统一）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §6 C-1 / §7.1
//!
//! # 职责
//!
//! 将既有接缝奖励类型（S9Reward 等）转换为 L0 `RewardSignal` 统一载荷，
//! 经 EventBus 奖励信号流传输（R1 数据面先接入；R2 训练面解冻后由训练
//! 服务消费）。依赖方向：L6 omega-learner → L0 nexus-contracts（合规）。

use event_bus::{EventMetadata, NexusEvent};
use nexus_contracts::reward::{RewardSignal, RewardSpec};

use crate::s9_route::S9Reward;

/// S9 路由接缝奖励 → 统一 RewardSignal
///
/// # 转换规则（与 `S9Reward::reward` 同款门控语义）
/// - `success=false` → raw = 0（成功门控，失败不产生正奖励）
/// - `success=true` → raw = quality − normalized_cost − normalized_latency
/// - weighted = raw × 层权重 × 组件缩放（`RewardSpec::apply`）
pub fn s9_reward_to_signal(
    spec: &RewardSpec,
    reward: &S9Reward,
    timestamp_ms: u64,
) -> RewardSignal {
    let raw = if reward.success {
        (reward.quality - reward.normalized_cost - reward.normalized_latency).max(0.0)
    } else {
        0.0
    };
    RewardSignal::new(spec, raw, timestamp_ms)
}

/// 构造奖励信号流事件（EventBus 载荷）
pub fn reward_signal_event(signal: RewardSignal) -> NexusEvent {
    NexusEvent::RewardSignalReported {
        metadata: EventMetadata::new("omega-learner"),
        signal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::reward::RewardLayer;

    #[test]
    fn clamp_negative_raw_to_zero() {
        let reward = S9Reward {
            success: true,
            quality: 0.1,
            normalized_cost: 0.5,
            normalized_latency: 0.5,
        };
        let spec = RewardSpec::new("rs-s9", RewardLayer::L6, "s9_route", 1.0);
        let signal = s9_reward_to_signal(&spec, &reward, 0);
        assert_eq!(signal.raw_reward, 0.0, "负奖励应钳制为 0");
    }
}
