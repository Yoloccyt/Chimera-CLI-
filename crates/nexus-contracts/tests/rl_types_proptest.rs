//! RL 共享类型 proptest 属性测试 — 序列化往返不变量
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 Milestone B-1 验收标准：
//! "proptest 序列化往返"）。验证任意合法 RLAction/RLExperience 经
//! serde_json / rmp-serde 序列化-反序列化后保持相等（ADR-004 MessagePack 协议）。

use nexus_contracts::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
use nexus_contracts::{
    ActivationStrategy, DecayProfile, DensityTier, MemoryStrategy, PrefetchStrategy, RecallQuota,
    SeamId, SelectorPolicy,
};
use proptest::prelude::*;

/// 任意密度档位（S1 臂集）
fn any_density() -> impl Strategy<Value = DensityTier> {
    prop_oneof![Just(DensityTier::default()), Just(DensityTier::default()),]
}

/// 任意预取策略（S3 臂集）
fn any_prefetch() -> impl Strategy<Value = PrefetchStrategy> {
    prop_oneof![
        Just(PrefetchStrategy::default()),
        Just(PrefetchStrategy::default()),
    ]
}

/// 任意记忆策略（S2 臂集 5 档全覆盖）
fn any_memory_strategy() -> impl Strategy<Value = MemoryStrategy> {
    prop_oneof![
        Just(MemoryStrategy::MinimalRecall),
        Just(MemoryStrategy::StandardTopK),
        Just(MemoryStrategy::TimeFocused),
        Just(MemoryStrategy::QueryReformulation),
        Just(MemoryStrategy::AggressivePruning),
    ]
}

/// 任意 RLAction（S1-S9 + 扩展点全覆盖）
fn any_rl_action() -> impl Strategy<Value = RLAction> {
    prop_oneof![
        any_density().prop_map(RLAction::Density),
        any_memory_strategy().prop_map(RLAction::Memory),
        any_prefetch().prop_map(RLAction::Prefetch),
        prop::sample::select(vec![SelectorPolicy::fallback(), SelectorPolicy::fallback(),])
            .prop_map(RLAction::Selector),
        Just(ActivationStrategy::default()).prop_map(RLAction::Parliament),
        Just(DecayProfile::default()).prop_map(RLAction::Decay),
        prop::sample::select(vec![
            RecallQuota::K5,
            RecallQuota::K10,
            RecallQuota::K20,
            RecallQuota::K50,
            RecallQuota::K100,
        ])
        .prop_map(RLAction::RecallQuota),
        prop::sample::select(vec![
            MemPiAction::Generate,
            MemPiAction::Retrieve,
            MemPiAction::Abstain,
        ])
        .prop_map(RLAction::MemPi),
        "[a-z]{2,6}/[a-z0-9-]{2,12}/[a-z]{2,8}".prop_map(RLAction::Route),
        "[a-z-]{2,16}".prop_map(RLAction::Custom),
    ]
}

/// 任意 RLState（context 0..=8 维 + 时间戳）
fn any_rl_state() -> impl Strategy<Value = RLState> {
    (
        prop::collection::vec(0.0f32..1.0, 0..=8usize),
        0u64..1_000_000,
    )
        .prop_map(|(context, ts)| RLState::new(context, ts))
}

/// 任意 RLExperience（四元组 + done + seam 随机化）
fn any_rl_experience() -> impl Strategy<Value = RLExperience> {
    (
        any_rl_state(),
        any_rl_action(),
        -1.0f32..1.0,
        any_rl_state(),
        any::<bool>(),
        prop::sample::select(vec![
            SeamId::S1Density,
            SeamId::S2Memory,
            SeamId::S3Prefetch,
            SeamId::S4Selector,
            SeamId::S5Parliament,
            SeamId::S6Decay,
            SeamId::S7RecallQuota,
            SeamId::S8MemPi,
            SeamId::S9TokenEfficiency,
        ]),
    )
        .prop_map(|(state, action, reward, next, done, seam)| RLExperience {
            state,
            action,
            reward,
            next_state: next,
            done,
            seam,
        })
}

// 不变量 1: RLAction JSON 序列化往返保持相等
proptest::proptest! {
    #[test]
    fn rl_action_json_roundtrip_invariant(action in any_rl_action()) {
        let json = serde_json::to_string(&action).expect("JSON 序列化应成功");
        let back: RLAction = serde_json::from_str(&json).expect("JSON 反序列化应成功");
        prop_assert_eq!(back, action);
    }
}

// 不变量 2: RLAction MessagePack 序列化往返保持相等（ADR-004）
proptest::proptest! {
    #[test]
    fn rl_action_msgpack_roundtrip_invariant(action in any_rl_action()) {
        let bytes = rmp_serde::to_vec(&action).expect("MsgPack 序列化应成功");
        let back: RLAction = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化应成功");
        prop_assert_eq!(back, action);
    }
}

// 不变量 3: RLExperience JSON 序列化往返保持相等
proptest::proptest! {
    #[test]
    fn rl_experience_json_roundtrip_invariant(exp in any_rl_experience()) {
        let json = serde_json::to_string(&exp).expect("JSON 序列化应成功");
        let back: RLExperience = serde_json::from_str(&json).expect("JSON 反序列化应成功");
        prop_assert_eq!(back, exp);
    }
}

// 不变量 4: RLExperience MessagePack 序列化往返保持相等
proptest::proptest! {
    #[test]
    fn rl_experience_msgpack_roundtrip_invariant(exp in any_rl_experience()) {
        let bytes = rmp_serde::to_vec(&exp).expect("MsgPack 序列化应成功");
        let back: RLExperience = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化应成功");
        prop_assert_eq!(back, exp);
    }
}

// 不变量 5: seam_id 映射一致性——S1-S8 变体必有接缝，Route/Custom 恒为 None
proptest::proptest! {
    #[test]
    fn seam_id_consistent_with_variant(action in any_rl_action()) {
        match &action {
            RLAction::Density(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S1Density)),
            RLAction::Memory(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S2Memory)),
            RLAction::Prefetch(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S3Prefetch)),
            RLAction::Selector(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S4Selector)),
            RLAction::Parliament(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S5Parliament)),
            RLAction::Decay(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S6Decay)),
            RLAction::RecallQuota(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S7RecallQuota)),
            RLAction::MemPi(_) => prop_assert_eq!(action.seam_id(), Some(SeamId::S8MemPi)),
            RLAction::Route(_) | RLAction::Custom(_) => prop_assert_eq!(action.seam_id(), None),
        }
    }
}
