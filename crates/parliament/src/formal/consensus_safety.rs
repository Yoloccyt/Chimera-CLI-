//! 共识安全性形式化验证 — Security 一票否决不可覆盖 + 2/3 多数阈值正确性
//!
//! 对应架构层: L8 Parliament (FormalVerifier L4 骨架消费层)
//! 对应安全属性:
//! 1. **Security 一票否决不可被多数覆盖**: 当 Security 角色投否决票时，
//!    无论其他角色如何投票，最终结果必须为否决
//! 2. **2/3 多数阈值正确性**: 当且仅当 yes_votes / total_votes ≥ 2/3 时通过
//! 3. **多数票不能覆盖 Security 否决**: 即使达到 2/3 多数，Security 否决仍有效
//!
//! # 设计决策(WHY)
//!
//! - 所有函数为纯函数（无状态、无副作用），便于形式化证明与属性测试
//! - 使用 `nexus_contracts::formal_props::VerificationResult` 作为验证结果类型，
//!   遵循 L0 契约层复用原则
//! - 输入类型为本模块定义的轻量枚举，与 Parliament 内部类型解耦

use nexus_contracts::formal_props::VerificationResult;

/// Security 角色投票 — 形式化验证用简化模型
///
/// WHY 独立枚举而非复用 `Opinion`: 形式化验证关注语义属性（否决/未否决），
/// 不关心立场浮点值或理由字符串，简化类型使验证逻辑更清晰
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityVote {
    /// Security 角色投否决票（一票否决）
    Veto,
    /// Security 角色未投否决票（赞成或弃权）
    Approve,
}

/// 共识结果 — 形式化验证用简化模型
///
/// WHY 独立枚举而非复用 `Consensus`: 形式化验证仅区分三种语义结果
/// （通过/否决/未通过），不关心决议哈希或冻结能力列表
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsensusOutcome {
    /// 提案通过（共识达成）
    Reached,
    /// 提案被否决（Security 否决触发）
    Vetoed,
    /// 提案未通过（赞成率不足，但非否决）
    Rejected,
}

/// 共识安全性验证器 — Parliament 共识安全属性的形式化验证
///
/// 所有方法为纯函数，无状态，线程安全。
/// 验证结果使用 `VerificationResult` 类型，与 FormalVerifier L4 框架对齐。
#[derive(Debug, Default, Clone, Copy)]
pub struct ConsensusSafetyChecker;

impl ConsensusSafetyChecker {
    /// 创建共识安全性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证 Security 一票否决不可被多数覆盖（核心安全属性）
    ///
    /// # 安全属性
    ///
    /// 当 `security_vote == Veto` 时，无论 `other_yes_votes` 为何值，
    /// `outcome` 必须不为 `Reached`。即 Security 否决不可被任何多数票覆盖。
    ///
    /// # 参数
    ///
    /// - `other_yes_votes`: 除 Security 外的其他赞成票数（此属性中不使用，
    ///   但保留以验证"无论其他票如何"的语义）
    /// - `security_vote`: Security 角色的投票
    /// - `outcome`: 实际共识结果
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 属性成立（Security 否决时结果不为 Reached）
    /// - `Violated`: 发现反例（Security 否决但结果仍为 Reached）
    /// - `Skipped`: Security 未投否决票，此属性不适用
    #[must_use]
    pub fn verify_security_veto_immutable(
        other_yes_votes: u64,
        security_vote: SecurityVote,
        outcome: ConsensusOutcome,
    ) -> VerificationResult {
        match security_vote {
            SecurityVote::Veto => {
                // 核心属性：Security 否决时，结果绝不能为 Reached
                if outcome == ConsensusOutcome::Reached {
                    VerificationResult::Violated {
                        counterexample: format!(
                            "Security 投否决票但结果仍为 Reached \
                             (other_yes_votes={other_yes_votes})"
                        ),
                        samples_tested: 1,
                    }
                } else {
                    VerificationResult::Satisfied { samples_tested: 1 }
                }
            }
            SecurityVote::Approve => {
                // Security 未否决时，此属性不适用（跳过）
                VerificationResult::Skipped {
                    reason: "Security 未投否决票，此属性不适用".into(),
                }
            }
        }
    }

    /// 验证 2/3 多数阈值的正确性
    ///
    /// # 安全属性
    ///
    /// 当且仅当 `yes_votes * 3 >= total_votes * 2`（即 yes_votes/total_votes ≥ 2/3）
    /// 时，`outcome` 应为 `Reached`；否则应为 `Rejected` 或 `Vetoed`。
    ///
    /// # 参数
    ///
    /// - `yes_votes`: 赞成票数
    /// - `total_votes`: 总投票数（必须 > 0）
    /// - `outcome`: 实际共识结果
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 阈值判定正确
    /// - `Violated`: 阈值判定错误（如恰好 2/3 但未通过，或不足 2/3 却通过）
    /// - `Skipped`: total_votes 为 0（无法计算阈值）
    #[must_use]
    pub fn verify_two_thirds_threshold(
        yes_votes: u64,
        total_votes: u64,
        outcome: ConsensusOutcome,
    ) -> VerificationResult {
        if total_votes == 0 {
            return VerificationResult::Skipped {
                reason: "total_votes 为 0，无法计算阈值".into(),
            };
        }

        // 使用整数运算避免浮点精度问题：yes_votes/total_votes >= 2/3
        // 等价于 yes_votes * 3 >= total_votes * 2
        let meets_threshold = yes_votes * 3 >= total_votes * 2;

        if meets_threshold {
            // 达到 2/3 阈值，结果应为 Reached
            if outcome == ConsensusOutcome::Reached {
                VerificationResult::Satisfied { samples_tested: 1 }
            } else {
                VerificationResult::Violated {
                    counterexample: format!(
                        "yes_votes={yes_votes}, total_votes={total_votes} \
                         达到 2/3 阈值但结果为 {outcome:?}"
                    ),
                    samples_tested: 1,
                }
            }
        } else {
            // 未达到 2/3 阈值，结果不应为 Reached
            if outcome == ConsensusOutcome::Reached {
                VerificationResult::Violated {
                    counterexample: format!(
                        "yes_votes={yes_votes}, total_votes={total_votes} \
                         未达到 2/3 阈值但结果为 Reached"
                    ),
                    samples_tested: 1,
                }
            } else {
                VerificationResult::Satisfied { samples_tested: 1 }
            }
        }
    }

    /// 验证多数票不能覆盖 Security 否决
    ///
    /// # 安全属性
    ///
    /// 当 `security_veto == true` 且多数票达到 2/3 阈值时（`majority_yes == true`），
    /// `outcome` 必须不为 `Reached`。即 Security 否决优先于多数票。
    ///
    /// # 参数
    ///
    /// - `security_veto`: Security 是否投否决票
    /// - `majority_yes`: 多数票是否达到 2/3 阈值
    /// - `outcome`: 实际共识结果
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 属性成立
    /// - `Violated`: Security 否决 + 多数票通过 → 结果仍为 Reached
    /// - `Skipped`: 前置条件不满足（如 Security 未否决或多数票未达标）
    #[must_use]
    pub fn verify_no_majority_override(
        security_veto: bool,
        majority_yes: bool,
        outcome: ConsensusOutcome,
    ) -> VerificationResult {
        if !security_veto {
            return VerificationResult::Skipped {
                reason: "Security 未投否决票，此属性不适用".into(),
            };
        }

        if !majority_yes {
            return VerificationResult::Skipped {
                reason: "多数票未达 2/3 阈值，此属性不适用".into(),
            };
        }

        // 核心属性：Security 否决 + 多数票达标 → 结果绝不能为 Reached
        if outcome == ConsensusOutcome::Reached {
            VerificationResult::Violated {
                counterexample: "Security 否决 + 2/3 多数票但结果仍为 Reached".into(),
                samples_tested: 1,
            }
        } else {
            VerificationResult::Satisfied { samples_tested: 1 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Security 否决不可覆盖 ──

    #[test]
    fn test_security_veto_with_all_yes_must_veto() {
        // Security=Veto + 全票通过 → 结果必须为否决
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            100,
            SecurityVote::Veto,
            ConsensusOutcome::Vetoed,
        );
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_security_veto_with_all_yes_reached_is_violation() {
        // Security=Veto + 全票通过 → 若结果为 Reached 则违规
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            100,
            SecurityVote::Veto,
            ConsensusOutcome::Reached,
        );
        assert!(result.is_violated());
    }

    #[test]
    fn test_security_veto_with_two_thirds_majority_still_veto() {
        // Security=Veto + 2/3 多数 → 仍为否决
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            67,
            SecurityVote::Veto,
            ConsensusOutcome::Vetoed,
        );
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_security_veto_with_zero_other_votes() {
        // Security=Veto + 0 其他票 → 否决
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            0,
            SecurityVote::Veto,
            ConsensusOutcome::Vetoed,
        );
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_security_approve_skips_veto_check() {
        // Security=Approve → 此属性不适用，跳过
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            50,
            SecurityVote::Approve,
            ConsensusOutcome::Reached,
        );
        assert!(result.is_skipped());
    }

    #[test]
    fn test_security_veto_rejected_is_satisfied() {
        // Security=Veto + 结果为 Rejected（非 Reached）→ 满足属性
        let result = ConsensusSafetyChecker::verify_security_veto_immutable(
            30,
            SecurityVote::Veto,
            ConsensusOutcome::Rejected,
        );
        assert!(result.is_satisfied());
    }

    // ── 2/3 阈值正确性 ──

    #[test]
    fn test_exactly_two_thirds_passes() {
        // 恰好 2/3 (4/6) → 通过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(4, 6, ConsensusOutcome::Reached);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_one_vote_below_two_thirds_fails() {
        // 差一票不足 2/3 (3/6 = 1/2) → 不通过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(3, 6, ConsensusOutcome::Rejected);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_below_threshold_but_reached_is_violation() {
        // 不足 2/3 但结果为 Reached → 违规
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(1, 3, ConsensusOutcome::Reached);
        assert!(result.is_violated());
    }

    #[test]
    fn test_above_threshold_but_rejected_is_violation() {
        // 超过 2/3 但结果为 Rejected → 违规
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(5, 6, ConsensusOutcome::Rejected);
        assert!(result.is_violated());
    }

    #[test]
    fn test_zero_votes_zero_total_skipped() {
        // 0 票 / 0 总 → 跳过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(0, 0, ConsensusOutcome::Rejected);
        assert!(result.is_skipped());
    }

    #[test]
    fn test_single_vote_yes_passes() {
        // 1 票 / 1 总 = 100% ≥ 2/3 → 通过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(1, 1, ConsensusOutcome::Reached);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_zero_votes_yes_total_rejected() {
        // 0 票 / 5 总 = 0% < 2/3 → 不通过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(0, 5, ConsensusOutcome::Rejected);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_all_votes_yes_passes() {
        // 全票通过 (5/5 = 100% ≥ 2/3) → 通过
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(5, 5, ConsensusOutcome::Reached);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_two_thirds_boundary_2_of_3() {
        // 边界：2/3 恰好达标
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(2, 3, ConsensusOutcome::Reached);
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_two_thirds_boundary_1_of_3_fails() {
        // 边界：1/3 不足 2/3
        let result =
            ConsensusSafetyChecker::verify_two_thirds_threshold(1, 3, ConsensusOutcome::Rejected);
        assert!(result.is_satisfied());
    }

    // ── 多数票不能覆盖 Security 否决 ──

    #[test]
    fn test_security_veto_plus_majority_must_not_reach() {
        // Security 否决 + 2/3 多数 → 不能通过
        let result = ConsensusSafetyChecker::verify_no_majority_override(
            true,
            true,
            ConsensusOutcome::Vetoed,
        );
        assert!(result.is_satisfied());
    }

    #[test]
    fn test_security_veto_plus_majority_reached_is_violation() {
        // Security 否决 + 2/3 多数 → 若结果为 Reached 则违规
        let result = ConsensusSafetyChecker::verify_no_majority_override(
            true,
            true,
            ConsensusOutcome::Reached,
        );
        assert!(result.is_violated());
    }

    #[test]
    fn test_no_security_veto_skips_check() {
        // Security 未否决 → 此属性不适用
        let result = ConsensusSafetyChecker::verify_no_majority_override(
            false,
            true,
            ConsensusOutcome::Reached,
        );
        assert!(result.is_skipped());
    }

    #[test]
    fn test_no_majority_skips_check() {
        // 多数票未达标 → 此属性不适用
        let result = ConsensusSafetyChecker::verify_no_majority_override(
            true,
            false,
            ConsensusOutcome::Vetoed,
        );
        assert!(result.is_skipped());
    }

    #[test]
    fn test_security_veto_plus_majority_rejected_is_satisfied() {
        // Security 否决 + 2/3 多数 → 结果为 Rejected 也满足属性
        let result = ConsensusSafetyChecker::verify_no_majority_override(
            true,
            true,
            ConsensusOutcome::Rejected,
        );
        assert!(result.is_satisfied());
    }
}
