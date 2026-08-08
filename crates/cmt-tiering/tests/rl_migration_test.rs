//! DQN 记忆迁移决策测试（Milestone D-2a，设计 §6.4 目标形态）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 D-2）：
//! DQN 记忆迁移 —— 预测式冷热迁移（设计 §6.4 `mlc-engine/src/rl_migration.rs`
//! 目标形态；实际落点 cmt-tiering 温度层，0 新增 crate）。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决：Q 网络为规则式线性权重（专家先验
//! 注入，非神经网络），`record` 仅回放记录不做梯度更新——训练面占位，
//! 解冻后替换为 TD 误差 + Q 权重更新，不得破坏接口契约。

#![forbid(unsafe_code)]

use cmt_tiering::rl_migration::{DQNMigrationPolicy, MigrationExperience, MigrationState};
use cmt_tiering::types::Tier;

/// 近期高频访问的 chunk（1m 内 50 次）
fn hot_state() -> MigrationState {
    MigrationState {
        chunk_id: "chunk-hot".into(),
        access_frequency_1m: 50,
        access_frequency_10m: 120,
        access_frequency_1h: 300,
        last_access_age_ms: 100,
    }
}

/// 长期低频访问的 chunk（1m 0 次，1h 仅 2 次，很久未访问）
fn ice_state() -> MigrationState {
    MigrationState {
        chunk_id: "chunk-ice".into(),
        access_frequency_1m: 0,
        access_frequency_10m: 1,
        access_frequency_1h: 2,
        last_access_age_ms: 7_200_000,
    }
}

/// 默认专家先验权重：近期高频 → Hot；长期低频 → Ice
fn default_policy() -> DQNMigrationPolicy {
    DQNMigrationPolicy::new(
        vec![
            [1.0, 0.5, 0.2, -1.0],  // Hot：近期频率权重高、久未访问惩罚
            [0.5, 1.0, 0.5, -0.3],  // Warm：中期频率权重
            [0.1, 0.5, 1.0, -0.1],  // Cold：长期频率权重
            [-0.5, -0.2, 0.5, 0.5], // Ice：低频偏好 + 久未访问加分
        ],
        0.0, // ε=0：确定性（测试可断言）
        64,
    )
}

/// 高近期访问频率 → Hot（ε=0 确定性）
#[test]
fn decide_tier_prefers_hot_for_recent_frequent_access() {
    let policy = default_policy();
    assert_eq!(policy.decide_tier(&hot_state()), Tier::Hot);
}

/// 长期低频访问 → Ice
#[test]
fn decide_tier_prefers_ice_for_aging_cold_data() {
    let policy = default_policy();
    assert_eq!(policy.decide_tier(&ice_state()), Tier::Ice);
}

/// 全部 4 层权重可枚举（动作空间完备）
#[test]
fn all_tiers_are_reachable() {
    let policy = DQNMigrationPolicy::new(
        vec![
            [1.0, 0.0, 0.0, 0.0], // 只认特征 0
            [0.0, 1.0, 0.0, 0.0], // 只认特征 1
            [0.0, 0.0, 1.0, 0.0], // 只认特征 2
            [0.0, 0.0, 0.0, 1.0], // 只认特征 3
        ],
        0.0,
        64,
    );
    let base = MigrationState {
        chunk_id: "x".into(),
        access_frequency_1m: 10,
        access_frequency_10m: 10,
        access_frequency_1h: 10,
        last_access_age_ms: 10,
    };
    // 各状态仅一个特征饱和为 1.0（其余继承 base 低值）→ 对应层 Q 唯一最大
    let states = [
        MigrationState {
            access_frequency_1m: 100,
            ..base.clone()
        },
        MigrationState {
            access_frequency_10m: 1_000,
            ..base.clone()
        },
        MigrationState {
            access_frequency_1h: 10_000,
            ..base.clone()
        },
        MigrationState {
            last_access_age_ms: 86_400_000,
            ..base.clone()
        },
    ];
    let expected = [Tier::Hot, Tier::Warm, Tier::Cold, Tier::Ice];
    for (state, tier) in states.iter().zip(expected.iter()) {
        assert_eq!(policy.decide_tier(state), *tier, "特征主导层应可达");
    }
}

/// ε=1 全探索：多次调用返回合法层（不崩溃、不越界）
#[test]
fn epsilon_one_explores_all_tiers() {
    let policy = DQNMigrationPolicy::new(
        vec![
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        1.0,
        64,
    );
    let mut seen = std::collections::HashSet::new();
    for _ in 0..200 {
        seen.insert(policy.decide_tier(&hot_state()));
    }
    assert_eq!(seen.len(), 4, "ε=1 应覆盖全部 4 层（200 次采样内）");
}

/// record：回放记录追加（R2：仅记录不训练）
#[test]
fn record_appends_replay_without_training() {
    let mut policy = default_policy();
    let state = hot_state();
    policy.record(MigrationExperience {
        state: state.clone(),
        tier: Tier::Hot,
        reward: 1.0,
        next_state: None,
    });
    policy.record(MigrationExperience {
        state: state.clone(),
        tier: Tier::Warm,
        reward: -0.5,
        next_state: Some(state),
    });
    assert_eq!(policy.replay_len(), 2, "回放应记录 2 条经验");
    assert_eq!(policy.replay_rewards().len(), 2);
}

/// 回放上限：超出后淘汰最旧（FIFO 保持内存有界）
#[test]
fn replay_respects_capacity_limit() {
    let mut policy = DQNMigrationPolicy::new(
        vec![
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        0.0,
        3,
    );
    let state = hot_state();
    for i in 0..5 {
        policy.record(MigrationExperience {
            state: state.clone(),
            tier: Tier::Hot,
            reward: i as f32,
            next_state: None,
        });
    }
    assert_eq!(policy.replay_len(), 3, "超出容量应淘汰最旧");
    assert_eq!(
        policy.replay_rewards(),
        vec![2.0, 3.0, 4.0],
        "保留最新 3 条"
    );
}

/// 特征归一化：各特征 ∈ [0,1]（Q 打分尺度一致）
#[test]
fn features_are_normalized() {
    let state = hot_state();
    let f = state.features();
    assert_eq!(f.len(), 4);
    for v in f {
        assert!((0.0..=1.0).contains(&v), "特征应归一化，实际 {v}");
    }
}

/// 八维度奖励接入（D-2e）：迁移奖励 × L3 权重 0.5（设计 §17 权重表）
#[test]
fn migration_reward_scaled_by_layer_weight() {
    use nexus_contracts::reward::{reward_layer_weight, RewardLayer};
    assert!(
        (reward_layer_weight(RewardLayer::L3) - 0.5).abs() < 1e-6,
        "L3 权重 0.5"
    );
    let raw = 1.0; // 迁移成功奖励
    let scaled = raw * reward_layer_weight(RewardLayer::L3);
    assert!((scaled - 0.5).abs() < 1e-6, "L3 权重应用，实际 {scaled}");
}
