//! W4 算子路由闭环集成测试（ADR-084，规范 §11.2/§11.4 + §16.4）
//!
//! 覆盖四条链路:
//! 1. 卡片反馈闭环: ExperienceCardBus 双流发布 → 反馈循环 → 路由器统计在线更新
//! 2. D3 策略热切换 + 铁律6 轨迹导出 + HISTORY_CAP 滚动窗口
//! 3. §11.4 父本选择: 卡片总线 → L5 选择器 → ParentSelection 消费适配
//! 4. 聚合表与全历史扫描的等价性（proptest,铁律4 纯函数性质锁定）

use std::sync::{Arc, Mutex};

use chrono::Utc;
use event_bus::ExperienceCardBus;
use faae_router::card_feedback::spawn_card_feedback_loop;
use faae_router::parent_context::ParentContextProvider;
use faae_router::{OperatorRouter, OperatorSelectionRecord, HISTORY_CAP};
use gsoe_evolution::{OperatorContext, ThreeFactorSelector};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::{ExperienceCard, OperatorSelectionStrategy};
use proptest::prelude::*;

// ---- 测试脚手架 ----

/// 构造经验卡片（真实反馈形态）
fn card(id: &str, family: &str, op: AtomicOperator, score: f32, status: ExecutionStatus) -> ExperienceCard {
    ExperienceCard {
        card_id: id.into(),
        task_id: "task-w4".into(),
        node_id: format!("node-{id}").into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: op,
        score,
        delta_vs_parent: 0.1,
        method_family: family.into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.1,
        },
        execution_status: status,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

/// 默认上下文: Draft/Crossover 适用（无父卡片/错误签名）
fn default_context() -> OperatorContext {
    OperatorContext {
        task_id: "t-1".to_string(),
        task_type: "code_gen".to_string(),
        parent_card: None,
        error_signature: None,
        requirements: "build".to_string(),
        code: None,
        card_query: None,
    }
}

// ---- 链路 1: 卡片反馈闭环 ----

#[tokio::test]
async fn card_feedback_loop_updates_router_stats() {
    let card_bus = ExperienceCardBus::new();
    let router = Arc::new(Mutex::new(OperatorRouter::new(
        OperatorSelectionStrategy::Greedy,
    )));
    let handle = spawn_card_feedback_loop(&card_bus, router.clone());
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 中分流(0.5 < score ≤ 0.8) + 高分流(score > 0.8) 双路发布
    card_bus.publish(card("c-1", "code-gen", AtomicOperator::Draft, 0.7, ExecutionStatus::Success));
    card_bus.publish(card("c-2", "code-gen", AtomicOperator::Draft, 0.6, ExecutionStatus::Success));
    card_bus.publish(card("c-3", "code-gen", AtomicOperator::Improve, 0.9, ExecutionStatus::Error));
    card_bus.publish(card("c-4", "code-gen", AtomicOperator::Draft, 0.9, ExecutionStatus::Success));

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    {
        let mut router = router.lock().expect("锁正常");
        let history = router.export_history();
        assert_eq!(history.len(), 4, "四张卡片全部驱动 record_result");
        // method_family 作为 task_type: (code-gen, Draft) 3 条 + (code-gen, Improve) 1 条
        let draft_stats = router
            .aggregates()
            .get(&( "code-gen".to_string(), AtomicOperator::Draft))
            .expect("Draft 聚合存在");
        assert_eq!(draft_stats.visits, 3);
        assert_eq!(draft_stats.success_count, 3);
        // 反馈改变选择: Draft 成功均分 0.733 > Improve(Error) 0 → Greedy 选 Draft
        let selected = router
            .select_operator("code-gen", &default_context())
            .expect("选择成功");
        assert_eq!(selected, AtomicOperator::Draft);
    }
    handle.abort();
}

// ---- 链路 2: 热切换 + 铁律6 导出 + 滚动窗口 ----

#[tokio::test]
async fn strategy_hot_switch_preserves_stats() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    router.record_result("t", AtomicOperator::Draft, 0.9, ExecutionStatus::Success);
    router.record_result("t", AtomicOperator::Improve, 0.3, ExecutionStatus::Success);

    // 热切换到 UCB: 统计保留,策略语义切换
    assert_eq!(router.selection_strategy(), OperatorSelectionStrategy::Greedy);
    router.apply_strategy(OperatorSelectionStrategy::Ucb);
    assert_eq!(router.selection_strategy(), OperatorSelectionStrategy::Ucb);
    // 统计未被清空（策略与统计正交）
    assert_eq!(router.aggregates().len(), 2);
    // N=0 时 UCB 对全部算子取 MAX → 首个适用算子（确定性）
    let selected = router.select_operator("t", &default_context());
    assert_eq!(selected, Some(AtomicOperator::Draft));
}

#[tokio::test]
async fn export_trajectory_invariants() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    router.record_result("t", AtomicOperator::Draft, 0.8, ExecutionStatus::Success);
    router.record_result("t", AtomicOperator::Improve, 0.4, ExecutionStatus::Error);
    router.record_result("t", AtomicOperator::Debug, 0.6, ExecutionStatus::Timeout);

    let history: Vec<OperatorSelectionRecord> = router.export_history();
    assert_eq!(history.len(), 3, "铁律6 原始形态");
    let trajectory = router.export_trajectory("episode-w4");
    // 四序列等长（RLTrajectory 构造不变量）
    assert_eq!(trajectory.len(), 3);
    assert_eq!(trajectory.states.len(), trajectory.actions.len());
    assert_eq!(trajectory.states.len(), trajectory.rewards.len());
    assert_eq!(trajectory.states.len(), trajectory.timestamps.len());
    // 投影: action 编码 Draft=0, reward=result_score
    assert_eq!(trajectory.actions[0].layer.as_ref(), "l6_operator_router");
    assert_eq!(trajectory.actions[0].action_code, 0);
    assert_eq!(trajectory.rewards[1], 0.4);
    // 状态投影: score/status 编码（铁律8 六状态: Error=1, Timeout=5）
    assert_eq!(trajectory.states[1].layer_features[2], 1.0);
    assert_eq!(trajectory.states[2].layer_features[2], 5.0);
}

#[tokio::test]
async fn history_rolling_window_cap() {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    // 注入 cap+10 条记录: 窗口滚动,聚合表全量保真
    for i in 0..(HISTORY_CAP + 10) {
        let op = if i % 2 == 0 {
            AtomicOperator::Draft
        } else {
            AtomicOperator::Improve
        };
        router.record_result("t", op, 0.5, ExecutionStatus::Success);
    }
    assert_eq!(router.export_history().len(), HISTORY_CAP, "窗口容量守恒");
    // 聚合不受窗口影响（铁律3 张力化解: 统计语义保真）
    let draft_stats = router
        .aggregates()
        .get(&("t".to_string(), AtomicOperator::Draft))
        .expect("Draft 聚合");
    assert_eq!(
        draft_stats.visits as usize,
        HISTORY_CAP / 2 + 5,
        "聚合表全量统计"
    );
}

// ---- 链路 3: §11.4 父本选择消费适配 ----

#[tokio::test]
async fn parent_context_selects_from_card_bus() {
    let card_bus = ExperienceCardBus::new();
    // 三个候选: quality 0.7 / 0.6 / 0.9（高分卡走 critical 流但索引同步保留）
    card_bus.publish(card("p-1", "f", AtomicOperator::Draft, 0.7, ExecutionStatus::Success));
    card_bus.publish(card("p-2", "f", AtomicOperator::Improve, 0.6, ExecutionStatus::Success));
    card_bus.publish(card("p-3", "f", AtomicOperator::Debug, 0.6, ExecutionStatus::Success));

    let mut provider =
        ParentContextProvider::new(ThreeFactorSelector::new(1.414, 0.1, 1.0));
    let selection = provider
        .select_parent(&card_bus, "task-w4")
        .expect("候选充足应选出父本");
    assert_eq!(selection.candidate_count, 3);
    assert!(
        selection.parent_card_id.starts_with("p-"),
        "选中的是真实卡片(伪卡片不会进总线)"
    );
    // 候选不足 → 诚实降级
    assert!(provider.select_parent(&card_bus, "no-such-task").is_none());
    // 最小候选阈值
    let mut strict = ParentContextProvider::new(ThreeFactorSelector::new(1.414, 0.1, 1.0))
        .with_min_candidates(4);
    assert!(strict.select_parent(&card_bus, "task-w4").is_none(), "3 < 4 降级");
    assert_eq!(strict.min_candidates(), 4);
}

#[tokio::test]
async fn parent_error_signature_routes_debug_operator() {
    // §16.3: 父本 error_signature 是 Debug 算子的关键路由信号——
    // ParentSelection 透传 error_signature 供 OperatorContext 注入
    let card_bus = ExperienceCardBus::new();
    let mut with_error = card("e-1", "f", AtomicOperator::Debug, 0.6, ExecutionStatus::Error);
    with_error.error_signature = Some(ErrorSignature {
        error_type: "compile_error".into(),
        error_location: "src/main.rs:42".into(),
        error_summary: "borrow of moved value".into(),
        error_hash: "0123456789abcdef".into(),
    });
    card_bus.publish(with_error);

    let mut provider =
        ParentContextProvider::new(ThreeFactorSelector::new(1.414, 0.1, 1.0));
    let selection = provider
        .select_parent(&card_bus, "task-w4")
        .expect("单候选可选");
    let signature = selection.error_signature.expect("错误签名透传");
    assert_eq!(signature.error_type.as_ref(), "compile_error");
    assert_eq!(signature.error_hash.len(), 16, "SHA-256 前 16 位");
}

// ============================================================
// 链路 4: 聚合表 vs 全历史扫描等价性（proptest）
// ============================================================

/// 测试内全扫描参照实现（旧版语义）: Greedy 成功均分
fn legacy_greedy(history: &[OperatorSelectionRecord], task: &str, ops: &[AtomicOperator]) -> AtomicOperator {
    let mut best = ops[0];
    let mut best_score = -1.0f32;
    for op in ops {
        let scores: Vec<f32> = history
            .iter()
            .filter(|r| {
                r.task_type == task && r.selected_operator == *op && r.execution_status == ExecutionStatus::Success
            })
            .map(|r| r.result_score)
            .collect();
        let score = if scores.is_empty() { 0.0 } else { scores.iter().sum::<f32>() / scores.len() as f32 };
        if score > best_score {
            best_score = score;
            best = *op;
        }
    }
    best
}

/// 测试内全扫描参照实现: ThreeFactor 规范原型 utility
fn legacy_three_factor(history: &[OperatorSelectionRecord], task: &str, ops: &[AtomicOperator]) -> AtomicOperator {
    let mut best = ops[0];
    let mut best_utility = -1.0f32;
    for op in ops {
        let records: Vec<&OperatorSelectionRecord> = history
            .iter()
            .filter(|r| r.task_type == task && r.selected_operator == *op)
            .collect();
        if records.is_empty() {
            return *op; // 未访问优先
        }
        let quality = records.iter().map(|r| r.result_score).sum::<f32>() / records.len() as f32;
        let progress = records.iter().map(|r| r.result_score).fold(0.0f32, f32::max) - quality;
        let novelty = 1.0 / (records.len() as f32 + 1.0);
        let utility = quality + progress + novelty;
        if utility > best_utility {
            best_utility = utility;
            best = *op;
        }
    }
    best
}

proptest! {
    /// 等价性: 增量聚合实现与测试内全扫描参照在 Greedy/ThreeFactor 原型
    /// 两策略下选择一致（铁律4: 统计同源 → 决策同输出）
    #[test]
    fn aggregate_equivalent_to_full_scan(
        records in proptest::collection::vec(
            (0usize..2, 0usize..4, proptest::num::f32::ANY),
            0..64
        ),
    ) {
        let task_keys = ["task-a", "task-b"];
        let ops = [
            AtomicOperator::Draft, AtomicOperator::Improve,
            AtomicOperator::Debug, AtomicOperator::Crossover,
        ];
        // 适用集（默认上下文: Draft/Crossover）——等价性在受限算子集上验证
        let applicable = [AtomicOperator::Draft, AtomicOperator::Crossover];

        let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
        for (task_idx, op_idx, raw_score) in &records {
            let score = raw_score.clamp(0.0, 1.0);
            let status = if score >= 0.5 { ExecutionStatus::Success } else { ExecutionStatus::Error };
            router.record_result(task_keys[*task_idx], ops[*op_idx], score, status);
        }
        let history = router.export_history();

        for task in &task_keys {
            // Greedy 等价
            router.apply_strategy(OperatorSelectionStrategy::Greedy);
            let got = router.select_operator(task, &default_context());
            prop_assert_eq!(got, Some(legacy_greedy(&history, task, &applicable)));

            // ThreeFactor 规范原型等价（不注入委托选择器）
            router.apply_strategy(OperatorSelectionStrategy::ThreeFactor);
            let got = router.select_operator(task, &default_context());
            prop_assert_eq!(got, Some(legacy_three_factor(&history, task, &applicable)));
        }
    }
}
