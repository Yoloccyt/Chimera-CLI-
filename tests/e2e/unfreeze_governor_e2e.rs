//! R2 解冻统一决策闸门端到端测试 — 4 项前置组合封套(capstone)
//!
//! 对应架构层: L5 gsoe-evolution(governor + 范围 + 验证器) × L4 decay-engine(熔断器)
//! × L6 omega-learner(后悔率采集)
//! 对应 ADR: ADR-052(4 前置组合)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置收敛
//!
//! # 闭环验证:4 项前置经 governor 组合为单一决策(用户要求 1)
//!
//! 本 E2E 把 4 项前置的真实组件串成完整链路,经 `UnfreezeGovernor` 产出统一决策:
//! - **前置 1** `RegretCollector.assess_trend()` → 后悔率趋势 VerificationResult
//! - **前置 2/3** `ShadowModeCircuitBreaker.observe()` → 验证许可(WHETHER)
//! - **前置 4** `UnfreezeScope`(governor 持有)→ 范围内(WHAT)
//! - **capstone** `UnfreezeGovernor.decide()` → 统一 UnfreezeDecision
//!
//! # 三路径覆盖(用户要求 1)
//!
//! - **正常路径**:收敛后悔率 + 范围内 → Allowed
//! - **边界条件**:全冻结默认拒绝;渐进解冻;审计计数
//! - **异常场景**:后悔率发散(熔断跳闸)/ 范围外 → Denied(指明维度)

use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use gsoe_evolution::unfreeze_governor::{DenialDimension, UnfreezeGovernor};
use gsoe_evolution::unfreeze_scope::{RlUpdateTarget, UnfreezeScope};
use omega_learner::regret_pipeline::RegretCollector;

/// 前置 1:采集收敛后悔率序列 → 趋势 VerificationResult
fn converging_regret_collector() -> RegretCollector {
    let mut c = RegretCollector::new(128, 2, 0.05);
    for (step, r) in [0.9, 0.7, 0.6, 0.4, 0.3, 0.1].iter().enumerate() {
        c.record_regret(step as u64 + 1, *r);
    }
    c
}

/// 前置 1:采集发散后悔率序列
fn diverging_regret_collector() -> RegretCollector {
    let mut c = RegretCollector::new(128, 2, 0.05);
    for (step, r) in [0.2, 0.2, 0.8, 0.8].iter().enumerate() {
        c.record_regret(step as u64 + 1, *r);
    }
    c
}

// ============================================================
// 正常路径:4 前置全通过 → Allowed
// ============================================================

#[test]
fn test_normal_full_chain_allows() {
    // 前置 4:范围纳入 GSOE 变体选择
    let mut gov = UnfreezeGovernor::new(
        UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection),
    );

    // 前置 1:收敛后悔率 → 趋势 Satisfied
    let collector = converging_regret_collector();
    let trend = collector.assess_trend();
    assert!(trend.is_satisfied());

    // 前置 3:熔断器消费后悔率趋势 → 许可
    let mut breaker = ShadowModeCircuitBreaker::new();
    let verification_permitted = breaker.observe(&[trend]).is_permitted();
    assert!(verification_permitted);

    // capstone:governor 组合范围 + 验证 → 统一决策
    let decision = gov.decide(
        &RlUpdateTarget::GsoeVariantSelection,
        verification_permitted,
    );
    assert!(decision.is_allowed(), "4 前置全通过应 Allowed");
    assert_eq!(gov.allowed_count(), 1);
}

// ============================================================
// 异常场景:某前置失败 → Denied(指明维度)
// ============================================================

/// 异常:后悔率发散(前置1/3 失败)但范围内 → Denied(VerificationNotPermitted)
#[test]
fn test_exception_diverging_regret_denies_verification() {
    let mut gov = UnfreezeGovernor::new(
        UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection),
    );

    // 前置 1:发散后悔率 → 趋势 Violated
    let collector = diverging_regret_collector();
    let trend = collector.assess_trend();
    assert!(trend.is_violated());

    // 前置 3:熔断器跳闸 → 不许可
    let mut breaker = ShadowModeCircuitBreaker::new();
    let verification_permitted = breaker.observe(&[trend]).is_permitted();
    assert!(!verification_permitted);
    assert!(breaker.is_tripped());

    // capstone:范围内但验证未许可 → Denied(VerificationNotPermitted)
    let decision = gov.decide(
        &RlUpdateTarget::GsoeVariantSelection,
        verification_permitted,
    );
    assert!(!decision.is_allowed());
    if let gsoe_evolution::unfreeze_governor::UnfreezeDecision::Denied { dimension, .. } = decision
    {
        assert_eq!(dimension, DenialDimension::VerificationNotPermitted);
    }
}

/// 异常:范围外(前置4 失败)但验证通过 → Denied(OutOfScope)
#[test]
fn test_exception_out_of_scope_denies_scope() {
    // 范围只纳入 GSOE,不含 AutoDPO
    let mut gov = UnfreezeGovernor::new(
        UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection),
    );

    // 验证通过(收敛后悔率)
    let collector = converging_regret_collector();
    let mut breaker = ShadowModeCircuitBreaker::new();
    let verification_permitted = breaker.observe(&[collector.assess_trend()]).is_permitted();
    assert!(verification_permitted);

    // 但目标 AutoDPO 不在范围 → Denied(OutOfScope)
    let decision = gov.decide(&RlUpdateTarget::AutoDpoPreference, verification_permitted);
    assert!(!decision.is_allowed());
    if let gsoe_evolution::unfreeze_governor::UnfreezeDecision::Denied { dimension, .. } = decision
    {
        assert_eq!(dimension, DenialDimension::OutOfScope);
    }
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:全冻结 governor 拒绝一切(即使 4 前置验证全通过)
#[test]
fn test_boundary_fully_frozen_denies_all() {
    let mut gov = UnfreezeGovernor::fully_frozen();
    let collector = converging_regret_collector();
    let mut breaker = ShadowModeCircuitBreaker::new();
    let verified = breaker.observe(&[collector.assess_trend()]).is_permitted();

    // 验证通过,但全冻结范围拒绝
    let decision = gov.decide(&RlUpdateTarget::GsoeVariantSelection, verified);
    assert!(!decision.is_allowed(), "全冻结应拒绝(即使验证通过)");
}

/// 边界:渐进解冻 + 审计计数(影子模式多周期)
#[test]
fn test_boundary_progressive_unfreeze_with_audit() {
    let mut gov = UnfreezeGovernor::fully_frozen();
    let collector = converging_regret_collector();

    // 周期 1-3:全冻结,S1 被拒
    for _ in 0..3 {
        let mut breaker = ShadowModeCircuitBreaker::new();
        let verified = breaker.observe(&[collector.assess_trend()]).is_permitted();
        assert!(!gov
            .decide(&RlUpdateTarget::SeamPolicy(1), verified)
            .is_allowed());
    }
    assert_eq!(gov.denied_count(), 3);

    // 渐进纳入 S1
    gov.scope_mut().allow(RlUpdateTarget::SeamPolicy(1));

    // 周期 4-5:S1 在范围内 + 验证通过 → 允许
    for _ in 0..2 {
        let mut breaker = ShadowModeCircuitBreaker::new();
        let verified = breaker.observe(&[collector.assess_trend()]).is_permitted();
        assert!(gov
            .decide(&RlUpdateTarget::SeamPolicy(1), verified)
            .is_allowed());
    }
    assert_eq!(gov.allowed_count(), 2);
    assert_eq!(gov.total_decisions(), 5);
}
