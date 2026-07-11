//! GRPO 训练器 — 封装完整的 GRPO 策略梯度训练循环
//!
//! 对应架构层: L5 Knowledge
//!
//! 将 GRPO 的核心算法（采样、优势计算、梯度估计、参数更新）封装为一个
//! 可复用的 Trainer 结构体，便于引擎集成和独立测试。
//!
//! # 设计目标
//! - 封装训练状态（参考策略、历史指标）
//! - 提供单次/多次训练步骤
//! - 支持指标追踪与早期停止信号

use crate::policy::grpo::{
    compute_advantage, compute_grpo_objective, compute_policy_gradient_analytical, compute_ratio,
    update_policy_with_grpo,
};
use crate::types::{EvolutionPolicy, GrpoObjectiveResult, GrpoRollout};

/// GRPO 训练器 — 管理参考策略与训练状态
///
/// 封装 GRPO 训练循环，提供单次/多次训练步骤，
/// 并追踪训练历史（surrogate、KL、entropy）用于监控与 early stopping。
#[derive(Debug, Clone)]
pub struct GrpoTrainer {
    /// 参考策略 (用于 KL 散度约束)
    reference_policy: EvolutionPolicy,
    /// 训练历史指标
    history: Vec<GrpoObjectiveResult>,
    /// 累计训练步数
    total_steps: u64,
}

impl GrpoTrainer {
    /// 构造新的 GRPO 训练器
    ///
    /// # 参数
    /// - `reference_policy`: 参考策略，用于 KL 约束
    pub fn new(reference_policy: EvolutionPolicy) -> Self {
        Self {
            reference_policy,
            history: Vec::new(),
            total_steps: 0,
        }
    }

    /// 执行单次训练步骤
    ///
    /// 流程：
    /// 1. 计算优势（若未计算）
    /// 2. 计算概率比（当前策略 vs 旧策略）
    /// 3. 计算解析梯度
    /// 4. 梯度上升更新参数
    /// 5. 计算目标函数（监控）
    ///
    /// # 参数
    /// - `rollouts`: 采样轨迹（advantage 可为 None，内部会重新计算）
    /// - `policy`: 待更新的策略（原地修改）
    ///
    /// # 返回
    /// 本次训练的目标函数结果
    pub fn train_step(
        &mut self,
        rollouts: &mut [GrpoRollout],
        policy: &mut EvolutionPolicy,
    ) -> GrpoObjectiveResult {
        // 步骤 1: 确保优势已计算
        compute_advantage(rollouts);

        // 步骤 2: 计算概率比（当前策略 vs 旧策略）
        let old_policy = policy.clone();
        for rollout in rollouts.iter_mut() {
            compute_ratio(rollout, policy, &old_policy);
        }

        // 步骤 3: 计算解析梯度
        let gradient = compute_policy_gradient_analytical(rollouts, policy, &self.reference_policy);

        // 步骤 4: 梯度上升更新
        let lr = policy.grpo_hyperparams.learning_rate;
        update_policy_with_grpo(policy, &gradient, lr);

        // 步骤 5: 计算目标函数（监控）
        let result = compute_grpo_objective(rollouts, policy, &old_policy, &self.reference_policy);
        self.history.push(result.clone());
        self.total_steps += 1;

        result
    }

    /// 执行多次训练步骤
    ///
    /// 对应 GRPO 论文 Algorithm 1 的内层循环。
    /// 每次迭代固定 old_policy（标准 PPO 做法），即固定为训练开始时的策略。
    ///
    /// # 参数
    /// - `rollouts`: 采样轨迹
    /// - `policy`: 待更新的策略
    /// - `iterations`: 迭代次数
    ///
    /// # 返回
    /// 每次迭代的目标函数结果列表
    pub fn train(
        &mut self,
        rollouts: &mut [GrpoRollout],
        policy: &mut EvolutionPolicy,
        iterations: u32,
    ) -> Vec<GrpoObjectiveResult> {
        let mut results = Vec::with_capacity(iterations as usize);
        let old_policy = policy.clone(); // 整个迭代过程固定

        for _ in 0..iterations {
            // 重新计算概率比（当前策略 vs 原始策略）
            for rollout in rollouts.iter_mut() {
                compute_ratio(rollout, policy, &old_policy);
            }

            // 计算解析梯度
            let gradient =
                compute_policy_gradient_analytical(rollouts, policy, &self.reference_policy);

            // 更新参数
            let lr = policy.grpo_hyperparams.learning_rate;
            update_policy_with_grpo(policy, &gradient, lr);

            // 计算目标函数（监控）
            let result =
                compute_grpo_objective(rollouts, policy, &old_policy, &self.reference_policy);
            self.history.push(result.clone());
            self.total_steps += 1;
            results.push(result);
        }

        results
    }

    /// 更新参考策略
    ///
    /// 通常在 KL 散度过大或阶段性检查点时调用。
    pub fn update_reference(&mut self, policy: &EvolutionPolicy) {
        self.reference_policy = policy.clone();
        tracing::info!(total_steps = self.total_steps, "GRPO 参考策略已更新");
    }

    /// 获取训练历史
    pub fn history(&self) -> &[GrpoObjectiveResult] {
        &self.history
    }

    /// 获取最新 KL 散度
    pub fn last_kl_divergence(&self) -> Option<f32> {
        self.history.last().map(|r| r.kl_divergence)
    }

    /// 获取最新熵值
    pub fn last_entropy(&self) -> Option<f32> {
        self.history.last().map(|r| r.entropy)
    }

    /// 获取总训练步数
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// 检查最近 KL 是否超过阈值
    pub fn is_kl_exceeded(&self, threshold: f32) -> bool {
        match self.last_kl_divergence() {
            Some(kl) => kl.total_cmp(&threshold).is_gt(),
            None => false,
        }
    }

    /// 计算最近 N 步的平均 surrogate
    pub fn mean_surrogate_last_n(&self, n: usize) -> Option<f32> {
        let n = n.min(self.history.len());
        if n == 0 {
            return None;
        }
        let sum: f32 = self
            .history
            .iter()
            .rev()
            .take(n)
            .map(|r| r.mean_surrogate)
            .sum();
        Some(sum / n as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> EvolutionPolicy {
        match EvolutionPolicy::new(0.1, 1.5, 0.2, 8) {
            Ok(p) => p,
            Err(e) => panic!("构造策略失败: {e}"),
        }
    }

    fn make_rollouts(policy: &EvolutionPolicy, count: u32) -> Vec<GrpoRollout> {
        use crate::policy::grpo::sample_rollouts;
        sample_rollouts(policy, count)
    }

    #[test]
    fn test_trainer_new() {
        let policy = make_policy();
        let trainer = GrpoTrainer::new(policy.clone());
        assert!(trainer.history().is_empty());
        assert!(trainer.total_steps() == 0);
        assert!(trainer.last_kl_divergence().is_none());
    }

    #[test]
    fn test_train_step_changes_policy() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        let original_mr = new_policy.mutation_rate;
        trainer.train_step(&mut rollouts, &mut new_policy);

        // 策略应被更新（概率非零，且数值合法）
        assert!(new_policy.mutation_rate.is_finite());
        assert!(
            new_policy.mutation_rate.total_cmp(&original_mr).is_ne()
                || new_policy
                    .selection_pressure
                    .total_cmp(&policy.selection_pressure)
                    .is_ne()
        );
    }

    #[test]
    fn test_train_step_returns_finite_result() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        let result = trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(result.objective.is_finite());
        assert!(result.mean_surrogate.is_finite());
        assert!(result.kl_divergence.is_finite());
        assert!(result.entropy.is_finite());
    }

    #[test]
    fn test_train_multiple_iterations() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        let results = trainer.train(&mut rollouts, &mut new_policy, 3);
        assert!(results.len() == 3);
        for r in &results {
            assert!(r.objective.is_finite());
        }
    }

    #[test]
    fn test_train_history_count() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train(&mut rollouts, &mut new_policy, 3);
        assert!(trainer.history().len() == 3);
        assert!(trainer.total_steps() == 3);
    }

    #[test]
    fn test_update_reference() {
        let mut policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        policy.mutation_rate = 0.2;
        trainer.update_reference(&policy);

        // 更新后，用新的参考策略训练应产生不同的 KL
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();
        trainer.train_step(&mut rollouts, &mut new_policy);
        let kl = match trainer.last_kl_divergence() {
            Some(k) => k,
            None => panic!("训练后应存在 KL 散度"),
        };
        assert!(kl.is_finite());
        assert!(kl.total_cmp(&0.0).is_ge());
    }

    #[test]
    fn test_last_kl_divergence_after_train() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train_step(&mut rollouts, &mut new_policy);
        let kl = match trainer.last_kl_divergence() {
            Some(k) => k,
            None => panic!("训练后应存在 KL 散度"),
        };
        assert!(kl.is_finite());
    }

    #[test]
    fn test_last_entropy_after_train() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train_step(&mut rollouts, &mut new_policy);
        let entropy = match trainer.last_entropy() {
            Some(e) => e,
            None => panic!("训练后应存在熵值"),
        };
        // 高斯策略熵在 σ 较小时可为负值(公式 H = 0.5*ln(2πeσ²))。
        // 此处仅验证熵值已计算且为有限数,不强制符号。
        assert!(entropy.is_finite());
    }

    #[test]
    fn test_is_kl_exceeded_true() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();
        new_policy.mutation_rate = 0.3; // 远离参考策略

        trainer.train_step(&mut rollouts, &mut new_policy);
        // 默认 KL 阈值 0.01 通常会被超过
        assert!(trainer.is_kl_exceeded(0.01));
    }

    #[test]
    fn test_is_kl_exceeded_false() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train_step(&mut rollouts, &mut new_policy);
        // 极大阈值不应被超过
        assert!(!trainer.is_kl_exceeded(100.0));
    }

    #[test]
    fn test_is_kl_exceeded_none() {
        let policy = make_policy();
        let trainer = GrpoTrainer::new(policy.clone());
        assert!(!trainer.is_kl_exceeded(0.5));
    }

    #[test]
    fn test_mean_surrogate_last_n() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train(&mut rollouts, &mut new_policy, 3);
        let mean = match trainer.mean_surrogate_last_n(2) {
            Some(m) => m,
            None => panic!("应有 mean surrogate"),
        };
        assert!(mean.is_finite());
    }

    #[test]
    fn test_train_step_preserves_rollout_count() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(rollouts.len() == 8);
    }

    #[test]
    fn test_train_step_with_empty_rollouts() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts: Vec<GrpoRollout> = vec![];
        let mut new_policy = policy.clone();

        let result = trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(result.objective.is_finite());
        assert!(result.mean_surrogate == 0.0);
    }

    #[test]
    fn test_train_step_with_single_rollout() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 1);
        let mut new_policy = policy.clone();

        let result = trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(result.objective.is_finite());
    }

    #[test]
    fn test_total_steps_increments() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        assert!(trainer.total_steps() == 0);
        trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(trainer.total_steps() == 1);
        trainer.train_step(&mut rollouts, &mut new_policy);
        assert!(trainer.total_steps() == 2);
    }

    #[test]
    fn test_history_does_not_lose_entries() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        trainer.train(&mut rollouts, &mut new_policy, 5);
        assert!(trainer.history().len() == 5);
        for (i, entry) in trainer.history().iter().enumerate() {
            assert!(
                entry.objective.is_finite(),
                "历史条目 {i} 的 objective 无效"
            );
        }
    }

    #[test]
    fn test_mean_surrogate_last_n_zero() {
        let policy = make_policy();
        let trainer = GrpoTrainer::new(policy.clone());
        assert!(trainer.mean_surrogate_last_n(1).is_none());
    }

    #[test]
    fn test_train_with_large_iterations() {
        let policy = make_policy();
        let mut trainer = GrpoTrainer::new(policy.clone());
        let mut rollouts = make_rollouts(&policy, 8);
        let mut new_policy = policy.clone();

        let results = trainer.train(&mut rollouts, &mut new_policy, 10);
        assert!(results.len() == 10);
        assert!(trainer.total_steps() == 10);
    }
}
