//! 动态验证深度 + 熵加权集成测试 — 五档门控 + 铁律6 轨迹导出（v3.4.0 §12.3）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 五档风险门控 /
//! EMA 有效性追踪 / RLTrajectory 导出（铁律6）/ 熵加权数学 /
//! proptest EMA 不变量

#![forbid(unsafe_code)]

use chrono::Utc;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;
use pvl_layer::{DynamicVerifier, EntropyWeightedScorer, TaskRisk, VerificationDepth};

fn risk(level: u8) -> TaskRisk {
    TaskRisk {
        level,
        factors: Vec::new(),
    }
}

fn card(quality: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: "c1".into(),
        task_id: "t1".into(),
        node_id: "n1".into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Draft,
        score: quality,
        delta_vs_parent: 0.0,
        method_family: "test".into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality,
            progress: 0.1,
            novelty: 0.2,
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
    let verifier = DynamicVerifier::new();
    assert!(verifier
        .effectiveness(VerificationDepth::StandardVerify)
        .is_some());
}

// ----------------------------------------------------------
// 五档风险门控端到端
// ----------------------------------------------------------

#[test]
fn five_level_risk_gating() {
    let verifier = DynamicVerifier::new();
    // 档位 1: 风险 > 80 → FullVerify
    assert_eq!(
        verifier.select_depth(&risk(95), &AtomicOperator::Draft),
        VerificationDepth::FullVerify
    );
    // 档位 2: Crossover + 风险 > 50 → FullVerify
    assert_eq!(
        verifier.select_depth(&risk(60), &AtomicOperator::Crossover),
        VerificationDepth::FullVerify
    );
    // 档位 3: Debug → StandardVerify
    assert_eq!(
        verifier.select_depth(&risk(5), &AtomicOperator::Debug),
        VerificationDepth::StandardVerify
    );
    // 档位 4: 风险 > 50 → StandardVerify
    assert_eq!(
        verifier.select_depth(&risk(70), &AtomicOperator::Improve),
        VerificationDepth::StandardVerify
    );
    // 档位 5: 低风险 → 历史最佳（初始 FullVerify 0.95 最高）
    assert_eq!(
        verifier.select_depth(&risk(5), &AtomicOperator::Improve),
        VerificationDepth::FullVerify
    );
}

// ----------------------------------------------------------
// 铁律6：RLTrajectory 导出预留
// ----------------------------------------------------------

#[test]
fn rl_trajectory_export_closure() {
    let mut verifier = DynamicVerifier::new();
    verifier.update_effectiveness(VerificationDepth::FullVerify, true);
    verifier.update_effectiveness(VerificationDepth::SkipVerify, false);
    verifier.update_effectiveness(VerificationDepth::StandardVerify, true);
    let traj = verifier.export_depth_history("ep-depth");
    // 四序列等长（轨迹完整性不变量）
    assert_eq!(traj.states.len(), 3);
    assert_eq!(traj.actions.len(), 3);
    assert_eq!(traj.rewards.len(), 3);
    assert_eq!(traj.timestamps.len(), 3);
    // 动作层标识 L7
    assert_eq!(traj.actions[0].layer.as_ref(), "L7");
    // 奖励序列与成功标志一致
    assert_eq!(traj.rewards, vec![1.0, 0.0, 1.0].into_boxed_slice());
}

// ----------------------------------------------------------
// 熵加权数学
// ----------------------------------------------------------

#[test]
fn entropy_weighting_amplifies_uncertainty() {
    // 均匀双候选 p=0.5 → 熵最大 → 加成最高
    let c1 = card(0.5);
    let c2 = card(0.5);
    let amplified = EntropyWeightedScorer::score(&c1, &[c1.clone(), c2]);
    let base = c1.three_factor.selection_utility();
    assert!(
        amplified > base,
        "熵加权应放大不确定候选（{amplified} > {base}）"
    );
}

#[test]
fn entropy_weighting_single_no_amplification() {
    let c = card(0.7);
    let score = EntropyWeightedScorer::score(&c, std::slice::from_ref(&c));
    assert!((score - c.three_factor.selection_utility()).abs() < 1e-6);
}

// ----------------------------------------------------------
// proptest：EMA 不变量
// ----------------------------------------------------------

proptest! {
    /// 任意成功/失败序列: 有效性恒 ∈ [0,1]，历史长度 = 更新次数
    #[test]
    fn ema_bounded(outcomes in prop::collection::vec(any::<bool>(), 0..30)) {
        let mut verifier = DynamicVerifier::new();
        for (i, success) in outcomes.iter().enumerate() {
            let depth = match i % 5 {
                0 => VerificationDepth::FullVerify,
                1 => VerificationDepth::StandardVerify,
                2 => VerificationDepth::IncrementalVerify,
                3 => VerificationDepth::SyntaxOnly,
                _ => VerificationDepth::SkipVerify,
            };
            verifier.update_effectiveness(depth, *success);
        }
        prop_assert_eq!(verifier.history_len(), outcomes.len());
        for depth in [
            VerificationDepth::FullVerify,
            VerificationDepth::StandardVerify,
            VerificationDepth::IncrementalVerify,
            VerificationDepth::SyntaxOnly,
            VerificationDepth::SkipVerify,
        ] {
            if let Some(eff) = verifier.effectiveness(depth) {
                prop_assert!((0.0..=1.0).contains(&eff), "EMA 应 ∈ [0,1]（实际 {eff}）");
            }
        }
    }
}
