//! 统计学习接口层 — v4.0 RL 升级的 Rust 侧统计先行（设计文档 §6.3 + §17）
//!
//! 对应架构层: **L1 Core**（nexus-core 新增模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6.3 / §17.2
//! 对应规划: RL 架构预留（统计先行：SlidingWindow/UCB/EMA/Softmax，v4.0 替换为神经网络）
//!
//! # 核心职责
//!
//! 承载 v4.0 RL 升级的统计学习接口：
//!
//! | 组件 | 实现 | v4.0 升级路径 |
//! |------|------|-------------|
//! | [`StatLearningPolicy`] | 统一 trait（State→Action 接口同构，铁律2） | 替换为 ONNX 策略网络 |
//! | [`SlidingWindowPolicy`] | 滑动窗口 EMA（OpenMLE 动态奖励归一化） | PPO Actor 网络 |
//! | [`UCBPolicy`] | 上置信界（OpenMLE 三因子选择基础） | 神经网络学习权重 |
//!
//! # 设计约束（铁律）
//!
//! - **铁律6**: `export_trajectory` 使所有统计学习历史可导出为
//!   `RLTrajectory`（为 v4.0 升级预留数据流）
//! - **铁律2**: 接口同构——`StatLearningPolicy` 与 RL `Policy` 接口同构
//!   （State→Action），策略可替换（RulePolicyFallback 为默认）
//! - **接口可替换**: 每层可独立升级 RL 策略，不影响其他层（§17.1）
//! - **f32 字段仅 `PartialEq`**: ActionStats 含浮点字段
//! - **泛型状态/动作**: `State: Clone + Hash + Eq`，`Action: Clone + Hash + Eq`
//!   （接缝动作空间封闭枚举均满足，如 L0 `RLAction`）

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};

// ============================================================
// 统计学习策略 trait
// ============================================================

/// 动作统计 — 单动作的学习状态快照
#[derive(Debug, Clone, PartialEq)]
pub struct ActionStats {
    /// 动作被选择的次数
    pub count: u32,
    /// 平均奖励（EMA 现值）
    pub avg_reward: f32,
    /// 最近一次奖励
    pub last_reward: f32,
    /// 置信度（探索项；越大越需探索）
    pub confidence: f32,
}

/// 统计学习策略 — 泛型接口（与 RL Policy 同构，铁律2）
///
/// # v4.0 升级
///
/// 实现方可替换为 ONNX 策略网络（§17.2）——接口不变，数据流不变。
pub trait StatLearningPolicy: Send + Sync {
    /// 状态类型（如 L0 `RLState` / 接缝上下文向量）
    type State: Clone + Hash + Eq;
    /// 动作类型（如 L0 `RLAction` / 接缝动作枚举）
    type Action: Clone + Hash + Eq;

    /// 预测动作（利用 + 探索）
    fn predict(&self, state: &Self::State) -> Self::Action;

    /// 更新策略（经验回填）
    fn update(&mut self, state: &Self::State, action: &Self::Action, reward: f32);

    /// 导出学习历史为 RL 轨迹（铁律6）
    ///
    /// `episode_id` 由调用方提供（如任务 ID），timestamps 按历史顺序生成。
    fn export_trajectory(&self, episode_id: &str) -> RLTrajectory;

    /// 各动作的统计快照
    fn get_action_stats(&self) -> HashMap<Self::Action, ActionStats>;

    /// 状态投影为 RL 状态向量（铁律6 轨迹导出的数据面形态）
    ///
    /// 默认全零投影（调用方覆盖为真实编码）。
    fn project_state(&self, _state: &Self::State) -> RLStateVector {
        RLStateVector::zeros()
    }

    /// 动作投影为 RL 动作向量（铁律6 轨迹导出的数据面形态）
    ///
    /// 默认 `layer: "stat"` + 动作码 0（调用方覆盖为真实编码）。
    fn project_action(&self, _action: &Self::Action) -> RLActionVector {
        RLActionVector::new("stat", 0, vec![])
    }
}

// ============================================================
// Sliding Window EMA Policy
// ============================================================

/// 滑动窗口 EMA 策略 — OpenMLE 动态奖励归一化
///
/// - **EMA 更新**: `value = value * 0.9 + reward * 0.1`（平滑跟踪非平稳奖励）
/// - **epsilon-greedy 探索**: 以 `epsilon` 概率随机选择已见动作
/// - **窗口滑动**: 历史超窗淘汰（保留近期经验，遗忘陈旧信号）
#[derive(Debug, Clone)]
pub struct SlidingWindowPolicy<S, A> {
    /// 历史窗口大小
    window_size: usize,
    /// 滑动历史 (state, action, reward)
    history: VecDeque<(S, A, f32)>,
    /// 动作选择计数
    action_counts: HashMap<A, u32>,
    /// 动作 EMA 奖励
    action_rewards: HashMap<A, f32>,
    /// 探索概率 [0,1]
    epsilon: f32,
}

impl<S, A> SlidingWindowPolicy<S, A>
where
    S: Clone + Hash + Eq,
    A: Clone + Hash + Eq,
{
    /// 创建滑动窗口策略
    ///
    /// - `window_size`: 历史窗口大小（> 0）
    /// - `epsilon`: 探索概率 [0,1]（0 = 纯利用，1 = 纯随机）
    pub fn new(window_size: usize, epsilon: f32) -> Self {
        assert!(window_size > 0, "window_size 必须 > 0");
        assert!((0.0..=1.0).contains(&epsilon), "epsilon 必须在 [0,1] 区间");
        Self {
            window_size,
            history: VecDeque::with_capacity(window_size),
            action_counts: HashMap::new(),
            action_rewards: HashMap::new(),
            epsilon,
        }
    }

    /// 以自定义投影导出轨迹 — 供包装型子类（如 ProjectedPolicy）复用历史数据
    ///
    /// WHY 独立方法: 静态分派下子类委托 `inner.export_trajectory` 会丢失
    /// 子类的投影覆盖（调用的是 SlidingWindowPolicy 的默认投影）；
    /// 本方法允许子类传入自己的投影闭包，保持历史数据共享。
    pub fn export_trajectory_with_projection<F1, F2>(
        &self,
        episode_id: &str,
        project_state: F1,
        project_action: F2,
    ) -> RLTrajectory
    where
        F1: Fn(&S) -> RLStateVector,
        F2: Fn(&A) -> RLActionVector,
    {
        let states: Vec<RLStateVector> = self
            .history
            .iter()
            .map(|(s, _, _)| project_state(s))
            .collect();
        let actions: Vec<RLActionVector> = self
            .history
            .iter()
            .map(|(_, a, _)| project_action(a))
            .collect();
        let rewards: Vec<f32> = self.history.iter().map(|(_, _, r)| *r).collect();
        let timestamps: Vec<u64> = (0..self.history.len() as u64)
            .map(|i| 1_700_000_000_000 + i * 1_000)
            .collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }
}

impl<S, A> StatLearningPolicy for SlidingWindowPolicy<S, A>
where
    S: Clone + Hash + Eq + Send + Sync,
    A: Clone + Hash + Eq + Default + Send + Sync,
{
    type State = S;
    type Action = A;

    fn predict(&self, _state: &Self::State) -> Self::Action {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        // epsilon-greedy 探索: 随机选择已见动作
        if self.epsilon > 0.0 && rng.gen::<f32>() < self.epsilon {
            let actions: Vec<&A> = self.action_counts.keys().collect();
            if !actions.is_empty() {
                let idx = rng.gen_range(0..actions.len());
                return actions[idx].clone();
            }
        }
        // 利用: 选择 EMA 奖励最高的动作（未见动作回退默认值）
        self.action_rewards
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(a, _)| a.clone())
            .unwrap_or_default()
    }

    fn update(&mut self, state: &Self::State, action: &Self::Action, reward: f32) {
        // 窗口滑动（先进先出淘汰）
        self.history
            .push_back((state.clone(), action.clone(), reward));
        if self.history.len() > self.window_size {
            self.history.pop_front();
        }
        // 计数 + EMA 更新（0.9/0.1 平滑系数）
        *self.action_counts.entry(action.clone()).or_insert(0) += 1;
        let current = self.action_rewards.entry(action.clone()).or_insert(0.0);
        *current = (*current * 0.9) + (reward * 0.1);
    }

    fn export_trajectory(&self, episode_id: &str) -> RLTrajectory {
        // 默认投影委托（子类可调用 with_projection 提供自定义编码）
        self.export_trajectory_with_projection(
            episode_id,
            |s| self.project_state(s),
            |a| self.project_action(a),
        )
    }

    fn get_action_stats(&self) -> HashMap<Self::Action, ActionStats> {
        self.action_rewards
            .iter()
            .map(|(action, reward)| {
                let count = self.action_counts.get(action).copied().unwrap_or(0);
                let last_reward = self
                    .history
                    .iter()
                    .rev()
                    .find(|(_, a, _)| a == action)
                    .map(|(_, _, r)| *r)
                    .unwrap_or(0.0);
                (
                    action.clone(),
                    ActionStats {
                        count,
                        avg_reward: *reward,
                        last_reward,
                        // 置信度 = avg / sqrt(count)（访问越多越确定）
                        confidence: if count > 0 {
                            reward.abs() / (count as f32).sqrt()
                        } else {
                            f32::MAX
                        },
                    },
                )
            })
            .collect()
    }
}

// ============================================================
// UCB Policy
// ============================================================

/// UCB 策略 — OpenMLE 三因子选择基础（探索-利用平衡）
///
/// UCB 分数: `value + c * sqrt(2 * ln(total_visits) / visits)`
/// - 未访问动作分数为 `f32::MAX`（必选——确保全覆盖）
/// - 访问越少的动作探索项越大（置信区间上界）
#[derive(Debug, Clone)]
pub struct UCBPolicy<S, A> {
    /// 总访问次数
    total_visits: u32,
    /// 动作访问次数
    action_visits: HashMap<A, u32>,
    /// 动作 EMA 价值
    action_values: HashMap<A, f32>,
    /// 探索常数 c（越大越探索）
    exploration_constant: f32,
    /// 状态类型标记（UCB 状态无关）
    _phantom: std::marker::PhantomData<S>,
}

impl<S, A> UCBPolicy<S, A>
where
    S: Clone + Hash + Eq,
    A: Clone + Hash + Eq,
{
    /// 创建 UCB 策略
    ///
    /// - `exploration_constant`: 探索常数（常见取值 sqrt(2) ≈ 1.414）
    pub fn new(exploration_constant: f32) -> Self {
        assert!(exploration_constant >= 0.0, "exploration_constant 必须非负");
        Self {
            total_visits: 0,
            action_visits: HashMap::new(),
            action_values: HashMap::new(),
            exploration_constant,
            _phantom: std::marker::PhantomData,
        }
    }

    /// UCB 分数 — 价值 + 探索项
    fn ucb_score(&self, action: &A) -> f32 {
        let value = self.action_values.get(action).copied().unwrap_or(0.0);
        let visits = self.action_visits.get(action).copied().unwrap_or(0);
        if visits == 0 {
            // 未访问动作必选（全探索）
            return f32::MAX;
        }
        value
            + self.exploration_constant
                * ((2.0 * (self.total_visits as f32).ln()) / (visits as f32)).sqrt()
    }
}

impl<S, A> StatLearningPolicy for UCBPolicy<S, A>
where
    S: Clone + Hash + Eq + Send + Sync,
    A: Clone + Hash + Eq + Default + Send + Sync,
{
    type State = S;
    type Action = A;

    fn predict(&self, _state: &Self::State) -> Self::Action {
        // 动作并集（含已访问与已估值）
        let actions: std::collections::HashSet<&A> = self
            .action_values
            .keys()
            .chain(self.action_visits.keys())
            .collect();
        actions
            .iter()
            .max_by(|a, b| {
                self.ucb_score(a)
                    .partial_cmp(&self.ucb_score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|a| (*a).clone())
            .unwrap_or_default()
    }

    fn update(&mut self, _state: &Self::State, action: &Self::Action, reward: f32) {
        self.total_visits += 1;
        *self.action_visits.entry(action.clone()).or_insert(0) += 1;
        // EMA 价值更新（与 SlidingWindow 同系数，便于对比）
        let value = self.action_values.entry(action.clone()).or_insert(0.0);
        *value = (*value * 0.9) + (reward * 0.1);
    }

    fn export_trajectory(&self, episode_id: &str) -> RLTrajectory {
        // UCB 为状态无关策略，不保留逐回合历史——
        // 导出当前估值快照作为可追溯轨迹（铁律6 最小满足）
        let actions: Vec<RLActionVector> = self
            .action_values
            .keys()
            .map(|a| self.project_action(a))
            .collect();
        let rewards: Vec<f32> = self.action_values.values().copied().collect();
        let states: Vec<RLStateVector> = vec![RLStateVector::zeros(); actions.len()];
        let timestamps: Vec<u64> = (0..actions.len() as u64)
            .map(|i| 1_700_000_000_000 + i * 1_000)
            .collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }

    fn get_action_stats(&self) -> HashMap<Self::Action, ActionStats> {
        self.action_values
            .iter()
            .map(|(action, reward)| {
                let count = self.action_visits.get(action).copied().unwrap_or(0);
                (
                    action.clone(),
                    ActionStats {
                        count,
                        avg_reward: *reward,
                        last_reward: *reward,
                        // 置信度 = 探索项（访问越少越需探索）
                        confidence: if count > 0 {
                            self.exploration_constant
                                * ((2.0 * (self.total_visits as f32).ln()) / (count as f32)).sqrt()
                        } else {
                            f32::MAX
                        },
                    },
                )
            })
            .collect()
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    type TestState = u8;
    type TestAction = u8;

    // ---------- SlidingWindow: EMA 收敛 ----------

    #[test]
    fn ema_converges_to_constant_reward() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(100, 0.0);
        // 恒定奖励 1.0 重复更新 → EMA 收敛至 ≈1.0
        for _ in 0..200 {
            policy.update(&0, &1, 1.0);
        }
        let stats = policy.get_action_stats();
        let s = stats.get(&1).expect("动作 1 已学习");
        assert!(
            (s.avg_reward - 1.0).abs() < 0.01,
            "EMA 应收敛至 1.0（实际 {})",
            s.avg_reward
        );
        // 利用模式: 应稳定选择动作 1
        assert_eq!(policy.predict(&0), 1);
    }

    #[test]
    fn ema_tracks_reward_change() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(100, 0.0);
        for _ in 0..100 {
            policy.update(&0, &1, 0.0);
        }
        // 奖励突变为 1.0 → EMA 应上升（但不立即到 1.0）
        for _ in 0..30 {
            policy.update(&0, &1, 1.0);
        }
        // 先绑定 stats，避免临时值借用（E0716）
        let stats = policy.get_action_stats();
        let s = stats.get(&1).expect("已学习");
        assert!(
            s.avg_reward > 0.5,
            "EMA 应追踪奖励变化（实际 {})",
            s.avg_reward
        );
    }

    // ---------- SlidingWindow: 窗口滑动 ----------

    #[test]
    fn window_slides_evicts_old_history() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(5, 0.0);
        for i in 0..20 {
            policy.update(&i, &(i % 3), 1.0);
        }
        // 历史窗口只保留最近 5 条（history 长度受窗口约束）
        assert_eq!(policy.history.len(), 5);
    }

    // ---------- SlidingWindow: epsilon 探索边界 ----------

    #[test]
    fn epsilon_zero_is_pure_exploitation() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(10, 0.0);
        policy.update(&0, &1, 1.0);
        policy.update(&0, &2, 0.0);
        for _ in 0..100 {
            assert_eq!(policy.predict(&0), 1, "epsilon=0 必须纯利用");
        }
    }

    #[test]
    fn epsilon_one_explores_seen_actions() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(10, 1.0);
        policy.update(&0, &1, 1.0);
        policy.update(&0, &2, 1.0);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            seen.insert(policy.predict(&0));
        }
        assert!(
            seen.len() > 1,
            "epsilon=1 必须探索到多个动作（实际 {:?}）",
            seen
        );
    }

    // ---------- SlidingWindow: 铁律6 轨迹导出 ----------

    #[test]
    fn sliding_window_export_trajectory() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(10, 0.0);
        for i in 0..4 {
            policy.update(&i, &i, 0.1 * i as f32);
        }
        let traj = policy.export_trajectory("ep-1");
        assert_eq!(traj.episode_id.as_ref(), "ep-1");
        assert_eq!(traj.len(), 4);
        assert_eq!(traj.rewards.len(), 4);
        assert_eq!(traj.timestamps.len(), 4);
        // 四序列等长（构造器已断言）
    }

    // ---------- UCB: 探索-利用平衡 ----------

    #[test]
    fn ucb_selects_from_known_actions() {
        let mut policy = UCBPolicy::<TestState, TestAction>::new(1.414);
        // UCB 候选集 = 已见过动作（action_values ∪ action_visits）
        policy.update(&0, &1, 1.0);
        policy.update(&0, &2, 0.5);
        // 预测必须在已知动作空间中（全覆盖语义：不遗漏任何候选）
        for _ in 0..20 {
            let picked = policy.predict(&0);
            assert!(picked == 1 || picked == 2, "UCB 必须从已知动作空间选择");
        }
        // 未见过动作不在候选集（动作空间由经验累积发现）
        assert!(!policy.get_action_stats().contains_key(&9));
    }

    #[test]
    fn ucb_prefers_high_value_action_over_time() {
        let mut policy = UCBPolicy::<TestState, TestAction>::new(0.5);
        // 播种候选动作空间（UCB 仅从已见过动作中选择）
        policy.update(&0, &1, 0.5);
        policy.update(&0, &2, 0.5);
        // 动作 1 高价值（奖励 1.0），动作 2 低价值（奖励 0.1）
        for _ in 0..50 {
            let a = policy.predict(&0);
            let reward = if a == 1 { 1.0 } else { 0.1 };
            policy.update(&0, &a, reward);
        }
        let stats = policy.get_action_stats();
        let s1 = stats.get(&1).expect("动作 1 已学习");
        let s2 = stats.get(&2).expect("动作 2 已学习");
        assert!(
            s1.count > s2.count,
            "高价值动作应被更多选择（动作1: {} vs 动作2: {}）",
            s1.count,
            s2.count
        );
    }

    #[test]
    fn ucb_export_trajectory_snapshot() {
        let mut policy = UCBPolicy::<TestState, TestAction>::new(1.0);
        policy.update(&0, &1, 0.8);
        policy.update(&0, &2, 0.3);
        let traj = policy.export_trajectory("ep-ucb");
        assert_eq!(traj.episode_id.as_ref(), "ep-ucb");
        assert_eq!(traj.len(), 2, "估值快照轨迹含 2 动作");
        // 四序列等长
        assert_eq!(traj.rewards.len(), 2);
        assert_eq!(traj.timestamps.len(), 2);
    }

    // ---------- 泛型投影 ----------

    #[test]
    fn default_projection_is_zero_vectors() {
        let policy = SlidingWindowPolicy::<TestState, TestAction>::new(10, 0.0);
        let state_vec = policy.project_state(&7);
        assert!(state_vec.clv.iter().all(|&x| x == 0.0));
        assert!(state_vec.layer_features.iter().all(|&x| x == 0.0));
        let action_vec = policy.project_action(&3);
        assert_eq!(action_vec.layer.as_ref(), "stat");
        assert_eq!(action_vec.action_code, 0);
    }

    // ---------- 统计快照 ----------

    #[test]
    fn action_stats_tracking() {
        let mut policy = SlidingWindowPolicy::<TestState, TestAction>::new(100, 0.0);
        policy.update(&0, &1, 0.5);
        policy.update(&0, &1, 1.0);
        let stats = policy.get_action_stats();
        let s = stats.get(&1).expect("动作 1 统计存在");
        assert_eq!(s.count, 2);
        assert_eq!(s.last_reward, 1.0);
        // EMA 数学: 0*0.9+0.5*0.1=0.05 → 0.05*0.9+1.0*0.1=0.145
        assert!(
            (s.avg_reward - 0.145).abs() < 1e-5,
            "EMA 两次更新应收敛至 0.145（实际 {})",
            s.avg_reward
        );
        // 未学习动作不在统计中
        assert!(!stats.contains_key(&9));
    }
}
