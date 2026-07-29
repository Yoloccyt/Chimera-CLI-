//! AEGIS Critic 单调性形式化验证
//!
//! 对应架构层: L4 FormalVerifier
//! 对应 ADR: ADR-050 决策 4(奖励欺骗启发式)
//!
//! # 核心保证
//!
//! 本模块提供三个纯函数验证器，确保 AEGIS Critic 评分满足：
//!
//! 1. **单调性**: 适应度提升 → Critic 评分单调不减
//! 2. **反奖励黑客**: Critic 评分提升不超过适应度提升的 tolerance 倍
//! 3. **有界性**: 所有评分在合法区间 `[min_bound, max_bound]` 内
//!
//! # 设计决策(WHY)
//!
//! - 纯函数: 无副作用，可安全在 FormalVerifier 管线中并发调用
//! - 返回 `VerificationResult`: 复用 L0 契约层类型，与 FormalVerifier 统一消费
//! - `f64` 序列: 适应度与 Critic 评分均为连续值，需浮点精度

use nexus_contracts::formal_props::VerificationResult;

/// AEGIS Critic 单调性验证器
///
/// 提供形式化验证方法，确保 Critic 评分满足单调性、反奖励黑客与有界性。
/// 所有方法为纯函数，不修改内部状态。
#[derive(Debug, Default, Clone, Copy)]
pub struct CriticMonotonicityChecker;

impl CriticMonotonicityChecker {
    /// 创建单调性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证适应度→评分的单调不减性质
    ///
    /// # 参数
    ///
    /// - `fitness_sequence`: 适应度序列（必须单调不减）
    /// - `score_sequence`: Critic 评分序列（与适应度序列等长）
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 适应度单调不减时，评分也单调不减
    /// - `Violated`: 存在相邻位置 i < i+1 使得 fitness[i] ≤ fitness[i+1]
    ///   但 score[i] > score[i+1]（适应度未降但评分降了）
    /// - `Skipped`: 序列长度不匹配或空序列
    ///
    /// # 语义
    ///
    /// 对于所有 i，若 fitness[i] ≤ fitness[i+1]，则 score[i] ≤ score[i+1]。
    /// 即：适应度不降时，Critic 评分也不降。
    #[must_use]
    pub fn verify_monotonicity(
        &self,
        fitness_sequence: &[f64],
        score_sequence: &[f64],
    ) -> VerificationResult {
        if fitness_sequence.len() != score_sequence.len() {
            return VerificationResult::Skipped {
                reason: format!(
                    "序列长度不匹配: fitness={} vs score={}",
                    fitness_sequence.len(),
                    score_sequence.len()
                ),
            };
        }

        if fitness_sequence.len() <= 1 {
            return VerificationResult::Satisfied {
                samples_tested: fitness_sequence.len() as u64,
            };
        }

        let mut violations: Vec<String> = Vec::new();
        let mut samples_tested: u64 = 0;

        for i in 0..fitness_sequence.len() - 1 {
            samples_tested += 1;
            let fitness_non_decreasing = fitness_sequence[i] <= fitness_sequence[i + 1];
            let score_decreasing = score_sequence[i] > score_sequence[i + 1];

            if fitness_non_decreasing && score_decreasing {
                violations.push(format!(
                    "位置 {i}: 适应度 {:.6} → {:.6} (不减), \
                     但评分 {:.6} → {:.6} (递减)",
                    fitness_sequence[i],
                    fitness_sequence[i + 1],
                    score_sequence[i],
                    score_sequence[i + 1],
                ));
            }
        }

        if violations.is_empty() {
            VerificationResult::Satisfied { samples_tested }
        } else {
            VerificationResult::Violated {
                counterexample: violations.join("; "),
                samples_tested,
            }
        }
    }

    /// 验证 Critic 评分不会被奖励黑客操纵
    ///
    /// # 参数
    ///
    /// - `fitness_scores`: 适应度序列
    /// - `critic_scores`: Critic 评分序列（与适应度序列等长）
    /// - `tolerance`: 允许的最大放大倍数（评分增量 / 适应度增量 ≤ tolerance）
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 所有相邻位置的评分提升不超过适应度提升的 tolerance 倍
    /// - `Violated`: 存在位置 i 使得 score 提升远超 fitness 提升
    /// - `Skipped`: 序列长度不匹配
    ///
    /// # 语义
    ///
    /// 对于所有相邻位置 i, i+1：
    /// 若 Δfitness = fitness[i+1] - fitness[i] > 0，
    /// 则 Δscore = score[i+1] - score[i] ≤ tolerance × Δfitness。
    /// 防止 Critic 评分被"游戏化"——微小的适应度提升却带来巨大的评分暴涨。
    #[must_use]
    pub fn verify_no_reward_hacking(
        &self,
        fitness_scores: &[f64],
        critic_scores: &[f64],
        tolerance: f64,
    ) -> VerificationResult {
        if fitness_scores.len() != critic_scores.len() {
            return VerificationResult::Skipped {
                reason: format!(
                    "序列长度不匹配: fitness={} vs critic={}",
                    fitness_scores.len(),
                    critic_scores.len()
                ),
            };
        }

        if fitness_scores.len() <= 1 {
            return VerificationResult::Satisfied {
                samples_tested: fitness_scores.len() as u64,
            };
        }

        let mut violations: Vec<String> = Vec::new();
        let mut samples_tested: u64 = 0;

        for i in 0..fitness_scores.len() - 1 {
            samples_tested += 1;
            let delta_fitness = fitness_scores[i + 1] - fitness_scores[i];
            let delta_score = critic_scores[i + 1] - critic_scores[i];

            // 仅当适应度有正向提升时才检查放大倍数
            if delta_fitness > f64::EPSILON {
                let max_allowed_score_increase = tolerance * delta_fitness;
                if delta_score > max_allowed_score_increase {
                    violations.push(format!(
                        "位置 {i}: Δfitness={delta_fitness:.6}, \
                         Δscore={delta_score:.6} > tolerance×Δfitness={max_allowed_score_increase:.6} \
                         (tolerance={tolerance})",
                    ));
                }
            }
        }

        if violations.is_empty() {
            VerificationResult::Satisfied { samples_tested }
        } else {
            VerificationResult::Violated {
                counterexample: violations.join("; "),
                samples_tested,
            }
        }
    }

    /// 验证所有评分在合法范围内
    ///
    /// # 参数
    ///
    /// - `scores`: 评分序列
    /// - `min_bound`: 下界（含）
    /// - `max_bound`: 上界（含）
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 所有评分 ∈ [min_bound, max_bound]
    /// - `Violated`: 存在越界评分
    ///
    /// # 语义
    ///
    /// 对于所有 i：min_bound ≤ scores[i] ≤ max_bound。
    /// 确保 Critic 评分不会漂移到无意义的区间。
    #[must_use]
    pub fn verify_score_bounded(
        &self,
        scores: &[f64],
        min_bound: f64,
        max_bound: f64,
    ) -> VerificationResult {
        if scores.is_empty() {
            return VerificationResult::Satisfied { samples_tested: 0 };
        }

        let mut violations: Vec<String> = Vec::new();
        let mut samples_tested: u64 = 0;

        for (i, &score) in scores.iter().enumerate() {
            samples_tested += 1;
            if score < min_bound || score > max_bound {
                violations.push(format!(
                    "位置 {i}: 评分 {score:.6} 越出合法区间 [{min_bound:.6}, {max_bound:.6}]",
                ));
            }
        }

        if violations.is_empty() {
            VerificationResult::Satisfied { samples_tested }
        } else {
            VerificationResult::Violated {
                counterexample: violations.join("; "),
                samples_tested,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> CriticMonotonicityChecker {
        CriticMonotonicityChecker::new()
    }

    // ── verify_monotonicity 测试 ──

    #[test]
    fn test_monotonicity_both_increasing() {
        let fitness = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let scores = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let result = checker().verify_monotonicity(&fitness, &scores);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_monotonicity_fitness_increasing_score_decreasing() {
        let fitness = vec![1.0, 2.0, 3.0, 4.0];
        let scores = vec![0.5, 0.4, 0.3, 0.2]; // 评分递减 → 违反
        let result = checker().verify_monotonicity(&fitness, &scores);
        assert!(result.is_violated());
        if let VerificationResult::Violated { counterexample, .. } = &result {
            assert!(counterexample.contains("位置 0"));
        }
    }

    #[test]
    fn test_monotonicity_empty_sequences() {
        let result = checker().verify_monotonicity(&[], &[]);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_monotonicity_single_element() {
        let result = checker().verify_monotonicity(&[1.0], &[0.5]);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_monotonicity_length_mismatch() {
        let result = checker().verify_monotonicity(&[1.0, 2.0], &[0.5]);
        assert!(result.is_skipped());
    }

    #[test]
    fn test_monotonicity_fitness_decreasing_score_any() {
        // 适应度递减时，评分任意变化都合法（单调性条件不触发）
        let fitness = vec![5.0, 3.0, 1.0];
        let scores = vec![0.1, 0.5, 0.9]; // 评分递增但适应度递减 → 不违反
        let result = checker().verify_monotonicity(&fitness, &scores);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_monotonicity_equal_fitness_any_score() {
        // 适应度相等（不减）时，评分也不能递减
        let fitness = vec![1.0, 1.0, 1.0];
        let scores = vec![0.3, 0.2, 0.1]; // 评分递减 → 违反
        let result = checker().verify_monotonicity(&fitness, &scores);
        assert!(result.is_violated());
    }

    // ── verify_no_reward_hacking 测试 ──

    #[test]
    fn test_reward_hacking_normal_range() {
        // 适应度提升 1.0，评分提升 0.5，tolerance=2.0 → 合法
        let fitness = vec![1.0, 2.0, 3.0];
        let scores = vec![0.1, 0.6, 1.1];
        let result = checker().verify_no_reward_hacking(&fitness, &scores, 2.0);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_reward_hacking_score_inflates_disproportionately() {
        // 适应度提升 0.1，评分提升 5.0，tolerance=2.0 → 违反
        let fitness = vec![1.0, 1.1, 1.2];
        let scores = vec![0.1, 5.1, 10.1];
        let result = checker().verify_no_reward_hacking(&fitness, &scores, 2.0);
        assert!(result.is_violated());
    }

    #[test]
    fn test_reward_hacking_fitness_decreasing_ignored() {
        // 适应度递减时不检查放大倍数
        let fitness = vec![3.0, 2.0, 1.0];
        let scores = vec![0.1, 100.0, 200.0]; // 评分暴涨但适应度递减 → 不违反
        let result = checker().verify_no_reward_hacking(&fitness, &scores, 2.0);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_reward_hacking_length_mismatch() {
        let result = checker().verify_no_reward_hacking(&[1.0, 2.0], &[0.5], 2.0);
        assert!(result.is_skipped());
    }

    // ── verify_score_bounded 测试 ──

    #[test]
    fn test_score_bounded_within_range() {
        let scores = vec![0.0, 0.25, 0.5, 0.75, 1.0];
        let result = checker().verify_score_bounded(&scores, 0.0, 1.0);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_score_bounded_below_min() {
        let scores = vec![0.0, -0.1, 0.5];
        let result = checker().verify_score_bounded(&scores, 0.0, 1.0);
        assert!(result.is_violated());
    }

    #[test]
    fn test_score_bounded_above_max() {
        let scores = vec![0.0, 0.5, 1.1];
        let result = checker().verify_score_bounded(&scores, 0.0, 1.0);
        assert!(result.is_violated());
    }

    #[test]
    fn test_score_bounded_empty_scores() {
        let result = checker().verify_score_bounded(&[], 0.0, 1.0);
        assert!(result.is_satisfied());
    }

    // ── 综合场景测试 ──

    #[test]
    fn test_all_checks_pass_on_healthy_data() {
        let c = checker();
        let fitness = vec![1.0, 2.0, 3.0, 4.0];
        let scores = vec![0.1, 0.2, 0.3, 0.4];

        assert!(c.verify_monotonicity(&fitness, &scores).is_satisfied());
        assert!(c
            .verify_no_reward_hacking(&fitness, &scores, 2.0)
            .is_satisfied());
        assert!(c.verify_score_bounded(&scores, 0.0, 1.0).is_satisfied());
    }

    #[test]
    fn test_default_trait_impl() {
        let c = CriticMonotonicityChecker::default();
        let result = c.verify_monotonicity(&[1.0], &[0.5]);
        assert!(result.is_satisfied());
    }
}
