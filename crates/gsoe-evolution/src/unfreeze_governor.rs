//! R2 解冻统一决策闸门 — 4 项前置的组合封套(capstone)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution,拥有 UnfreezeScope)
//! 对应 ADR: ADR-052(4 项工程前置组合)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置收敛
//!
//! # 职责:把 4 项前置组合成单一 fail-closed 决策入口
//!
//! ADR-052 的 4 项工程前置此前是 4 个独立组件,"完整解冻决策"只以测试辅助
//! 函数形式存在(前置 4 E2E 的 `unfreeze_permitted()`)。本模块把该组合逻辑
//! 收敛为生产级 `UnfreezeGovernor`——影子模式下每次 RL 更新前的**唯一决策入口**:
//!
//! - **WHAT(前置 4)**:目标是否在解冻范围内(`UnfreezeScope`,本 governor 持有)
//! - **WHETHER(前置 1/3)**:形式化验证状态是否许可(由调用方运行熔断器后传入)
//! - **统一决策**:两维度 AND —— 任一不满足即 `Denied`,并**指明失败维度**
//!
//! # 决策闸门 ≠ 解冻(WHY 本模块不违反 R2 冻结)
//!
//! 本 governor 是**决策的执行点**,不是解冻动作本身。冻结范围(`fully_frozen`)下
//! 它恒返回 `Denied`——构建闸门 ≠ 打开闸门。它不执行 RL 训练、无梯度更新、
//! 标识符规避 5 个 R2 扫描关键词。是解冻前必须先就位的**统一强制点**,收紧约束。
//!
//! # WHY 消费验证 bool 而非持有熔断器(依赖铁律 + 职责分离)
//!
//! 熔断器 `ShadowModeCircuitBreaker` 在 decay-engine(L4)。若 governor 持有它需
//! gsoe(L5)→ decay-engine(L4)向下依赖(虽合规但增加耦合)。更重要的是**职责
//! 分离**:熔断器负责"验证状态是否许可"(有状态、可跳闸),governor 负责"范围 ×
//! 验证的组合裁决"(无状态组合)。调用方运行熔断器得到 `is_permitted()` bool 后
//! 传入 governor,两者解耦,各自可独立测试。

use crate::unfreeze_scope::{RlUpdateTarget, UnfreezeScope};

/// 决策被拒绝的维度 — 指明 WHAT / WHETHER 哪一维度(或两者)未通过
///
/// WHY 携带维度而非裸 bool:审计与调试需知道拒绝根因——是目标越界(范围问题)、
/// 验证未通过(形式化质量问题),还是两者兼有。这是 governor 相较裸 `&&` 的核心价值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialDimension {
    /// 目标不在解冻范围内(前置 4 WHAT 维度失败)
    OutOfScope,
    /// 形式化验证未许可(前置 1/3 WHETHER 维度失败)
    VerificationNotPermitted,
    /// 两个维度均失败(既越界又未通过验证)
    Both,
}

impl DenialDimension {
    /// 人类可读标识
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutOfScope => "out_of_scope",
            Self::VerificationNotPermitted => "verification_not_permitted",
            Self::Both => "both",
        }
    }
}

/// R2 解冻统一决策
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnfreezeDecision {
    /// 允许:目标在范围内 且 验证许可(两维度均通过)
    Allowed,
    /// 拒绝:携带失败维度与人类可读原因
    Denied {
        /// 失败维度(WHAT / WHETHER / Both)
        dimension: DenialDimension,
        /// 拒绝原因(供审计)
        reason: String,
    },
}

impl UnfreezeDecision {
    /// 是否允许
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// R2 解冻统一决策闸门 — 组合范围(WHAT)与验证(WHETHER)
///
/// 持有 `UnfreezeScope`(WHAT 策略),`decide` 时结合调用方传入的验证许可
/// (WHETHER)产出统一决策。记录审计计数供影子模式监控。
#[derive(Debug, Clone)]
pub struct UnfreezeGovernor {
    /// 解冻范围策略(WHAT 维度)
    scope: UnfreezeScope,
    /// 累计允许决策数(审计)
    allowed_count: u64,
    /// 累计拒绝决策数(审计)
    denied_count: u64,
}

impl UnfreezeGovernor {
    /// 用指定范围创建决策闸门
    pub fn new(scope: UnfreezeScope) -> Self {
        Self {
            scope,
            allowed_count: 0,
            denied_count: 0,
        }
    }

    /// 创建全冻结决策闸门(范围为空,任何目标都拒绝)
    ///
    /// 这是 fail-closed 的默认起点:未显式放开范围前,一切 RL 更新拒绝。
    pub fn fully_frozen() -> Self {
        Self::new(UnfreezeScope::frozen())
    }

    /// 统一决策:目标在范围内 AND 验证许可 → Allowed,否则 Denied(指明维度)
    ///
    /// # 参数
    /// - `target`: 待决策的 RL 更新目标(WHAT)
    /// - `verification_permitted`: 验证是否许可(WHETHER,由调用方运行熔断器
    ///   `ShadowModeCircuitBreaker::observe(...).is_permitted()` 后传入)
    ///
    /// # fail-closed 组合
    /// | 范围内 | 验证许可 | 决策 |
    /// |--------|---------|------|
    /// | ✓ | ✓ | Allowed |
    /// | ✗ | ✓ | Denied(OutOfScope) |
    /// | ✓ | ✗ | Denied(VerificationNotPermitted) |
    /// | ✗ | ✗ | Denied(Both) |
    pub fn decide(
        &mut self,
        target: &RlUpdateTarget,
        verification_permitted: bool,
    ) -> UnfreezeDecision {
        let in_scope = self.scope.contains(target);

        let decision = match (in_scope, verification_permitted) {
            (true, true) => UnfreezeDecision::Allowed,
            (false, true) => UnfreezeDecision::Denied {
                dimension: DenialDimension::OutOfScope,
                reason: format!("目标 {target:?} 不在解冻范围内(验证已许可但范围外)"),
            },
            (true, false) => UnfreezeDecision::Denied {
                dimension: DenialDimension::VerificationNotPermitted,
                reason: format!("目标 {target:?} 在范围内但形式化验证未许可"),
            },
            (false, false) => UnfreezeDecision::Denied {
                dimension: DenialDimension::Both,
                reason: format!("目标 {target:?} 既不在范围内,验证也未许可"),
            },
        };

        // 审计计数
        if decision.is_allowed() {
            self.allowed_count += 1;
        } else {
            self.denied_count += 1;
        }
        decision
    }

    /// 只读访问范围策略
    #[must_use]
    pub fn scope(&self) -> &UnfreezeScope {
        &self.scope
    }

    /// 可变访问范围策略(渐进式解冻:逐个 `allow` 纳入目标)
    pub fn scope_mut(&mut self) -> &mut UnfreezeScope {
        &mut self.scope
    }

    /// 累计允许决策数
    #[must_use]
    pub fn allowed_count(&self) -> u64 {
        self.allowed_count
    }

    /// 累计拒绝决策数
    #[must_use]
    pub fn denied_count(&self) -> u64 {
        self.denied_count
    }

    /// 累计决策总数
    #[must_use]
    pub fn total_decisions(&self) -> u64 {
        self.allowed_count + self.denied_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造纳入 GSOE 变体选择的 governor
    fn governor_allowing_gsoe() -> UnfreezeGovernor {
        UnfreezeGovernor::new(
            UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection),
        )
    }

    #[test]
    fn test_in_scope_and_verified_allows() {
        let mut gov = governor_allowing_gsoe();
        let d = gov.decide(&RlUpdateTarget::GsoeVariantSelection, true);
        assert!(d.is_allowed());
        assert_eq!(gov.allowed_count(), 1);
        assert_eq!(gov.denied_count(), 0);
    }

    #[test]
    fn test_out_of_scope_but_verified_denies_scope() {
        let mut gov = governor_allowing_gsoe();
        let d = gov.decide(&RlUpdateTarget::AutoDpoPreference, true);
        match d {
            UnfreezeDecision::Denied { dimension, .. } => {
                assert_eq!(dimension, DenialDimension::OutOfScope);
            }
            UnfreezeDecision::Allowed => panic!("范围外应拒绝"),
        }
        assert_eq!(gov.denied_count(), 1);
    }

    #[test]
    fn test_in_scope_but_not_verified_denies_verification() {
        let mut gov = governor_allowing_gsoe();
        let d = gov.decide(&RlUpdateTarget::GsoeVariantSelection, false);
        match d {
            UnfreezeDecision::Denied { dimension, .. } => {
                assert_eq!(dimension, DenialDimension::VerificationNotPermitted);
            }
            UnfreezeDecision::Allowed => panic!("验证未许可应拒绝"),
        }
    }

    #[test]
    fn test_both_dimensions_fail() {
        let mut gov = governor_allowing_gsoe();
        // 范围外 + 验证未许可
        let d = gov.decide(&RlUpdateTarget::AutoDpoPreference, false);
        match d {
            UnfreezeDecision::Denied { dimension, .. } => {
                assert_eq!(dimension, DenialDimension::Both);
            }
            UnfreezeDecision::Allowed => panic!("两维度失败应拒绝"),
        }
    }

    #[test]
    fn test_fully_frozen_denies_everything() {
        let mut gov = UnfreezeGovernor::fully_frozen();
        // 即使验证许可,全冻结范围也拒绝
        let d = gov.decide(&RlUpdateTarget::GsoeVariantSelection, true);
        assert!(!d.is_allowed());
        match d {
            UnfreezeDecision::Denied { dimension, .. } => {
                assert_eq!(dimension, DenialDimension::OutOfScope);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_progressive_unfreeze_via_scope_mut() {
        let mut gov = UnfreezeGovernor::fully_frozen();
        // 初始全冻结:拒绝
        assert!(!gov
            .decide(&RlUpdateTarget::SeamPolicy(1), true)
            .is_allowed());
        // 渐进纳入 S1
        gov.scope_mut().allow(RlUpdateTarget::SeamPolicy(1));
        // 现在 S1 在范围内 + 验证许可 → 允许
        assert!(gov
            .decide(&RlUpdateTarget::SeamPolicy(1), true)
            .is_allowed());
    }

    #[test]
    fn test_audit_counts_accumulate() {
        let mut gov = governor_allowing_gsoe();
        gov.decide(&RlUpdateTarget::GsoeVariantSelection, true); // allowed
        gov.decide(&RlUpdateTarget::GsoeVariantSelection, false); // denied
        gov.decide(&RlUpdateTarget::AutoDpoPreference, true); // denied
        assert_eq!(gov.allowed_count(), 1);
        assert_eq!(gov.denied_count(), 2);
        assert_eq!(gov.total_decisions(), 3);
    }

    #[test]
    fn test_denial_dimension_as_str() {
        assert_eq!(DenialDimension::OutOfScope.as_str(), "out_of_scope");
        assert_eq!(
            DenialDimension::VerificationNotPermitted.as_str(),
            "verification_not_permitted"
        );
        assert_eq!(DenialDimension::Both.as_str(), "both");
    }

    // ============================================================
    // proptest 属性(fail-closed 组合不变量)
    // ============================================================

    use proptest::prelude::*;

    fn any_target() -> impl Strategy<Value = RlUpdateTarget> {
        prop_oneof![
            Just(RlUpdateTarget::GsoeVariantSelection),
            Just(RlUpdateTarget::AutoDpoPreference),
            (1u8..=8).prop_map(RlUpdateTarget::SeamPolicy),
        ]
    }

    proptest! {
        /// 属性 1: 全冻结 governor 对任意 (target, verification) 恒拒绝
        #[test]
        fn prop_fully_frozen_denies_any(
            target in any_target(),
            verified in any::<bool>(),
        ) {
            let mut gov = UnfreezeGovernor::fully_frozen();
            prop_assert!(!gov.decide(&target, verified).is_allowed());
        }

        /// 属性 2: Allowed 当且仅当 范围内 AND 验证许可(fail-closed 核心)
        #[test]
        fn prop_allowed_iff_both_pass(
            allowed_target in any_target(),
            probe in any_target(),
            verified in any::<bool>(),
        ) {
            let mut gov = UnfreezeGovernor::new(
                UnfreezeScope::frozen().with_target(allowed_target),
            );
            let in_scope = probe == allowed_target;
            let decision = gov.decide(&probe, verified);
            // 允许 当且仅当 两维度都通过
            prop_assert_eq!(decision.is_allowed(), in_scope && verified);
        }

        /// 属性 3: 决策计数守恒(allowed + denied == total)
        #[test]
        fn prop_audit_counts_conserved(
            decisions in proptest::collection::vec((any_target(), any::<bool>()), 0..30),
        ) {
            let mut gov = governor_allowing_gsoe();
            for (t, v) in &decisions {
                gov.decide(t, *v);
            }
            prop_assert_eq!(
                gov.allowed_count() + gov.denied_count(),
                gov.total_decisions()
            );
            prop_assert_eq!(gov.total_decisions(), decisions.len() as u64);
        }
    }
}
