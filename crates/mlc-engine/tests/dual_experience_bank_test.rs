//! 双层经验库集成测试 — 案例级→全局蒸馏全链路（v3.4.0 §7.2）
//!
//! 覆盖: 顶层 API 可达性 / L0 ExperienceCard 蒸馏全链路 / 成功失败模式提取 /
//! 检索过滤 / 蒸馏幂等 / proptest 蒸馏不变量

#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use mlc_engine::{CaseExperience, DualExperienceBank, TaskQuery};
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ErrorSignature, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;

fn card(score: f32, status: ExecutionStatus, method: &str) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card-{}", uuid_like())),
        task_id: Box::from("t1"),
        node_id: Box::from(format!("n-{}", uuid_like())),
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

/// 简易唯一 ID（测试用，避免卡片 ID 冲突）
static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn uuid_like() -> u64 {
    COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}

fn case(score: f32, status: ExecutionStatus, task_type: &str, method: &str) -> CaseExperience {
    CaseExperience {
        card: card(score, status, method),
        task_type: task_type.to_string(),
        distilled: false,
        inserted_at: Utc::now(),
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use mlc_engine::prelude::*;
    let bank = DualExperienceBank::new(5);
    assert_eq!(bank.case_count(), 0);
    assert_eq!(bank.global_count(), 0);
    let _ = FailurePattern {
        error_signature: "h".into(),
        error_type: "t".into(),
        fix_strategy: "f".into(),
        frequency: 1,
        avg_fix_time_ms: 0,
    };
    let _ = SuccessPattern {
        method_family: "m".into(),
        score_range: (0.0, 1.0),
        key_factors: vec![],
        avg_token_usage: 0,
    };
    let _ = StrategyRecord {
        dimension: "operator".into(),
        strategy_value: "Draft".into(),
        avg_improvement: 0.5,
        sample_count: 1,
    };
}

// ----------------------------------------------------------
// 蒸馏全链路
// ----------------------------------------------------------

#[test]
fn distillation_end_to_end_from_l0_cards() {
    let mut bank = DualExperienceBank::new(3);
    // 3 张 L0 卡片（2 高分成功 + 1 失败）
    bank.add_case(case(
        0.9,
        ExecutionStatus::Success,
        "code_gen",
        "draft_pipeline",
    ));
    bank.add_case(case(
        0.8,
        ExecutionStatus::Success,
        "code_gen",
        "draft_pipeline",
    ));
    let mut fail = case(0.2, ExecutionStatus::Error, "code_gen", "draft_pipeline");
    fail.card.error_signature = Some(ErrorSignature {
        error_type: Box::from("compile_error"),
        error_location: Box::from("src/a.rs"),
        error_summary: Box::from("E0308"),
        error_hash: Box::from("hash-fail"),
    });
    bank.add_case(fail);
    // 达阈值 3 触发蒸馏
    assert_eq!(bank.global_count(), 1, "达阈值应蒸馏出 1 个全局经验");
    assert_eq!(bank.undistilled_count(), 0);

    let retrieved = bank.retrieve(&TaskQuery {
        task_type: "code_gen".into(),
        min_score: 0.0,
        max_results: 10,
    });
    let global = retrieved.global[0];
    // 成功模式（2 张 score>0.7）
    assert!(!global.success_patterns.is_empty(), "应提取成功模式");
    // 失败模式（1 张错误签名）
    assert_eq!(global.failure_patterns.len(), 1, "应提取失败模式");
    // 有效策略（Draft 算子）
    assert!(!global.effective_strategies.is_empty(), "应提取策略记录");
    assert_eq!(global.source_case_count, 3);
}

#[test]
fn distillation_groups_by_task_type() {
    let mut bank = DualExperienceBank::new(4);
    bank.add_case(case(0.9, ExecutionStatus::Success, "type_a", "fam1"));
    bank.add_case(case(0.8, ExecutionStatus::Success, "type_a", "fam1"));
    bank.add_case(case(0.85, ExecutionStatus::Success, "type_b", "fam2"));
    bank.add_case(case(0.7, ExecutionStatus::Success, "type_b", "fam2"));
    // 4 案例达阈值，按 task_type 分组 → 2 个全局经验
    assert_eq!(bank.global_count(), 2, "应按 task_type 分组蒸馏 2 个");
}

#[test]
fn distillation_idempotent_no_double_count() {
    let mut bank = DualExperienceBank::new(2);
    bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen", "fam"));
    bank.add_case(case(0.8, ExecutionStatus::Success, "code_gen", "fam"));
    let first = bank.global_count();
    bank.distill_global();
    assert_eq!(bank.global_count(), first, "重复蒸馏不新增");
}

// ----------------------------------------------------------
// 检索过滤
// ----------------------------------------------------------

#[test]
fn retrieve_filters_by_score_and_type() {
    let mut bank = DualExperienceBank::new(100);
    bank.add_case(case(0.9, ExecutionStatus::Success, "code_gen", "fam"));
    bank.add_case(case(0.4, ExecutionStatus::Success, "code_gen", "fam"));
    bank.add_case(case(0.8, ExecutionStatus::Success, "refactor", "fam"));
    // task_type + min_score 过滤
    let result = bank.retrieve(&TaskQuery {
        task_type: "code_gen".into(),
        min_score: 0.5,
        max_results: 10,
    });
    assert_eq!(result.cases.len(), 1, "min_score=0.5 只返回 0.9");
    // max_results 截断
    let all = bank.retrieve(&TaskQuery {
        task_type: "code_gen".into(),
        min_score: 0.0,
        max_results: 1,
    });
    assert_eq!(all.cases.len(), 1, "max_results=1 截断");
}

// ----------------------------------------------------------
// proptest: 蒸馏不变量
// ----------------------------------------------------------

proptest! {
    /// 任意案例数，蒸馏后全部标记 distilled，global_count ≥ 1（非空时）
    #[test]
    fn distillation_marks_all_distilled(
        n in 1usize..10,
    ) {
        let mut bank = DualExperienceBank::new(1); // 阈值 1，每次 add 都蒸馏
        for i in 0..n {
            bank.add_case(case(0.5 + (i as f32 * 0.01), ExecutionStatus::Success, "code_gen", "fam"));
        }
        prop_assert_eq!(bank.undistilled_count(), 0, "全部案例应已蒸馏");
        prop_assert!(bank.global_count() >= 1);
        prop_assert_eq!(bank.case_count(), n);
    }
}
