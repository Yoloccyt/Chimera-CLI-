//! FormalVerifier M1 — omega-learner 学习单调性形式化验证(P7-T5)
//!
//! 对应架构层: L4 FormalVerifier(omega-learner 内验证器实现)
//! 对应 ADR: ADR-047(M1 Property #5:学习单调性)+ ADR-031(学习边界)
//! 对应计划: `IMPLEMENTATION_PLAN_Harness_Engineering_V3.md` Phase 7 P7-T5
//!
//! # 核心保证(Property #5)
//!
//! 本模块提供三个纯函数验证器,确保学习层的观测轨迹满足:
//!
//! 1. **步数单调性**: 学习步数快照序列严格单调递增
//!    (违反 = 学习状态回退/快照乱序,持久化或恢复路径有缺陷)
//! 2. **奖励有界性**: 观测奖励全部落在声明区间
//!    (违反 = 奖励函数实现漂移,LinUCB regret 上界假设失效)
//! 3. **后悔率非增趋势**: 滑动窗口平均后悔率随时间非增(容差内)
//!    (违反 = 学习器未收敛甚至发散,应触发熔断回退 Static 策略)
//!
//! # 设计决策(WHY)
//!
//! - **验证观测轨迹而非内部状态**: LinUCB 的 A_a/b_a 矩阵是实现细节,
//!   形式化性质应表达在可观测行为上(与 gsoe/parliament 验证器同哲学);
//!   上层编排器采集 (step, reward, regret) 快照序列投喂本验证器
//! - **f64 序列**: 后悔率为累计统计量,与 CriticMonotonicityChecker 对齐用 f64
//! - **窗口趋势而非逐点单调**: bandit 的探索步天然引入奖励抖动,
//!   逐点单调必然误报;窗口均值趋势 + 容差是可满足且有判别力的形式化性质

use nexus_contracts::formal_props::VerificationResult;

/// 学习单调性验证器
///
/// 所有方法为纯函数,不修改内部状态,可在 FormalVerifier 管线中并发调用。
#[derive(Debug, Default, Clone, Copy)]
pub struct LearningMonotonicityChecker;

impl LearningMonotonicityChecker {
    /// 创建学习单调性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证学习步数快照严格单调递增
    ///
    /// # 返回
    /// - `Satisfied`: 相邻快照恒满足 steps[i] < steps[i+1]
    /// - `Violated`: 存在回退或重复(携带位置反例)
    /// - `Skipped`: 快照数 < 2(无相邻对可验证)
    #[must_use]
    pub fn verify_steps_monotonic(&self, step_snapshots: &[u64]) -> VerificationResult {
        if step_snapshots.len() < 2 {
            return VerificationResult::Skipped {
                reason: format!("快照数 {} < 2,无相邻对可验证", step_snapshots.len()),
            };
        }

        let mut violations: Vec<String> = Vec::new();
        for (i, window) in step_snapshots.windows(2).enumerate() {
            if window[0] >= window[1] {
                violations.push(format!(
                    "位置 {i}: steps {} → {} 非严格递增",
                    window[0], window[1]
                ));
            }
        }

        Self::to_result(violations, (step_snapshots.len() - 1) as u64)
    }

    /// 验证观测奖励全部落在声明区间 [min_reward, max_reward]
    ///
    /// # 返回
    /// - `Skipped`: 空序列或 min > max(参数自身非法)
    #[must_use]
    pub fn verify_reward_bounded(
        &self,
        rewards: &[f64],
        min_reward: f64,
        max_reward: f64,
    ) -> VerificationResult {
        if rewards.is_empty() {
            return VerificationResult::Skipped {
                reason: "奖励序列为空".to_string(),
            };
        }
        if min_reward > max_reward {
            return VerificationResult::Skipped {
                reason: format!("非法边界: min {min_reward} > max {max_reward}"),
            };
        }

        let violations: Vec<String> = rewards
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_finite() || **r < min_reward || **r > max_reward)
            .map(|(i, r)| format!("位置 {i}: 奖励 {r} 越界 [{min_reward}, {max_reward}]"))
            .collect();

        Self::to_result(violations, rewards.len() as u64)
    }

    /// 验证滑动窗口平均后悔率非增趋势(容差内)
    ///
    /// # 参数
    /// - `regrets`: 每步后悔率观测序列(regret ≥ 0)
    /// - `window`: 滑动窗口大小(必须 ≥ 1)
    /// - `tolerance`: 相邻窗口均值允许的最大上升幅度(吸收探索抖动)
    ///
    /// # 语义
    /// 将序列按 `window` 切为连续不重叠窗口,相邻窗口均值必须满足
    /// `mean[k+1] <= mean[k] + tolerance`。窗口数 < 2 时 Skipped。
    #[must_use]
    pub fn verify_regret_non_increasing(
        &self,
        regrets: &[f64],
        window: usize,
        tolerance: f64,
    ) -> VerificationResult {
        if window == 0 {
            return VerificationResult::Skipped {
                reason: "窗口大小必须 ≥ 1".to_string(),
            };
        }
        let window_means: Vec<f64> = regrets
            .chunks_exact(window)
            .map(|chunk| chunk.iter().sum::<f64>() / window as f64)
            .collect();
        if window_means.len() < 2 {
            return VerificationResult::Skipped {
                reason: format!(
                    "完整窗口数 {} < 2(序列长 {},窗口 {})",
                    window_means.len(),
                    regrets.len(),
                    window
                ),
            };
        }

        let mut violations: Vec<String> = Vec::new();
        for (k, pair) in window_means.windows(2).enumerate() {
            if pair[1] > pair[0] + tolerance {
                violations.push(format!(
                    "窗口 {k}→{}: 平均后悔率 {:.6} → {:.6} 上升超容差 {:.6}",
                    k + 1,
                    pair[0],
                    pair[1],
                    tolerance
                ));
            }
        }

        Self::to_result(violations, (window_means.len() - 1) as u64)
    }

    /// 违规列表 → VerificationResult(三验证器共享的收敛逻辑)
    fn to_result(violations: Vec<String>, samples_tested: u64) -> VerificationResult {
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
    use proptest::prelude::*;

    // ============================================================
    // 步数单调性
    // ============================================================

    #[test]
    fn test_steps_monotonic_satisfied() {
        let checker = LearningMonotonicityChecker::new();
        assert!(checker
            .verify_steps_monotonic(&[1, 5, 10, 100])
            .is_satisfied());
    }

    #[test]
    fn test_steps_regression_violated() {
        let checker = LearningMonotonicityChecker::new();
        let result = checker.verify_steps_monotonic(&[10, 5]);
        assert!(matches!(result, VerificationResult::Violated { .. }));
    }

    #[test]
    fn test_steps_duplicate_violated() {
        // 严格递增语义:重复快照也违反
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_steps_monotonic(&[5, 5]),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_steps_short_sequence_skipped() {
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_steps_monotonic(&[42]),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 奖励有界性
    // ============================================================

    #[test]
    fn test_reward_bounded_satisfied() {
        let checker = LearningMonotonicityChecker::new();
        assert!(checker
            .verify_reward_bounded(&[0.1, 0.9, 0.5], 0.0, 1.0)
            .is_satisfied());
    }

    #[test]
    fn test_reward_out_of_range_violated() {
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_reward_bounded(&[0.5, 1.5], 0.0, 1.0),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_reward_nan_violated() {
        // NaN 非有限值:必须判违反(数值稳定性防线)
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_reward_bounded(&[0.5, f64::NAN], 0.0, 1.0),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_reward_invalid_bounds_skipped() {
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_reward_bounded(&[0.5], 1.0, 0.0),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 后悔率非增趋势
    // ============================================================

    #[test]
    fn test_regret_decreasing_satisfied() {
        // 窗口均值 0.8 → 0.5 → 0.2:标准收敛曲线
        let checker = LearningMonotonicityChecker::new();
        let regrets = [0.9, 0.7, 0.6, 0.4, 0.3, 0.1];
        assert!(checker
            .verify_regret_non_increasing(&regrets, 2, 0.05)
            .is_satisfied());
    }

    #[test]
    fn test_regret_rise_within_tolerance_satisfied() {
        // 窗口均值 0.5 → 0.52:上升 0.02 在容差 0.05 内(探索抖动)
        let checker = LearningMonotonicityChecker::new();
        let regrets = [0.5, 0.5, 0.52, 0.52];
        assert!(checker
            .verify_regret_non_increasing(&regrets, 2, 0.05)
            .is_satisfied());
    }

    #[test]
    fn test_regret_divergence_violated() {
        // 窗口均值 0.2 → 0.8:发散,必须违反
        let checker = LearningMonotonicityChecker::new();
        let regrets = [0.2, 0.2, 0.8, 0.8];
        assert!(matches!(
            checker.verify_regret_non_increasing(&regrets, 2, 0.05),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_regret_zero_window_skipped() {
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_regret_non_increasing(&[0.5, 0.4], 0, 0.05),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_regret_insufficient_windows_skipped() {
        // 序列长 3 / 窗口 2 → 完整窗口仅 1 个
        let checker = LearningMonotonicityChecker::new();
        assert!(matches!(
            checker.verify_regret_non_increasing(&[0.5, 0.4, 0.3], 2, 0.05),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // proptest 属性(M1 覆盖强化)
    // ============================================================

    proptest! {
        /// 属性 1: 严格递增序列恒满足步数单调性
        #[test]
        fn prop_increasing_steps_satisfied(start in 0u64..1000, count in 2usize..50) {
            let checker = LearningMonotonicityChecker::new();
            let steps: Vec<u64> = (0..count as u64).map(|i| start + i + 1).collect();
            prop_assert!(checker.verify_steps_monotonic(&steps).is_satisfied());
        }

        /// 属性 2: [0,1] 内的奖励序列恒满足有界性
        #[test]
        fn prop_rewards_in_unit_interval_satisfied(
            rewards in proptest::collection::vec(0.0f64..=1.0, 1..50),
        ) {
            let checker = LearningMonotonicityChecker::new();
            prop_assert!(checker.verify_reward_bounded(&rewards, 0.0, 1.0).is_satisfied());
        }

        /// 属性 3: 单调非增的后悔率序列在零容差下恒满足趋势验证
        #[test]
        fn prop_non_increasing_regret_satisfied(
            start in 0.5f64..1.0,
            count in 2usize..10,
        ) {
            let checker = LearningMonotonicityChecker::new();
            // 构造严格递减的窗口序列(每窗口 2 个等值样本)
            let regrets: Vec<f64> = (0..count)
                .flat_map(|k| {
                    let v = start / (k + 1) as f64;
                    [v, v]
                })
                .collect();
            prop_assert!(checker
                .verify_regret_non_increasing(&regrets, 2, 0.0)
                .is_satisfied());
        }
    }
}
