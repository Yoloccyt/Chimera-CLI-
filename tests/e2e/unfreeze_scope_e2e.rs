//! 解冻范围守卫端到端测试 — R2 解冻阶段③ 前置 4(范围 × 熔断安全封套)
//!
//! 对应架构层: L5 gsoe-evolution(范围守卫) × L4 decay-engine(熔断器)
//! 对应 ADR: ADR-052 待办 4(解冻范围界定)+ 待办 3(熔断)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 4
//!
//! # 闭环验证:完整解冻决策 = 范围内(WHAT)AND 验证通过(WHETHER)
//!
//! 本 E2E 验证前置 4(范围)与前置 3(熔断)组合成完整安全封套:
//! - **前置 4 `UnfreezeScope`**:目标是否在解冻范围内(WHAT)
//! - **前置 3 `ShadowModeCircuitBreaker`**:形式化验证状态是否许可(WHETHER)
//! - **最终决策**:两者 AND —— 任一不满足即拒绝 RL 更新
//!
//! # 三路径覆盖(用户要求 1)
//!
//! - **正常路径**:目标在范围内 + 验证通过 → 允许
//! - **边界条件**:范围恰好纳入/未纳入边界目标;全冻结默认拒绝
//! - **异常场景**:范围内但验证失败 → 拒绝;验证通过但范围外 → 拒绝

use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use gsoe_evolution::formal::invariant_closure::{InvariantClosureChecker, InvariantEdge};
use gsoe_evolution::unfreeze_scope::{RlUpdateTarget, UnfreezeScope};
use nexus_contracts::formal_props::VerificationResult;

/// 完整解冻决策:范围内 AND 验证通过
///
/// 演示前置 4 × 前置 3 的组合封套——这是阶段③ 影子模式下每次 RL 更新前
/// 应执行的完整安全检查。
fn unfreeze_permitted(
    scope: &UnfreezeScope,
    target: &RlUpdateTarget,
    breaker: &mut ShadowModeCircuitBreaker,
    verification_results: &[VerificationResult],
) -> bool {
    // WHAT:目标必须在解冻范围内
    let in_scope = scope.is_in_scope(target).is_in_scope();
    // WHETHER:形式化验证状态必须许可
    let verified = breaker.observe(verification_results).is_permitted();
    in_scope && verified
}

fn satisfied() -> VerificationResult {
    VerificationResult::Satisfied { samples_tested: 10 }
}

// ============================================================
// 正常路径:范围内 + 验证通过 → 允许
// ============================================================

#[test]
fn test_normal_in_scope_and_verified_permits() {
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    let mut breaker = ShadowModeCircuitBreaker::new();

    let permitted = unfreeze_permitted(
        &scope,
        &RlUpdateTarget::GsoeVariantSelection,
        &mut breaker,
        &[satisfied(), satisfied()],
    );
    assert!(permitted, "范围内 + 验证通过应允许");
}

// ============================================================
// 异常场景:任一维度不满足 → 拒绝
// ============================================================

/// 异常:范围内但验证失败(真实不变量环)→ 拒绝
#[test]
fn test_exception_in_scope_but_verification_fails() {
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    let mut breaker = ShadowModeCircuitBreaker::new();

    // 真实验证器产出 Violated(依赖环)
    let closure = InvariantClosureChecker::new();
    let cyclic = [InvariantEdge::new("a", "b"), InvariantEdge::new("b", "a")];
    let bad = closure.verify_dependency_acyclic(&cyclic);

    let permitted = unfreeze_permitted(
        &scope,
        &RlUpdateTarget::GsoeVariantSelection, // 在范围内
        &mut breaker,
        &[bad], // 但验证失败
    );
    assert!(!permitted, "验证失败应拒绝(即使目标在范围内)");
    assert!(breaker.is_tripped());
}

/// 异常:验证通过但目标不在范围内 → 拒绝
#[test]
fn test_exception_verified_but_out_of_scope() {
    // 范围只纳入 GSOE 变体,不含 AutoDPO 偏好
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    let mut breaker = ShadowModeCircuitBreaker::new();

    let permitted = unfreeze_permitted(
        &scope,
        &RlUpdateTarget::AutoDpoPreference, // 不在范围内
        &mut breaker,
        &[satisfied()], // 验证通过
    );
    assert!(!permitted, "范围外应拒绝(即使验证通过)");
}

/// 异常:全冻结范围拒绝一切(即使验证通过)
#[test]
fn test_exception_fully_frozen_denies_all() {
    let scope = UnfreezeScope::frozen(); // 全冻结
    let mut breaker = ShadowModeCircuitBreaker::new();

    for target in [
        RlUpdateTarget::GsoeVariantSelection,
        RlUpdateTarget::AutoDpoPreference,
        RlUpdateTarget::SeamPolicy(4),
    ] {
        let permitted = unfreeze_permitted(&scope, &target, &mut breaker, &[satisfied()]);
        assert!(!permitted, "全冻结范围应拒绝 {target:?}");
    }
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:接缝级粒度范围(仅 S4 纳入,S5 拒绝)
#[test]
fn test_boundary_granular_seam_scope() {
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::SeamPolicy(4));
    let mut breaker1 = ShadowModeCircuitBreaker::new();
    let mut breaker2 = ShadowModeCircuitBreaker::new();

    // S4 在范围内 + 验证通过 → 允许
    assert!(unfreeze_permitted(
        &scope,
        &RlUpdateTarget::SeamPolicy(4),
        &mut breaker1,
        &[satisfied()]
    ));
    // S5 不在范围 → 拒绝
    assert!(!unfreeze_permitted(
        &scope,
        &RlUpdateTarget::SeamPolicy(5),
        &mut breaker2,
        &[satisfied()]
    ));
}

/// 边界:渐进式解冻(逐个纳入目标,范围逐步扩大)
#[test]
fn test_boundary_progressive_unfreeze() {
    let mut scope = UnfreezeScope::frozen();
    assert!(scope.is_fully_frozen());

    // 第一阶段:仅纳入 GSOE 变体选择
    scope.allow(RlUpdateTarget::GsoeVariantSelection);
    assert_eq!(scope.allowed_count(), 1);
    assert!(scope.contains(&RlUpdateTarget::GsoeVariantSelection));
    assert!(!scope.contains(&RlUpdateTarget::AutoDpoPreference));

    // 第二阶段:再纳入 S1 接缝(渐进扩大,避免一次性放开)
    scope.allow(RlUpdateTarget::SeamPolicy(1));
    assert_eq!(scope.allowed_count(), 2);
    // AutoDPO 偏好始终未纳入(最谨慎的目标保持冻结)
    assert!(!scope.contains(&RlUpdateTarget::AutoDpoPreference));
}

/// 边界:范围内但验证证据不足(空验证结果)→ 拒绝
#[test]
fn test_boundary_in_scope_but_insufficient_evidence() {
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    let mut breaker = ShadowModeCircuitBreaker::new();

    // 空验证结果 → 熔断器 fail-closed 拒绝(证据不足)
    let permitted = unfreeze_permitted(
        &scope,
        &RlUpdateTarget::GsoeVariantSelection,
        &mut breaker,
        &[], // 无验证证据
    );
    assert!(!permitted, "证据不足应拒绝(即使目标在范围内)");
    assert!(!breaker.is_tripped(), "证据不足非违规,不跳闸");
}
