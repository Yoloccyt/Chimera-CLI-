//! 三因子裁决器集成测试 — L0 契约消费 + 决策矩阵（v3.4.0 §13.1）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ L0 ThreeFactorScore 契约消费闭环 /
//! 三角色投票决策矩阵 / L7 process_score 填充协同 / proptest 决策确定性不变量

#![forbid(unsafe_code)]

use nexus_contracts::VariantId;
use parliament::{
    AdjudicationResult, ParliamentDecision, SmokeResults, ThreeFactorAdjudicator,
    VariantPerformance, Vote,
};
use proptest::prelude::*;

fn adjudicator() -> ThreeFactorAdjudicator {
    ThreeFactorAdjudicator::new(0.05, 0.6, 0.6, 0.1)
}

fn variant(avg_score: f32, config_hash: u64, history_len: usize) -> VariantPerformance {
    VariantPerformance {
        variant_id: VariantId::new("spec-a", 1),
        avg_score,
        history_scores: vec![avg_score; history_len],
        config_hash,
        process_score: None,
    }
}

fn smoke(has_regression: bool) -> SmokeResults {
    SmokeResults {
        tests_passed: 10,
        tests_failed: if has_regression { 2 } else { 0 },
        has_regression,
        regression_details: Vec::new(),
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use parliament::prelude::*;
    let adj = ThreeFactorAdjudicator::new(0.05, 0.6, 0.6, 0.1);
    let result = adj.adjudicate_variant(&variant(0.8, 1, 5), &variant(0.6, 1, 5), &smoke(false));
    assert_eq!(result.votes.len(), 3);
}

// ----------------------------------------------------------
// L0 ThreeFactorScore 契约消费闭环
// ----------------------------------------------------------

#[test]
fn three_factor_contract_consumption() {
    let result: AdjudicationResult =
        adjudicator().adjudicate_variant(&variant(0.8, 2, 50), &variant(0.6, 1, 0), &smoke(false));
    // L0 ThreeFactorScore 契约字段完整填充
    assert!((result.three_factor.quality - 0.8).abs() < 1e-6);
    // progress = quality_delta = 0.2
    assert!((result.three_factor.progress - 0.2).abs() < 1e-6);
    // novelty = config_diff(0.5) + history_bonus(50/100=0.5) = 1.0
    assert!((result.three_factor.novelty - 1.0).abs() < 1e-6);
    // selection_utility（L0 契约纯函数）可用于下游选择器
    assert!(result.three_factor.selection_utility() > 0.0);
}

// ----------------------------------------------------------
// 三角色投票决策矩阵
// ----------------------------------------------------------

#[test]
fn decision_matrix_full_coverage() {
    let adj = adjudicator();
    // 全 Approve → Approve
    let r1 = adj.adjudicate_variant(&variant(0.8, 1, 10), &variant(0.6, 1, 10), &smoke(false));
    assert_eq!(r1.decision, ParliamentDecision::Approve);
    // 回归一票否决 → Security Reject
    let r2 = adj.adjudicate_variant(&variant(0.9, 1, 10), &variant(0.6, 1, 10), &smoke(true));
    assert_eq!(
        r2.decision,
        ParliamentDecision::Reject("Security: regression detected".to_string())
    );
    // 三态投票记录完整（角色名固定）
    let roles: Vec<&str> = r1.votes.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(roles, vec!["Skeptic", "Security", "Execution"]);
}

#[test]
fn abstain_vote_semantics() {
    // progress ∈ (0, 阈值] → Skeptic Abstain（证据不足但未拒绝）
    let adj = ThreeFactorAdjudicator::new(0.5, 0.6, 0.5, 0.1);
    let result = adj.adjudicate_variant(&variant(0.61, 1, 0), &variant(0.6, 1, 0), &smoke(false));
    let skeptic = result
        .votes
        .iter()
        .find(|(name, _)| name == "Skeptic")
        .map(|(_, vote)| vote)
        .expect("Skeptic 投票存在");
    assert_eq!(*skeptic, Vote::Abstain);
}

// ----------------------------------------------------------
// L7 process_score 填充协同（D-5 数值松耦合）
// ----------------------------------------------------------

#[test]
fn process_score_input_passthrough() {
    // 调用方从 L7 TrajectoryProcessScore.overall() 填充（数值松耦合）
    let mut v = variant(0.8, 1, 5);
    v.process_score = Some(0.87); // L7 九维总分
    assert_eq!(v.process_score, Some(0.87));
    // 裁决流程不受影响（process_score 为可观测输入）
    let result = adjudicator().adjudicate_variant(&v, &variant(0.6, 1, 5), &smoke(false));
    assert_eq!(result.decision, ParliamentDecision::Approve);
}

// ----------------------------------------------------------
// proptest：决策确定性不变量
// ----------------------------------------------------------

proptest! {
    /// 同输入恒同决策（铁律4 纯函数）；决策恒为三枚举之一
    #[test]
    fn decision_deterministic(
        variant_score in 0.0f32..1.0,
        baseline_score in 0.0f32..1.0,
        has_regression in any::<bool>(),
    ) {
        let adj = adjudicator();
        let v = variant(variant_score, 1, 5);
        let b = variant(baseline_score, 1, 5);
        let r1 = adj.adjudicate_variant(&v, &b, &smoke(has_regression));
        let r2 = adj.adjudicate_variant(&v, &b, &smoke(has_regression));
        prop_assert_eq!(&r1.decision, &r2.decision, "同输入恒同决策");
        prop_assert!(matches!(
            &r1.decision,
            ParliamentDecision::Approve
                | ParliamentDecision::Reject(_)
                | ParliamentDecision::RequestMoreData(_)
        ));
        // 回归场景恒被 Security 否决
        if has_regression {
            prop_assert_eq!(
                &r1.decision,
                &ParliamentDecision::Reject("Security: regression detected".to_string())
            );
        }
    }
}
