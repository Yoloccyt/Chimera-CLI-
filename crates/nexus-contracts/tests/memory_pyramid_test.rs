//! 记忆金字塔契约集成测试 — 金字塔 ↔ 归档层级映射与跨模块协同（v3.4.0 §5.4）
//!
//! 覆盖: MemoryPyramidLevel ↔ ArchiveTier 静态映射（含 ArchiveTier 新 serde）/
//! 金字塔层级与经验卡片/Token 证据的跨模块组装 / RL 快照卡片全链路 / proptest 属性

#![forbid(unsafe_code)]

use nexus_contracts::{
    ArchiveTier, AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary,
    RLActionVector, RLStateVector, SceneBlock,
};
use proptest::prelude::*;

// ----------------------------------------------------------
// 金字塔 ↔ 归档层级映射（L0 同层协同）
// ----------------------------------------------------------

#[test]
fn pyramid_to_archive_mapping_full() {
    // 静态映射: 层级 → 存储温度基线
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L0RawLog),
        ArchiveTier::Cold
    );
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L1AtomicMemory),
        ArchiveTier::Warm
    );
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L2SceneBlock),
        ArchiveTier::Warm
    );
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L3Persona),
        ArchiveTier::Hot
    );
    // 层级数值单调
    assert_eq!(MemoryPyramidLevel::L0RawLog.level_value(), 0);
    assert_eq!(MemoryPyramidLevel::L3Persona.level_value(), 3);
}

#[test]
fn archive_tier_serde_roundtrip() {
    // 审计修复 A1 验证: ArchiveTier 现可序列化（归档事件/检查点传输）
    for tier in [
        ArchiveTier::Hot,
        ArchiveTier::Warm,
        ArchiveTier::Cold,
        ArchiveTier::Ice,
    ] {
        let json = serde_json::to_string(&tier).expect("JSON 序列化失败");
        let back: ArchiveTier = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(back, tier);
    }
    assert_eq!(
        serde_json::to_string(&ArchiveTier::Cold).expect("JSON 序列化失败"),
        "\"cold\""
    );
}

// ----------------------------------------------------------
// 金字塔与经验证据的跨模块组装
// ----------------------------------------------------------

#[test]
fn atomic_card_with_rl_snapshot_full_chain() {
    // L1 原子卡片携带 RL 快照（rl_hooks 类型）→ 序列化 → 还原
    let state = RLStateVector::zeros();
    let action = RLActionVector::new("S2", 0, vec![0.3, 0.6]);
    let card = AtomicMemoryCard::new(
        "card-rl-1",
        AtomicCardType::Policy,
        200,
        "routing",
        "S2 记忆策略经验",
        Some("traj-9"),
        Some("观察到策略切换"),
        Some("MinimalRecall 更优"),
        Some(0.85),
        1_700_000_000_000,
    )
    .with_rl_snapshot(state, action);

    // MsgPack 全链路 roundtrip（训练数据面形态）
    let bytes = rmp_serde::to_vec(&card).expect("MsgPack 序列化失败");
    let back: AtomicMemoryCard = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
    assert_eq!(back, card);
    assert!(back.is_rl_card());
    // 先取引用再断言（避免 expect 移动字段后二次借用）
    let action = back.action_record.as_ref().expect("动作存在");
    assert_eq!(action.action_code, 0);
    assert_eq!(action.layer.as_ref(), "S2");
}

#[test]
fn scene_block_aggregates_cards_across_pyramid() {
    // L2 场景聚合 L1 卡片（场景 → 卡片 ID 引用 → 人格摘要映射）
    let scene = SceneBlock::new(
        "scene-code-review",
        "code-review",
        vec![Box::from("card-1"), Box::from("card-2")],
        "代码评审场景档案",
    )
    .with_msce_elements(
        "当提交代码时",
        "运行评审流程",
        "评审通过",
        "非生产代码",
        0.9,
    );
    let persona = PersonaSummary::new(
        "persona-1",
        "user-1",
        "偏好简洁高效",
        vec![Box::from("简洁注释")],
        vec![Box::from("不引入未要求抽象")],
        1_700_000_000_000,
    );
    // 场景热度与人格偏好可组合为注入上下文
    assert!(scene.heat_value == 0); // 新场景默认热度 0
    assert_eq!(persona.preferences.len(), 1);
    // 全链路 JSON roundtrip
    let scene_back: SceneBlock =
        serde_json::from_str(&serde_json::to_string(&scene).expect("序列化失败"))
            .expect("反序列化失败");
    assert_eq!(scene_back, scene);
    let persona_back: PersonaSummary =
        serde_json::from_str(&serde_json::to_string(&persona).expect("序列化失败"))
            .expect("反序列化失败");
    assert_eq!(persona_back, persona);
}

// ----------------------------------------------------------
// proptest 属性: 层级数值单调与映射完备
// ----------------------------------------------------------

proptest! {
    /// 任意金字塔层级映射后均为合法归档层级（穷举等价 + 随机抽样双保险）
    #[test]
    fn pyramid_mapping_always_valid_level(
        level in prop_oneof![
            Just(MemoryPyramidLevel::L0RawLog),
            Just(MemoryPyramidLevel::L1AtomicMemory),
            Just(MemoryPyramidLevel::L2SceneBlock),
            Just(MemoryPyramidLevel::L3Persona),
        ]
    ) {
        let tier = ArchiveTier::from(level);
        prop_assert!(matches!(tier, ArchiveTier::Hot | ArchiveTier::Warm | ArchiveTier::Cold | ArchiveTier::Ice));
        // 映射单调性: 任意层级温度不冷于 Raw 基线（L0RawLog→Cold），
        // 即金字塔越高存储越热（tier.level() 数值越小）
        prop_assert!(tier.level() <= ArchiveTier::from(MemoryPyramidLevel::L0RawLog).level());
    }

    /// 任意优先级 u8 均可在卡片中无损往返
    #[test]
    fn atomic_card_priority_roundtrip(priority in any::<u8>()) {
        let card = AtomicMemoryCard::new(
            "card-prop",
            AtomicCardType::Event,
            priority,
            "scene",
            "content",
            None, None, None, None,
            1_700_000_000_000,
        );
        let bytes = rmp_serde::to_vec(&card).expect("MsgPack 序列化失败");
        let back: AtomicMemoryCard = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        prop_assert_eq!(back.priority, priority);
    }
}
