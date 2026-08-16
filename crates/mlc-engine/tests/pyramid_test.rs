//! 记忆金字塔集成测试 — 四层 + 检索三方式 + 注入 + 降级链全链路（v3.4.0 §7.3）
//!
//! 覆盖: 顶层 API 可达性 / L0 契约类型四层全链路 / 检索三方式融合 /
//! 注入策略 / 降级链四态 / L0→L1 提炼 / proptest 检索不变量

#![forbid(unsafe_code)]

use mlc_engine::{
    DegradationChain, MemoryPyramid, RetrievalResult, RetrievalSource, RetrieveStrategy,
};
use nexus_contracts::memory_pyramid::{
    AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock,
};
use nexus_contracts::ArchiveTier;
use nexus_core::CLV;
use proptest::prelude::*;

fn atomic_card(id: &str, content: &str, card_type: AtomicCardType) -> AtomicMemoryCard {
    AtomicMemoryCard::new(
        id,
        card_type,
        100,
        "scene-1",
        content,
        None,
        None,
        None,
        None,
        1_700_000_000_000,
    )
}

fn unit_clv(dim: usize) -> CLV {
    let mut v = vec![0.0f32; CLV::DIMENSION];
    v[dim] = 1.0;
    CLV::from_vec(v).expect("512 维合法")
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use mlc_engine::prelude::*;
    let pyramid = MemoryPyramid::new();
    assert_eq!(pyramid.level_counts(), (0, 0, 0, 0));
    let _ = RawLogEntry {
        id: "r".into(),
        session_id: "s".into(),
        user_message: "u".into(),
        assistant_message: "a".into(),
        timestamp: 0,
    };
    let _ = RetrievalSource::Hybrid;
    let _ = RetrieveStrategy::Hybrid;
}

// ----------------------------------------------------------
// L0 契约类型四层全链路
// ----------------------------------------------------------

#[test]
fn four_levels_with_l0_contract_types() {
    let mut pyramid = MemoryPyramid::new();
    // L0 Raw
    pyramid.write_raw_log("s1", "用户消息", "助手回复");
    // L1 Atomic（L0 契约类型）
    pyramid.insert_atomic_card(atomic_card("c1", "原子记忆", AtomicCardType::Event), None);
    // L2 Scene（L0 契约类型）
    pyramid.insert_scene_block(SceneBlock::new("b1", "场景", vec![], "场景摘要"));
    // L3 Persona（L0 契约类型）
    pyramid.insert_persona(PersonaSummary::new(
        "p1",
        "u1",
        "人格画像",
        vec![],
        vec![],
        1_700_000_000_000,
    ));
    assert_eq!(pyramid.level_counts(), (1, 1, 1, 1));
}

#[test]
fn memory_pyramid_level_to_archive_tier_mapping() {
    // L0 MemoryPyramidLevel ↔ ArchiveTier 静态映射（复用 Phase 0 先例）
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L0RawLog),
        ArchiveTier::Cold
    );
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L1AtomicMemory),
        ArchiveTier::Warm
    );
    assert_eq!(
        ArchiveTier::from(MemoryPyramidLevel::L3Persona),
        ArchiveTier::Hot
    );
}

// ----------------------------------------------------------
// L0 → L1 提炼
// ----------------------------------------------------------

#[test]
fn raw_to_atomic_distillation() {
    let mut pyramid = MemoryPyramid::new();
    pyramid.write_raw_log("s1", "消息1", "回复1");
    pyramid.write_raw_log("s1", "消息2", "回复2");
    pyramid.write_raw_log("s2", "其他会话", "回复");
    let cards = pyramid.distill_atomic_cards("s1");
    assert_eq!(cards.len(), 2, "应只提炼 s1 的 2 条");
    // 提炼后 L1 层级填充
    assert_eq!(pyramid.level_counts().1, 2);
}

// ----------------------------------------------------------
// 检索三方式融合
// ----------------------------------------------------------

#[test]
fn retrieval_three_way_hybrid_fusion() {
    let mut pyramid = MemoryPyramid::new();
    let clv = unit_clv(0);
    // c1: 字面 + 语义都可命中
    pyramid.insert_atomic_card(
        atomic_card("c1", "rust 类型错误 E0308", AtomicCardType::Event),
        Some(clv.clone()),
    );
    // c2: 仅语义命中
    pyramid.insert_atomic_card(
        atomic_card("c2", "完全不同内容", AtomicCardType::Event),
        Some(clv.clone()),
    );
    // c3: 仅字面命中
    pyramid.insert_atomic_card(
        atomic_card("c3", "rust 性能优化", AtomicCardType::Event),
        None,
    );
    // Hybrid: 字面 "rust" + 语义 clv
    let results = pyramid.retrieve("rust", Some(&clv), 1000);
    assert!(results.len() >= 2, "Hybrid 应融合字面+语义命中");
    // 所有结果标记为 Hybrid source
    for r in &results {
        assert_eq!(r.source, RetrievalSource::Hybrid);
    }
}

#[test]
fn semantic_only_degradation_excludes_literal() {
    let mut pyramid =
        MemoryPyramid::new().with_degradation_chain(DegradationChain::new(true, false));
    let clv = unit_clv(3);
    // c1 字面匹配 "unique_keyword"，但语义不匹配（不同 CLV）
    pyramid.insert_atomic_card(
        atomic_card("c1", "unique_keyword 内容", AtomicCardType::Event),
        Some(unit_clv(5)), // 与 query clv(3) 正交
    );
    // SemanticOnly + 语义不匹配 → 空结果（字面被排除）
    let results = pyramid.retrieve("unique_keyword", Some(&clv), 1000);
    assert!(
        results.is_empty(),
        "SemanticOnly 不含字面命中，语义不匹配应为空"
    );
}

// ----------------------------------------------------------
// 注入策略
// ----------------------------------------------------------

#[test]
fn injection_dynamic_cards_and_persona_separated() {
    let pyramid = MemoryPyramid::new();
    let retrieved = vec![
        RetrievalResult {
            card: atomic_card("c1", "动态修复方案", AtomicCardType::Event),
            score: 1.0,
            source: RetrievalSource::Hybrid,
        },
        RetrievalResult {
            card: atomic_card("p1", "偏好简洁代码", AtomicCardType::Preference),
            score: 1.0,
            source: RetrievalSource::Hybrid,
        },
    ];
    let mut user_msg = "实现功能".to_string();
    let mut system = "你是助手".to_string();
    pyramid.inject_context(&retrieved, &mut user_msg, &mut system);
    // 动态卡片 → 用户消息前；人格 → 系统提示末尾
    assert!(user_msg.contains("[记忆]"), "动态卡片注入用户消息");
    assert!(user_msg.contains("动态修复方案"));
    assert!(system.contains("[用户画像]"), "人格注入系统提示");
    assert!(system.contains("偏好简洁代码"));
    // 互不混淆
    assert!(!user_msg.contains("偏好简洁代码"));
    assert!(!system.contains("动态修复方案"));
}

#[test]
fn injection_limits_dynamic_cards_to_three() {
    let pyramid = MemoryPyramid::new();
    let retrieved: Vec<RetrievalResult> = (0..5)
        .map(|i| RetrievalResult {
            card: atomic_card(&format!("c{i}"), &format!("内容{i}"), AtomicCardType::Event),
            score: 1.0,
            source: RetrievalSource::Hybrid,
        })
        .collect();
    let mut user_msg = "问题".to_string();
    let mut system = String::new();
    pyramid.inject_context(&retrieved, &mut user_msg, &mut system);
    // 动态卡片最多注入 3 条
    let inject_count = user_msg.matches("[记忆]").count();
    assert_eq!(
        inject_count, 3,
        "动态卡片注入上限 3 条（实际 {})",
        inject_count
    );
}

// ----------------------------------------------------------
// 降级链四态
// ----------------------------------------------------------

#[test]
fn degradation_chain_all_four_states() {
    let states = [
        (DegradationChain::new(true, true), RetrieveStrategy::Hybrid),
        (
            DegradationChain::new(true, false),
            RetrieveStrategy::SemanticOnly,
        ),
        (
            DegradationChain::new(false, true),
            RetrieveStrategy::KeywordOnly,
        ),
        (DegradationChain::new(false, false), RetrieveStrategy::Empty),
    ];
    for (chain, expected) in states {
        assert_eq!(chain.retrieve_strategy(), expected);
    }
}

// ----------------------------------------------------------
// proptest: 检索不变量
// ----------------------------------------------------------

proptest! {
    /// 任意字面查询，Empty 降级链恒返回空（可用性底线）
    #[test]
    fn empty_chain_always_returns_empty(
        query_len in 1usize..20,
    ) {
        let mut pyramid = MemoryPyramid::new()
            .with_degradation_chain(DegradationChain::new(false, false));
        pyramid.insert_atomic_card(
            atomic_card("c1", "任意内容", AtomicCardType::Event),
            None,
        );
        let query = "x".repeat(query_len);
        let results = pyramid.retrieve(&query, None, 1000);
        prop_assert!(results.is_empty(), "Empty 降级链应恒返回空");
    }

    /// 字面命中的结果分数恒为 1.0（精确包含语义）
    #[test]
    fn literal_hit_score_is_one(
        _n in 0usize..5,
    ) {
        let mut pyramid = MemoryPyramid::new()
            .with_degradation_chain(DegradationChain::new(false, true));
        pyramid.insert_atomic_card(
            atomic_card("c1", "target_keyword", AtomicCardType::Event),
            None,
        );
        let results = pyramid.retrieve("target_keyword", None, 1000);
        prop_assert_eq!(results.len(), 1);
        prop_assert!((results[0].score - 1.0).abs() < 1e-6);
    }
}
