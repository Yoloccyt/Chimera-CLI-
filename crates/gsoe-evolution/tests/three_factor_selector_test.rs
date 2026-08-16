//! 三因子父本选择器集成测试 — 顶层 API + UCB/Softmax/冷却行为（v3.4.0 §10.2）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 空候选边界 / UCB 未访问优先 /
//! Softmax 低温集中性 / visit_counts 更新 / 冷却随访问数增长

#![forbid(unsafe_code)]

use chrono::Utc;
use gsoe_evolution::ThreeFactorSelector;
use nexus_contracts::experience_card::{AtomicOperator, CardMetadata, ExecutionStatus};
use nexus_contracts::{ExperienceCard, ThreeFactorScore};

fn card(node: &str, quality: f32, progress: f32, novelty: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: format!("card-{node}").into(),
        task_id: "task-1".into(),
        node_id: node.into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Draft,
        score: quality,
        delta_vs_parent: progress,
        method_family: "test".into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality,
            progress,
            novelty,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    // ThreeFactorSelector 可通过 crate 顶层访问
    let selector = ThreeFactorSelector::new(1.414, 0.1, 1.0);
    assert_eq!(selector.total_visits(), 0);
    assert_eq!(selector.visit_count("any"), 0);
}

// ----------------------------------------------------------
// 边界: 空候选返回 None
// ----------------------------------------------------------

#[test]
fn select_empty_candidates_returns_none() {
    let mut selector = ThreeFactorSelector::new(1.414, 0.1, 1.0);
    assert!(selector.select(&[]).is_none());
    // 空选择不更新访问计数
    assert_eq!(selector.total_visits(), 0);
}

// ----------------------------------------------------------
// UCB 未访问节点优先
// ----------------------------------------------------------

#[test]
fn unvisited_nodes_selected_first_deterministically() {
    // 未访问节点 UCB bonus = MAX，按候选顺序确定性选择
    let mut selector = ThreeFactorSelector::new(1.414, 0.0, 1.0);
    let c1 = card("n1", 0.1, 0.0, 0.1);
    let c2 = card("n2", 0.9, 0.5, 0.9);
    // 第一次: 两节点均未访问 → 返回第一个 MAX 候选（n1）
    let first = selector
        .select(&[c1.clone(), c2.clone()])
        .expect("选择成功");
    assert_eq!(first.node_id.as_ref(), "n1");
    // 第二次: n2 仍未访问 → MAX 优先（即使 n1 分数低也不重复选）
    let second = selector
        .select(&[c1.clone(), c2.clone()])
        .expect("选择成功");
    assert_eq!(second.node_id.as_ref(), "n2");
    assert_eq!(selector.visit_count("n1"), 1);
    assert_eq!(selector.visit_count("n2"), 1);
}

// ----------------------------------------------------------
// Softmax 温度采样: 低温下高 utility 节点主导
// ----------------------------------------------------------

#[test]
fn softmax_low_temperature_favors_high_utility() {
    let mut selector = ThreeFactorSelector::new(0.0, 0.0, 0.1);
    let high = card("high", 0.9, 0.5, 0.9);
    let low = card("low", 0.1, 0.0, 0.1);
    // 先各访问一次消除 UCB MAX 优先
    selector.select(std::slice::from_ref(&high));
    selector.select(std::slice::from_ref(&low));
    let mut high_count = 0u32;
    for _ in 0..50 {
        if let Some(selected) = selector.select(&[high.clone(), low.clone()]) {
            if selected.node_id.as_ref() == "high" {
                high_count += 1;
            }
        }
    }
    // 低温（0.1）下高 utility 节点应被多数选择（>60%）
    assert!(
        high_count > 30,
        "低温下高效用节点应主导（实际 {high_count}/50）"
    );
}

#[test]
fn select_single_candidate_always_returns_it() {
    let mut selector = ThreeFactorSelector::new(0.0, 0.0, 1.0);
    let only = card("only", 0.5, 0.1, 0.5);
    for _ in 0..10 {
        let selected = selector
            .select(std::slice::from_ref(&only))
            .expect("选择成功");
        assert_eq!(selected.node_id.as_ref(), "only");
    }
    assert_eq!(selector.total_visits(), 10);
    assert_eq!(selector.visit_count("only"), 10);
}

// ----------------------------------------------------------
// visit_counts / total_visits 可观测性更新
// ----------------------------------------------------------

#[test]
fn visit_counts_accumulate_across_selections() {
    let mut selector = ThreeFactorSelector::new(0.0, 0.0, 1.0);
    let c1 = card("n1", 0.9, 0.1, 0.5);
    let c2 = card("n2", 0.1, 0.0, 0.1);
    // 消除 UCB MAX（各访问一次）
    selector.select(&[c1.clone(), c2.clone()]);
    selector.select(&[c1.clone(), c2.clone()]);
    // 后续选择计数守恒: 各节点计数之和 = 总访问数
    for _ in 0..20 {
        selector.select(&[c1.clone(), c2.clone()]);
    }
    assert_eq!(selector.total_visits(), 22);
    assert_eq!(selector.visit_count("n1") + selector.visit_count("n2"), 22);
}

// ----------------------------------------------------------
// 冷却: 全候选已被访问后仍可正常选择（收敛验证）
// ----------------------------------------------------------

#[test]
fn cooling_does_not_starve_selection() {
    // 冷却系数较大时，utility 被扣减但仍产生有效选择（无 NaN/panic）
    let mut selector = ThreeFactorSelector::new(0.0, 1.0, 1.0);
    let c1 = card("n1", 0.9, 0.1, 0.5);
    for _ in 0..30 {
        assert!(
            selector.select(std::slice::from_ref(&c1)).is_some(),
            "冷却增长不应阻断选择"
        );
    }
    assert_eq!(selector.total_visits(), 30);
}
