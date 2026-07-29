//! AEGIS Stage 2: AdaptationPlanner — 规则表驱动的适应方向规划
//!
//! 对应 ADR:ADR-050 决策 2(Planner 降级为静态规则表)
//!
//! # R2 冻结声明(ADR-042)
//! 本阶段为静态规则表实现:失败模式 → 适应方向的确定性映射,
//! 无 MetaModel 根因分析、无学习参数(FormalVerifier 落地前无条件冻结)。
//! R2 解冻后升级为学习驱动须新 ADR 评审(ADR-050 §3)。

use serde::{Deserialize, Serialize};

use super::digester::DigestedTrajectories;

/// 触发进化的失败率阈值 — 成功率低于 (1 - 阈值) 才规划适应
///
/// WHY 0.3:偶发失败(<30%)由既有重试机制吸收,不值得变体开销;
/// 持续性失败(≥30%)才是 Harness 参数失配的信号。
const FAILURE_RATE_THRESHOLD: f32 = 0.3;

/// 适应方向 — 规则表的输出动作空间
///
/// WHY 封闭枚举:动作空间受限是 AEGIS-lite 的安全边界——Evolver 只能
/// 在这些预定义方向上变异 HarnessSpec 参数,不存在开放式代码生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdaptationDirection {
    /// 放宽重试:提高 max_attempts + 延长 backoff(应对 timeout 类失败)
    RelaxRetries,
    /// 收紧重试:降低 max_attempts(应对重试无效的确定性失败,快速失败止损)
    TightenRetries,
    /// 无需变更:失败率在阈值内或无可行动作
    NoChange,
}

/// 适应计划 — Stage 3 Evolver 的输入
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdaptationPlan {
    /// 规划的适应方向(去重后)
    pub directions: Vec<AdaptationDirection>,
    /// 规划依据(人类可读,供审计与 ADR 追溯)
    pub rationale: String,
}

impl AdaptationPlan {
    /// 是否为空计划(仅 NoChange 或无方向)
    pub fn is_noop(&self) -> bool {
        self.directions.is_empty()
            || self
                .directions
                .iter()
                .all(|d| *d == AdaptationDirection::NoChange)
    }
}

/// 适应规划器 — Stage 2
#[derive(Debug, Default, Clone, Copy)]
pub struct AdaptationPlanner;

impl AdaptationPlanner {
    /// 创建规划器
    pub fn new() -> Self {
        Self
    }

    /// 依据消化摘要规划适应方向(静态规则表)
    ///
    /// # 规则表(ADR-050 决策 2)
    ///
    /// | 条件 | 方向 | 依据 |
    /// |---|---|---|
    /// | 失败率 < 30% | NoChange | 偶发失败由既有重试吸收 |
    /// | 主导失败 = timeout / network 类 | RelaxRetries | 瞬态故障,重试有效 |
    /// | 主导失败 = verification / assertion 类 | TightenRetries | 确定性失败,重试无效应快速止损 |
    /// | 其他失败类别 | NoChange | 未知模式不盲动(保守策略) |
    pub fn plan(&self, digested: &DigestedTrajectories) -> AdaptationPlan {
        let failure_rate = 1.0 - digested.success_rate;

        // 规则 1:失败率低于阈值 → 不进化
        if digested.total_count == 0 || failure_rate < FAILURE_RATE_THRESHOLD {
            return AdaptationPlan {
                directions: vec![AdaptationDirection::NoChange],
                rationale: format!(
                    "失败率 {:.0}% 低于阈值 {:.0}%,维持现状",
                    failure_rate * 100.0,
                    FAILURE_RATE_THRESHOLD * 100.0
                ),
            };
        }

        // 规则 2/3:按主导失败模式的错误类别映射方向
        match digested.dominant_failure() {
            Some(pattern) => {
                let kind = pattern.error_kind.to_lowercase();
                if kind.contains("timeout") || kind.contains("network") {
                    AdaptationPlan {
                        directions: vec![AdaptationDirection::RelaxRetries],
                        rationale: format!(
                            "主导失败 '{}'@'{}' 频次 {} 属瞬态类,放宽重试",
                            pattern.error_kind, pattern.error_location, pattern.frequency
                        ),
                    }
                } else if kind.contains("verification") || kind.contains("assertion") {
                    AdaptationPlan {
                        directions: vec![AdaptationDirection::TightenRetries],
                        rationale: format!(
                            "主导失败 '{}'@'{}' 频次 {} 属确定性类,收紧重试快速止损",
                            pattern.error_kind, pattern.error_location, pattern.frequency
                        ),
                    }
                } else {
                    // 规则 4:未知失败类别不盲动
                    AdaptationPlan {
                        directions: vec![AdaptationDirection::NoChange],
                        rationale: format!("主导失败 '{}' 类别未知,保守不变异", pattern.error_kind),
                    }
                }
            }
            // 失败率高但无失败模式(理论矛盾,防御处理)
            None => AdaptationPlan {
                directions: vec![AdaptationDirection::NoChange],
                rationale: "无失败模式可依据".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aegis::digester::TrajectoryDigester;
    use crate::aegis::TrajectoryOutcome;

    fn digest(trajectories: &[TrajectoryOutcome]) -> DigestedTrajectories {
        TrajectoryDigester::new().digest(trajectories)
    }

    #[test]
    fn test_plan_noop_when_failure_rate_below_threshold() {
        // 10% 失败率 < 30% 阈值 → NoChange
        let mut trajectories: Vec<TrajectoryOutcome> = (0..9)
            .map(|i| TrajectoryOutcome::succeeded(format!("t{i}"), 100))
            .collect();
        trajectories.push(TrajectoryOutcome::failed("t9", "timeout", "loc", 100));

        let plan = AdaptationPlanner::new().plan(&digest(&trajectories));
        assert!(plan.is_noop());
    }

    #[test]
    fn test_plan_relax_retries_on_timeout_dominance() {
        let trajectories = vec![
            TrajectoryOutcome::failed("t1", "timeout", "loc", 100),
            TrajectoryOutcome::failed("t2", "timeout", "loc", 100),
            TrajectoryOutcome::succeeded("t3", 100),
        ];
        let plan = AdaptationPlanner::new().plan(&digest(&trajectories));
        assert_eq!(plan.directions, vec![AdaptationDirection::RelaxRetries]);
        assert!(plan.rationale.contains("timeout"));
    }

    #[test]
    fn test_plan_tighten_retries_on_verification_dominance() {
        let trajectories = vec![
            TrajectoryOutcome::failed("t1", "verification_failed", "pvl", 100),
            TrajectoryOutcome::failed("t2", "verification_failed", "pvl", 100),
            TrajectoryOutcome::succeeded("t3", 100),
        ];
        let plan = AdaptationPlanner::new().plan(&digest(&trajectories));
        assert_eq!(plan.directions, vec![AdaptationDirection::TightenRetries]);
    }

    #[test]
    fn test_plan_conservative_on_unknown_failure_kind() {
        let trajectories = vec![
            TrajectoryOutcome::failed("t1", "cosmic_ray", "loc", 100),
            TrajectoryOutcome::failed("t2", "cosmic_ray", "loc", 100),
        ];
        let plan = AdaptationPlanner::new().plan(&digest(&trajectories));
        // 未知类别保守不变异
        assert!(plan.is_noop());
        assert!(plan.rationale.contains("未知"));
    }

    #[test]
    fn test_plan_empty_batch_is_noop() {
        let plan = AdaptationPlanner::new().plan(&digest(&[]));
        assert!(plan.is_noop());
    }
}
