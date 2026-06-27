//! GRPO 组采样与优势计算 — 基于 DeepSeek V4 GRPO 算法
//!
//! 对应架构层:L5 Knowledge
//!
//! # 算法核心
//! GRPO(Group Relative Policy Optimization)省去 critic 网络,直接用同一
//! prompt 的一组采样奖励的均值/标准差作为 baseline,计算组内相对优势:
//!
//! ```text
//! A_i = (r_i - mean(r)) / (std(r) + eps)
//! ```
//!
//! 本周为占位实现:基于规则的采样(不引入真实模型推理)。
//! // TODO(Week 7): 接入 MCP Mesh 真实模型

use crate::types::{EvolutionPolicy, GrpoRollout};

/// 防除零常数(GRPO 论文标准值)
const EPS: f32 = 1e-8;

/// 单条轨迹的动作维度(本周占位固定为 10)
const ACTION_DIM: usize = 10;

/// 最优解动作(本周占位:全 1.0 向量,reward 基于距离该向量的负距离)
const OPTIMAL_ACTION: f32 = 1.0;

/// 线性同余 PRNG — 零外部依赖的伪随机数生成器
///
/// WHY:本周占位不引入 rand crate(避免 workspace 依赖膨胀)。
/// 使用 glibc 同余参数(a=1103515245, c=12345, m=2^31),周期 2^31
/// 足够覆盖采样需求。Week 7 接入真实模型后由模型 logits 替代。
pub(crate) struct Lcg {
    state: u64,
}

impl Lcg {
    /// 以 seed 构造 PRNG
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(1),
        }
    }

    /// 生成下一个 u32 伪随机数
    pub(crate) fn next_u32(&mut self) -> u32 {
        // glibc LCG 参数:m=2^31, a=1103515245, c=12345
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        // 取高 31 位作为输出(低位的周期较短)
        ((self.state >> 16) & 0x7FFF_FFFF) as u32
    }

    /// 生成 [-1.0, 1.0) 范围的 f32
    pub(crate) fn next_f32(&mut self) -> f32 {
        let raw = self.next_u32() as f32 / (u32::MAX as f32 / 2.0);
        raw - 1.0
    }
}

/// 基于 policy 与 seed 采样一组 rollout
///
/// 本周占位逻辑:
/// 1. 为每条轨迹生成 ACTION_DIM=10 维动作向量
/// 2. 动作 = 基准值(1.0)+ mutation_rate * 随机扰动
/// 3. reward = -|actions - optimal| 的均值(越接近最优解 reward 越高,最高 0.0)
///
/// // TODO(Week 7): 接入 MCP Mesh 真实模型,用 logits 采样替代规则采样
pub fn sample_rollouts(policy: &EvolutionPolicy, count: u32) -> Vec<GrpoRollout> {
    let mut rng = Lcg::new(42);
    let mut rollouts = Vec::with_capacity(count as usize);

    for i in 0..count {
        let mut actions = Vec::with_capacity(ACTION_DIM);
        for _ in 0..ACTION_DIM {
            // 动作 = 基准 + mutation_rate * 扰动
            let action = OPTIMAL_ACTION + policy.mutation_rate * rng.next_f32();
            actions.push(action);
        }

        // reward = 负距离(越接近最优解 reward 越接近 0.0)
        let distance = actions
            .iter()
            .map(|a| (a - OPTIMAL_ACTION).abs())
            .sum::<f32>()
            / ACTION_DIM as f32;
        let reward = -distance;

        let trajectory_id = format!("traj-{i}");
        rollouts.push(GrpoRollout::new(trajectory_id, actions, reward));
    }

    rollouts
}

/// 计算组内相对优势(GRPO 核心公式)
///
/// 原地修改 rollouts 中每个元素的 advantage 字段:
/// ```text
/// A_i = (r_i - mean(r)) / (std(r) + eps)
/// ```
///
/// 边界处理:
/// - 空 rollout:直接返回(无操作)
/// - 单个 rollout:advantage = 0.0(无组内对比基准)
/// - 全相同 reward:advantage = 0.0(std=0,分子=0)
pub fn compute_advantage(rollouts: &mut [GrpoRollout]) {
    if rollouts.is_empty() {
        return;
    }

    let n = rollouts.len() as f32;
    let mean: f32 = rollouts.iter().map(|r| r.reward).sum::<f32>() / n;

    // 样本标准差(除以 n 而非 n-1,与 GRPO 原始实现一致)
    let variance: f32 = rollouts
        .iter()
        .map(|r| {
            let diff = r.reward - mean;
            diff * diff
        })
        .sum::<f32>()
        / n;
    let std = variance.sqrt();

    let denom = std + EPS;
    for rollout in rollouts.iter_mut() {
        let advantage = (rollout.reward - mean) / denom;
        rollout.advantage = Some(advantage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> EvolutionPolicy {
        EvolutionPolicy::new(0.1, 1.5, 0.2, 8).unwrap()
    }

    #[test]
    fn test_sample_rollouts_count() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 8);
        assert_eq!(rollouts.len(), 8);
    }

    #[test]
    fn test_sample_rollouts_count_zero() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 0);
        assert!(rollouts.is_empty());
    }

    #[test]
    fn test_sample_rollouts_action_dim() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 4);
        for rollout in &rollouts {
            assert_eq!(rollout.actions.len(), ACTION_DIM);
        }
    }

    #[test]
    fn test_sample_rollouts_advantage_none_before_compute() {
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 4);
        for rollout in &rollouts {
            assert!(rollout.advantage.is_none());
        }
    }

    #[test]
    fn test_sample_rollouts_reward_non_positive() {
        // reward = -distance <= 0.0(距离非负)
        let policy = make_policy();
        let rollouts = sample_rollouts(&policy, 8);
        for rollout in &rollouts {
            assert!(rollout.reward <= 0.0, "reward 应为非正: {}", rollout.reward);
        }
    }

    #[test]
    fn test_compute_advantage_empty() {
        let mut rollouts: Vec<GrpoRollout> = vec![];
        compute_advantage(&mut rollouts);
        assert!(rollouts.is_empty());
    }

    #[test]
    fn test_compute_advantage_single_rollout() {
        let mut rollouts = vec![GrpoRollout::new("t-0".into(), vec![1.0], 0.5)];
        compute_advantage(&mut rollouts);
        // 单个 rollout:mean=reward,std=0,advantage=(0)/(0+eps)=0.0
        assert_eq!(rollouts[0].advantage, Some(0.0));
    }

    #[test]
    fn test_compute_advantage_all_same_reward() {
        let mut rollouts: Vec<GrpoRollout> = (0..5)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], 0.3))
            .collect();
        compute_advantage(&mut rollouts);
        // 全相同 reward:分子=0,advantage=0.0
        for rollout in &rollouts {
            assert_eq!(rollout.advantage, Some(0.0));
        }
    }

    #[test]
    fn test_compute_advantage_monotonicity() {
        // reward 单调递增 → advantage 应单调递增
        let mut rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], i as f32 * 0.1))
            .collect();
        compute_advantage(&mut rollouts);

        for i in 1..rollouts.len() {
            let prev = rollouts[i - 1].advantage.unwrap();
            let curr = rollouts[i].advantage.unwrap();
            assert!(
                curr >= prev,
                "advantage 应单调递增: idx {i} prev={prev} curr={curr}"
            );
        }
    }

    #[test]
    fn test_compute_advantage_mean_zero() {
        // 优势值之和应接近 0(因为 (r_i - mean) 之和 = 0)
        let mut rollouts: Vec<GrpoRollout> = (0..8)
            .map(|i| GrpoRollout::new(format!("t-{i}"), vec![1.0], (i as f32) * 0.5 - 1.75))
            .collect();
        compute_advantage(&mut rollouts);

        let sum: f32 = rollouts.iter().map(|r| r.advantage.unwrap()).sum();
        assert!(sum.abs() < 1e-5, "优势值之和应接近 0,实际 {sum}");
    }

    #[test]
    fn test_compute_advantage_extreme_values() {
        // 一个极端高 reward,其余相同 → 高 reward 的 advantage 应显著为正
        let mut rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| {
                let reward = if i == 0 { 10.0 } else { -1.0 };
                GrpoRollout::new(format!("t-{i}"), vec![1.0], reward)
            })
            .collect();
        compute_advantage(&mut rollouts);

        let top_advantage = rollouts[0].advantage.unwrap();
        assert!(
            top_advantage > 0.0,
            "最高 reward 的 advantage 应为正,实际 {top_advantage}"
        );
        // 其余 advantage 应为负
        for rollout in rollouts.iter().skip(1) {
            assert!(
                rollout.advantage.unwrap() < 0.0,
                "低 reward 的 advantage 应为负"
            );
        }
    }
}
