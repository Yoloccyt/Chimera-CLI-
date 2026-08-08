//! RL 共享类型集成测试 — RLState/RLAction/RLExperience 接缝映射与序列化往返
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P1 / §6 Milestone B-1）：
//! 关闭 ADR-049 裁决漂移（rl-types 部分落地），补齐 RL 共享类型
//! （纯类型零逻辑，ADR-033 合规；Serde + 接缝映射，不含训练逻辑——R2 冻结面外）。

use nexus_contracts::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
use nexus_contracts::{
    ActivationStrategy, DecayProfile, DensityTier, MemoryStrategy, PrefetchStrategy, RecallQuota,
    SeamId, SelectorPolicy,
};

// ============================================================
// 接缝双向映射：S1-S9 接缝契约类型 ↔ RLAction 变体
// ============================================================

/// S1: 密度档位 → RLAction::Density → 解构还原 + seam 标识
#[test]
fn seam_s1_density_roundtrip() {
    let tier = DensityTier::default();
    let action = RLAction::Density(tier);
    assert_eq!(action.seam_id(), Some(SeamId::S1Density));
    match action {
        RLAction::Density(t) => assert_eq!(t, tier),
        other => panic!("S1 映射错误: {other:?}"),
    }
}

/// S2: 记忆策略 → RLAction::Memory（双向映射）
#[test]
fn seam_s2_memory_roundtrip() {
    let strategy = MemoryStrategy::StandardTopK;
    let action = RLAction::Memory(strategy);
    assert_eq!(action.seam_id(), Some(SeamId::S2Memory));
    match action {
        RLAction::Memory(s) => assert_eq!(s, strategy),
        other => panic!("S2 映射错误: {other:?}"),
    }
}

/// S3: 预取策略 → RLAction::Prefetch
#[test]
fn seam_s3_prefetch_roundtrip() {
    let strategy = PrefetchStrategy::default();
    let action = RLAction::Prefetch(strategy);
    assert_eq!(action.seam_id(), Some(SeamId::S3Prefetch));
    match action {
        RLAction::Prefetch(s) => assert_eq!(s, strategy),
        other => panic!("S3 映射错误: {other:?}"),
    }
}

/// S4: 选择器策略 → RLAction::Selector
#[test]
fn seam_s4_selector_roundtrip() {
    let policy = SelectorPolicy::fallback();
    let action = RLAction::Selector(policy);
    assert_eq!(action.seam_id(), Some(SeamId::S4Selector));
    match action {
        RLAction::Selector(p) => assert_eq!(p, policy),
        other => panic!("S4 映射错误: {other:?}"),
    }
}

/// S5: Parliament 激活策略 → RLAction::Parliament
#[test]
fn seam_s5_parliament_roundtrip() {
    let policy = ActivationStrategy::default();
    let action = RLAction::Parliament(policy);
    assert_eq!(action.seam_id(), Some(SeamId::S5Parliament));
    match action {
        RLAction::Parliament(p) => assert_eq!(p, policy),
        other => panic!("S5 映射错误: {other:?}"),
    }
}

/// S6: 衰减档位 → RLAction::Decay
#[test]
fn seam_s6_decay_roundtrip() {
    let profile = DecayProfile::default();
    let action = RLAction::Decay(profile);
    assert_eq!(action.seam_id(), Some(SeamId::S6Decay));
    match action {
        RLAction::Decay(p) => assert_eq!(p, profile),
        other => panic!("S6 映射错误: {other:?}"),
    }
}

/// S7: 召回配额 → RLAction::RecallQuota（R1 离线 RL 接缝）
#[test]
fn seam_s7_recall_quota_roundtrip() {
    let quota = RecallQuota::K10;
    let action = RLAction::RecallQuota(quota);
    assert_eq!(action.seam_id(), Some(SeamId::S7RecallQuota));
    match action {
        RLAction::RecallQuota(q) => assert_eq!(q, quota),
        other => panic!("S7 映射错误: {other:?}"),
    }
}

/// S8: Mem-π 三臂 → RLAction::MemPi（Generate/Retrieve/Abstain 全覆盖）
#[test]
fn seam_s8_mem_pi_all_arms_roundtrip() {
    for arm in [
        MemPiAction::Generate,
        MemPiAction::Retrieve,
        MemPiAction::Abstain,
    ] {
        let action = RLAction::MemPi(arm);
        assert_eq!(action.seam_id(), Some(SeamId::S8MemPi));
        match action {
            RLAction::MemPi(a) => assert_eq!(a, arm),
            other => panic!("S8 映射错误: {other:?}"),
        }
    }
}

/// S9: 路由臂（provider/model/mode 组合串）→ RLAction::Route
///
/// SeamId 枚举的 9 号位为 S9TokenEfficiency（ADR-069），未覆盖 S9Route 接缝
/// （omega-learner s9_route，RouteLLM 落点）——故 seam_id() 返回 None（诚实表达）。
#[test]
fn seam_s9_route_roundtrip() {
    let arm = "zhipu/glm-5.2/standard".to_string();
    let action = RLAction::Route(arm.clone());
    assert_eq!(action.seam_id(), None);
    match action {
        RLAction::Route(a) => assert_eq!(a, arm),
        other => panic!("S9 映射错误: {other:?}"),
    }
}

/// 扩展点：自定义臂不映射到既有接缝，seam_id() 返回 None（保持可审计）
#[test]
fn custom_action_is_explicit() {
    let action = RLAction::Custom("future-seam".into());
    assert!(matches!(action, RLAction::Custom(_)));
    assert_eq!(action.seam_id(), None);
}

// ============================================================
// RLState / RLExperience 行为
// ============================================================

/// RLState 构建与字段访问
#[test]
fn rl_state_construct_and_access() {
    let state = RLState::new(vec![0.5, 0.2, 0.8], 1_000);
    assert_eq!(state.context(), &[0.5, 0.2, 0.8]);
    assert_eq!(state.timestamp_ms(), 1_000);
    // 默认无任务阶段/预算水位（可选字段）
    assert!(state.task_phase().is_none());
    assert!(state.budget_watermark().is_none());
}

/// RLExperience 四元组构造（state, action, reward, next_state + done + seam）
#[test]
fn rl_experience_construct() {
    let exp = RLExperience::new(
        RLState::new(vec![0.1], 1),
        RLAction::Density(DensityTier::default()),
        0.5,
        RLState::new(vec![0.2], 2),
    );
    assert!(!exp.done);
    assert_eq!(exp.reward, 0.5);
    assert_eq!(exp.seam, SeamId::S1Density);
}

// ============================================================
// 序列化往返（serde_json + rmp-serde，ADR-004 MessagePack 协议）
// ============================================================

/// RLAction JSON 序列化往返
#[test]
fn rl_action_json_roundtrip() {
    let actions = [
        RLAction::Density(DensityTier::default()),
        RLAction::Memory(MemoryStrategy::StandardTopK),
        RLAction::MemPi(MemPiAction::Abstain),
        RLAction::Route("moonshot/kimi-k3/deep".into()),
    ];
    for action in actions {
        let json = serde_json::to_string(&action).expect("JSON 序列化应成功");
        let back: RLAction = serde_json::from_str(&json).expect("JSON 反序列化应成功");
        assert_eq!(back, action, "JSON 往返不等: {action:?}");
    }
}

/// RLAction MessagePack 序列化往返（ADR-004）
#[test]
fn rl_action_msgpack_roundtrip() {
    let action = RLAction::RecallQuota(RecallQuota::K50);
    let bytes = rmp_serde::to_vec(&action).expect("MsgPack 序列化应成功");
    let back: RLAction = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化应成功");
    assert_eq!(back, action);
}

/// RLExperience JSON 序列化往返（含 f32 奖励与向量）
#[test]
fn rl_experience_json_roundtrip() {
    let exp = RLExperience::new(
        RLState::new(vec![0.3, 0.7], 10),
        RLAction::MemPi(MemPiAction::Generate),
        0.9,
        RLState::new(vec![0.4, 0.8], 11),
    );
    let json = serde_json::to_string(&exp).expect("JSON 序列化应成功");
    let back: RLExperience = serde_json::from_str(&json).expect("JSON 反序列化应成功");
    assert_eq!(back, exp);
}

/// RLExperience MessagePack 序列化往返
#[test]
fn rl_experience_msgpack_roundtrip() {
    let exp = RLExperience::new(
        RLState::new(vec![1.0], 100),
        RLAction::Selector(SelectorPolicy::fallback()),
        -0.25,
        RLState::new(vec![1.1], 101),
    );
    let bytes = rmp_serde::to_vec(&exp).expect("MsgPack 序列化应成功");
    let back: RLExperience = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化应成功");
    assert_eq!(back, exp);
}
