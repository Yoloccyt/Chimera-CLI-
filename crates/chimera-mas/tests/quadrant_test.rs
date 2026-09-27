//! 四象限模型集成测试 (§3 / ADR-027 决策 1-3)
//!
//! 覆盖:
//! - `Quadrant` 枚举(index / tag / name / axis)
//! - `task_scope` 尾缀编解码往返 (encode_scope / from_task_scope)
//! - 象限激活矩阵 (activated_quadrants, §3.4) —— 精确象限而非仅计数
//! - 六维质量 / 三步验证 稳定映射 (§3.6)
//! - `QuadrantPlan` INV-3(扇出≤4) / INV-4(象限唯一)强制
//! - serde 序列化往返(JSON)

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use chimera_mas::delegation::TaskComplexity;
use chimera_mas::error::MasError;
use chimera_mas::quadrant::{
    activated_quadrants, CoreCross, ProduceAssure, Quadrant, QuadrantPlan, QualityDimension,
    ValidationStep, MAX_QUADRANT_FANOUT,
};

// ============================================================
// Quadrant 枚举基础
// ============================================================

#[test]
fn test_all_contains_four_quadrants_in_stable_order() {
    assert_eq!(Quadrant::ALL.len(), 4);
    assert_eq!(
        Quadrant::ALL,
        [
            Quadrant::Implementation,
            Quadrant::Integration,
            Quadrant::Verification,
            Quadrant::Hardening
        ]
    );
}

#[test]
fn test_quadrant_index_maps_q1_to_q4() {
    assert_eq!(Quadrant::Implementation.index(), 1);
    assert_eq!(Quadrant::Integration.index(), 2);
    assert_eq!(Quadrant::Verification.index(), 3);
    assert_eq!(Quadrant::Hardening.index(), 4);
}

#[test]
fn test_quadrant_tag_matches_index() {
    assert_eq!(Quadrant::Implementation.tag(), "#Q1");
    assert_eq!(Quadrant::Integration.tag(), "#Q2");
    assert_eq!(Quadrant::Verification.tag(), "#Q3");
    assert_eq!(Quadrant::Hardening.tag(), "#Q4");
}

#[test]
fn test_quadrant_name_is_english_role() {
    assert_eq!(Quadrant::Implementation.name(), "Implementation");
    assert_eq!(Quadrant::Integration.name(), "Integration");
    assert_eq!(Quadrant::Verification.name(), "Verification");
    assert_eq!(Quadrant::Hardening.name(), "Hardening");
}

#[test]
fn test_axis_coordinates_match_design_3_2() {
    // Q1 Produce×Core, Q2 Produce×Cross, Q3 Assure×Core, Q4 Assure×Cross
    assert_eq!(
        Quadrant::Implementation.axis(),
        (ProduceAssure::Produce, CoreCross::Core)
    );
    assert_eq!(
        Quadrant::Integration.axis(),
        (ProduceAssure::Produce, CoreCross::CrossCutting)
    );
    assert_eq!(
        Quadrant::Verification.axis(),
        (ProduceAssure::Assure, CoreCross::Core)
    );
    assert_eq!(
        Quadrant::Hardening.axis(),
        (ProduceAssure::Assure, CoreCross::CrossCutting)
    );
}

// ============================================================
// task_scope 编解码往返 (ADR-027 决策 2)
// ============================================================

#[test]
fn test_encode_scope_appends_tag() {
    assert_eq!(
        Quadrant::Verification.encode_scope("refactor-parser"),
        "refactor-parser#Q3"
    );
}

#[test]
fn test_encode_decode_roundtrip_all_quadrants() {
    for q in Quadrant::ALL {
        let scope = q.encode_scope("base-task");
        assert_eq!(
            Quadrant::from_task_scope(&scope),
            Some(q),
            "{} 编解码往返失败",
            q.name()
        );
    }
}

#[test]
fn test_from_task_scope_without_tag_returns_none() {
    assert_eq!(Quadrant::from_task_scope("plain-scope"), None);
    assert_eq!(Quadrant::from_task_scope(""), None);
}

#[test]
fn test_from_task_scope_only_matches_suffix() {
    // 尾缀在中间不应匹配(仅尾缀 #Qn 有效)
    assert_eq!(Quadrant::from_task_scope("#Q1-in-middle-task"), None);
}

// ============================================================
// 激活矩阵 (§3.4) — 精确象限
// ============================================================

#[test]
fn test_activation_simple_only_q1() {
    assert_eq!(
        activated_quadrants(TaskComplexity::Simple),
        vec![Quadrant::Implementation]
    );
}

#[test]
fn test_activation_medium_q1_q3() {
    assert_eq!(
        activated_quadrants(TaskComplexity::Medium),
        vec![Quadrant::Implementation, Quadrant::Verification]
    );
}

#[test]
fn test_activation_complex_q1_q2_q3() {
    assert_eq!(
        activated_quadrants(TaskComplexity::Complex),
        vec![
            Quadrant::Implementation,
            Quadrant::Integration,
            Quadrant::Verification
        ]
    );
}

#[test]
fn test_activation_very_complex_all_four() {
    assert_eq!(
        activated_quadrants(TaskComplexity::VeryComplex),
        Quadrant::ALL.to_vec()
    );
}

#[test]
fn test_activation_never_exceeds_fanout_bound() {
    for c in [
        TaskComplexity::Simple,
        TaskComplexity::Medium,
        TaskComplexity::Complex,
        TaskComplexity::VeryComplex,
    ] {
        assert!(activated_quadrants(c).len() <= MAX_QUADRANT_FANOUT);
    }
}

// ============================================================
// §3.6 稳定映射:六维质量 / 三步验证
// ============================================================

#[test]
fn test_quality_dimension_mapping() {
    assert_eq!(
        Quadrant::Implementation.quality_dimensions(),
        &[
            QualityDimension::D1ModularLogic,
            QualityDimension::D2Readability
        ]
    );
    assert_eq!(
        Quadrant::Integration.quality_dimensions(),
        &[QualityDimension::D3NoTechDebt]
    );
    assert_eq!(
        Quadrant::Verification.quality_dimensions(),
        &[QualityDimension::D6ErrorHandling]
    );
    assert_eq!(
        Quadrant::Hardening.quality_dimensions(),
        &[
            QualityDimension::D4Documentation,
            QualityDimension::D5BestPractices
        ]
    );
}

#[test]
fn test_validation_step_mapping() {
    assert_eq!(
        Quadrant::Implementation.validation_step(),
        ValidationStep::Step3AtomicImpl
    );
    assert_eq!(
        Quadrant::Integration.validation_step(),
        ValidationStep::Step3AtomicImpl
    );
    assert_eq!(
        Quadrant::Verification.validation_step(),
        ValidationStep::Step2RiskDesign
    );
    assert_eq!(
        Quadrant::Hardening.validation_step(),
        ValidationStep::Step1PlanImpact
    );
}

// ============================================================
// QuadrantPlan — INV-3 / INV-4 强制
// ============================================================

#[test]
fn test_plan_from_complexity_matches_activation() {
    let plan = QuadrantPlan::from_complexity("t", TaskComplexity::Complex);
    assert_eq!(plan.fanout(), 3);
    assert_eq!(plan.base_scope(), "t");
    assert!(plan.is_active(Quadrant::Implementation));
    assert!(plan.is_active(Quadrant::Integration));
    assert!(plan.is_active(Quadrant::Verification));
    assert!(!plan.is_active(Quadrant::Hardening));
}

#[test]
fn test_plan_scoped_assignments_encode_each_quadrant() {
    let plan = QuadrantPlan::from_complexity("refactor", TaskComplexity::Medium);
    let pairs = plan.scoped_assignments();
    assert_eq!(
        pairs,
        vec![
            (Quadrant::Implementation, "refactor#Q1".to_string()),
            (Quadrant::Verification, "refactor#Q3".to_string()),
        ]
    );
}

#[test]
fn test_plan_from_quadrants_valid_within_bound() {
    let plan =
        QuadrantPlan::from_quadrants("t", vec![Quadrant::Implementation, Quadrant::Hardening])
            .expect("2 个唯一象限应构造成功");
    assert_eq!(plan.fanout(), 2);
}

#[test]
fn test_plan_full_four_quadrants_is_valid() {
    let plan = QuadrantPlan::from_quadrants("t", Quadrant::ALL.to_vec())
        .expect("恰 4 个唯一象限应满足 INV-3");
    assert_eq!(plan.fanout(), MAX_QUADRANT_FANOUT);
}

#[test]
fn test_inv3_rejects_fanout_over_four() {
    let five = vec![
        Quadrant::Implementation,
        Quadrant::Integration,
        Quadrant::Verification,
        Quadrant::Hardening,
        Quadrant::Implementation,
    ];
    let err = QuadrantPlan::from_quadrants("t", five).unwrap_err();
    match err {
        MasError::QuadrantFanoutExceeded { requested, max } => {
            assert_eq!(requested, 5);
            assert_eq!(max, 4);
        }
        other => panic!("期望 QuadrantFanoutExceeded, 实际 {other:?}"),
    }
}

#[test]
fn test_inv4_rejects_duplicate_quadrant() {
    let dup = vec![Quadrant::Verification, Quadrant::Verification];
    let err = QuadrantPlan::from_quadrants("t", dup).unwrap_err();
    match err {
        MasError::QuadrantConflict { quadrant } => {
            assert_eq!(quadrant, "Verification");
        }
        other => panic!("期望 QuadrantConflict, 实际 {other:?}"),
    }
}

// ============================================================
// serde 序列化往返(JSON)
// ============================================================

#[test]
fn test_quadrant_json_roundtrip() {
    for q in Quadrant::ALL {
        let json = serde_json::to_string(&q).expect("序列化成功");
        let back: Quadrant = serde_json::from_str(&json).expect("反序列化成功");
        assert_eq!(back, q);
    }
}

#[test]
fn test_plan_json_roundtrip() {
    let plan = QuadrantPlan::from_complexity("scope", TaskComplexity::VeryComplex);
    let json = serde_json::to_string(&plan).expect("序列化成功");
    let back: QuadrantPlan = serde_json::from_str(&json).expect("反序列化成功");
    assert_eq!(back, plan);
}
