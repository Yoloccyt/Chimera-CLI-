//! 核心领域类型 — GSOE 在线进化的数据模型
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:GSOE(Guided Self-Organizing Evolution)
//!
//! # 类型职责
//! - `EvolutionPolicy`: 进化策略参数 (变异率/选择压力/精英比例/采样数)
//! - `GrpoRollout`: GRPO 单次采样轨迹 (动作序列 + 奖励 + 优势)
//! - `MutationCandidate`: 变异候选 (类型 + 幅度)
//! - `FitnessReport`: 适应度评估报告 (分数 + 置信度 + 证据)
//! - `EvolutionResult`: 单轮进化结果 (新策略 + 改进幅度 + 世代数)
//! - `GrpoHyperparams`: GRPO 训练超参数
//! - `GrpoObjectiveResult`: GRPO 目标函数计算结果
//! - `PolicyGradient`: 策略参数梯度

use serde::{Deserialize, Serialize};

/// GRPO 训练超参数 — 控制策略梯度更新的行为
///
/// 各超参数参考 DeepSeek-Math (Shao et al., 2024) 和 PPO (Schulman et al., 2017)
/// 的实验值, 可根据任务特性调整。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrpoHyperparams {
    /// PPO clip 范围 (ε), 典型值 0.1-0.2
    /// 限制概率比 ρ 在 [1-ε, 1+ε] 范围内
    pub epsilon: f32,
    /// KL 散度惩罚系数 (β), 典型值 0.01-0.1
    /// 防止策略偏移过大
    pub kl_beta: f32,
    /// 熵奖励系数 (η), 典型值 0.01-0.05
    /// 鼓励探索
    pub entropy_coef: f32,
    /// 学习率 (α), 典型值 0.01-0.1
    /// 控制参数更新步长
    pub learning_rate: f32,
    /// 策略更新迭代次数 (μ), 典型值 1-3
    /// 每轮采样后更新策略的次数
    pub update_iterations: u32,
    /// 最小标准差 (防止数值不稳定)
    pub min_std: f32,
    /// 最大标准差 (防止过度探索)
    pub max_std: f32,
    /// 优势裁剪阈值 (防止极端优势值)
    pub advantage_clip: f32,
}

impl Default for GrpoHyperparams {
    /// 默认超参数: 参考 DeepSeek-Math 和 DAPO 论文经验值
    ///
    /// ε=0.2: 标准 PPO clip 范围
    /// β=0.04: 中等 KL 约束 (参考 Target Policy Optimization 论文)
    /// η=0.01: 轻微熵奖励
    /// α=0.05: 保守学习率
    fn default() -> Self {
        Self {
            epsilon: 0.2,
            kl_beta: 0.04,
            entropy_coef: 0.01,
            learning_rate: 0.05,
            update_iterations: 2,
            min_std: 1e-4,
            max_std: 0.5,
            advantage_clip: 10.0,
        }
    }
}

/// 进化策略 — 控制 GRPO 在线进化的超参数集合
///
/// 各字段语义:
/// - `mutation_rate`: 变异幅度系数, 越大探索越强 (典型 0.05-0.3)
/// - `selection_pressure`: 选择压力, 放大优势差异 (典型 1.0-2.0)
/// - `elite_ratio`: 精英保留比例 [0.0, 1.0], 直接传承到下一代
/// - `rollout_count`: 每轮 GRPO 组采样数 (GRP0 要求 ≥ 2 才能计算组内优势)
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
    /// GRPO 训练超参数
    #[serde(default)]
    pub grpo_hyperparams: GrpoHyperparams,
}

impl EvolutionPolicy {
    /// 构造进化策略, 校验各字段合法范围
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
                reason: format!("rollout_count={rollout_count} 至少为 2 (GRPO 组内优势需要)"),
            });
        }
        Ok(Self {
            mutation_rate,
            selection_pressure,
            elite_ratio,
            rollout_count,
            grpo_hyperparams: GrpoHyperparams::default(),
        })
    }
}

impl Default for EvolutionPolicy {
    /// 默认进化策略: 保守变异 + 适度选择压力 + 20% 精英 + 8 轮采样
    ///
    /// WHY 这些参数: 0.1 变异率避免过度震荡, 1.5 选择压力放大优势差异,
    /// 0.2 精英比例保留最优个体, 8 轮采样满足 GRPO 组内优势统计需求。
    fn default() -> Self {
        Self {
            mutation_rate: 0.1,
            selection_pressure: 1.5,
            elite_ratio: 0.2,
            rollout_count: 8,
            grpo_hyperparams: GrpoHyperparams::default(),
        }
    }
}

/// GRPO 采样轨迹 — 单次 rollout 的动作序列与奖励
///
/// `advantage` 在采样时为 None, 经 `compute_advantage` 后填充为组内相对优势值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrpoRollout {
    /// 轨迹唯一标识
    pub trajectory_id: String,
    /// 动作序列 (本周占位为 f32 向量, Week 7 接入真实模型后为 token/logits)
    pub actions: Vec<f32>,
    /// 轨迹奖励 (环境反馈)
    pub reward: f32,
    /// 组内相对优势 (GRPO 核心量), 初始为 None
    pub advantage: Option<f32>,
    /// 旧策略下动作序列的对数概率
    #[serde(default)]
    pub old_logprob: Option<f32>,
    /// 当前策略下动作序列的对数概率
    #[serde(default)]
    pub current_logprob: Option<f32>,
    /// 概率比 π_θ / π_θ_old
    #[serde(default)]
    pub ratio: Option<f32>,
}

impl GrpoRollout {
    /// 构造采样轨迹, advantage 初始为 None
    pub fn new(trajectory_id: String, actions: Vec<f32>, reward: f32) -> Self {
        Self {
            trajectory_id,
            actions,
            reward,
            advantage: None,
            old_logprob: None,
            current_logprob: None,
            ratio: None,
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
    /// 高斯变异: 幅度 = rate * N(0,1) 近似, 适合精细调优
    Gaussian,
    /// 均匀变异: 幅度 = rate * U(-1,1), 适合大范围探索
    Uniform,
    /// 精英不变异: magnitude = 0, 直接传承
    Elite,
}

/// 变异候选 — 描述一次待应用的策略扰动
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MutationCandidate {
    /// 策略 ID (本周占位, Week 7 接入模型版本管理)
    pub policy_id: String,
    /// 变异类型
    pub mutation_type: MutationType,
    /// 变异幅度
    pub magnitude: f32,
}

/// 适应度报告 — 单次轨迹的评估结果
///
/// `fitness_score` ∈ [0.0, 1.0], `confidence` ∈ [0.0, 1.0]。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FitnessReport {
    /// 适应度分数 [0.0, 1.0]
    pub fitness_score: f32,
    /// 评估置信度 `[0.0, 1.0]` (动作越少越确信)
    pub confidence: f32,
    /// 评估证据 (人类可读的依据列表)
    pub evidence: Vec<String>,
}

/// 进化结果 — 单轮 `evolve_once` 的产出
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionResult {
    /// 进化后的新策略
    pub new_policy: EvolutionPolicy,
    /// 相对上一代的改进幅度 (新平均适应度 - 旧平均适应度)
    pub improvement: f32,
    /// 进化世代数 (从 1 开始)
    pub generation: u64,
}

/// GRPO 目标函数计算结果
///
/// 包含策略优化过程中各子目标的数值, 用于监控和调试。
#[derive(Debug, Clone, PartialEq)]
pub struct GrpoObjectiveResult {
    /// 总目标函数值 (策略梯度 ascent 时最大化)
    pub objective: f32,
    /// 平均 surrogate objective
    pub mean_surrogate: f32,
    /// KL 散度 (新策略 vs 参考策略)
    pub kl_divergence: f32,
    /// 策略熵
    pub entropy: f32,
}

/// 策略参数梯度
///
/// 由 GRPO 目标函数的梯度计算得到, 用于参数更新。
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyGradient {
    /// mutation_rate 的梯度
    pub grad_mutation_rate: f32,
    /// selection_pressure 的梯度
    pub grad_selection_pressure: f32,
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

    #[test]
    fn test_grpo_hyperparams_default() {
        let hp = GrpoHyperparams::default();
        assert_eq!(hp.epsilon, 0.2);
        assert_eq!(hp.kl_beta, 0.04);
        assert_eq!(hp.entropy_coef, 0.01);
        assert_eq!(hp.learning_rate, 0.05);
        assert_eq!(hp.update_iterations, 2);
        assert_eq!(hp.min_std, 1e-4);
        assert_eq!(hp.max_std, 0.5);
        assert_eq!(hp.advantage_clip, 10.0);
    }

    #[test]
    fn test_grpo_objective_result_fields() {
        let result = GrpoObjectiveResult {
            objective: 1.0,
            mean_surrogate: 0.8,
            kl_divergence: 0.1,
            entropy: 2.0,
        };
        assert_eq!(result.objective, 1.0);
        assert_eq!(result.mean_surrogate, 0.8);
    }

    #[test]
    fn test_policy_gradient_fields() {
        let grad = PolicyGradient {
            grad_mutation_rate: 0.1,
            grad_selection_pressure: 0.0,
        };
        assert_eq!(grad.grad_mutation_rate, 0.1);
    }
}
