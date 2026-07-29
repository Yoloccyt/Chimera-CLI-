//! 影子模式熔断开关端到端测试 — R2 解冻阶段③ 前置 3(Phase 9.1)
//!
//! 对应架构层: L4 decay-engine(熔断器 + 属性 #6 验证器)× L5 gsoe-evolution(属性 #7)
//! 对应 ADR: ADR-052 待办 3(影子模式回滚熔断)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 3
//!
//! # 闭环验证:熔断器与真实 FormalVerifier 协同(用户要求 1)
//!
//! 本 E2E 验证熔断器消费**真实验证器输出**而非手构造:
//! - 属性 #6 DecayConsistencyChecker(decay-engine)的真实 VerificationResult
//! - 属性 #7 InvariantClosureChecker(gsoe-evolution)的真实 VerificationResult
//! → 喂给 ShadowModeCircuitBreaker → 验证 fail-closed 门控行为
//!
//! # 三路径覆盖(用户要求 1)
//!
//! - **正常路径**:验证器全 Satisfied → 熔断器许可 RL 更新
//! - **边界条件**:全 Skipped(证据不足)→ 拒绝但不跳闸;空观测
//! - **异常场景**:任一 Violated → 永久跳闸,后续不可逆直至复位

use decay_engine::formal::{DecayConsistencyChecker, DecayEventKind, LevelTransition};
use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use gsoe_evolution::formal::invariant_closure::{
    InvariantClosureChecker, InvariantEdge, InvariantNode,
};

fn tr(event: DecayEventKind, before: f32, after: f32) -> LevelTransition {
    LevelTransition::new(event, before, after)
}

// ============================================================
// 正常路径:真实验证器全 Satisfied → 熔断器许可
// ============================================================

#[test]
fn test_normal_real_verifiers_satisfied_permits() {
    // 属性 #6:合法衰减序列 → 三性质 Satisfied
    let decay = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::TimeDecay, 1.0, 0.9),
        tr(DecayEventKind::ViolationPenalty, 0.9, 0.5),
        tr(DecayEventKind::Freeze, 0.5, 0.0),
    ];
    // 属性 #7:合法不变量系统 → Satisfied
    let closure = InvariantClosureChecker::new();
    let nodes = [
        InvariantNode::new("inv-9", true, true),
        InvariantNode::new("derived", true, false),
    ];
    let edges = [InvariantEdge::new("derived", "inv-9")];

    let results = vec![
        decay.verify_decay_monotonic(&seq),
        decay.verify_level_bounded(&seq),
        decay.verify_freeze_zero_irreversible(&seq),
        closure.verify_dependency_acyclic(&edges),
        closure.verify_satisfaction_propagation(&nodes, &edges),
        closure.verify_terminal_anchored(&nodes),
    ];

    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&results);
    assert!(
        verdict.is_permitted(),
        "真实验证器全 Satisfied 应许可 RL 更新"
    );
    assert!(!cb.is_tripped());
}

// ============================================================
// 异常场景:真实验证器 Violated → 永久跳闸
// ============================================================

#[test]
fn test_exception_real_decay_violation_trips() {
    // 属性 #6:衰减单调性被违反(权限提升攻击面)
    let decay = DecayConsistencyChecker::new();
    let bad_seq = [tr(DecayEventKind::ViolationPenalty, 0.5, 0.9)]; // 上升!
    let results = vec![decay.verify_decay_monotonic(&bad_seq)];

    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&results);
    assert!(!verdict.is_permitted(), "衰减违规应拒绝");
    assert!(cb.is_tripped(), "衰减违规应触发永久跳闸");
    assert!(cb.trip_cause().is_some());
}

#[test]
fn test_exception_real_invariant_cycle_trips() {
    // 属性 #7:不变量依赖环 → Violated
    let closure = InvariantClosureChecker::new();
    let cyclic = [InvariantEdge::new("a", "b"), InvariantEdge::new("b", "a")];
    let results = vec![closure.verify_dependency_acyclic(&cyclic)];

    let mut cb = ShadowModeCircuitBreaker::new();
    cb.observe(&results);
    assert!(cb.is_tripped(), "不变量环应触发跳闸");
}

/// 异常:跳闸后续观测全通过仍拒绝(不可逆),复位后恢复
#[test]
fn test_exception_trip_irreversible_then_reset_recovers() {
    let decay = DecayConsistencyChecker::new();
    let bad = [tr(DecayEventKind::TimeDecay, 0.3, 0.8)]; // 上升
    let good = [tr(DecayEventKind::TimeDecay, 1.0, 0.8)]; // 合法

    let mut cb = ShadowModeCircuitBreaker::new();
    cb.observe(&[decay.verify_decay_monotonic(&bad)]);
    assert!(cb.is_tripped());

    // 后续真实全通过仍拒绝
    let good_result = vec![decay.verify_decay_monotonic(&good)];
    assert!(
        !cb.observe(&good_result).is_permitted(),
        "跳闸不可逆:后续通过仍拒绝"
    );

    // 人工复位(模拟排查修复后)→ 恢复许可(评审 S-2.1:须携带授权凭证)
    cb.reset(
        decay_engine::shadow_breaker::ResetAuthorization::new("E01+E02", "跳闸根因已修复")
            .expect("授权凭证非空"),
    );
    assert!(cb.observe(&good_result).is_permitted(), "复位后应恢复许可");
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:验证器全 Skipped(无数据)→ 证据不足拒绝,不跳闸
#[test]
fn test_boundary_all_skipped_denies_no_trip() {
    let decay = DecayConsistencyChecker::new();
    // 空迁移序列 → 验证器返回 Skipped
    let skipped_results = vec![
        decay.verify_decay_monotonic(&[]),
        decay.verify_level_bounded(&[]),
    ];
    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&skipped_results);
    assert!(!verdict.is_permitted(), "证据不足应 fail-closed 拒绝");
    assert!(!cb.is_tripped(), "Skipped 非违规,不应跳闸");
}

/// 边界:空观测 → fail-closed 拒绝
#[test]
fn test_boundary_empty_observation_denies() {
    let mut cb = ShadowModeCircuitBreaker::new();
    assert!(!cb.observe(&[]).is_permitted(), "空观测 fail-closed 拒绝");
    assert!(!cb.is_tripped());
}

/// 边界:混合 Satisfied + Skipped(有正面证据无违规)→ 许可
#[test]
fn test_boundary_satisfied_with_skipped_permits() {
    let decay = DecayConsistencyChecker::new();
    let seq = [tr(DecayEventKind::TimeDecay, 1.0, 0.9)];
    // verify_decay_monotonic 有数据 Satisfied,verify_freeze 无 Freeze 事件 Skipped
    let results = vec![
        decay.verify_decay_monotonic(&seq),
        decay.verify_freeze_zero_irreversible(&seq),
    ];
    let mut cb = ShadowModeCircuitBreaker::new();
    assert!(
        cb.observe(&results).is_permitted(),
        "有正面证据且无违规应许可"
    );
}

/// 影子模式多周期观测:连续正常 → 持续许可,累计观测计数
#[test]
fn test_shadow_multi_cycle_observation() {
    let decay = DecayConsistencyChecker::new();
    let seq = [tr(DecayEventKind::TimeDecay, 1.0, 0.9)];
    let mut cb = ShadowModeCircuitBreaker::new();

    // 模拟影子模式连续 5 个观测周期,全部合法
    for _ in 0..5 {
        let verdict = cb.observe(&[decay.verify_decay_monotonic(&seq)]);
        assert!(verdict.is_permitted());
    }
    assert_eq!(cb.observations(), 5);
    assert!(!cb.is_tripped());
}
