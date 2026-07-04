//! 核心领域类型 — GSOE 在线进化的数据模型
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:GSOE(Guided Self-Organizing Evolution)
//!
//! # 类型职责
//! - `EvolutionPolicy`:进化策略参数(变异率/选择压力/精英比例/采样数)
//! - `GrpoRollout`:GRPO 单次采样轨迹(动作序列 + 奖励 + 优势)
//! - `MutationCandidate`:变异候选(类型 + 幅度)
//! - `FitnessReport`:适应度评估报告(分数 + 置信度 + 证据)
//! - `EvolutionResult`:单轮进化结果(新策略 + 改进幅度 + 世代数)

use serde::{Deserialize, Serialize};

/// 进化策略 — 控制 GRPO 在线进化的超参数集合
///
/// 各字段语义:
/// - `mutation_rate`:变异幅度系数,越大探索越强(典型 0.05-0.3)
/// - `selection_pressure`:选择压力,放大优势差异(典型 1.0-2.0)
/// - `elite_ratio`:精英保留比例 [0.0, 1.0],直接传承到下一代
/// - `rollout_count`:每轮 GRPO 组采样数(GRP0 要求 ≥ 2 才能计算组内优势)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionPolicy {
    /// 变异幅度系数
    pub mutation_rate: f32,
    /// 选择压力系数
    pub selection_pressure: f32,
    /// 精英保留比例 [0.0, 1.0]
    pub elite_ratio: f32,
    /// 每轮采样轨迹数
    pub rollout_count: u32,
}

impl EvolutionPolicy {
    /// 构造进化策略,校验各字段合法范围
    pub fn new(
        mutation_rate: f32,
        selection_pressure: f32,
        elite_ratio: f32,
        rollout_count: u32,
    ) -> Result<Self, crate::error::GsoeError> {
        if !(0.0..=1.0).contains(&mutation_rate) {
            return Err(crate::error::GsoeError::InvalidPolicy {
                reason: format!("mutation_rate={mutation_rate} 超出 [0.0, 1.0]"),
            });
        }
        if selection_pressure < 0.0 {
            return Err(crate::error::GsoeError::InvalidPolicy {
                reason: format!("selection_pressure={selection_pressure} 不能为负"),
            });
        }
        if !(0.0..=1.0).contains(&elite_ratio) {
            return Err(crate::error::GsoeError::InvalidPolicy {
                reason: format!("elite_ratio={elite_ratio} 超出 [0.0, 1.0]"),
            });
        }
        if rollout_count < 2 {
            return Err(crate::error::GsoeError::InvalidPolicy {
                reason: format!("rollout_count={rollout_count} 至少为 2(GRPO 组内优势需要)"),
            });
        }
        Ok(Self {
            mutation_rate,
            selection_pressure,
            elite_ratio,
            rollout_count,
        })
    }
}

impl Default for EvolutionPolicy {
    /// 默认进化策略:保守变异 + 适度选择压力 + 20% 精英 + 8 轮采样
    ///
    /// WHY 这些参数:0.1 变异率避免过度震荡,1.5 选择压力放大优势差异,
    /// 0.2 精英比例保留最优个体,8 轮采样满足 GRPO 组内优势统计需求。
    fn default() -> Self {
        Self {
            mutation_rate: 0.1,
            selection_pressure: 1.5,
            elite_ratio: 0.2,
            rollout_count: 8,
        }
    }
}

/// GRPO 采样轨迹 — 单次 rollout 的动作序列与奖励
///
/// `advantage` 在采样时为 None,经 `compute_advantage` 后填充为组内相对优势值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrpoRollout {
    /// 轨迹唯一标识
    pub trajectory_id: String,
    /// 动作序列(本周占位为 f32 向量,Week 7 接入真实模型后为 token/logits)
    pub actions: Vec<f32>,
    /// 轨迹奖励(环境反馈)
    pub reward: f32,
    /// 组内相对优势(GRPO 核心量),初始为 None
    pub advantage: Option<f32>,
}

impl GrpoRollout {
    /// 构造采样轨迹,advantage 初始为 None
    pub fn new(trajectory_id: String, actions: Vec<f32>, reward: f32) -> Self {
        Self {
            trajectory_id,
            actions,
            reward,
            advantage: None,
        }
    }

    /// 链式设置优势值
    pub fn with_advantage(mut self, advantage: f32) -> Self {
        self.advantage = Some(advantage);
        self
    }
}

/// 变异类型 — 控制策略参数扰动的分布形态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MutationType {
    /// 高斯变异:幅度 = rate * N(0,1) 近似,适合精细调优
    Gaussian,
    /// 均匀变异:幅度 = rate * U(-1,1),适合大范围探索
    Uniform,
    /// 精英不变异:magnitude = 0,直接传承
    Elite,
}

/// 变异候选 — 描述一次待应用的策略扰动
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MutationCandidate {
    /// 策略 ID(本周占位,Week 7 接入模型版本管理)
    pub policy_id: String,
    /// 变异类型
    pub mutation_type: MutationType,
    /// 变异幅度
    pub magnitude: f32,
}

/// 适应度报告 — 单次轨迹的评估结果
///
/// `fitness_score` ∈ [0.0, 1.0],`confidence` ∈ [0.0, 1.0]。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FitnessReport {
    /// 适应度分数 [0.0, 1.0]
    pub fitness_score: f32,
    /// 评估置信度 `[0.0, 1.0]`(动作越少越确信)
    pub confidence: f32,
    /// 评估证据(人类可读的依据列表)
    pub evidence: Vec<String>,
}

/// 进化结果 — 单轮 `evolve_once` 的产出
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionResult {
    /// 进化后的新策略
    pub new_policy: EvolutionPolicy,
    /// 相对上一代的改进幅度(新平均适应度 - 旧平均适应度)
    pub improvement: f32,
    /// 进化世代数(从 1 开始)
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evolution_policy_new_valid() {
        let policy = EvolutionPolicy::new(0.1, 1.5, 0.2, 8).unwrap();
        assert_eq!(policy.mutation_rate, 0.1);
        assert_eq!(policy.selection_pressure, 1.5);
        assert_eq!(policy.elite_ratio, 0.2);
        assert_eq!(policy.rollout_count, 8);
    }

    #[test]
    fn test_evolution_policy_invalid_mutation_rate() {
        let err = EvolutionPolicy::new(1.5, 1.5, 0.2, 8).unwrap_err();
        assert!(matches!(err, crate::error::GsoeError::InvalidPolicy { .. }));
    }

    #[test]
    fn test_evolution_policy_invalid_elite_ratio() {
        let err = EvolutionPolicy::new(0.1, 1.5, 1.5, 8).unwrap_err();
        assert!(matches!(err, crate::error::GsoeError::InvalidPolicy { .. }));
    }

    #[test]
    fn test_evolution_policy_rollout_count_too_small() {
        let err = EvolutionPolicy::new(0.1, 1.5, 0.2, 1).unwrap_err();
        assert!(matches!(err, crate::error::GsoeError::InvalidPolicy { .. }));
    }

    #[test]
    fn test_grpo_rollout_new_advantage_none() {
        let rollout = GrpoRollout::new("t-1".into(), vec![0.1, 0.2], 0.5);
        assert_eq!(rollout.trajectory_id, "t-1");
        assert_eq!(rollout.actions, vec![0.1, 0.2]);
        assert_eq!(rollout.reward, 0.5);
        assert!(rollout.advantage.is_none());
    }

    #[test]
    fn test_grpo_rollout_with_advantage() {
        let rollout = GrpoRollout::new("t-1".into(), vec![0.1], 0.5).with_advantage(0.3);
        assert_eq!(rollout.advantage, Some(0.3));
    }

    #[test]
    fn test_mutation_type_variants() {
        assert_ne!(MutationType::Gaussian, MutationType::Uniform);
        assert_ne!(MutationType::Uniform, MutationType::Elite);
    }
}
