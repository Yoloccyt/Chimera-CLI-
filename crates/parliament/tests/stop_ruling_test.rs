//! 停止策略裁决集成测试 — RSIBench 四分支 + L5 语义对齐（v3.4.0 §13.1）
//!
//! 覆盖: 顶层 API 可达性 / 停止策略四分支端到端 / Ω₉-Preserve 保留最佳 /
//! L5 CheckpointPreserver 语义对齐（score 类型转换接线）/ proptest 停止单调性

#![forbid(unsafe_code)]

use nexus_contracts::experience_card::AtomicOperator;
use parliament::{StopCheckpoint, StopContext, StopRuling, ThreeFactorAdjudicator};
use proptest::prelude::*;

fn adjudicator() -> ThreeFactorAdjudicator {
    ThreeFactorAdjudicator::new(0.05, 0.6, 0.6, 0.1)
}

fn context(attempts: u32, current_score: f32) -> StopContext {
    StopContext {
        attempts,
        max_attempts: 50,
        stagnation_count: 0,
        stagnation_threshold: 5,
        current_score,
        best_score: 0.9,
        score_gap_threshold: 0.95,
        current_operator: AtomicOperator::Draft,
        best_checkpoint: Some(StopCheckpoint {
            task_type: "t1".into(),
            score: 0.9,
            metadata: "ckpt-best".into(),
        }),
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use parliament::prelude::*;
    let ruling = adjudicator().adjudicate_stop(&context(5, 0.8));
    assert_eq!(ruling, StopRuling::Continue);
}

// ----------------------------------------------------------
// RSIBench 四分支端到端
// ----------------------------------------------------------

#[test]
fn branch_max_attempts() {
    let ruling = adjudicator().adjudicate_stop(&context(50, 0.8));
    match ruling {
        StopRuling::Stop {
            reason,
            preserve_best,
            selected_checkpoint,
        } => {
            assert!(reason.contains("Max attempts"));
            assert!(preserve_best);
            assert_eq!(
                selected_checkpoint.expect("携带检查点").metadata.as_str(),
                "ckpt-best"
            );
        }
        other => panic!("应为 Stop，实际 {other:?}"),
    }
}

#[test]
fn branch_stagnation() {
    let mut ctx = context(5, 0.8);
    ctx.stagnation_count = 5;
    let ruling = adjudicator().adjudicate_stop(&ctx);
    match ruling {
        StopRuling::Stop { reason, .. } => assert!(reason.contains("Stagnation")),
        other => panic!("应为 Stop，实际 {other:?}"),
    }
}

#[test]
fn branch_score_gap_after_10_attempts() {
    // attempts=11 > 10 且 current/best = 0.5/0.9 ≈ 0.56 < 0.95
    let ruling = adjudicator().adjudicate_stop(&context(11, 0.5));
    match ruling {
        StopRuling::Stop { reason, .. } => assert!(reason.contains("ratio")),
        other => panic!("应为 Stop，实际 {other:?}"),
    }
}

#[test]
fn branch_no_score_gap_before_10_attempts() {
    // attempts=10（未 > 10）不触发 score_gap 检查 → Continue
    let ruling = adjudicator().adjudicate_stop(&context(10, 0.5));
    assert_eq!(ruling, StopRuling::Continue);
}

#[test]
fn branch_late_stage_suggest_switch() {
    // attempts=21 > 20，current/best=1.0 不触发 gap，Draft 算子 → SuggestSwitch
    let mut ctx = context(21, 0.9);
    ctx.max_attempts = 100;
    let ruling = adjudicator().adjudicate_stop(&ctx);
    match ruling {
        StopRuling::SuggestSwitch { suggested, reason } => {
            assert_eq!(suggested, AtomicOperator::Crossover);
            assert!(reason.contains("Late-stage"));
        }
        other => panic!("应为 SuggestSwitch，实际 {other:?}"),
    }
}

#[test]
fn late_stage_no_switch_for_crossover() {
    // 已是 Crossover → 不建议切换 → Continue
    let mut ctx = context(21, 0.9);
    ctx.max_attempts = 100;
    ctx.current_operator = AtomicOperator::Crossover;
    assert_eq!(adjudicator().adjudicate_stop(&ctx), StopRuling::Continue);
}

// ----------------------------------------------------------
// L5 CheckpointPreserver 语义对齐（调用方转换接线）
// ----------------------------------------------------------

#[test]
fn l5_checkpoint_conversion_alignment() {
    // L5 gsoe_evolution::Checkpoint（f64 score）→ StopCheckpoint（f32）转换接线
    let l5_checkpoint = gsoe_evolution::Checkpoint::new("t1", 0.9f64, "ckpt-l5");
    let stop_ckpt = StopCheckpoint {
        task_type: l5_checkpoint.task_type.clone(),
        score: l5_checkpoint.score as f32,
        metadata: l5_checkpoint.metadata.clone(),
    };
    let mut ctx = context(50, 0.8);
    ctx.best_checkpoint = Some(stop_ckpt);
    let ruling = adjudicator().adjudicate_stop(&ctx);
    match ruling {
        StopRuling::Stop {
            selected_checkpoint,
            ..
        } => {
            let ckpt = selected_checkpoint.expect("携带转换后检查点");
            assert_eq!(ckpt.metadata.as_str(), "ckpt-l5");
            assert!((ckpt.score - 0.9).abs() < 1e-6);
        }
        other => panic!("应为 Stop，实际 {other:?}"),
    }
}

// ----------------------------------------------------------
// proptest：停止单调性不变量
// ----------------------------------------------------------

proptest! {
    /// attempts 增加不应使 Continue 变为更宽松（停止单调性）：
    /// 若 attempts=A 时 Stop，则 attempts>A（其他条件同）也必 Stop 或 SuggestSwitch
    #[test]
    fn stop_monotonic_in_attempts(
        base_attempts in 50u32..80,
        score in 0.0f32..1.0,
    ) {
        let adj = adjudicator();
        let mut ctx = context(base_attempts, score);
        ctx.max_attempts = 50; // attempts ≥ max → 恒 Stop
        let r1 = adj.adjudicate_stop(&ctx);
        // WHY 中间变量: prop_assert! 将表达式嵌入格式串，matches! 花括号会破坏解析
        let r1_is_stop = matches!(r1, StopRuling::Stop { .. });
        prop_assert!(r1_is_stop);
        ctx.attempts = base_attempts + 10;
        let r2 = adj.adjudicate_stop(&ctx);
        let r2_is_stop = matches!(r2, StopRuling::Stop { .. });
        prop_assert!(r2_is_stop, "attempts 增加后仍应 Stop");
    }
}
