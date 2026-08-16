//! 四套原子算子集成测试 — 顶层 API + 算子协同 + L0 契约消费（v3.4.0 §10.1）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 四算子类型覆盖 /
//! 算子链协同（Draft→Improve→Debug）/ CardQuery 依赖倒置注入

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use gsoe_evolution::{
    AtomicOperatorTrait, CardQuery, CrossoverOperator, DebugOperator, DraftOperator,
    ImproveOperator, OperatorContext, OperatorError,
};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;

fn parent_card(score: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: "parent-1".into(),
        task_id: "task-1".into(),
        node_id: "node-1".into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Draft,
        score,
        delta_vs_parent: 0.0,
        method_family: "test".into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

fn base_context() -> OperatorContext {
    OperatorContext {
        task_id: "task-1".to_string(),
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
    // 四算子均可通过 crate 顶层访问（re-export 验证）
    let _draft = DraftOperator;
    let _improve = ImproveOperator;
    let _debug = DebugOperator;
    let _crossover = CrossoverOperator;
    assert_eq!(DraftOperator.operator_type(), AtomicOperator::Draft);
    assert_eq!(ImproveOperator.operator_type(), AtomicOperator::Improve);
    assert_eq!(DebugOperator.operator_type(), AtomicOperator::Debug);
    assert_eq!(CrossoverOperator.operator_type(), AtomicOperator::Crossover);
}

// ----------------------------------------------------------
// 四算子类型覆盖
// ----------------------------------------------------------

#[test]
fn all_four_operator_types_covered() {
    let types = [
        DraftOperator.operator_type(),
        ImproveOperator.operator_type(),
        DebugOperator.operator_type(),
        CrossoverOperator.operator_type(),
    ];
    assert_eq!(types[0], AtomicOperator::Draft);
    assert_eq!(types[1], AtomicOperator::Improve);
    assert_eq!(types[2], AtomicOperator::Debug);
    assert_eq!(types[3], AtomicOperator::Crossover);
}

// ----------------------------------------------------------
// 算子链协同（Draft → Improve → Debug）
// ----------------------------------------------------------

#[tokio::test]
async fn operator_chain_draft_improve_debug() {
    // 1. Draft 起草
    let draft = DraftOperator;
    let ctx = base_context();
    let draft_result = draft.execute(&ctx).await.expect("Draft 成功");
    assert_eq!(draft_result.operator, AtomicOperator::Draft);

    // 2. Improve 基于 Draft 结果改进（构造父卡片）
    let improve = ImproveOperator;
    let mut improve_ctx = base_context();
    improve_ctx.parent_card = Some(parent_card(draft_result.score));
    improve_ctx.code = Some(draft_result.code.clone());
    let improve_result = improve.execute(&improve_ctx).await.expect("Improve 成功");
    assert_eq!(improve_result.operator, AtomicOperator::Improve);
    assert!(improve_result.score > draft_result.score, "改进应提升评分");

    // 3. Debug 基于错误签名修复
    let debug = DebugOperator;
    let mut debug_ctx = base_context();
    debug_ctx.parent_card = Some(parent_card(improve_result.score));
    debug_ctx.code = Some(improve_result.code);
    debug_ctx.error_signature = Some(ErrorSignature {
        error_type: "compile_error".into(),
        error_location: "src/x.rs".into(),
        error_summary: "E0308".into(),
        error_hash: "hash-x".into(),
    });
    let debug_result = debug.execute(&debug_ctx).await.expect("Debug 成功");
    assert_eq!(debug_result.operator, AtomicOperator::Debug);
    assert_eq!(debug_result.execution_status, ExecutionStatus::Success);
}

// ----------------------------------------------------------
// CardQuery 依赖倒置注入
// ----------------------------------------------------------

/// Mock CardQuery — 验证依赖倒置注入（D-3）
struct MockCardQuery {
    cards: Vec<ExperienceCard>,
}

#[async_trait]
impl CardQuery for MockCardQuery {
    async fn query_by_error_signature(
        &self,
        _error_hash: &str,
        limit: usize,
    ) -> Vec<ExperienceCard> {
        self.cards.iter().take(limit).cloned().collect()
    }

    async fn query_by_three_factor(
        &self,
        _task_id: &str,
        _min_quality: f32,
        k: usize,
    ) -> Vec<ExperienceCard> {
        self.cards.iter().take(k).cloned().collect()
    }
}

#[tokio::test]
async fn crossover_with_injected_card_query() {
    let c1 = parent_card(0.8);
    let mut c2 = parent_card(0.6);
    c2.card_id = "parent-2".into();
    let mut ctx = base_context();
    ctx.card_query = Some(Arc::new(MockCardQuery {
        cards: vec![c1, c2],
    }));
    let op = CrossoverOperator;
    let result = op.execute(&ctx).await.expect("Crossover 成功");
    assert_eq!(result.operator, AtomicOperator::Crossover);
    assert!((result.score - 0.7).abs() < 1e-6, "(0.8+0.6)/2=0.7");
}

#[tokio::test]
async fn crossover_without_card_query_fails() {
    // 无 card_query 注入 → 候选为空 → InsufficientCandidates（D-3 解耦验证）
    let ctx = base_context();
    let op = CrossoverOperator;
    assert!(matches!(
        op.execute(&ctx).await,
        Err(OperatorError::InsufficientCandidates)
    ));
}

// ----------------------------------------------------------
// 资源成本估算
// ----------------------------------------------------------

#[test]
fn estimate_cost_ordering() {
    let ctx = base_context();
    // Draft > Crossover > Improve > Debug（token 消耗）
    let draft_cost = DraftOperator.estimate_cost(&ctx).estimated_tokens;
    let crossover_cost = CrossoverOperator.estimate_cost(&ctx).estimated_tokens;
    let improve_cost = ImproveOperator.estimate_cost(&ctx).estimated_tokens;
    let debug_cost = DebugOperator.estimate_cost(&ctx).estimated_tokens;
    assert!(draft_cost > crossover_cost);
    assert!(crossover_cost > improve_cost);
    assert!(improve_cost > debug_cost);
}
