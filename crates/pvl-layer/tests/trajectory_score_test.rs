//! KAT 轨迹九维过程评分集成测试 — 九维数学 + 权重和（v3.4.0 §12.2）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 九维端到端计算 /
//! 权重和 = 1.0 / 与既有观测九维（ProcessScore）并存语义 /
//! proptest overall ∈ [0,1] 不变量

#![forbid(unsafe_code)]

use nexus_contracts::experience_card::AtomicOperator;
use proptest::prelude::*;
use pvl_layer::{
    CodeChange, ProcessScore, ProcessTrajectory, TrajectoryAction, TrajectoryProcessScore,
    VerificationStep,
};

fn action(operator: AtomicOperator, success: bool) -> TrajectoryAction {
    TrajectoryAction {
        operator,
        timestamp_ms: 0,
        success,
    }
}

fn full_trajectory() -> ProcessTrajectory {
    ProcessTrajectory {
        actions: vec![
            action(AtomicOperator::Draft, true),
            action(AtomicOperator::Improve, true),
            action(AtomicOperator::Debug, true),
            action(AtomicOperator::Crossover, true),
        ],
        total_tokens: 5_000,
        final_score: 0.9,
        target_score: 1.0,
        code_changes: vec![CodeChange {
            file_path: "src/a.rs".into(),
            lines_added: 20,
            lines_removed: 5,
        }],
        verification_steps: vec![VerificationStep {
            step_type: "cargo_test".into(),
            passed: true,
            coverage_percent: 90.0,
        }],
        reported_errors: vec!["err-a".into()],
        actual_errors: vec!["err-a".into()],
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let traj = full_trajectory();
    let score = TrajectoryProcessScore::from_trajectory(&traj);
    assert!(score.overall() >= 0.0);
}

// ----------------------------------------------------------
// 九维端到端计算
// ----------------------------------------------------------

#[test]
fn full_trajectory_nine_dimensions() {
    let score = TrajectoryProcessScore::from_trajectory(&full_trajectory());
    // 探索: 4/4 算子全覆盖
    assert!((score.exploration - 1.0).abs() < 1e-6);
    // 定位: Debug 全成功
    assert!((score.localization - 1.0).abs() < 1e-6);
    // 忠实: 0.9/1.0
    assert!((score.fidelity - 0.9).abs() < 1e-6);
    // 最小: 50/(25+50) = 0.667
    assert!((score.minimality - (50.0 / 75.0)).abs() < 1e-5);
    // 验证: 90% 覆盖率
    assert!((score.verification - 0.9).abs() < 1e-6);
    // 诚实: 完全匹配 F1=1
    assert!((score.honesty - 1.0).abs() < 1e-6);
    // 效率: 5000/0.9 < 10000 → 满分
    assert!((score.efficiency - 1.0).abs() < 1e-6);
    // 预留维度
    assert_eq!(score.robustness, 0.5);
    assert_eq!(score.readability, 0.5);
}

// ----------------------------------------------------------
// 权重和 = 1.0（规范数值钉住）
// ----------------------------------------------------------

#[test]
fn overall_weights_sum_to_one() {
    let full = TrajectoryProcessScore {
        exploration: 1.0,
        localization: 1.0,
        fidelity: 1.0,
        minimality: 1.0,
        verification: 1.0,
        honesty: 1.0,
        efficiency: 1.0,
        robustness: 1.0,
        readability: 1.0,
    };
    assert!((full.overall() - 1.0).abs() < 1e-6, "权重和应为 1.0");
}

// ----------------------------------------------------------
// D-2 命名协调：轨迹九维与观测九维并存
// ----------------------------------------------------------

#[test]
fn trajectory_and_observational_scores_coexist() {
    // 两套九维类型独立存在（编译期验证语义边界）
    let _observational = ProcessScore {
        real_execution: 1.0,
        coverage: 1.0,
        verification: 1.0,
        confidence: 1.0,
        efficiency: 1.0,
        retry_discipline: 1.0,
        output_substance: 1.0,
        orphan_free: 1.0,
        sandbox_clean: 1.0,
        total: 1.0,
    };
    let _trajectory = TrajectoryProcessScore::from_trajectory(&full_trajectory());
}

// ----------------------------------------------------------
// proptest：overall ∈ [0,1] 不变量
// ----------------------------------------------------------

proptest! {
    /// 任意轨迹: 九维各 ∈ [0,1]，overall ∈ [0,1]
    #[test]
    fn overall_bounded(
        n_actions in 0usize..10,
        final_score in 0.0f32..2.0,
        lines in 0i32..500,
    ) {
        let traj = ProcessTrajectory {
            actions: (0..n_actions)
                .map(|i| action(match i % 4 {
                    0 => AtomicOperator::Draft,
                    1 => AtomicOperator::Improve,
                    2 => AtomicOperator::Debug,
                    _ => AtomicOperator::Crossover,
                }, i % 2 == 0))
                .collect(),
            total_tokens: 1000,
            final_score,
            target_score: 1.0,
            code_changes: vec![CodeChange {
                file_path: "f.rs".into(),
                lines_added: lines,
                lines_removed: 0,
            }],
            verification_steps: Vec::new(),
            reported_errors: Vec::new(),
            actual_errors: Vec::new(),
        };
        let score = TrajectoryProcessScore::from_trajectory(&traj);
        let overall = score.overall();
        prop_assert!((0.0..=1.0).contains(&overall), "overall 应 ∈ [0,1]（实际 {overall}）");
        prop_assert!((0.0..=1.0).contains(&score.exploration));
        prop_assert!((0.0..=1.0).contains(&score.honesty));
    }
}
