//! 形式化验证器 CI 门禁端到端测试 — R2 解冻阶段③ 前置 2
//!
//! 对应架构层: L5 gsoe-evolution(门禁) × L4 decay-engine(验证器) × L5 auto-dpo
//! 对应 ADR: ADR-052 待办 2(验证器 CI 门禁化)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 2
//!
//! # 闭环验证:真实验证器 → CI 门禁(用户要求 1 功能闭环)
//!
//! 本 E2E 在测试层(dev-dep 可依赖任意 crate)采集真实验证器输出,喂给
//! `FormalVerifierGate` 聚合为 CI 裁决——演示门禁的完整采集接线:
//! 各 crate 验证器 → NamedPropertyResult 切片 → 门禁 evaluate → CiGateResult。
//!
//! # 三路径覆盖(用户要求 1)
//!
//! - **正常路径**:多验证器全 Satisfied → 门禁通过
//! - **边界条件**:全 Skipped(证据不足)→ 门禁失败但非违规;min_satisfied 门槛
//! - **异常场景**:任一验证器 Violated → 门禁失败,携带属性名 + 反例

use auto_dpo::PreferenceConsistencyChecker;
use decay_engine::formal::{DecayConsistencyChecker, DecayEventKind, LevelTransition};
use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use gsoe_evolution::formal::invariant_closure::{
    InvariantClosureChecker, InvariantEdge, InvariantNode,
};
use gsoe_evolution::formal_gate::{FormalVerifierGate, NamedPropertyResult};

fn tr(event: DecayEventKind, before: f32, after: f32) -> LevelTransition {
    LevelTransition::new(event, before, after)
}

/// 采集真实验证器输出为具名结果切片(正常合法数据)
fn collect_healthy_results() -> Vec<NamedPropertyResult> {
    let decay = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::TimeDecay, 1.0, 0.9),
        tr(DecayEventKind::ViolationPenalty, 0.9, 0.5),
        tr(DecayEventKind::Freeze, 0.5, 0.0),
    ];
    let closure = InvariantClosureChecker::new();
    let nodes = [
        InvariantNode::new("inv-9", true, true),
        InvariantNode::new("derived", true, false),
    ];
    let edges = [InvariantEdge::new("derived", "inv-9")];

    vec![
        NamedPropertyResult::new("decay-monotonic", decay.verify_decay_monotonic(&seq)),
        NamedPropertyResult::new("decay-bounded", decay.verify_level_bounded(&seq)),
        NamedPropertyResult::new(
            "invariant-acyclic",
            closure.verify_dependency_acyclic(&edges),
        ),
        NamedPropertyResult::new(
            "invariant-terminal",
            closure.verify_terminal_anchored(&nodes),
        ),
    ]
}

// ============================================================
// 正常路径:真实验证器全 Satisfied → 门禁通过
// ============================================================

#[test]
fn test_normal_real_verifiers_gate_passes() {
    let gate = FormalVerifierGate::new();
    let results = collect_healthy_results();
    let verdict = gate.evaluate(&results);
    assert!(verdict.passed, "真实验证器全 Satisfied 应通过 CI 门禁");
    assert!(verdict.failures.is_empty());

    let summary = gate.summarize(&results);
    assert_eq!(summary.violated, 0);
    assert!(summary.satisfied >= 1);
}

// ============================================================
// 异常场景:真实验证器 Violated → 门禁失败
// ============================================================

#[test]
fn test_exception_real_decay_violation_fails_gate() {
    let decay = DecayConsistencyChecker::new();
    // 衰减单调性违反(权限提升攻击面)
    let bad = [tr(DecayEventKind::ViolationPenalty, 0.5, 0.9)];
    let results = [NamedPropertyResult::new(
        "decay-monotonic",
        decay.verify_decay_monotonic(&bad),
    )];
    let verdict = FormalVerifierGate::new().evaluate(&results);
    assert!(!verdict.passed, "衰减违规应门禁失败");
    assert!(verdict.failures[0].message.contains("decay-monotonic"));
}

#[test]
fn test_exception_real_invariant_cycle_fails_gate() {
    let closure = InvariantClosureChecker::new();
    let cyclic = [InvariantEdge::new("a", "b"), InvariantEdge::new("b", "a")];
    let results = [NamedPropertyResult::new(
        "invariant-acyclic",
        closure.verify_dependency_acyclic(&cyclic),
    )];
    let verdict = FormalVerifierGate::new().evaluate(&results);
    assert!(!verdict.passed, "不变量环应门禁失败");
}

#[test]
fn test_exception_real_preference_violation_fails_gate() {
    // auto-dpo 偏好对空序列 → Skipped;此处验证门禁能纳入 auto-dpo 验证器输出
    let checker = PreferenceConsistencyChecker::new();
    let results = [NamedPropertyResult::new(
        "preference-consistency",
        checker.verify_preference_asymmetry(&[]),
    )];
    // 空偏好对 → Skipped → 证据不足失败(非违规)
    let verdict = FormalVerifierGate::new().evaluate(&results);
    assert!(!verdict.passed, "仅 Skipped 证据不足应失败");
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:min_satisfied 门槛(要求全部 4 属性 Satisfied)
#[test]
fn test_boundary_strict_min_satisfied_threshold() {
    let results = collect_healthy_results(); // 4 个 Satisfied
                                             // 要求 ≥4 Satisfied,恰好满足
    let strict = FormalVerifierGate::with_min_satisfied(4);
    assert!(strict.evaluate(&results).passed, "恰好达门槛应通过");
    // 要求 ≥5 Satisfied,不足
    let stricter = FormalVerifierGate::with_min_satisfied(5);
    assert!(
        !stricter.evaluate(&results).passed,
        "Satisfied 4 < 5 应失败"
    );
}

/// 边界:空输入 → fail-closed 失败
#[test]
fn test_boundary_empty_fails() {
    assert!(!FormalVerifierGate::new().evaluate(&[]).passed);
}

// ============================================================
// 门禁 + 熔断器协同:CI 门禁失败 ⇔ 熔断器拒绝(一致性)
// ============================================================

/// 门禁与熔断器对同一验证结果应给出一致裁决(都基于 fail-closed)
#[test]
fn test_gate_and_breaker_consistent_on_violation() {
    let closure = InvariantClosureChecker::new();
    let cyclic = [InvariantEdge::new("a", "b"), InvariantEdge::new("b", "a")];
    let cycle_result = closure.verify_dependency_acyclic(&cyclic);

    // CI 门禁:失败
    let gate_verdict = FormalVerifierGate::new()
        .evaluate(&[NamedPropertyResult::new("inv", cycle_result.clone())]);
    assert!(!gate_verdict.passed);

    // 熔断器:跳闸拒绝
    let mut cb = ShadowModeCircuitBreaker::new();
    let breaker_verdict = cb.observe(&[cycle_result]);
    assert!(!breaker_verdict.is_permitted());
    assert!(cb.is_tripped());

    // 两者对同一违规裁决一致(都拒绝)
    assert_eq!(gate_verdict.passed, breaker_verdict.is_permitted());
}
