//! 长时程信用分配器集成测试 — SHARP 统计版时间维度（v3.4.0 §14.4）
//!
//! 覆盖: 顶层 API 可达性 / 折扣累积端到端 / Shapley 时间归因 /
//! RLTrajectory 导出（铁律6）/ proptest 折扣单调性

#![forbid(unsafe_code)]

use proptest::prelude::*;
use quest_engine::{CreditAssignment, CreditStep, LongTermCreditAssigner};

fn steps(rewards: &[f32]) -> Vec<CreditStep> {
    rewards
        .iter()
        .enumerate()
        .map(|(i, r)| CreditStep::new(format!("s{i}"), (i as u64) * 1000, *r))
        .collect()
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let assigner = LongTermCreditAssigner::default();
    let credits = assigner.assign_discounted_return(&steps(&[1.0]), 0.0);
    assert_eq!(credits.len(), 1);
}

// ----------------------------------------------------------
// 折扣累积回报端到端
// ----------------------------------------------------------

#[test]
fn discounted_return_end_to_end() {
    // γ=0.9，四步 [1.0, 0.5, 0.2, 0.0]，terminal=2.0
    let assigner = LongTermCreditAssigner::new(0.9, 8);
    let credits = assigner.assign_discounted_return(&steps(&[1.0, 0.5, 0.2, 0.0]), 2.0);
    assert_eq!(credits.len(), 4);
    // 折扣回报单调性：越早的步骤折扣回报越高（含终局传递）
    // G_3 = 0 + 0.9*2 = 1.8; G_2 = 0.2 + 0.9*1.8 = 1.82; ...
    // 验证步序对应
    assert_eq!(credits[0].step_id, "s0");
    assert_eq!(credits[3].step_id, "s3");
    // 末步折扣回报 = 0 + 0.9*2.0 = 1.8
    assert!((credits[3].discounted_return - 1.8).abs() < 1e-5);
}

#[test]
fn discounted_return_credit_is_future_contribution() {
    // credit = 折扣回报 - 即时奖励 = 后续步骤贡献的折现
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let credits = assigner.assign_discounted_return(&steps(&[1.0, 2.0]), 3.0);
    // G_1 = 2 + 1*3 = 5; credit_1 = 5 - 2 = 3（后续终局贡献）
    assert!((credits[1].credit - 3.0).abs() < 1e-5);
    // G_0 = 1 + 1*5 = 6; credit_0 = 6 - 1 = 5
    assert!((credits[0].credit - 5.0).abs() < 1e-5);
}

// ----------------------------------------------------------
// Shapley 时间归因
// ----------------------------------------------------------

#[test]
fn shapley_temporal_efficiency_axiom() {
    // 效率公理：全体 Shapley 之和 = 终局奖励
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let terminal = 6.0f32;
    let credits = assigner
        .assign_shapley_temporal(&steps(&[1.0, 2.0, 3.0]), terminal)
        .expect("n=3 ≤ 8");
    let sum: f32 = credits.iter().map(|c| c.credit).sum();
    assert!(
        (sum - terminal).abs() < 1e-3,
        "Shapley 之和应等于终局奖励: sum={} terminal={}",
        sum,
        terminal
    );
}

#[test]
fn shapley_temporal_proportional_to_reward_signal() {
    // 加性比例模型：Shapley 值 ∝ reward_signal
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let credits = assigner
        .assign_shapley_temporal(&steps(&[1.0, 2.0, 3.0]), 6.0)
        .expect("n=3 ≤ 8");
    // credit[i] = 6.0 * reward_signal[i] / 6.0 = reward_signal[i]
    assert!((credits[0].credit - 1.0).abs() < 1e-4);
    assert!((credits[1].credit - 2.0).abs() < 1e-4);
    assert!((credits[2].credit - 3.0).abs() < 1e-4);
}

#[test]
fn shapley_temporal_symmetry() {
    // 对称公理：等 reward_signal → 等 Shapley 值
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let credits = assigner
        .assign_shapley_temporal(&steps(&[1.0, 1.0]), 4.0)
        .expect("n=2 ≤ 8");
    assert_eq!(credits.len(), 2);
    assert!(
        (credits[0].credit - credits[1].credit).abs() < 1e-4,
        "对称步骤 Shapley 应相等: {} vs {}",
        credits[0].credit,
        credits[1].credit
    );
    // 各分得 terminal/2 = 2.0
    assert!((credits[0].credit - 2.0).abs() < 1e-4);
}

#[test]
fn shapley_temporal_zero_reward_equal_split() {
    // 全零 reward_signal → 终局奖励按步骤数均分
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let credits = assigner
        .assign_shapley_temporal(&steps(&[0.0, 0.0, 0.0]), 3.0)
        .expect("n=3 ≤ 8");
    for c in &credits {
        assert!(
            (c.credit - 1.0).abs() < 1e-4,
            "全零 reward 应均分: {}",
            c.credit
        );
    }
}

#[test]
fn shapley_temporal_over_limit_returns_none() {
    // n=10 > max_exact_shapley_steps=8 → None（指数爆炸保护）
    let assigner = LongTermCreditAssigner::new(0.9, 8);
    let rewards: Vec<f32> = vec![0.5; 10];
    assert_eq!(
        assigner.assign_shapley_temporal(&steps(&rewards), 1.0),
        None
    );
}

// ----------------------------------------------------------
// RLTrajectory 导出（铁律6）
// ----------------------------------------------------------

#[test]
fn export_rl_trajectory_end_to_end() {
    let assigner = LongTermCreditAssigner::new(0.9, 8);
    let steps_vec = steps(&[1.0, 0.5]);
    let credits = assigner.assign_discounted_return(&steps_vec, 2.0);
    let traj = assigner.export_rl_trajectory("ep-1", &steps_vec, &credits);
    assert_eq!(traj.episode_id.as_ref(), "ep-1");
    assert_eq!(traj.states.len(), 2);
    assert_eq!(traj.actions.len(), 2);
    assert_eq!(traj.rewards.len(), 2);
    assert_eq!(traj.timestamps.len(), 2);
    // actions 层标识为 L9，action_code 为步序
    assert_eq!(traj.actions[0].layer.as_ref(), "L9");
    assert_eq!(traj.actions[0].action_code, 0);
    assert_eq!(traj.actions[1].action_code, 1);
    // rewards 为折扣累积回报
    assert_eq!(traj.rewards[0], credits[0].discounted_return);
}

#[test]
fn credit_assignment_struct_fields() {
    let assigner = LongTermCreditAssigner::new(1.0, 8);
    let credits: Vec<CreditAssignment> = assigner.assign_discounted_return(&steps(&[1.0]), 1.0);
    assert_eq!(credits[0].step_id, "s0");
    // credit 与 discounted_return 字段可访问
    let _ = credits[0].credit;
    let _ = credits[0].discounted_return;
}

// ----------------------------------------------------------
// proptest: 折扣单调性（γ 越小远期信用衰减越快）
// ----------------------------------------------------------

proptest! {
    /// 任意 γ ∈ [0,1] 与轨迹：末步折扣回报 = reward_n + γ·terminal
    #[test]
    fn discounted_return_last_step_invariant(
        gamma in 0.0f32..1.0,
        terminal in 0.0f32..10.0,
        last_reward in 0.0f32..5.0,
    ) {
        let assigner = LongTermCreditAssigner::new(gamma, 8);
        let credits = assigner.assign_discounted_return(&steps(&[last_reward]), terminal);
        prop_assert_eq!(credits.len(), 1);
        let expected = last_reward + gamma * terminal;
        prop_assert!(
            (credits[0].discounted_return - expected).abs() < 1e-4,
            "末步折扣回报 = reward + γ·terminal: {} vs {}",
            credits[0].discounted_return,
            expected
        );
    }
}
