//! 按需记忆合成集成测试 — ExperienceCardSystem 协同 + 算子差异化（v3.4.0 §7.1）
//!
//! 覆盖: 顶层 API 可达性 / 合成器与卡片系统协同 / 四算子差异化上下文 /
//! 懒加载边界约束（铁律5）/ proptest 上下文规模不变量

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use mlc_engine::{ExperienceCardSystem, OnDemandSynthesizer};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;

fn card(node: &str, parent: Option<&str>, score: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card-{node}")),
        task_id: Box::from("t1"),
        node_id: Box::from(node),
        parent_id: parent.map(Box::from),
        created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
        operator: AtomicOperator::Draft,
        score,
        delta_vs_parent: 0.0,
        method_family: Box::from(format!("fam-{node}")),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.2,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

/// 构建链式卡片系统: root → a → target；target 的兄弟 sib1/sib2
fn build_chain_system() -> ExperienceCardSystem {
    let mut system = ExperienceCardSystem::new(1.414, 0.1);
    system.add_card(card("root", None, 0.5));
    system.add_card(card("a", Some("root"), 0.6));
    system.add_card(card("target", Some("a"), 0.7));
    system.add_card(card("sib1", Some("a"), 0.8));
    system.add_card(card("sib2", Some("a"), 0.9));
    system
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use mlc_engine::prelude::*;
    let synth = OnDemandSynthesizer::new();
    let system = ExperienceCardSystem::new(1.414, 0.1);
    let target = card("t", None, 0.5);
    let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 3, 3);
    assert_eq!(mem.target.node_id.as_ref(), "t");
}

// ----------------------------------------------------------
// 合成器与卡片系统协同
// ----------------------------------------------------------

#[test]
fn synthesizer_with_card_system_cross_module() {
    let system = build_chain_system();
    let target = system.get_card_by_node("target").expect("存在").clone();
    let synth = OnDemandSynthesizer::new();
    // Draft: 祖先 a + root
    let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 5, 5);
    assert!(!mem.context_cards.is_empty(), "Draft 应有祖先上下文");
    assert!(!mem.ancestor_insights.is_empty());
    // 祖先链应含 a（直接父）
    let ancestor_nodes: Vec<&str> = mem
        .context_cards
        .iter()
        .map(|c| c.node_id.as_ref())
        .collect();
    assert!(ancestor_nodes.contains(&"a"), "应含直接父节点 a");
}

#[test]
fn four_operators_differential_context() {
    let system = build_chain_system();
    let target = system.get_card_by_node("target").expect("存在").clone();
    let synth = OnDemandSynthesizer::new();
    // 四算子各自产生不同的上下文选择
    let draft = synth.synthesize(&system, &target, &AtomicOperator::Draft, 5, 5);
    let improve = synth.synthesize(&system, &target, &AtomicOperator::Improve, 5, 5);
    let crossover = synth.synthesize(&system, &target, &AtomicOperator::Crossover, 5, 5);
    let debug = synth.synthesize(&system, &target, &AtomicOperator::Debug, 5, 5);
    // Draft 有祖先无兄弟模式
    assert!(draft.sibling_patterns.is_empty());
    // Crossover 无祖先洞察
    assert!(crossover.ancestor_insights.is_empty());
    // Debug 无错误签名 → 空上下文
    assert!(debug.context_cards.is_empty());
    // Improve 选高分兄弟
    assert!(!improve.context_cards.is_empty());
}

#[test]
fn debug_reuses_same_error_hash_fix() {
    let mut system = ExperienceCardSystem::new(1.414, 0.1);
    let mut target = card("target", Some("a"), 0.3);
    target.error_signature = Some(ErrorSignature {
        error_type: Box::from("compile_error"),
        error_location: Box::from("src/x.rs"),
        error_summary: Box::from("E0308"),
        error_hash: Box::from("hash-shared"),
    });
    let mut fix = card("fix", Some("a"), 0.9);
    fix.error_signature = Some(ErrorSignature {
        error_type: Box::from("compile_error"),
        error_location: Box::from("src/y.rs"),
        error_summary: Box::from("E0308 fixed"),
        error_hash: Box::from("hash-shared"),
    });
    system.add_card(card("a", None, 0.5));
    system.add_card(target.clone());
    system.add_card(fix);
    let synth = OnDemandSynthesizer::new();
    let mem = synth.synthesize(&system, &target, &AtomicOperator::Debug, 3, 5);
    assert_eq!(mem.context_cards.len(), 1, "Debug 应复用同哈希修复卡片");
    assert_eq!(mem.context_cards[0].node_id.as_ref(), "fix");
}

// ----------------------------------------------------------
// 懒加载边界约束（铁律5）
// ----------------------------------------------------------

#[test]
fn lazy_loading_respects_max_ancestors() {
    // 深链: root → m1 → m2 → m3 → m4 → target
    let mut system = ExperienceCardSystem::new(1.414, 0.1);
    system.add_card(card("root", None, 0.5));
    system.add_card(card("m1", Some("root"), 0.5));
    system.add_card(card("m2", Some("m1"), 0.5));
    system.add_card(card("m3", Some("m2"), 0.5));
    system.add_card(card("m4", Some("m3"), 0.5));
    system.add_card(card("target", Some("m4"), 0.7));
    let target = system.get_card_by_node("target").expect("存在").clone();
    let synth = OnDemandSynthesizer::new();
    // max_ancestors=2 → 只回溯 m4, m3（铁律5 懒加载约束）
    let mem = synth.synthesize(&system, &target, &AtomicOperator::Draft, 2, 0);
    assert!(
        mem.ancestor_insights.len() <= 2,
        "懒加载应受 max_ancestors 约束（实际 {})",
        mem.ancestor_insights.len()
    );
}

// ----------------------------------------------------------
// proptest: 上下文规模不变量（铁律5）
// ----------------------------------------------------------

proptest! {
    /// 任意 max 边界下，合成上下文规模不超过约束（懒加载保证）
    #[test]
    fn synthesized_context_bounded(
        max_ancestors in 0usize..5,
        max_siblings in 0usize..5,
    ) {
        let system = build_chain_system();
        let target = system.get_card_by_node("target").prop_test_clone();
        let synth = OnDemandSynthesizer::new();
        let mem = synth.synthesize(&system, &target, &AtomicOperator::Improve, max_ancestors, max_siblings);
        // 上下文卡片数 ≤ 祖先上限 + 兄弟上限（Improve 策略）
        prop_assert!(mem.context_cards.len() <= max_ancestors + max_siblings);
    }
}

/// proptest 辅助 trait: 克隆卡片
trait PropTestClone {
    fn prop_test_clone(&self) -> ExperienceCard;
}
impl PropTestClone for Option<&ExperienceCard> {
    fn prop_test_clone(&self) -> ExperienceCard {
        self.expect("存在").clone()
    }
}
