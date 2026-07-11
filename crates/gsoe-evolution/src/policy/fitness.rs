//! 适应度评估 — 基于规则的轨迹适应度计算
//!
//! 对应架构层: L5 Knowledge
//!
//! # 评估规则
//! - 综合适应度 = 0.4 * base_fitness + 0.4 * correctness + 0.2 * process_reward
//! - `base_fitness = (reward + 1.0) / 2.0` ∈ [0.0, 1.0]
//! - `correctness` 基于动作与最优解的接近程度
//! - `process_reward` 基于动作简洁性和一致性
//! - `confidence = 1.0 / (1.0 + action_count as f32)` (动作越少越确信)
//! - `evidence` 记录评估依据, 便于审计与调试
//!
//! // TODO(Week 7): 接入 MCP Mesh 真实模型, 用验证集准确率替代规则评估

use crate::types::{FitnessReport, GrpoRollout};

/// 最优解动作值（占位）
const OPTIMAL_ACTION: f32 = 1.0;

/// 计算正确性奖励
///
/// 基于动作序列与最优解的接近程度:
/// correctness = 1.0 - mean(|a_i - optimal|)
///
/// 返回值 ∈ [0.0, 1.0], 1.0 表示完全正确。
pub fn compute_correctness_reward(actions: &[f32], optimal: f32) -> f32 {
    if actions.is_empty() {
        return 0.0;
    }
    let mean_distance =
        actions.iter().map(|a| (a - optimal).abs()).sum::<f32>() / actions.len() as f32;
    (1.0 - mean_distance).clamp(0.0, 1.0)
}

/// 计算过程奖励
///
/// 鼓励简短、一致的决策:
/// - 动作越少, 过程奖励越高
/// - 动作方差越小, 过程奖励越高
///
/// 返回值 ∈ [0.0, 1.0]。
pub fn compute_process_reward(actions: &[f32]) -> f32 {
    if actions.is_empty() {
        return 1.0;
    }
    // 简短奖励: 动作越少奖励越高
    let len_bonus = 1.0 / (1.0 + actions.len() as f32 * 0.1);

    // 一致性奖励: 方差越小奖励越高
    let mean = actions.iter().sum::<f32>() / actions.len() as f32;
    let variance = actions
        .iter()
        .map(|a| {
            let diff = a - mean;
            diff * diff
        })
        .sum::<f32>()
        / actions.len() as f32;
    let consistency_bonus = (-variance).exp();

    (len_bonus * 0.5 + consistency_bonus * 0.5).clamp(0.0, 1.0)
}

/// 评估单次轨迹的适应度（增强版）
///
/// 综合 reward、正确性奖励、过程奖励和优势值:
/// 1. base_fitness = (reward + 1.0) / 2.0, 将 [-1.0, 1.0] 映射到 [0.0, 1.0]
/// 2. correctness = compute_correctness_reward(actions, optimal)
/// 3. process = compute_process_reward(actions)
/// 4. fitness_score = 0.4 * base + 0.4 * correctness + 0.2 * process
/// 5. confidence = 1.0 / (1.0 + n), n 为动作数量
/// 6. evidence 记录 reward、correctness、process_reward 等评估依据
pub fn evaluate_fitness(rollout: &GrpoRollout) -> FitnessReport {
    let correctness = compute_correctness_reward(&rollout.actions, OPTIMAL_ACTION);
    let process = compute_process_reward(&rollout.actions);

    // base_fitness: 将 reward 从 [-1, 1] 映射到 [0, 1]
    let base_fitness = (rollout.reward + 1.0) / 2.0;

    // 加权综合适应度
    let fitness_score = (base_fitness * 0.4 + correctness * 0.4 + process * 0.2).clamp(0.0, 1.0);

    // confidence: 动作越多越不确信 (贝叶斯先验: 简短决策更可靠)
    let action_count = rollout.actions.len() as f32;
    let confidence = (1.0 / (1.0 + action_count)).clamp(0.0, 1.0);

    // evidence: 记录评估依据
    let advantage_str = match rollout.advantage {
        Some(a) => format!("{a:.4}"),
        None => "未计算".into(),
    };
    let evidence = vec![
        format!("reward={:.4}", rollout.reward),
        format!("correctness={:.4}", correctness),
        format!("process_reward={:.4}", process),
        format!("action_count={}", rollout.actions.len()),
        format!("advantage={advantage_str}"),
        format!("fitness_rule=weighted_sum"),
    ];

    FitnessReport {
        fitness_score,
        confidence,
        evidence,
    }
}

/// 批量评估一组轨迹的适应度
///
/// 对每个 rollout 调用 `evaluate_fitness`, 返回对应的适应度报告列表。
/// 输入为空时返回空 Vec。
pub fn evaluate_population(rollouts: &[GrpoRollout]) -> Vec<FitnessReport> {
    rollouts.iter().map(evaluate_fitness).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rollout(reward: f32, action_count: usize) -> GrpoRollout {
        let actions = vec![0.5; action_count];
        GrpoRollout::new("t-1".into(), actions, reward)
    }

    #[test]
    fn test_compute_correctness_reward_empty() {
        let r = compute_correctness_reward(&[], 1.0);
        assert!(r.total_cmp(&0.0).is_eq());
    }

    #[test]
    fn test_compute_correctness_reward_perfect() {
        // 所有动作都等于最优解
        let r = compute_correctness_reward(&[1.0, 1.0, 1.0], 1.0);
        assert!(r.total_cmp(&1.0).is_eq());
    }

    #[test]
    fn test_compute_correctness_reward_zero() {
        // 所有动作都远离最优解
        let r = compute_correctness_reward(&[0.0, 0.0, 0.0], 1.0);
        assert!(r.total_cmp(&0.0).is_eq());
    }

    #[test]
    fn test_compute_process_reward_empty() {
        let r = compute_process_reward(&[]);
        assert!(r.total_cmp(&1.0).is_eq());
    }

    #[test]
    fn test_compute_process_reward_single_action() {
        // 单个动作方差为 0, 一致性最高
        let r = compute_process_reward(&[0.5]);
        assert!(r.total_cmp(&0.0).is_gt());
    }

    #[test]
    fn test_evaluate_fitness_score_in_range() {
        let rollout = make_rollout(0.5, 10);
        let report = evaluate_fitness(&rollout);
        assert!(
            report.fitness_score.total_cmp(&0.0).is_ge()
                && report.fitness_score.total_cmp(&1.0).is_le()
        );
    }

    #[test]
    fn test_evaluate_fitness_contains_correctness() {
        let rollout = make_rollout(0.5, 5);
        let report = evaluate_fitness(&rollout);
        let has_correctness = report.evidence.iter().any(|e| e.contains("correctness"));
        assert!(has_correctness, "evidence 应包含 correctness 信息");
    }

    #[test]
    fn test_evaluate_fitness_contains_process() {
        let rollout = make_rollout(0.5, 5);
        let report = evaluate_fitness(&rollout);
        let has_process = report.evidence.iter().any(|e| e.contains("process_reward"));
        assert!(has_process, "evidence 应包含 process_reward 信息");
    }

    #[test]
    fn test_evaluate_fitness_clamp_high_reward() {
        // reward=2.0 超出 [-1,1] 范围,base_fitness 经 clamp 后不会使加权总分超过 1.0
        let rollout = make_rollout(2.0, 5);
        let report = evaluate_fitness(&rollout);
        assert!(
            report.fitness_score.total_cmp(&1.0).is_le(),
            "适应度评分应被 clamp 到 [0,1] 上限,实际 {}",
            report.fitness_score
        );
        assert!(report.fitness_score.total_cmp(&0.0).is_ge());
    }

    #[test]
    fn test_evaluate_fitness_clamp_low_reward() {
        // reward=-3.0 → base=-1.0 → clamp 到 0.0
        let rollout = make_rollout(-3.0, 5);
        let report = evaluate_fitness(&rollout);
        assert!(report.fitness_score.total_cmp(&0.0).is_eq());
    }

    #[test]
    fn test_evaluate_fitness_confidence_in_range() {
        let rollout = make_rollout(0.5, 10);
        let report = evaluate_fitness(&rollout);
        assert!(
            report.confidence.total_cmp(&0.0).is_ge() && report.confidence.total_cmp(&1.0).is_le()
        );
    }

    #[test]
    fn test_evaluate_fitness_confidence_decreases_with_actions() {
        let rollout_few = make_rollout(0.5, 2);
        let rollout_many = make_rollout(0.5, 20);
        let report_few = evaluate_fitness(&rollout_few);
        let report_many = evaluate_fitness(&rollout_many);
        assert!(
            report_few
                .confidence
                .total_cmp(&report_many.confidence)
                .is_gt(),
            "动作少的置信度应更高: {} vs {}",
            report_few.confidence,
            report_many.confidence
        );
    }

    #[test]
    fn test_evaluate_fitness_confidence_formula() {
        // 10 个动作 → confidence = 1/(1+10) ≈ 0.0909
        let rollout = make_rollout(0.5, 10);
        let report = evaluate_fitness(&rollout);
        let expected = 1.0 / 11.0;
        assert!((report.confidence - expected)
            .abs()
            .total_cmp(&1e-6)
            .is_lt());
    }

    #[test]
    fn test_evaluate_fitness_evidence_non_empty() {
        let rollout = make_rollout(0.5, 5);
        let report = evaluate_fitness(&rollout);
        assert!(!report.evidence.is_empty());
        let has_reward = report.evidence.iter().any(|e| e.contains("reward"));
        assert!(has_reward, "evidence 应包含 reward 信息");
    }

    #[test]
    fn test_evaluate_fitness_empty_actions() {
        let rollout = make_rollout(0.5, 0);
        let report = evaluate_fitness(&rollout);
        // 0 个动作 → confidence = 1/(1+0) = 1.0
        assert!((report.confidence - 1.0).abs().total_cmp(&1e-6).is_lt());
    }

    #[test]
    fn test_evaluate_population_empty() {
        let reports = evaluate_population(&[]);
        assert!(reports.is_empty());
    }

    #[test]
    fn test_evaluate_population_count_matches() {
        let rollouts: Vec<GrpoRollout> = (0..5).map(|i| make_rollout(i as f32 * 0.1, 10)).collect();
        let reports = evaluate_population(&rollouts);
        assert!(reports.len() == 5);
    }

    #[test]
    fn test_evaluate_population_all_in_range() {
        let rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| make_rollout(i as f32 * 0.2 - 1.0, 8))
            .collect();
        let reports = evaluate_population(&rollouts);
        for report in &reports {
            assert!(
                report.fitness_score.total_cmp(&0.0).is_ge()
                    && report.fitness_score.total_cmp(&1.0).is_le()
            );
            assert!(
                report.confidence.total_cmp(&0.0).is_ge()
                    && report.confidence.total_cmp(&1.0).is_le()
            );
        }
    }

    #[test]
    fn test_correctness_decreases_with_distance() {
        let near = compute_correctness_reward(&[0.9, 0.9, 0.9], 1.0);
        let far = compute_correctness_reward(&[0.1, 0.1, 0.1], 1.0);
        assert!(
            near.total_cmp(&far).is_gt(),
            "接近最优解的动作应有更高正确性奖励"
        );
    }

    #[test]
    fn test_process_reward_decreases_with_variance() {
        let consistent = compute_process_reward(&[0.5, 0.5, 0.5]);
        let varied = compute_process_reward(&[0.0, 0.5, 1.0]);
        assert!(
            consistent.total_cmp(&varied).is_ge(),
            "一致性高的动作应有更高过程奖励"
        );
    }
}
