//! 长时程信用分配器 — SHARP 统计版时间维度信用分配（设计文档 §14.4）
//!
//! 对应架构层: **L9 Quest**（quest-engine 子模块，内嵌落点 D-1）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §14.4 / §373 / §3894
//! 对应论文: SHARP（Shapley 信用分配，时间维度长程应用）
//!
//! # 核心职责
//!
//! 将终局奖励沿时间步/分段**反向**分配给各历史步骤（长时程信用分配），
//! 为 v4.0 离线 RL 训练提供时间维度的信用数据流（铁律6 RLTrajectory 导出）。
//!
//! # 与 L8 SHARP 的语义区分（D-2）
//!
//! - **L8 SHARP**（`parliament/sharp.rs`）：**空间维度** — 同一时刻多 agent
//!   协作的 Shapley 边际贡献归因（`compute_shapley` 针对 agent 联盟）。
//! - **本模块 LongTermCreditAssigner**：**时间维度** — 将终局奖励沿时间步
//!   反向分配给各历史步骤。两者语义不同，本模块在 quest-engine 内独立实现，
//!   **不直接依赖 L8 SHARP**（避免 L9→L8 语义耦合），复用 L0 RLTrajectory
//!   作为导出格式。
//!
//! # 设计约束（铁律）
//!
//! - **铁律4**: `assign_*` 为纯函数（无副作用，同输入同输出）
//! - **铁律6**: `export_rl_trajectory` 导出 L0 [`RLTrajectory`]
//! - **指数爆炸保护**: Shapley 时间归因 n > `max_exact_shapley_steps` 时
//!   返回 `None`（回退折扣累积，复用 L8 SHARP `MAX_EXACT_AGENTS` 思路）
//! - **f64 中间精度**: Shapley 幂集阶乘权重与边际乘积在 f32 下可能消减，
//!   用 f64 中间计算（同 L8 SHARP `factorial` 先例）

use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};

/// 轨迹步骤 — 时间维度信用分配的输入单元
#[derive(Debug, Clone, PartialEq)]
pub struct CreditStep {
    /// 步骤标识
    pub step_id: String,
    /// 时间戳（Unix 毫秒）
    pub timestamp_ms: u64,
    /// 本步即时奖励信号（过程奖励，可为 0）
    pub reward_signal: f32,
}

impl CreditStep {
    /// 创建轨迹步骤
    pub fn new(step_id: impl Into<String>, timestamp_ms: u64, reward_signal: f32) -> Self {
        Self {
            step_id: step_id.into(),
            timestamp_ms,
            reward_signal,
        }
    }
}

/// 信用分配结果 — 单步骤分配到的信用值
#[derive(Debug, Clone, PartialEq)]
pub struct CreditAssignment {
    /// 对应步骤标识
    pub step_id: String,
    /// 分配到的信用值（Shapley 边际贡献或折扣回报份额）
    pub credit: f32,
    /// 折扣累积回报 `G_t = r_t + γ·G_{t+1}`
    pub discounted_return: f32,
}

/// 长时程信用分配器 — SHARP 统计版（时间维度）
///
/// `discount_gamma` 为折扣因子 ∈ [0,1]（越小远期信用衰减越快）；
/// `max_exact_shapley_steps` 为精确 Shapley 幂集枚举的步骤上限（指数爆炸保护）。
#[derive(Debug, Clone)]
pub struct LongTermCreditAssigner {
    /// 折扣因子 γ ∈ [0,1]
    discount_gamma: f32,
    /// 精确 Shapley 时间归因的步骤上限（n 超限回退折扣）
    max_exact_shapley_steps: usize,
}

impl Default for LongTermCreditAssigner {
    fn default() -> Self {
        Self::new(0.9, 8)
    }
}

impl LongTermCreditAssigner {
    /// 创建长时程信用分配器
    ///
    /// - `discount_gamma`: 折扣因子，钳制至 [0,1]
    /// - `max_exact_shapley_steps`: 精确 Shapley 上限（同 L8 SHARP MAX_EXACT_AGENTS=8）
    pub fn new(discount_gamma: f32, max_exact_shapley_steps: usize) -> Self {
        Self {
            discount_gamma: discount_gamma.clamp(0.0, 1.0),
            max_exact_shapley_steps,
        }
    }

    /// 折扣累积回报分配（规范 §14.4 统计版基础方法）
    ///
    /// 反向遍历，终局奖励注入末步：`G_t = r_t + γ·G_{t+1}`，
    /// 末步 `G_n = r_n + terminal_reward`。credit 取折扣回报与即时奖励之差
    /// （即"后续步骤贡献的折现"），便于与即时奖励区分。
    ///
    /// 空轨迹返回空 Vec。纯函数（铁律4）。
    pub fn assign_discounted_return(
        &self,
        steps: &[CreditStep],
        terminal_reward: f32,
    ) -> Vec<CreditAssignment> {
        let n = steps.len();
        if n == 0 {
            return Vec::new();
        }
        let gamma = self.discount_gamma;
        // 反向累积折扣回报
        let mut returns = vec![0.0f32; n];
        let mut g = terminal_reward;
        for t in (0..n).rev() {
            g = steps[t].reward_signal + gamma * g;
            returns[t] = g;
        }
        steps
            .iter()
            .enumerate()
            .map(|(t, step)| CreditAssignment {
                step_id: step.step_id.clone(),
                // credit = 折扣回报 - 即时奖励 = 后续步骤贡献的折现
                credit: returns[t] - step.reward_signal,
                discounted_return: returns[t],
            })
            .collect()
    }

    /// Shapley 时间归因（规范 §14.4 SHARP 统计版精确方法）
    ///
    /// 将终局奖励视为"全体步骤协作"的总价值，按 Shapley 值分配各步骤的
    /// 边际贡献。步骤子集的价值 `v(S)` 定义为子集内即时奖励之和按折扣
    /// 累积后与终局奖励的加权（子集越接近完整轨迹，价值越接近终局）。
    ///
    /// - `n > max_exact_shapley_steps` → `None`（指数爆炸保护，回退折扣）
    /// - 空轨迹 → `Some(空 Vec)`
    ///
    /// 纯函数（铁律4）；f64 中间精度（阶乘权重与边际乘积）。
    pub fn assign_shapley_temporal(
        &self,
        steps: &[CreditStep],
        terminal_reward: f32,
    ) -> Option<Vec<CreditAssignment>> {
        let n = steps.len();
        if n > self.max_exact_shapley_steps {
            return None;
        }
        if n == 0 {
            return Some(Vec::new());
        }
        let gamma = self.discount_gamma;
        // 预计算各步骤折扣回报（用于 CreditAssignment.discounted_return 字段）
        let mut returns = vec![0.0f32; n];
        let mut g = terminal_reward;
        for t in (0..n).rev() {
            g = steps[t].reward_signal + gamma * g;
            returns[t] = g;
        }
        // 总信用 = 终局奖励，按各步 reward_signal 比例分配（加性博弈）。
        // 效率公理：全体 Shapley 之和 = terminal_reward；对称公理：等 reward_signal 等信用。
        let terminal_f64 = f64::from(terminal_reward);
        // 各步骤 Shapley 值（f64 中间精度）
        let mut shapley = vec![0.0f64; n];
        let fact_n = factorial(n);
        for (i, shapley_i) in shapley.iter_mut().enumerate() {
            let others: Vec<usize> = (0..n).filter(|&j| j != i).collect();
            for mask in 0..(1u64 << others.len()) {
                let mut subset: Vec<usize> = Vec::with_capacity(others.len());
                for (k, &j) in others.iter().enumerate() {
                    if mask & (1u64 << k) != 0 {
                        subset.push(j);
                    }
                }
                let v_without = coalition_value(&subset, steps, terminal_f64);
                let mut with = subset.clone();
                with.push(i);
                let v_with = coalition_value(&with, steps, terminal_f64);
                let marginal = v_with - v_without;
                let s = subset.len();
                let weight = factorial(s) * factorial(n - s - 1) / fact_n;
                *shapley_i += weight * marginal;
            }
        }
        Some(
            steps
                .iter()
                .enumerate()
                .map(|(t, step)| CreditAssignment {
                    step_id: step.step_id.clone(),
                    credit: shapley[t] as f32,
                    discounted_return: returns[t],
                })
                .collect(),
        )
    }

    /// 导出 L0 RLTrajectory（铁律6）
    ///
    /// states 用 `RLStateVector::zeros()`（时间维度信用分配无空间状态观测），
    /// actions 用 `RLActionVector::new("L9", step_index, vec![credit])`，
    /// rewards 用折扣累积回报，timestamps 从步骤复制。
    ///
    /// # Panics
    ///
    /// `steps` 与 `credits` 长度不一致时 panic（RLTrajectory 四序列等长不变量）。
    pub fn export_rl_trajectory(
        &self,
        episode_id: &str,
        steps: &[CreditStep],
        credits: &[CreditAssignment],
    ) -> RLTrajectory {
        assert_eq!(
            steps.len(),
            credits.len(),
            "export_rl_trajectory 不变量: steps 与 credits 必须等长"
        );
        let states: Vec<RLStateVector> = steps.iter().map(|_| RLStateVector::zeros()).collect();
        let actions: Vec<RLActionVector> = credits
            .iter()
            .enumerate()
            .map(|(idx, c)| RLActionVector::new("L9", idx as u32, vec![c.credit]))
            .collect();
        let rewards: Vec<f32> = credits.iter().map(|c| c.discounted_return).collect();
        let timestamps: Vec<u64> = steps.iter().map(|s| s.timestamp_ms).collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }
}

/// 阶乘（f64 中间精度；n ≤ 8 时 40320，无溢出风险，同 L8 SHARP 先例）
fn factorial(n: usize) -> f64 {
    let mut r = 1.0f64;
    for i in 2..=n {
        r *= i as f64;
    }
    r
}

/// 步骤子集的联盟价值 `v(S)`（时间维度 Shapley 的子集价值函数，加性比例模型）
///
/// 总信用 = 终局奖励 `terminal_reward`，按各步 `reward_signal` 比例分配：
/// - 空集恒 0；全集 = terminal_reward（效率公理：全体步骤分得全部终局信用）
/// - 加性博弈：`v(S) = terminal_reward × sum_S(reward_signal) / sum_all(reward_signal)`，
///   故步骤 i 的 Shapley 值 = `terminal_reward × reward_signal[i] / sum_all`
/// - 全零 reward_signal 时终局奖励按步骤数均分（防御除零）
fn coalition_value(subset: &[usize], steps: &[CreditStep], terminal_reward: f64) -> f64 {
    if subset.is_empty() {
        return 0.0;
    }
    let n = steps.len();
    let sum_all: f64 = steps.iter().map(|s| f64::from(s.reward_signal)).sum();
    if sum_all.abs() < 1e-9 {
        // 全零 reward_signal: 终局奖励按步骤数均分（加性）
        return terminal_reward * subset.len() as f64 / n as f64;
    }
    // 按 reward_signal 比例分配终局奖励（加性博弈）
    let subset_sum: f64 = subset
        .iter()
        .map(|&i| f64::from(steps[i].reward_signal))
        .sum();
    terminal_reward * subset_sum / sum_all
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(rewards: &[f32]) -> Vec<CreditStep> {
        rewards
            .iter()
            .enumerate()
            .map(|(i, r)| CreditStep::new(format!("s{i}"), (i as u64) * 1000, *r))
            .collect()
    }

    #[test]
    fn discounted_return_three_steps() {
        // γ=0.9，三步 [1.0, 0.0, 0.0]，terminal=0.0
        // G_2 = 0 + 0.9*0 = 0; G_1 = 0 + 0.9*0 = 0; G_0 = 1.0 + 0.9*0 = 1.0
        let assigner = LongTermCreditAssigner::new(0.9, 8);
        let credits = assigner.assign_discounted_return(&steps(&[1.0, 0.0, 0.0]), 0.0);
        assert_eq!(credits.len(), 3);
        assert!((credits[0].discounted_return - 1.0).abs() < 1e-6);
        assert!((credits[1].discounted_return - 0.0).abs() < 1e-6);
        assert!((credits[2].discounted_return - 0.0).abs() < 1e-6);
    }

    #[test]
    fn discounted_return_terminal_injected_to_last() {
        // γ=1.0，三步 [0,0,0]，terminal=3.0 → 各步回报均为 3.0（无衰减传递）
        let assigner = LongTermCreditAssigner::new(1.0, 8);
        let credits = assigner.assign_discounted_return(&steps(&[0.0, 0.0, 0.0]), 3.0);
        for c in &credits {
            assert!((c.discounted_return - 3.0).abs() < 1e-6, "γ=1 无衰减传递");
        }
    }

    #[test]
    fn discounted_return_gamma_decay() {
        // γ=0.5，两步 [0,0]，terminal=4.0
        // G_1 = 0 + 0.5*4 = 2.0; G_0 = 0 + 0.5*2 = 1.0
        let assigner = LongTermCreditAssigner::new(0.5, 8);
        let credits = assigner.assign_discounted_return(&steps(&[0.0, 0.0]), 4.0);
        assert!((credits[0].discounted_return - 1.0).abs() < 1e-6);
        assert!((credits[1].discounted_return - 2.0).abs() < 1e-6);
    }

    #[test]
    fn empty_steps_returns_empty() {
        let assigner = LongTermCreditAssigner::new(0.9, 8);
        assert!(assigner.assign_discounted_return(&[], 1.0).is_empty());
        assert_eq!(assigner.assign_shapley_temporal(&[], 1.0), Some(Vec::new()));
    }

    #[test]
    fn shapley_temporal_symmetry() {
        // 对称奖励 [1.0, 1.0]，terminal=0 → 两步 Shapley 应相等（对称公理）
        let assigner = LongTermCreditAssigner::new(1.0, 8);
        let credits = assigner
            .assign_shapley_temporal(&steps(&[1.0, 1.0]), 0.0)
            .expect("n=2 ≤ 8");
        assert_eq!(credits.len(), 2);
        assert!(
            (credits[0].credit - credits[1].credit).abs() < 1e-4,
            "对称步骤 Shapley 应相等: {} vs {}",
            credits[0].credit,
            credits[1].credit
        );
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

    #[test]
    fn shapley_efficiency_axiom() {
        // 效率公理：全体步骤 Shapley 之和 = 总价值（终局价值）
        let assigner = LongTermCreditAssigner::new(1.0, 8);
        let terminal = 6.0f32;
        let credits = assigner
            .assign_shapley_temporal(&steps(&[1.0, 2.0, 3.0]), terminal)
            .expect("n=3 ≤ 8");
        let sum: f32 = credits.iter().map(|c| c.credit).sum();
        assert!(
            (sum - terminal).abs() < 1e-3,
            "Shapley 之和应等于终局价值: sum={} terminal={}",
            sum,
            terminal
        );
    }

    #[test]
    fn export_rl_trajectory_four_sequences_equal_length() {
        let assigner = LongTermCreditAssigner::new(0.9, 8);
        let steps_vec = steps(&[1.0, 0.5]);
        let credits = assigner.assign_discounted_return(&steps_vec, 2.0);
        let traj = assigner.export_rl_trajectory("ep-1", &steps_vec, &credits);
        assert_eq!(traj.episode_id.as_ref(), "ep-1");
        assert_eq!(traj.states.len(), 2);
        assert_eq!(traj.actions.len(), 2);
        assert_eq!(traj.rewards.len(), 2);
        assert_eq!(traj.timestamps.len(), 2);
        // actions 层标识为 L9
        assert_eq!(traj.actions[0].layer.as_ref(), "L9");
    }

    #[test]
    #[should_panic(expected = "steps 与 credits 必须等长")]
    fn export_rl_trajectory_panics_on_length_mismatch() {
        let assigner = LongTermCreditAssigner::new(0.9, 8);
        let steps_vec = steps(&[1.0, 0.5]);
        let credits = assigner.assign_discounted_return(&steps_vec, 2.0);
        // 截断 credits 制造长度不一致
        let truncated: Vec<CreditAssignment> = credits.into_iter().take(1).collect();
        let _ = assigner.export_rl_trajectory("ep-x", &steps_vec, &truncated);
    }

    #[test]
    fn gamma_clamped_to_unit_interval() {
        let assigner = LongTermCreditAssigner::new(1.5, 8);
        assert_eq!(assigner.discount_gamma, 1.0);
        let assigner2 = LongTermCreditAssigner::new(-0.5, 8);
        assert_eq!(assigner2.discount_gamma, 0.0);
    }
}
