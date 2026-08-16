//! 经验卡片系统集成测试 — L0 契约消费 + L1 总线接线闭环（v3.4.0 §7.1 + D-7）
//!
//! 覆盖: 顶层 API 可达性 / L0 ExperienceCard 全链路消费 / 三因子父本选择 /
//! **L1 ExperienceCardBus → L2 MlcEngine 消费闭环（D-7 接线）** / proptest 不变量

#![forbid(unsafe_code)]

use std::time::Duration;

use chrono::{DateTime, Utc};
use event_bus::{EventBus, ExperienceCardBus};
use mlc_engine::{ExperienceCardSystem, MlcEngine};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;

/// 构造样例卡片
fn card(node: &str, method: &str, score: f32, status: ExecutionStatus) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card-{node}")),
        task_id: Box::from("t1"),
        node_id: Box::from(node),
        parent_id: None,
        created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("合法时间戳"),
        operator: AtomicOperator::Draft,
        score,
        delta_vs_parent: 0.0,
        method_family: Box::from(method),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: status,
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
    use mlc_engine::prelude::*;
    let system = ExperienceCardSystem::new(1.414, 0.1);
    assert_eq!(system.card_count(), 0);
    let _board = GlobalExperienceBoard::default();
    let _stats = MethodStatistics::default();
}

// ----------------------------------------------------------
// L0 ExperienceCard 全链路消费
// ----------------------------------------------------------

#[test]
fn l0_card_full_consumption_flow() {
    let mut system = ExperienceCardSystem::new(1.414, 0.1);
    // 消费多张 L0 卡片（Phase 0 契约类型）
    system.add_card(card("n1", "draft_pipeline", 0.9, ExecutionStatus::Success));
    system.add_card(card("n2", "draft_pipeline", 0.4, ExecutionStatus::Error));
    system.add_card(card("n3", "two_pass_debug", 0.7, ExecutionStatus::Success));

    // 全局板聚合
    let board = system.global_board();
    assert_eq!(board.total_nodes, 3);
    assert_eq!(board.total_evaluated, 2);
    assert!((board.best_score - 0.9).abs() < 1e-6);
    // 方法分布
    assert_eq!(board.method_distribution.get("draft_pipeline"), Some(&2));
    assert_eq!(board.method_distribution.get("two_pass_debug"), Some(&1));
    // 方法家族统计
    assert_eq!(system.method_stats().len(), 2);
}

#[test]
fn three_factor_parent_selection_end_to_end() {
    let mut system = ExperienceCardSystem::new(1.414, 0.0);
    let c1 = card("n1", "fam_a", 0.9, ExecutionStatus::Success);
    let c2 = card("n2", "fam_a", 0.3, ExecutionStatus::Success);
    // 先各访问一次消除 UCB MAX
    let _ = system.select_parent(&[&c1]);
    let _ = system.select_parent(&[&c2]);
    // 三因子高效用优先
    let selected = system.select_parent(&[&c1, &c2]).expect("非空");
    assert_eq!(selected.node_id.as_ref(), "n1");
}

#[test]
fn error_signature_clustering_from_l0_card() {
    let mut system = ExperienceCardSystem::new(1.414, 0.1);
    let mut c = card("n1", "fam_a", 0.3, ExecutionStatus::Error);
    c.error_signature = Some(ErrorSignature {
        error_type: Box::from("compile_error"),
        error_location: Box::from("src/lib.rs:42"),
        error_summary: Box::from("E0308 type mismatch"),
        error_hash: Box::from("abc123def456"),
    });
    system.add_card(c);
    let clusters = &system.global_board().error_clusters;
    assert_eq!(clusters.get("compile_error").map(Vec::len), Some(1));
}

// ----------------------------------------------------------
// D-7: L1 ExperienceCardBus → L2 MlcEngine 消费闭环
// ----------------------------------------------------------

#[tokio::test]
async fn card_bus_to_mlc_engine_consumption_closed_loop() {
    // L1 经验卡片总线 + L1 EventBus
    let card_bus = ExperienceCardBus::new();
    let event_bus = EventBus::new();
    // L2 MlcEngine 注入 card_bus（D-7 接线，启动后台消费）
    let engine = MlcEngine::new_in_memory(event_bus)
        .expect("引擎创建成功")
        .with_card_bus(&card_bus);

    // 发布中分卡片（0.5-0.8 走 broadcast，后台任务消费）
    card_bus.publish(card("n1", "draft_pipeline", 0.7, ExecutionStatus::Success));
    card_bus.publish(card("n2", "draft_pipeline", 0.6, ExecutionStatus::Success));

    // 等待后台消费任务处理（broadcast 异步投递）
    tokio::time::sleep(Duration::from_millis(200)).await;

    // L2 card_system 应已填充 2 张卡片
    let (count, total_nodes) = engine.card_system_snapshot();
    assert_eq!(count, 2, "L2 应消费 L1 总线的 2 张卡片");
    assert_eq!(total_nodes, 2);
}

#[tokio::test]
async fn high_score_cards_not_in_broadcast_consumption() {
    // 高分卡片（>0.8）走 Critical mpsc，不进 broadcast
    // with_card_bus 订阅 broadcast，故高分卡不被此路径消费
    let card_bus = ExperienceCardBus::new();
    let event_bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(event_bus)
        .expect("引擎创建成功")
        .with_card_bus(&card_bus);

    card_bus.publish(card("n1", "fam", 0.95, ExecutionStatus::Success)); // 高分 → Critical
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (count, _) = engine.card_system_snapshot();
    assert_eq!(count, 0, "高分卡走 Critical，broadcast 消费路径不应收到");
}

#[test]
fn manual_ingest_experience_card() {
    // 手动注入（非 runtime 场景，与 with_card_bus 互补）
    let event_bus = EventBus::new();
    let engine = MlcEngine::new_in_memory(event_bus).expect("引擎创建成功");
    engine.ingest_experience_card(card("n1", "fam", 0.8, ExecutionStatus::Success));
    let (count, _) = engine.card_system_snapshot();
    assert_eq!(count, 1, "手动注入应填充 card_system");
}

// ----------------------------------------------------------
// proptest: 消费不变量
// ----------------------------------------------------------

proptest! {
    /// 任意卡片序列消费后，total_nodes 恒等于卡片数，best_score ≥ 各卡片分
    #[test]
    fn consumption_invariants(
        scores in proptest::collection::vec(0.0f32..1.0, 1..20),
    ) {
        let mut system = ExperienceCardSystem::new(1.414, 0.1);
        for (i, score) in scores.iter().enumerate() {
            system.add_card(card(&format!("n{i}"), "fam", *score, ExecutionStatus::Success));
        }
        let board = system.global_board();
        prop_assert_eq!(board.total_nodes as usize, scores.len());
        let max_score = scores.iter().cloned().fold(0.0f32, f32::max);
        prop_assert!(board.best_score >= max_score - 1e-6);
        prop_assert_eq!(system.card_count(), scores.len());
    }
}
