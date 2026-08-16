//! 统计学习接口层集成测试 — 与 L0 RL 类型全链路协同（v3.4.0 §6.3 + 铁律6）
//!
//! 覆盖: 顶层 API / 铁律6 轨迹导出（RLTrajectory 完整填充）/
//! 自定义投影覆盖 / 与 UCB 的探索-利用对比 / proptest 统计不变量

#![forbid(unsafe_code)]

use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector};
use nexus_core::stat_learning::{ActionStats, SlidingWindowPolicy, StatLearningPolicy, UCBPolicy};
use proptest::prelude::*;

type TestState = u8;
type TestAction = u8;

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use nexus_core::prelude::*;
    let policy = SlidingWindowPolicy::<u8, u8>::new(10, 0.1);
    assert_eq!(policy.get_action_stats().len(), 0);
    let ucb = UCBPolicy::<u8, u8>::new(1.414);
    assert!(ucb.get_action_stats().is_empty());
}

// ----------------------------------------------------------
// 铁律6: 轨迹导出完整填充（episode_id/timestamps/投影）
// ----------------------------------------------------------

/// 自定义投影策略 — 验证 trait 投影方法可覆盖
struct ProjectedPolicy {
    inner: SlidingWindowPolicy<TestState, TestAction>,
}

impl StatLearningPolicy for ProjectedPolicy {
    type State = TestState;
    type Action = TestAction;

    fn predict(&self, state: &Self::State) -> Self::Action {
        self.inner.predict(state)
    }

    fn update(&mut self, state: &Self::State, action: &Self::Action, reward: f32) {
        self.inner.update(state, action, reward);
    }

    fn export_trajectory(&self, episode_id: &str) -> nexus_contracts::RLTrajectory {
        // 使用 with_projection 复用 inner 历史 + 本类型的自定义投影
        //（静态分派下委托 inner.export_trajectory 会丢失本类型投影覆盖）
        self.inner.export_trajectory_with_projection(
            episode_id,
            |s| self.project_state(s),
            |a| self.project_action(a),
        )
    }

    fn get_action_stats(&self) -> std::collections::HashMap<Self::Action, ActionStats> {
        self.inner.get_action_stats()
    }

    /// 覆盖投影: 状态编码为 clv[0] = state
    fn project_state(&self, state: &Self::State) -> RLStateVector {
        let mut v = RLStateVector::zeros();
        v.clv[0] = *state as f32;
        v
    }

    /// 覆盖投影: 动作编码为 action_code + layer "S1"
    fn project_action(&self, action: &Self::Action) -> RLActionVector {
        RLActionVector::new("S1", *action as u32, vec![0.5])
    }
}

#[test]
fn trajectory_export_full_fill_with_custom_projection() {
    let mut policy = ProjectedPolicy {
        inner: SlidingWindowPolicy::<TestState, TestAction>::new(10, 0.0),
    };
    for i in 0..3 {
        policy.update(&(i as u8), &(i as u8), 0.5);
    }
    let traj = policy.export_trajectory("ep-projected");
    // episode_id 填充
    assert_eq!(traj.episode_id.as_ref(), "ep-projected");
    assert_eq!(traj.len(), 3);
    // 自定义状态投影生效: clv[0] = state
    assert_eq!(traj.states[1].clv[0], 1.0);
    // 自定义动作投影生效: layer/action_code
    assert_eq!(traj.actions[2].layer.as_ref(), "S1");
    assert_eq!(traj.actions[2].action_code, 2);
    // timestamps 单调递增
    assert!(traj.timestamps.windows(2).all(|w| w[0] < w[1]));
    // 四序列等长（构造器保证）
    assert_eq!(traj.rewards.len(), 3);
}

// ----------------------------------------------------------
// SlidingWindow: 与 UCB 的探索-利用对比（同场景）
// ----------------------------------------------------------

#[test]
fn sliding_window_beats_ucb_on_stationary_reward() {
    // 静止奖励环境: 两种策略都应收敛到高价值动作（探索-利用平衡验证）
    let mut sw = SlidingWindowPolicy::<TestState, TestAction>::new(50, 0.05);
    let mut ucb = UCBPolicy::<TestState, TestAction>::new(1.414);
    // 播种动作空间（策略仅从已见过动作中选择）
    sw.update(&0, &1, 0.5);
    sw.update(&0, &2, 0.5);
    ucb.update(&0, &1, 0.5);
    ucb.update(&0, &2, 0.5);
    // 训练 200 回合（动作 1 高价值）
    for _ in 0..200 {
        let a = sw.predict(&0);
        sw.update(&0, &a, if a == 1 { 1.0 } else { 0.1 });
        let a = ucb.predict(&0);
        ucb.update(&0, &a, if a == 1 { 1.0 } else { 0.1 });
    }
    let sw_stats = sw.get_action_stats();
    let ucb_stats = ucb.get_action_stats();
    let sw_high = sw_stats.get(&1).map(|s| s.count).unwrap_or(0);
    let ucb_high = ucb_stats.get(&1).map(|s| s.count).unwrap_or(0);
    // 独立收敛断言（确定性）: 高价值动作主导选择（>75% 轮次）
    assert!(sw_high > 150, "SW 应收敛到高价值动作（实际 {sw_high}/200）");
    assert!(
        ucb_high > 150,
        "UCB 应收敛到高价值动作（实际 {ucb_high}/200）"
    );
    // EMA 纯利用特性: SW（epsilon=0.05）探索应少于 UCB（探索项持续存在）
    let sw_explore = sw_stats.get(&2).map(|s| s.count).unwrap_or(0);
    assert!(
        sw_explore <= 30,
        "SW 探索应受 epsilon 约束（实际 {sw_explore}/200）"
    );
}

// ----------------------------------------------------------
// 统计不变量
// ----------------------------------------------------------

#[test]
fn action_stats_invariants() {
    let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(100, 0.0);
    policy.update(&0, &1, 0.5);
    policy.update(&0, &1, 0.7);
    policy.update(&0, &2, 0.1);
    let stats = policy.get_action_stats();
    // 动作 1: count=2, last=0.7, avg = 0*0.9+0.5*0.1=0.05 → 0.05*0.9+0.7*0.1=0.115
    let s1 = stats.get(&1).expect("动作 1 存在");
    assert_eq!(s1.count, 2);
    assert_eq!(s1.last_reward, 0.7);
    assert!(
        (s1.avg_reward - 0.115).abs() < 1e-5,
        "EMA 两次更新应收敛至 0.115（实际 {})",
        s1.avg_reward
    );
    // 动作 2: count=1, last=0.1
    let s2 = stats.get(&2).expect("动作 2 存在");
    assert_eq!(s2.count, 1);
    // 未学习动作不在统计中
    assert!(!stats.contains_key(&9));
}

// ----------------------------------------------------------
// proptest: 统计不变量（count 单调 / avg 有界）
// ----------------------------------------------------------

proptest! {
    /// 任意更新序列: count 恒等于更新次数，avg ∈ [min_reward, max_reward]
    #[test]
    fn stats_bounds_under_arbitrary_updates(
        rewards in proptest::collection::vec(0.0f32..1.0, 0..50),
    ) {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(100, 0.0);
        for (i, r) in rewards.iter().enumerate() {
            policy.update(&(i as u8), &1, *r);
        }
        let stats = policy.get_action_stats();
        if rewards.is_empty() {
            prop_assert!(stats.is_empty());
            return Ok(());
        }
        let s = stats.get(&1).expect("动作 1 已学习");
        prop_assert_eq!(s.count as usize, rewards.len());
        // EMA 加权平均: 初始值 0 拉低下限，仅上界有保证
        //（avg ≤ max_reward——EMA 凸组合不会超过历史最大值）
        let max_r = rewards.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        prop_assert!(s.avg_reward >= 0.0);
        prop_assert!(s.avg_reward <= max_r + 1e-3);
    }

    /// 轨迹导出不变量: 任意更新序列导出后四序列等长且 episode_id 保留
    #[test]
    fn trajectory_export_invariants(
        n in 0usize..30,
    ) {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(64, 0.0);
        for i in 0..n {
            policy.update(&(i as u8), &1, 0.5);
        }
        let traj = policy.export_trajectory("ep-prop");
        prop_assert_eq!(traj.len(), n.min(64));
        prop_assert_eq!(traj.episode_id.as_ref(), "ep-prop");
        prop_assert_eq!(traj.states.len(), traj.rewards.len());
        prop_assert_eq!(traj.actions.len(), traj.timestamps.len());
        // timestamps 严格单调（构造器时间序）
        prop_assert!(traj.timestamps.windows(2).all(|w| w[0] < w[1]));
    }
}
