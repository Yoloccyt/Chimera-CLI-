//! R2 解冻就绪检查器测试（Milestone C-5 前置，ADR-053 rev4 阶段③ 自动化载体）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 C-5）：
//! 解冻五要素组合判定（熔断器 Armed + 范围 Allowed + 阶段③ 前置 + 影子期 2 周
//! + E2E 绿 + FormalVerifier 全绿）——R2 冻结面外（只读状态，不触训练）。

#![forbid(unsafe_code)]

use chimera_mas::shadow::orchestrator::Stage3Prerequisites;
use chimera_mas::shadow::readiness::{R2UnfreezeReadiness, R2UnfreezeReadinessInput};
use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use gsoe_evolution::unfreeze_governor::{UnfreezeDecision, UnfreezeGovernor};
use gsoe_evolution::unfreeze_scope::{RlUpdateTarget, UnfreezeScope};

/// 全就绪装配：Armed 熔断器 + 含目标的范围 + 阶段③ 全前置
fn ready_assembly() -> R2UnfreezeReadiness {
    let breaker = ShadowModeCircuitBreaker::new(); // Armed
    let scope = UnfreezeScope::frozen().with_target(RlUpdateTarget::GsoeVariantSelection);
    let mut governor = UnfreezeGovernor::new(scope);
    // 验证许可的决策应为 Allowed（在范围内 + permitted）
    assert!(matches!(
        governor.decide(&RlUpdateTarget::GsoeVariantSelection, true),
        UnfreezeDecision::Allowed
    ));
    let stage3 = Stage3Prerequisites {
        alpha_composite_calibrated: true,
        power_intra_batch_verified: true,
        binomial_sf_comments_corrected: true,
        payload_rotation_ready: true,
        coverage_instrumentation_ready: true,
        s_min_final_confirmed: true,
    };
    R2UnfreezeReadiness::new(breaker, governor, stage3)
}

fn ready_input() -> R2UnfreezeReadinessInput {
    R2UnfreezeReadinessInput {
        shadow_period_days: 14, // ADR-043 影子期 2 周
        r2_freeze_e2e_passed: true,
        formal_verifier_all_green: true,
    }
}

/// 全要素就绪 → ready
#[test]
fn all_elements_ready_is_ready() {
    let readiness = ready_assembly();
    let report = readiness.evaluate(&ready_input());
    assert!(report.ready, "全就绪应判定 ready: {:?}", report.missing);
    assert!(report.missing.is_empty());
}

/// 影子期不足（13 天 < 14）→ not_ready
#[test]
fn shadow_period_below_two_weeks_blocks() {
    let readiness = ready_assembly();
    let mut input = ready_input();
    input.shadow_period_days = 13;
    let report = readiness.evaluate(&input);
    assert!(!report.ready);
    assert!(
        report.missing.iter().any(|m| m.contains("影子期")),
        "应标记影子期不足: {:?}",
        report.missing
    );
}

/// 断路器跳闸（Tripped）→ not_ready（fail-closed 永久拒绝）
#[test]
fn tripped_breaker_blocks() {
    let mut breaker = ShadowModeCircuitBreaker::new();
    // 触发跳闸：违规验证结果
    let verdict = breaker.observe(&[]);
    let _ = verdict;
    // 直接构造跳闸态：通过 observe 违规结果（简化：模拟一次违规观察）
    let mut readiness = ready_assembly();
    readiness.breaker = ShadowModeCircuitBreaker::new();
    // 用违规结果触发跳闸
    use nexus_contracts::VerificationResult;
    let _ = readiness.breaker.observe(&[VerificationResult::Violated {
        counterexample: "模拟违规".into(),
        samples_tested: 1,
    }]);
    assert!(readiness.breaker.is_tripped(), "观察违规应跳闸");
    let report = readiness.evaluate(&ready_input());
    assert!(!report.ready);
    assert!(
        report.missing.iter().any(|m| m.contains("熔断")),
        "应标记熔断器: {:?}",
        report.missing
    );
}

/// 解冻范围全冻结（无目标）→ not_ready
#[test]
fn frozen_scope_blocks() {
    let governor = UnfreezeGovernor::new(UnfreezeScope::frozen());
    let readiness = R2UnfreezeReadiness::new(
        ShadowModeCircuitBreaker::new(),
        governor,
        Stage3Prerequisites::default(),
    );
    let report = readiness.evaluate(&ready_input());
    assert!(!report.ready);
    assert!(
        report.missing.iter().any(|m| m.contains("范围")),
        "应标记解冻范围: {:?}",
        report.missing
    );
}

/// E2E 未绿 → not_ready
#[test]
fn e2e_not_passed_blocks() {
    let readiness = ready_assembly();
    let mut input = ready_input();
    input.r2_freeze_e2e_passed = false;
    let report = readiness.evaluate(&input);
    assert!(!report.ready);
    assert!(
        report
            .missing
            .iter()
            .any(|m| m.contains("e2e") || m.contains("E2E")),
        "应标记 e2e: {:?}",
        report.missing
    );
}

/// FormalVerifier 未全绿 → not_ready
#[test]
fn formal_verifier_not_green_blocks() {
    let readiness = ready_assembly();
    let mut input = ready_input();
    input.formal_verifier_all_green = false;
    let report = readiness.evaluate(&input);
    assert!(!report.ready);
    assert!(
        report.missing.iter().any(|m| m.contains("Formal")),
        "应标记 FormalVerifier: {:?}",
        report.missing
    );
}
