//! R2 解冻范围界定守卫 — R2 解冻阶段③ 前置 4(fail-closed 白名单)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution,与 R2 冻结声明 + FormalVerifierGate 同 crate)
//! 对应 ADR: ADR-052 待办 4(解冻范围界定)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 4
//!
//! # 职责:把"解冻范围"从文档承诺升级为可执行强制
//!
//! ADR-052 待办 4 要求"明确阶段③ 影子模式的 RL 更新边界(仅 GSOE 变体选择?
//! 还是含 AutoDPO 偏好更新?),避免一次性放开全部 R2 冻结面"。本模块**不写
//! 一纸文档承诺**,而是构建 fail-closed 白名单守卫 `UnfreezeScope`:未显式纳入
//! 范围的更新目标一律拒绝(默认全冻结),使"范围边界"在运行时可强制、可测试。
//!
//! # 与前置 2/3 构成完整安全封套
//!
//! - **前置 4(本模块)= WHAT**:哪些更新目标在解冻范围内(范围边界)
//! - **前置 3 熔断器 = WHETHER(运行时)**:当前形式化验证状态是否许可
//! - **前置 2 CI 门禁 = WHETHER(提交时)**:进化提交是否通过形式化门
//!
//! 完整解冻决策 = `范围内(WHAT)` AND `验证通过(WHETHER)`。任一不满足即拒绝。
//!
//! # fail-closed 语义(安全核心)
//!
//! - **默认全冻结**:`UnfreezeScope::frozen()` 空白名单 → 所有目标 OutOfScope
//! - **显式纳入**:`allow` / `with_target` 逐个纳入,避免"一次性放开全部"
//! - **未知目标拒绝**:任何未显式纳入的目标 → OutOfScope(不给"默认允许"的口子)
//!
//! # R2 冻结声明(ADR-042)
//!
//! 纯白名单策略判定,无 RL 训练、无梯度更新;标识符规避 5 个 R2 扫描关键词。
//! 本守卫**收紧**范围(默认拒绝),是解冻前的边界基建,不解冻。

use std::collections::HashSet;

/// R2 解冻范围内的可更新目标标识
///
/// WHY 自包含枚举而非复用 omega-learner `SeamId`:引用 L6 类型会构成
/// gsoe(L5)→ omega-learner(L6)向上依赖(§2.2 铁律禁止);故接缝用 u8
/// 编号(S1-S8)自包含表达,不跨层依赖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RlUpdateTarget {
    /// GSOE 变体选择(进化候选选择路径)
    GsoeVariantSelection,
    /// AutoDPO 偏好对更新路径
    AutoDpoPreference,
    /// 接缝策略更新(接缝编号 1-8,对应 omega-learner S1-S8)
    SeamPolicy(u8),
}

/// 范围判定裁决
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeVerdict {
    /// 目标在解冻范围内(已显式纳入)
    InScope,
    /// 目标不在解冻范围内(fail-closed 默认拒绝)
    OutOfScope {
        /// 拒绝原因(人类可读,供审计)
        reason: String,
    },
}

impl ScopeVerdict {
    /// 是否在范围内
    #[must_use]
    pub fn is_in_scope(&self) -> bool {
        matches!(self, Self::InScope)
    }
}

/// R2 解冻范围守卫 — fail-closed 白名单
///
/// 声明哪些 `RlUpdateTarget` 被纳入解冻范围;未纳入者一律拒绝(默认全冻结)。
/// 纯策略判定,不执行任何 RL 训练。
#[derive(Debug, Clone, Default)]
pub struct UnfreezeScope {
    /// 已显式纳入解冻范围的目标白名单
    allowed: HashSet<RlUpdateTarget>,
}

impl UnfreezeScope {
    /// 创建全冻结范围(空白名单,所有目标 OutOfScope)
    ///
    /// 这是 fail-closed 的初始态:未显式 `allow` 前,一切拒绝。
    pub fn frozen() -> Self {
        Self {
            allowed: HashSet::new(),
        }
    }

    /// 纳入一个目标到解冻范围(builder 风格)
    #[must_use]
    pub fn with_target(mut self, target: RlUpdateTarget) -> Self {
        self.allowed.insert(target);
        self
    }

    /// 纳入一个目标到解冻范围(可变方法)
    pub fn allow(&mut self, target: RlUpdateTarget) {
        self.allowed.insert(target);
    }

    /// 从解冻范围移除一个目标(重新冻结单个目标)
    pub fn revoke(&mut self, target: &RlUpdateTarget) {
        self.allowed.remove(target);
    }

    /// 判定目标是否在解冻范围内(fail-closed)
    ///
    /// # 返回
    /// - `InScope`: 目标已显式纳入白名单
    /// - `OutOfScope`: 目标未纳入(默认拒绝,携带诊断原因)
    #[must_use]
    pub fn is_in_scope(&self, target: &RlUpdateTarget) -> ScopeVerdict {
        if self.allowed.contains(target) {
            ScopeVerdict::InScope
        } else {
            ScopeVerdict::OutOfScope {
                reason: format!("目标 {target:?} 未纳入解冻范围(fail-closed 默认拒绝)"),
            }
        }
    }

    /// 目标是否在范围内(便捷布尔判定)
    #[must_use]
    pub fn contains(&self, target: &RlUpdateTarget) -> bool {
        self.allowed.contains(target)
    }

    /// 当前纳入范围的目标数
    #[must_use]
    pub fn allowed_count(&self) -> usize {
        self.allowed.len()
    }

    /// 是否完全冻结(白名单为空)
    #[must_use]
    pub fn is_fully_frozen(&self) -> bool {
        self.allowed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frozen_denies_everything() {
        let scope = UnfreezeScope::frozen();
        assert!(scope.is_fully_frozen());
        assert_eq!(scope.allowed_count(), 0);
        // 所有目标均 OutOfScope
        assert!(!scope
            .is_in_scope(&RlUpdateTarget::GsoeVariantSelection)
            .is_in_scope());
        assert!(!scope
            .is_in_scope(&RlUpdateTarget::AutoDpoPreference)
            .is_in_scope());
        assert!(!scope
            .is_in_scope(&RlUpdateTarget::SeamPolicy(1))
            .is_in_scope());
    }

    #[test]
    fn test_allow_single_target_only_that_in_scope() {
        let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
        // 纳入的在范围内
        assert!(scope
            .is_in_scope(&RlUpdateTarget::GsoeVariantSelection)
            .is_in_scope());
        // 未纳入的仍拒绝(fail-closed)
        assert!(!scope
            .is_in_scope(&RlUpdateTarget::AutoDpoPreference)
            .is_in_scope());
        assert!(!scope.is_fully_frozen());
        assert_eq!(scope.allowed_count(), 1);
    }

    #[test]
    fn test_out_of_scope_carries_reason() {
        let scope = UnfreezeScope::frozen();
        match scope.is_in_scope(&RlUpdateTarget::AutoDpoPreference) {
            ScopeVerdict::OutOfScope { reason } => {
                assert!(reason.contains("未纳入解冻范围"));
            }
            ScopeVerdict::InScope => panic!("全冻结下不应 InScope"),
        }
    }

    #[test]
    fn test_seam_policy_granular_scope() {
        // 仅纳入 S4 接缝,S5 不在范围
        let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::SeamPolicy(4));
        assert!(scope
            .is_in_scope(&RlUpdateTarget::SeamPolicy(4))
            .is_in_scope());
        assert!(!scope
            .is_in_scope(&RlUpdateTarget::SeamPolicy(5))
            .is_in_scope());
    }

    #[test]
    fn test_allow_multiple_targets() {
        let mut scope = UnfreezeScope::frozen();
        scope.allow(RlUpdateTarget::GsoeVariantSelection);
        scope.allow(RlUpdateTarget::SeamPolicy(1));
        assert_eq!(scope.allowed_count(), 2);
        assert!(scope.contains(&RlUpdateTarget::GsoeVariantSelection));
        assert!(scope.contains(&RlUpdateTarget::SeamPolicy(1)));
        assert!(!scope.contains(&RlUpdateTarget::AutoDpoPreference));
    }

    #[test]
    fn test_revoke_refreezes_target() {
        let mut scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
        assert!(scope.contains(&RlUpdateTarget::GsoeVariantSelection));
        scope.revoke(&RlUpdateTarget::GsoeVariantSelection);
        // 撤销后重新冻结
        assert!(!scope.contains(&RlUpdateTarget::GsoeVariantSelection));
        assert!(scope.is_fully_frozen());
    }

    #[test]
    fn test_idempotent_allow() {
        let mut scope = UnfreezeScope::frozen();
        scope.allow(RlUpdateTarget::GsoeVariantSelection);
        scope.allow(RlUpdateTarget::GsoeVariantSelection); // 重复纳入
        assert_eq!(scope.allowed_count(), 1, "重复纳入不应增加计数");
    }

    #[test]
    fn test_default_is_frozen() {
        // Default 派生应等价于 frozen()
        let scope = UnfreezeScope::default();
        assert!(scope.is_fully_frozen());
    }

    // ============================================================
    // proptest 属性(fail-closed 不变量)
    // ============================================================

    use proptest::prelude::*;

    /// 生成任意 RlUpdateTarget
    fn any_target() -> impl Strategy<Value = RlUpdateTarget> {
        prop_oneof![
            Just(RlUpdateTarget::GsoeVariantSelection),
            Just(RlUpdateTarget::AutoDpoPreference),
            (1u8..=8).prop_map(RlUpdateTarget::SeamPolicy),
        ]
    }

    proptest! {
        /// 属性 1: 全冻结范围拒绝任意目标(fail-closed 核心不变量)
        #[test]
        fn prop_frozen_denies_any_target(target in any_target()) {
            let scope = UnfreezeScope::frozen();
            prop_assert!(!scope.is_in_scope(&target).is_in_scope());
        }

        /// 属性 2: 纳入的目标必在范围内,未纳入的必拒绝(白名单精确性)
        #[test]
        fn prop_only_allowed_in_scope(
            allowed in any_target(),
            probe in any_target(),
        ) {
            let scope = UnfreezeScope::frozen().with_target(allowed);
            // 探测目标在范围内 当且仅当 它等于被纳入的目标
            let expected = probe == allowed;
            prop_assert_eq!(scope.is_in_scope(&probe).is_in_scope(), expected);
        }

        /// 属性 3: 纳入后撤销 → 回到拒绝(可逆冻结)
        #[test]
        fn prop_allow_then_revoke_denies(target in any_target()) {
            let mut scope = UnfreezeScope::frozen();
            scope.allow(target);
            prop_assert!(scope.is_in_scope(&target).is_in_scope());
            scope.revoke(&target);
            prop_assert!(!scope.is_in_scope(&target).is_in_scope());
        }
    }
}
