//! 算子路由器集成测试 — 四策略 + L5 算子执行协同 + L0 契约接线（v3.4.0 §11.2）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ L0 OperatorSelectionStrategy 消费 /
//! 四策略分派 / select→execute 执行链路（L5 协同）/ 历史 append-only /
//! ThreeFactorSelector Softmax 委托 / proptest 计数不变量

#![forbid(unsafe_code)]

use faae_router::{OperatorRouter, OperatorSelectionRecord};
use gsoe_evolution::{OperatorContext, ThreeFactorSelector};
use nexus_contracts::experience_card::{AtomicOperator, ExecutionStatus};
use nexus_contracts::OperatorSelectionStrategy;
use proptest::prelude::*;

fn context() -> OperatorContext {
    OperatorContext {
        task_id: "t-1".to_string(),
        task_type: "code_gen".to_string(),
        parent_card: None,
        error_signature: None,
        requirements: "build a parser".to_string(),
        code: None,
        card_query: None,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let router = OperatorRouter::new(OperatorSelectionStrategy::default());
    // L0 契约默认策略 = ThreeFactor（D3 六维控制面）
    assert_eq!(
        router.selection_strategy(),
        OperatorSelectionStrategy::ThreeFactor
    );
    assert_eq!(router.total_selections(), 0);
    assert!(router.export_history().is_empty());
}

// ----------------------------------------------------------
// L0 OperatorSelectionStrategy 消费接线（四策略全覆盖）
// ----------------------------------------------------------

#[test]
fn all_four_strategies_dispatch() {
    let strategies = [
        OperatorSelectionStrategy::Greedy,
        OperatorSelectionStrategy::ThreeFactor,
        OperatorSelectionStrategy::Ucb,
        OperatorSelectionStrategy::Cooling,
    ];
    for strategy in strategies {
        let mut router = OperatorRouter::new(strategy);
        let selected = router.select_operator("code_gen", &context());
        assert!(selected.is_some(), "策略 {strategy:?} 应返回适用算子");
        assert_eq!(router.total_selections(), 1);
    }
}

// ----------------------------------------------------------
// select → execute 执行链路（L5 四算子协同）
// ----------------------------------------------------------

#[tokio::test]
async fn select_then_execute_closed_loop() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Ucb);
    // 空历史 UCB: 未访问算子 MAX 优先（确定性取首个适用）
    let selected = router
        .select_operator("code_gen", &context())
        .expect("选择成功");
    // 通过注册表获取算子实例并执行（L6 路由 → L5 算子闭环）
    let operator = router.get_operator(selected).expect("算子已注册");
    let result = operator.execute(&context()).await.expect("执行成功");
    assert_eq!(result.operator, selected);
    // 执行结果回录历史（闭环）
    router.record_result("code_gen", selected, result.score, result.execution_status);
    assert_eq!(router.export_history().len(), 1);
}

// ----------------------------------------------------------
// 历史记录 append-only（铁律3）
// ----------------------------------------------------------

#[test]
fn history_append_only_preserves_records() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    router.record_result("t", AtomicOperator::Draft, 0.5, ExecutionStatus::Success);
    router.record_result("t", AtomicOperator::Debug, 0.2, ExecutionStatus::Error);
    router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
    let history: Vec<OperatorSelectionRecord> = router.export_history();
    assert_eq!(history.len(), 3);
    // 既有记录不被修改（顺序与分数保持）
    assert_eq!(history[0].result_score, 0.5);
    assert_eq!(history[1].execution_status, ExecutionStatus::Error);
    assert_eq!(history[2].result_score, 0.9);
}

// ----------------------------------------------------------
// 历史驱动的策略偏好（Greedy 利用导向）
// ----------------------------------------------------------

#[test]
fn greedy_learns_from_history() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    // Draft 高分历史 / Crossover 低分历史
    for _ in 0..3 {
        router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
        router.record_result(
            "t",
            AtomicOperator::Crossover,
            0.2,
            ExecutionStatus::Success,
        );
    }
    // 多次选择应稳定偏好 Draft（Greedy 利用导向）
    let mut draft_count = 0;
    for _ in 0..10 {
        if router.select_operator("t", &context()) == Some(AtomicOperator::Draft) {
            draft_count += 1;
        }
    }
    assert_eq!(draft_count, 10, "Greedy 应稳定偏好高分算子");
}

// ----------------------------------------------------------
// ThreeFactorSelector Softmax 委托（D-3 闭环）
// ----------------------------------------------------------

#[test]
fn three_factor_softmax_delegation_closed_loop() {
    let selector = ThreeFactorSelector::new(1.414, 0.1, 1.0);
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::ThreeFactor)
        .with_three_factor_selector(selector);
    // 委托路径多轮选择不 panic 且只返回适用算子（默认上下文 Draft/Crossover 适用）
    for _ in 0..30 {
        let selected = router.select_operator("t", &context());
        assert!(matches!(
            selected,
            Some(AtomicOperator::Draft) | Some(AtomicOperator::Crossover)
        ));
    }
    assert_eq!(router.total_selections(), 30);
}

// ----------------------------------------------------------
// proptest: 计数不变量
// ----------------------------------------------------------

proptest! {
    /// 任意记录序列: total_selections 单调递增，历史长度 = 记录次数
    #[test]
    fn counting_invariants(
        n_selects in 0u32..20,
        n_records in 0usize..20,
        score in 0.0f32..1.0,
    ) {
        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        for _ in 0..n_selects {
            router.select_operator("t", &context());
        }
        prop_assert_eq!(router.total_selections(), n_selects);
        for i in 0..n_records {
            router.record_result("t", AtomicOperator::Draft, score, ExecutionStatus::Success);
            prop_assert_eq!(router.export_history().len(), i + 1);
        }
    }
}
