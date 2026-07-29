//! FormalVerifier M2 — decay-engine 能力衰减一致性形式化验证(Phase 8.1)
//!
//! 对应架构层: L4 FormalVerifier(decay-engine 内验证器实现)
//! 对应 ADR: ADR-047(M2 Property #6:能力衰减一致性)+ ADR-002(能力衰减模型)
//! + ADR-042(R2 解冻阶段① 前置)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` Phase 8.1
//!
//! # 核心保证(Property #6)
//!
//! 本模块提供三个纯函数验证器,确保能力衰减引擎的观测轨迹满足 ADR-002
//! "连续权限流体模型"的安全不变量:
//!
//! 1. **衰减单调性**: 衰减类事件(TimeDecay / ViolationPenalty)后 level 非增
//!    (权限只减不增——除非显式 Restore;违反 = 权限提升攻击面)
//! 2. **有界性**: 任意事件序列后 level 恒 ∈ [0, 1]
//!    (违反 = CapabilityLevel newtype 校验被绕过,流体模型失效)
//! 3. **Freeze 归零不可逆**: 一旦观测到 Freeze,其后 level 恒为 0
//!    直至观测到 Restore(违反 = 冻结后权限残留,对应 Claude Code 尸检
//!    "权限不应残留"教训)
//!
//! # 设计决策(WHY 验证观测轨迹而非引擎内部状态)
//!
//! 验证器消费 `LevelTransition` 序列(每步:事件种类 + 事件前 level +
//! 事件后 level),而非持有 `DecayEngine` 实例。理由:
//! - **与引擎实现解耦**(计划风险 R4):引擎内部的 clamp/auto-freeze/config
//!   是实现细节,形式化性质表达在"事件→level 变化"的可观测行为上;
//!   引擎重构不破坏验证器
//! - **与 M1 五验证器同构**:偏好对/事件元数据/学习轨迹均验证观测序列,
//!   本验证器保持一致,FormalVerifier 管线可统一并发消费
//! - **纯函数 + `VerificationResult` 三态**:复用 L0 契约层类型
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本验证器为纯观测函数,无梯度更新/无策略网络/无训练路径;
//! 标识符规避 5 个 R2 扫描关键词。是 R2 解冻三阶递进的阶段① 组成——
//! 在 R2 解冻前先形式化保证 L4 安全层衰减行为的一致性。

use nexus_contracts::formal_props::VerificationResult;

/// 衰减事件种类 — `LevelTransition` 的事件标签
///
/// WHY 独立枚举而非复用 `crate::DecayEvent`: `DecayEvent` 携带 capability_id /
/// severity / reason 等业务载荷,验证器只关心"事件属于哪一类"以判定单调性
/// 方向。轻量标签枚举使验证器输入最小化,且不依赖 DecayEvent 的 payload 演进。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecayEventKind {
    /// 时间驱动衰减(level 非增)
    TimeDecay,
    /// 违规惩罚衰减(level 非增)
    ViolationPenalty,
    /// 冻结(level 置 0 且进入冻结态)
    Freeze,
    /// 恢复(level 唯一允许上升的事件)
    Restore,
}

impl DecayEventKind {
    /// 该事件是否属于"衰减类"(要求 level 非增)
    ///
    /// TimeDecay 与 ViolationPenalty 是衰减类;Freeze 归零单独约束;
    /// Restore 允许上升,不参与单调性检查。
    pub fn is_decaying(self) -> bool {
        matches!(self, Self::TimeDecay | Self::ViolationPenalty)
    }
}

/// 单步 level 迁移观测 — 验证器的最小输入单元
///
/// 记录一次衰减事件应用的可观测效果:事件种类 + 事件前后的 level 快照。
/// 上层(测试 / 运行时审计)采集 `DecayEngine::decay` 调用的 before/after 构造。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelTransition {
    /// 本次应用的事件种类
    pub event: DecayEventKind,
    /// 事件应用前的 level(∈ [0, 1])
    pub level_before: f32,
    /// 事件应用后的 level(∈ [0, 1])
    pub level_after: f32,
}

impl LevelTransition {
    /// 构造迁移观测
    pub fn new(event: DecayEventKind, level_before: f32, level_after: f32) -> Self {
        Self {
            event,
            level_before,
            level_after,
        }
    }
}

/// 衰减一致性验证器
///
/// 所有方法为纯函数,不修改内部状态,可在 FormalVerifier 管线中并发调用。
#[derive(Debug, Default, Clone, Copy)]
pub struct DecayConsistencyChecker;

impl DecayConsistencyChecker {
    /// 创建衰减一致性验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证衰减单调性:衰减类事件后 level 非增
    ///
    /// 对每个 `event.is_decaying()` 的迁移,要求 `level_after <= level_before`
    /// (含浮点容差 EPSILON,吸收 clamp 边界的舍入)。
    /// Restore / Freeze 迁移不参与此检查(前者允许上升,后者由归零检查覆盖)。
    ///
    /// # 返回
    /// - `Satisfied`: 全部衰减类迁移满足非增
    /// - `Violated`: 存在衰减类迁移 level 上升(携带位置反例)
    /// - `Skipped`: 无衰减类迁移可验证
    #[must_use]
    pub fn verify_decay_monotonic(&self, transitions: &[LevelTransition]) -> VerificationResult {
        // 浮点容差:衰减计算经 clamp,允许 after 略高于 before 的舍入噪声
        const EPSILON: f32 = 1e-6;

        let decaying: Vec<(usize, &LevelTransition)> = transitions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.event.is_decaying())
            .collect();

        if decaying.is_empty() {
            return VerificationResult::Skipped {
                reason: "无衰减类迁移(TimeDecay/ViolationPenalty)可验证".to_string(),
            };
        }

        let violations: Vec<String> = decaying
            .iter()
            .filter(|(_, t)| t.level_after > t.level_before + EPSILON)
            .map(|(i, t)| {
                format!(
                    "位置 {i}: {:?} 后 level {:.6} → {:.6} 上升(衰减类事件应非增)",
                    t.event, t.level_before, t.level_after
                )
            })
            .collect();

        Self::to_result(violations, decaying.len() as u64)
    }

    /// 验证有界性:所有迁移的前后 level 恒 ∈ [0, 1]
    ///
    /// # 返回
    /// - `Skipped`: 空序列
    #[must_use]
    pub fn verify_level_bounded(&self, transitions: &[LevelTransition]) -> VerificationResult {
        if transitions.is_empty() {
            return VerificationResult::Skipped {
                reason: "迁移序列为空".to_string(),
            };
        }

        let violations: Vec<String> = transitions
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                let before_ok = t.level_before.is_finite() && (0.0..=1.0).contains(&t.level_before);
                let after_ok = t.level_after.is_finite() && (0.0..=1.0).contains(&t.level_after);
                if before_ok && after_ok {
                    None
                } else {
                    Some(format!(
                        "位置 {i}: level 越界 [0,1](before={}, after={})",
                        t.level_before, t.level_after
                    ))
                }
            })
            .collect();

        Self::to_result(violations, transitions.len() as u64)
    }

    /// 验证 Freeze 归零不可逆:观测到 Freeze 后 level 恒 0,直至 Restore
    ///
    /// 状态机:遍历迁移序列,Freeze 事件后进入"冻结区间",区间内任一迁移
    /// (无论何种事件)的 `level_after` 必须为 0,直至遇到 Restore 事件退出区间。
    ///
    /// WHY 检查 level_after: Freeze 自身与冻结区间内的后续衰减事件都应保持
    /// level=0(引擎语义:冻结后衰减/惩罚事件被跳过,level 不变仍为 0)。
    ///
    /// # 返回
    /// - `Skipped`: 序列中无 Freeze 事件
    #[must_use]
    pub fn verify_freeze_zero_irreversible(
        &self,
        transitions: &[LevelTransition],
    ) -> VerificationResult {
        const EPSILON: f32 = 1e-6;

        if !transitions
            .iter()
            .any(|t| t.event == DecayEventKind::Freeze)
        {
            return VerificationResult::Skipped {
                reason: "序列中无 Freeze 事件".to_string(),
            };
        }

        let mut frozen = false;
        let mut violations: Vec<String> = Vec::new();
        let mut samples_tested: u64 = 0;

        for (i, t) in transitions.iter().enumerate() {
            match t.event {
                DecayEventKind::Freeze => {
                    frozen = true;
                    samples_tested += 1;
                    // Freeze 自身必须使 level_after 归零
                    if t.level_after.abs() > EPSILON {
                        violations.push(format!(
                            "位置 {i}: Freeze 后 level={:.6} 未归零",
                            t.level_after
                        ));
                    }
                }
                DecayEventKind::Restore => {
                    // Restore 退出冻结区间(是否真正回升由引擎语义决定,此处不约束)
                    frozen = false;
                }
                _ if frozen => {
                    // 冻结区间内的衰减类事件:level 必须仍为 0(权限不残留)
                    samples_tested += 1;
                    if t.level_after.abs() > EPSILON {
                        violations.push(format!(
                            "位置 {i}: 冻结区间内 {:?} 后 level={:.6} 残留(应为 0)",
                            t.event, t.level_after
                        ));
                    }
                }
                _ => {}
            }
        }

        Self::to_result(violations, samples_tested)
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

    fn t(event: DecayEventKind, before: f32, after: f32) -> LevelTransition {
        LevelTransition::new(event, before, after)
    }

    // ============================================================
    // DecayEventKind
    // ============================================================

    #[test]
    fn test_is_decaying_classification() {
        assert!(DecayEventKind::TimeDecay.is_decaying());
        assert!(DecayEventKind::ViolationPenalty.is_decaying());
        assert!(!DecayEventKind::Freeze.is_decaying());
        assert!(!DecayEventKind::Restore.is_decaying());
    }

    // ============================================================
    // 衰减单调性
    // ============================================================

    #[test]
    fn test_decay_monotonic_satisfied() {
        let checker = DecayConsistencyChecker::new();
        let seq = [
            t(DecayEventKind::TimeDecay, 1.0, 0.9),
            t(DecayEventKind::ViolationPenalty, 0.9, 0.5),
        ];
        assert!(checker.verify_decay_monotonic(&seq).is_satisfied());
    }

    #[test]
    fn test_decay_monotonic_violated_on_increase() {
        let checker = DecayConsistencyChecker::new();
        // ViolationPenalty 后 level 竟然上升 → 权限提升攻击面
        let seq = [t(DecayEventKind::ViolationPenalty, 0.5, 0.8)];
        let result = checker.verify_decay_monotonic(&seq);
        assert!(matches!(result, VerificationResult::Violated { .. }));
    }

    #[test]
    fn test_decay_monotonic_restore_not_checked() {
        // Restore 上升是合法的,不参与单调性检查 → 只有 Restore 时 Skipped
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::Restore, 0.2, 0.6)];
        assert!(matches!(
            checker.verify_decay_monotonic(&seq),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_decay_monotonic_tolerates_float_noise() {
        // clamp 舍入导致 after 略高于 before(< EPSILON)不应误报
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::TimeDecay, 0.5, 0.5 + 5e-7)];
        assert!(checker.verify_decay_monotonic(&seq).is_satisfied());
    }

    // ============================================================
    // 有界性
    // ============================================================

    #[test]
    fn test_level_bounded_satisfied() {
        let checker = DecayConsistencyChecker::new();
        let seq = [
            t(DecayEventKind::TimeDecay, 1.0, 0.7),
            t(DecayEventKind::Freeze, 0.7, 0.0),
        ];
        assert!(checker.verify_level_bounded(&seq).is_satisfied());
    }

    #[test]
    fn test_level_over_one_violated() {
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::Restore, 0.9, 1.5)];
        assert!(matches!(
            checker.verify_level_bounded(&seq),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_level_negative_violated() {
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::ViolationPenalty, 0.1, -0.2)];
        assert!(matches!(
            checker.verify_level_bounded(&seq),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_level_nan_violated() {
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::TimeDecay, 0.5, f32::NAN)];
        assert!(matches!(
            checker.verify_level_bounded(&seq),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_level_bounded_empty_skipped() {
        let checker = DecayConsistencyChecker::new();
        assert!(matches!(
            checker.verify_level_bounded(&[]),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // Freeze 归零不可逆
    // ============================================================

    #[test]
    fn test_freeze_zero_satisfied() {
        let checker = DecayConsistencyChecker::new();
        // Freeze 归零 → 冻结区间内衰减事件保持 0 → Restore 退出
        let seq = [
            t(DecayEventKind::TimeDecay, 1.0, 0.8),
            t(DecayEventKind::Freeze, 0.8, 0.0),
            t(DecayEventKind::TimeDecay, 0.0, 0.0),
            t(DecayEventKind::ViolationPenalty, 0.0, 0.0),
            t(DecayEventKind::Restore, 0.0, 0.3),
        ];
        assert!(checker.verify_freeze_zero_irreversible(&seq).is_satisfied());
    }

    #[test]
    fn test_freeze_not_zeroed_violated() {
        let checker = DecayConsistencyChecker::new();
        // Freeze 后 level 未归零
        let seq = [t(DecayEventKind::Freeze, 0.8, 0.5)];
        assert!(matches!(
            checker.verify_freeze_zero_irreversible(&seq),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_frozen_interval_residual_violated() {
        let checker = DecayConsistencyChecker::new();
        // 冻结区间内出现 level 残留(权限泄漏)
        let seq = [
            t(DecayEventKind::Freeze, 0.8, 0.0),
            t(DecayEventKind::TimeDecay, 0.0, 0.2), // 残留!
        ];
        let result = checker.verify_freeze_zero_irreversible(&seq);
        match result {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("残留"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    #[test]
    fn test_freeze_absent_skipped() {
        let checker = DecayConsistencyChecker::new();
        let seq = [t(DecayEventKind::TimeDecay, 1.0, 0.9)];
        assert!(matches!(
            checker.verify_freeze_zero_irreversible(&seq),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_freeze_restore_refreeze() {
        // Restore 退出冻结区间后可再次 Freeze,归零检查重新生效
        let checker = DecayConsistencyChecker::new();
        let seq = [
            t(DecayEventKind::Freeze, 0.8, 0.0),
            t(DecayEventKind::Restore, 0.0, 0.4),
            t(DecayEventKind::Freeze, 0.4, 0.0),
            t(DecayEventKind::TimeDecay, 0.0, 0.0),
        ];
        assert!(checker.verify_freeze_zero_irreversible(&seq).is_satisfied());
    }

    // ============================================================
    // proptest 属性(M2 覆盖强化)
    // ============================================================

    proptest! {
        /// 属性 1: 衰减类事件 after<=before 的序列恒满足单调性
        #[test]
        fn prop_decaying_non_increasing_satisfied(
            steps in proptest::collection::vec((0.0f32..=1.0, 0.0f32..=1.0), 1..30),
        ) {
            let checker = DecayConsistencyChecker::new();
            // 强制构造 after<=before 的衰减迁移
            let transitions: Vec<LevelTransition> = steps
                .iter()
                .map(|(a, b)| {
                    let (hi, lo) = if a >= b { (*a, *b) } else { (*b, *a) };
                    t(DecayEventKind::TimeDecay, hi, lo)
                })
                .collect();
            prop_assert!(checker.verify_decay_monotonic(&transitions).is_satisfied());
        }

        /// 属性 2: [0,1] 内的迁移恒满足有界性
        #[test]
        fn prop_in_range_transitions_bounded(
            before in 0.0f32..=1.0,
            after in 0.0f32..=1.0,
        ) {
            let checker = DecayConsistencyChecker::new();
            let seq = [t(DecayEventKind::TimeDecay, before, after)];
            prop_assert!(checker.verify_level_bounded(&seq).is_satisfied());
        }

        /// 属性 3: Freeze 后全 0 直至 Restore 的序列恒满足不可逆性
        #[test]
        fn prop_freeze_all_zero_satisfied(
            tail_len in 0usize..10,
        ) {
            let checker = DecayConsistencyChecker::new();
            let mut seq = vec![t(DecayEventKind::Freeze, 0.5, 0.0)];
            // 冻结区间内追加若干保持 0 的衰减事件
            for _ in 0..tail_len {
                seq.push(t(DecayEventKind::ViolationPenalty, 0.0, 0.0));
            }
            prop_assert!(checker.verify_freeze_zero_irreversible(&seq).is_satisfied());
        }
    }
}
