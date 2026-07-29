//! AutoDPO 偏好对一致性形式化验证(FormalVerifier M1,P7-T3)
//!
//! 对应架构层: L4 FormalVerifier(auto-dpo 内验证器实现)
//! 对应 ADR: ADR-047(FormalVerifier L4 路线图 M1)+ ADR-042(R2 解冻前置)
//! 对应计划: `IMPLEMENTATION_PLAN_Harness_Engineering_V3.md` Phase 7 P7-T3
//!
//! # 核心保证(M1 Property #2:偏好对一致性 + 奖惩单调性)
//!
//! 本模块提供三个纯函数验证器,确保 AutoDPO 生成的偏好对满足:
//!
//! 1. **偏好非对称性**: 每对必须 chosen_score > rejected_score
//!    (违反 = 偏好语义倒置,训练信号污染)
//! 2. **反自偏好**: chosen 与 rejected 内容不得相同
//!    (自偏好对的 margin 恒为 0,是奖励黑客"刷对数"的典型手法)
//! 3. **margin 有界性**: 偏好差值 ∈ [min_margin, max_margin]
//!    (过小 = 噪声对无训练价值;过大 = 疑似评分被操纵)
//!
//! # 设计决策(WHY)
//!
//! - **纯函数 + `VerificationResult`**: 与 gsoe `CriticMonotonicityChecker` /
//!   parliament `ConsensusSafetyChecker` 同款模式,FormalVerifier 管线统一消费
//! - **f32 全程**: PreferencePair 评分为 f32,验证不做 f64 提升(§4.4 反模式 6)
//! - **R2 关联**: 偏好对是未来 R2(GSOE×AutoDPO 约束 RL)的训练输入,
//!   本验证器是 ADR-042 决策 3 阶段 1(FormalVerifier 落地)的组成部分——
//!   在 R2 解冻前先形式化保证其数据源质量

use crate::types::PreferencePair;
use nexus_contracts::formal_props::VerificationResult;

/// 偏好对一致性验证器
///
/// 所有方法为纯函数,不修改内部状态,可在 FormalVerifier 管线中并发调用。
#[derive(Debug, Default, Clone, Copy)]
pub struct PreferenceConsistencyChecker;

impl PreferenceConsistencyChecker {
    /// 创建一致性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证偏好非对称性:每对 chosen_score 必须严格大于 rejected_score
    ///
    /// # 返回
    /// - `Satisfied`: 全部偏好对满足 chosen > rejected
    /// - `Violated`: 存在评分倒置或持平的对(携带 pair_id 反例)
    /// - `Skipped`: 空输入(无对可验证)
    #[must_use]
    pub fn verify_preference_asymmetry(&self, pairs: &[PreferencePair]) -> VerificationResult {
        if pairs.is_empty() {
            return VerificationResult::Skipped {
                reason: "偏好对集合为空".to_string(),
            };
        }

        let violations: Vec<String> = pairs
            .iter()
            .filter(|p| p.chosen_score <= p.rejected_score)
            .map(|p| {
                format!(
                    "pair {}: chosen_score {:.4} <= rejected_score {:.4}",
                    p.pair_id, p.chosen_score, p.rejected_score
                )
            })
            .collect();

        Self::to_result(violations, pairs.len() as u64)
    }

    /// 验证反自偏好:chosen 与 rejected 内容不得相同
    ///
    /// WHY: 自偏好对(chosen == rejected)的训练 margin 恒为 0,
    /// 批量生成自偏好对是"刷对数"式奖励黑客的典型手法。
    #[must_use]
    pub fn verify_no_self_preference(&self, pairs: &[PreferencePair]) -> VerificationResult {
        if pairs.is_empty() {
            return VerificationResult::Skipped {
                reason: "偏好对集合为空".to_string(),
            };
        }

        let violations: Vec<String> = pairs
            .iter()
            .filter(|p| p.chosen == p.rejected)
            .map(|p| format!("pair {}: chosen 与 rejected 内容相同", p.pair_id))
            .collect();

        Self::to_result(violations, pairs.len() as u64)
    }

    /// 验证 margin 有界性:偏好差值必须落在 [min_margin, max_margin]
    ///
    /// # 参数
    /// - `min_margin`: 最小有效差值(过小 = 噪声对,无训练价值)
    /// - `max_margin`: 最大合法差值(过大 = 疑似评分操纵)
    ///
    /// # 返回
    /// - `Skipped`: 空输入或 min_margin > max_margin(参数自身非法)
    #[must_use]
    pub fn verify_margin_bounded(
        &self,
        pairs: &[PreferencePair],
        min_margin: f32,
        max_margin: f32,
    ) -> VerificationResult {
        if pairs.is_empty() {
            return VerificationResult::Skipped {
                reason: "偏好对集合为空".to_string(),
            };
        }
        if min_margin > max_margin {
            return VerificationResult::Skipped {
                reason: format!("非法边界: min_margin {min_margin} > max_margin {max_margin}"),
            };
        }

        let violations: Vec<String> = pairs
            .iter()
            .filter_map(|p| {
                let margin = p.chosen_score - p.rejected_score;
                if margin < min_margin || margin > max_margin {
                    Some(format!(
                        "pair {}: margin {:.4} 越界 [{:.4}, {:.4}]",
                        p.pair_id, margin, min_margin, max_margin
                    ))
                } else {
                    None
                }
            })
            .collect();

        Self::to_result(violations, pairs.len() as u64)
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
    use crate::types::SampleQuality;
    use proptest::prelude::*;

    /// 构造测试偏好对(评分显式指定,其余字段占位)
    fn pair(id: &str, chosen: &str, rejected: &str, cs: f32, rs: f32) -> PreferencePair {
        PreferencePair {
            pair_id: id.to_string(),
            chosen: chosen.to_string(),
            rejected: rejected.to_string(),
            chosen_score: cs,
            rejected_score: rs,
            quality: SampleQuality::High,
        }
    }

    // ============================================================
    // 偏好非对称性
    // ============================================================

    #[test]
    fn test_asymmetry_satisfied() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![
            pair("p1", "a", "b", 0.9, 0.3),
            pair("p2", "c", "d", 0.7, 0.6),
        ];
        assert!(checker.verify_preference_asymmetry(&pairs).is_satisfied());
    }

    #[test]
    fn test_asymmetry_violated_on_inversion() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 0.3, 0.9)];
        let result = checker.verify_preference_asymmetry(&pairs);
        assert!(matches!(result, VerificationResult::Violated { .. }));
    }

    #[test]
    fn test_asymmetry_violated_on_tie() {
        // 持平也违反(严格大于语义)
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 0.5, 0.5)];
        assert!(matches!(
            checker.verify_preference_asymmetry(&pairs),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_asymmetry_skipped_on_empty() {
        let checker = PreferenceConsistencyChecker::new();
        assert!(matches!(
            checker.verify_preference_asymmetry(&[]),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 反自偏好
    // ============================================================

    #[test]
    fn test_no_self_preference_satisfied() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "output-a", "output-b", 0.9, 0.3)];
        assert!(checker.verify_no_self_preference(&pairs).is_satisfied());
    }

    #[test]
    fn test_self_preference_violated() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "same", "same", 0.9, 0.3)];
        let result = checker.verify_no_self_preference(&pairs);
        match result {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("p1"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    // ============================================================
    // margin 有界性
    // ============================================================

    #[test]
    fn test_margin_bounded_satisfied() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 0.8, 0.5)]; // margin 0.3
        assert!(checker
            .verify_margin_bounded(&pairs, 0.1, 0.5)
            .is_satisfied());
    }

    #[test]
    fn test_margin_too_small_violated() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 0.51, 0.5)]; // margin 0.01 < 0.1
        assert!(matches!(
            checker.verify_margin_bounded(&pairs, 0.1, 0.5),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_margin_too_large_violated() {
        // margin 0.9 > 0.5:疑似评分操纵
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 1.0, 0.1)];
        assert!(matches!(
            checker.verify_margin_bounded(&pairs, 0.1, 0.5),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_margin_invalid_bounds_skipped() {
        let checker = PreferenceConsistencyChecker::new();
        let pairs = vec![pair("p1", "a", "b", 0.8, 0.5)];
        assert!(matches!(
            checker.verify_margin_bounded(&pairs, 0.5, 0.1),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // proptest 属性(M1 覆盖强化)
    // ============================================================

    proptest! {
        /// 属性 1: 任意 chosen > rejected 的对集恒满足非对称性
        #[test]
        fn prop_asymmetry_holds_for_valid_pairs(
            base in 0.0f32..0.5,
            margin in 0.001f32..0.5,
            count in 1usize..20,
        ) {
            let checker = PreferenceConsistencyChecker::new();
            let pairs: Vec<PreferencePair> = (0..count)
                .map(|i| pair(&format!("p{i}"), "a", "b", base + margin, base))
                .collect();
            prop_assert!(checker.verify_preference_asymmetry(&pairs).is_satisfied());
        }

        /// 属性 2: margin 验证的 Satisfied ⇔ 全部 margin ∈ [min, max]
        #[test]
        fn prop_margin_bounded_soundness(
            cs in 0.0f32..1.0,
            rs in 0.0f32..1.0,
        ) {
            let checker = PreferenceConsistencyChecker::new();
            let pairs = vec![pair("p1", "a", "b", cs, rs)];
            let margin = cs - rs;
            let in_bounds = (0.05..=0.8).contains(&margin);
            let satisfied = checker
                .verify_margin_bounded(&pairs, 0.05, 0.8)
                .is_satisfied();
            prop_assert_eq!(satisfied, in_bounds);
        }
    }
}
