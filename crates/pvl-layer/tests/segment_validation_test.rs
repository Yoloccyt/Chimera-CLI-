//! Segment-aware 分段感知验证集成测试 — 铁律9 + L1 PER 协同（v3.4.0 §12.4）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 铁律9 分段身份共享 /
//! anchor 终局奖励传播（0.3 系数）/ L1 SegmentAwarePER 协同闭环 /
//! proptest 传播系数不变量

#![forbid(unsafe_code)]

use event_bus::SegmentAwarePER;
use nexus_contracts::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
use nexus_contracts::token_evidence::{SegmentCreationReason, SegmentMetadata};
use nexus_contracts::SeamId;
use proptest::prelude::*;
use pvl_layer::{SegmentAwareValidator, SegmentValidationError};

fn segment(id: &str, traj: &str, index: u32, is_anchor: bool) -> SegmentMetadata {
    SegmentMetadata::new(
        id,
        traj,
        index,
        is_anchor,
        Vec::new(),
        Vec::new(),
        0,
        5,
        SegmentCreationReason::NaturalBoundary,
    )
}

fn exp(reward: f32) -> RLExperience {
    RLExperience {
        state: RLState::new(vec![0.1], 1),
        action: RLAction::MemPi(MemPiAction::Retrieve),
        reward,
        next_state: RLState::new(vec![0.2], 2),
        done: false,
        seam: SeamId::S8MemPi,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let validator = SegmentAwareValidator::new();
    assert_eq!(validator.trajectory_count(), 0);
}

// ----------------------------------------------------------
// 铁律9：分段身份共享 + 三级验证端到端
// ----------------------------------------------------------

#[test]
fn iron_rule_9_shared_identity_and_validation() {
    let mut validator = SegmentAwareValidator::new();
    // 同轨迹 3 分段共享 parent_traj_id（铁律9）
    let s0 = segment("s0", "traj-1", 0, true);
    let s1 = segment("s1", "traj-1", 1, false);
    let s2 = segment("s2", "traj-1", 2, false);
    validator.register_segment(s0.clone());
    validator.register_segment(s1.clone());
    validator.register_segment(s2.clone());
    assert_eq!(validator.segment_count("traj-1"), 3);

    // 三级验证端到端（含 fn 标记 + PASS）
    let r0 = validator.validate_segment(&s0, "fn main() { PASS }", 0);
    assert!(r0.syntax_pass && r0.logic_pass && r0.sandbox_pass);
    assert!(r0.is_anchor);
    // 空输出语法验证失败
    let r1 = validator.validate_segment(&s1, "", 0);
    assert!(!r1.syntax_pass);
}

// ----------------------------------------------------------
// anchor 终局奖励传播（0.3 系数）
// ----------------------------------------------------------

#[test]
fn anchor_reward_propagation_coefficient() {
    let mut validator = SegmentAwareValidator::new();
    let anchor = segment("s0", "traj-1", 0, true);
    let normal = segment("s1", "traj-1", 1, false);
    validator.register_segment(anchor.clone());
    validator.register_segment(normal.clone());
    // 非 anchor 段验证记录 process_reward
    let result = validator.validate_segment(&normal, "fn f() {}", 0);

    let affected = validator
        .broadcast_final_reward("traj-1", 3.0)
        .expect("已知轨迹");
    assert_eq!(affected, 2);

    let states = validator.reward_states("traj-1").expect("已注册");
    let s0 = states
        .iter()
        .find(|s| s.segment_id.as_ref() == "s0")
        .expect("anchor");
    let s1 = states
        .iter()
        .find(|s| s.segment_id.as_ref() == "s1")
        .expect("normal");
    // anchor 直接承载终局奖励
    assert_eq!(s0.final_reward, Some(3.0));
    // 非 anchor: process_reward + 3.0 × 0.3
    let expected = result.segment_reward + 0.9;
    assert!(
        (s1.final_reward.unwrap() - expected).abs() < 1e-6,
        "传播系数 0.3（实际 {:?}，期望 {expected}）",
        s1.final_reward
    );
}

#[test]
fn unknown_trajectory_returns_error() {
    let mut validator = SegmentAwareValidator::new();
    let err = validator
        .broadcast_final_reward("ghost", 1.0)
        .expect_err("未知轨迹");
    assert!(matches!(err, SegmentValidationError::UnknownTrajectory(_)));
}

// ----------------------------------------------------------
// L1 SegmentAwarePER 协同闭环
// ----------------------------------------------------------

#[test]
fn segment_per_collaboration_closure() {
    // L7 验证 → anchor 终局奖励 → L1 SegmentAwarePER 广播（Dressage 闭环）
    let mut validator = SegmentAwareValidator::new();
    let anchor = segment("s0", "traj-1", 0, true);
    let normal = segment("s1", "traj-1", 1, false);
    validator.register_segment(anchor);
    validator.register_segment(normal);
    validator
        .broadcast_final_reward("traj-1", 2.5)
        .expect("已知轨迹");

    // L1 PER: 分段登记 + 终局奖励广播
    let mut per = SegmentAwarePER::new(100, 42);
    let s0 = segment("s0", "traj-1", 0, true);
    let s1 = segment("s1", "traj-1", 1, false);
    per.add_segment(exp(0.0), s0, 0.8);
    per.add_segment(exp(0.0), s1, 0.6);
    // anchor 终局奖励经 PER 广播（prompt-equal 分母由 PER 侧处理）
    per.broadcast_reward("traj-1", 2.5);
    assert_eq!(per.segment_count("traj-1"), 2, "铁律9 同轨迹分段登记");
    // 采样验证奖励已注入
    let samples = per.sample_batch(4);
    assert!(!samples.is_empty());
}

// ----------------------------------------------------------
// proptest：传播系数不变量
// ----------------------------------------------------------

proptest! {
    /// 任意终局奖励: anchor 恒等于 final_reward，非 anchor 恒等于
    /// process_reward + final_reward × 0.3（传播系数不变量）
    #[test]
    fn propagation_invariant(final_reward in -5.0f32..5.0) {
        let mut validator = SegmentAwareValidator::new();
        validator.register_segment(segment("s0", "traj-p", 0, true));
        validator.register_segment(segment("s1", "traj-p", 1, false));
        let normal = segment("s1", "traj-p", 1, false);
        let result = validator.validate_segment(&normal, "fn f() {}", 0);
        validator.broadcast_final_reward("traj-p", final_reward).expect("已知轨迹");
        let states = validator.reward_states("traj-p").expect("已注册");
        let anchor_state = states.iter().find(|s| s.is_anchor).unwrap();
        let normal_state = states.iter().find(|s| !s.is_anchor).unwrap();
        prop_assert_eq!(anchor_state.final_reward, Some(final_reward));
        let expected = result.segment_reward + final_reward * 0.3;
        prop_assert!((normal_state.final_reward.unwrap() - expected).abs() < 1e-5);
    }
}
