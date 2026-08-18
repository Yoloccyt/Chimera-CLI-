//! 经验卡片生成器集成测试 — L0 契约消费 + L1 总线投递 + L4 协同（v3.4.0 §12.1）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ L0 ExperienceCard 契约消费 /
//! L1 ExperienceCardBus 双通道投递闭环 / L4 classify + compute_error_hash 协同 /
//! proptest 三因子不变量

#![forbid(unsafe_code)]

use event_bus::ExperienceCardBus;
use nexus_contracts::experience_card::{AtomicOperator, ExecutionStatus};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;
use pvl_layer::{CardValidationInput, ExecutionMetadata, ExperienceCardGenerator};

fn metadata(operator: AtomicOperator) -> ExecutionMetadata {
    ExecutionMetadata {
        task_id: "task-1".to_string(),
        parent_id: None,
        operator,
        skills_used: vec!["skill-a".to_string()],
    }
}

fn validation(success: bool, score: f32) -> CardValidationInput {
    CardValidationInput {
        success,
        score,
        error_type: if success {
            None
        } else {
            Some("compile_error".to_string())
        },
        error_location: if success {
            None
        } else {
            Some("src/x.rs:1".to_string())
        },
        error_message: if success {
            None
        } else {
            Some("mismatched types".to_string())
        },
        timed_out: false,
        execution_time_ms: 100,
        prompt_tokens: 1000,
        completion_tokens: 1000,
        lines_changed: 5,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let gen = ExperienceCardGenerator::new("2.26.0-omega");
    assert_eq!(gen.generated_count(), 0);
}

// ----------------------------------------------------------
// L0 契约消费：生成卡片字段完整性
// ----------------------------------------------------------

#[test]
fn generated_card_consumes_l0_contract() {
    let gen = ExperienceCardGenerator::new("2.26.0-omega");
    let card: ExperienceCard =
        gen.generate(&metadata(AtomicOperator::Improve), &validation(true, 0.7));
    assert_eq!(card.operator, AtomicOperator::Improve);
    assert_eq!(card.task_id.as_ref(), "task-1");
    assert_eq!(card.method_family.as_ref(), "iterative_improvement");
    assert_eq!(card.execution_status, ExecutionStatus::Success);
    // 元数据完整性
    assert_eq!(card.metadata.execution_time_ms, 100);
    assert_eq!(card.metadata.skills_used.len(), 1);
    assert_eq!(card.metadata.token_usage.total_tokens, 2000);
}

// ----------------------------------------------------------
// L4 协同：classify + compute_error_hash 单一来源
// ----------------------------------------------------------

#[test]
fn error_hash_matches_l4_single_source() {
    let gen = ExperienceCardGenerator::new("v1");
    let card = gen.generate(&metadata(AtomicOperator::Debug), &validation(false, 0.2));
    let sig = card.error_signature.expect("失败卡片应有签名");
    // 与 L4 compute_error_hash 直接计算一致（铁律7 单一来源）
    let expected = seccore::compute_error_hash("compile_error", "mismatched types");
    assert_eq!(sig.error_hash.as_ref(), expected.as_str());
}

#[test]
fn status_classification_matches_l4_integrator() {
    let gen = ExperienceCardGenerator::new("v1");
    // 超时输入 → Timeout（与 L4 classify 一致，铁律8）
    let mut v = validation(false, 0.1);
    v.timed_out = true;
    let card = gen.generate(&metadata(AtomicOperator::Draft), &v);
    let expected = seccore::ExecutionFeedbackIntegrator::classify(
        false,
        true,
        true,
        Some(0.1),
        true,
        Some("mismatched types"),
    );
    assert_eq!(card.execution_status, expected);
    assert_eq!(card.execution_status, ExecutionStatus::Timeout);
}

// ----------------------------------------------------------
// L1 总线投递闭环（D-5 双通道分级）
// ----------------------------------------------------------

#[tokio::test]
async fn card_bus_delivery_closed_loop() {
    let bus = ExperienceCardBus::new();
    let mut critical_rx = bus.subscribe_critical();
    let mut broadcast_rx = bus.subscribe();
    let gen = ExperienceCardGenerator::new("v1").with_card_bus(bus);

    // 高分卡片（0.95 > 0.8）→ Critical 通道
    gen.generate_and_publish(&metadata(AtomicOperator::Draft), &validation(true, 0.95));
    let critical_card = critical_rx.try_recv();
    assert!(critical_card.is_ok(), "高分卡片应经 Critical 通道");

    // 中分卡片（0.6 ∈ (0.5, 0.8]）→ broadcast 通道
    gen.generate_and_publish(&metadata(AtomicOperator::Draft), &validation(true, 0.6));
    let broadcast_card = broadcast_rx.try_recv();
    assert!(broadcast_card.is_ok(), "中分卡片应经 broadcast 通道");
}

// ----------------------------------------------------------
// proptest：三因子不变量
// ----------------------------------------------------------

proptest! {
    /// 任意评分生成卡片: quality ∈ [0,1]，novelty ∈ [0,1]，card_id 唯一单调
    #[test]
    fn three_factor_invariants(score in 0.0f32..2.0) {
        let gen = ExperienceCardGenerator::new("v1");
        let card = gen.generate(&metadata(AtomicOperator::Draft), &validation(score <= 1.0, score));
        prop_assert!((0.0..=1.0).contains(&card.three_factor.quality));
        prop_assert!((0.0..=1.0).contains(&card.three_factor.novelty));
        prop_assert_eq!(card.three_factor.progress, 0.0, "生成时 progress 恒 0（消费方回填）");
    }
}
