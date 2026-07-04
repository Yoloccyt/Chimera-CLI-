//! 适应度评估 — 基于规则的轨迹适应度计算
//!
//! 对应架构层:L5 Knowledge
//!
//! # 评估规则(本周占位)
//! - `fitness_score = (reward + 1.0) / 2.0` ∈ [0.0, 1.0]
//! - `confidence = 1.0 / (1.0 + action_count as f32)`(动作越少越确信)
//! - `evidence` 记录评估依据,便于审计与调试
//!
//! // TODO(Week 7): 接入 MCP Mesh 真实模型,用验证集准确率替代规则评估

use crate::types::{FitnessReport, GrpoRollout};

/// 评估单次轨迹的适应度
///
/// 本周占位逻辑:
/// 1. fitness_score = (reward + 1.0) / 2.0,将 [-1.0, 1.0] 映射到 [0.0, 1.0]
/// 2. confidence = 1.0 / (1.0 + n),n 为动作数量(动作越多越不确信)
/// 3. evidence 记录 reward、action_count、advantage 等评估依据
pub fn evaluate_fitness(rollout: &GrpoRollout) -> FitnessReport {
    // fitness_score: 将 reward 从 [-1, 1] 映射到 [0, 1]
    let raw_fitness = (rollout.reward + 1.0) / 2.0;
    let fitness_score = raw_fitness.clamp(0.0, 1.0);

    // confidence: 动作越多越不确信(贝叶斯先验:简短决策更可靠)
    let action_count = rollout.actions.len() as f32;
    let confidence = (1.0 / (1.0 + action_count)).clamp(0.0, 1.0);

    // evidence: 记录评估依据
    let advantage_str = rollout
        .advantage
        .map(|a| format!("{a:.4}"))
        .unwrap_or_else(|| "未计算".into());
    let evidence = vec![
        format!("reward={:.4}", rollout.reward),
        format!("action_count={}", rollout.actions.len()),
        format!("advantage={advantage_str}"),
        format!("fitness_rule=(reward+1)/2"),
    ];

    FitnessReport {
        fitness_score,
        confidence,
        evidence,
    }
}

/// 批量评估一组轨迹的适应度
///
/// 对每个 rollout 调用 `evaluate_fitness`,返回对应的适应度报告列表。
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
    fn test_evaluate_fitness_score_in_range() {
        let rollout = make_rollout(0.5, 10);
        let report = evaluate_fitness(&rollout);
        assert!((0.0..=1.0).contains(&report.fitness_score));
    }

    #[test]
    fn test_evaluate_fitness_score_formula() {
        // reward=0.5 → fitness = (0.5+1)/2 = 0.75
        let rollout = make_rollout(0.5, 5);
        let report = evaluate_fitness(&rollout);
        assert!((report.fitness_score - 0.75).abs() < 1e-6);
    }

    #[test]
    fn test_evaluate_fitness_negative_reward() {
        // reward=-0.8 → fitness = (-0.8+1)/2 = 0.1
        let rollout = make_rollout(-0.8, 5);
        let report = evaluate_fitness(&rollout);
        assert!((report.fitness_score - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_evaluate_fitness_clamp_high_reward() {
        // reward=2.0 → raw=1.5 → clamp 到 1.0
        let rollout = make_rollout(2.0, 5);
        let report = evaluate_fitness(&rollout);
        assert_eq!(report.fitness_score, 1.0);
    }

    #[test]
    fn test_evaluate_fitness_clamp_low_reward() {
        // reward=-3.0 → raw=-1.0 → clamp 到 0.0
        let rollout = make_rollout(-3.0, 5);
        let report = evaluate_fitness(&rollout);
        assert_eq!(report.fitness_score, 0.0);
    }

    #[test]
    fn test_evaluate_fitness_confidence_in_range() {
        let rollout = make_rollout(0.5, 10);
        let report = evaluate_fitness(&rollout);
        assert!((0.0..=1.0).contains(&report.confidence));
    }

    #[test]
    fn test_evaluate_fitness_confidence_decreases_with_actions() {
        let rollout_few = make_rollout(0.5, 2);
        let rollout_many = make_rollout(0.5, 20);
        let report_few = evaluate_fitness(&rollout_few);
        let report_many = evaluate_fitness(&rollout_many);
        assert!(
            report_few.confidence > report_many.confidence,
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
        assert!((report.confidence - expected).abs() < 1e-6);
    }

    #[test]
    fn test_evaluate_fitness_evidence_non_empty() {
        let rollout = make_rollout(0.5, 5);
        let report = evaluate_fitness(&rollout);
        assert!(!report.evidence.is_empty());
        // evidence 应包含 reward 信息
        let has_reward = report.evidence.iter().any(|e| e.contains("reward"));
        assert!(has_reward, "evidence 应包含 reward 信息");
    }

    #[test]
    fn test_evaluate_fitness_empty_actions() {
        let rollout = make_rollout(0.5, 0);
        let report = evaluate_fitness(&rollout);
        // 0 个动作 → confidence = 1/(1+0) = 1.0
        assert!((report.confidence - 1.0).abs() < 1e-6);
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
        assert_eq!(reports.len(), 5);
    }

    #[test]
    fn test_evaluate_population_all_in_range() {
        let rollouts: Vec<GrpoRollout> = (0..10)
            .map(|i| make_rollout(i as f32 * 0.2 - 1.0, 8))
            .collect();
        let reports = evaluate_population(&rollouts);
        for report in &reports {
            assert!((0.0..=1.0).contains(&report.fitness_score));
            assert!((0.0..=1.0).contains(&report.confidence));
        }
    }
}
