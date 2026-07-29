//! FormalVerifier M2 端到端闭环测试(Phase 8.1)
//!
//! 对应架构层: L4 FormalVerifier + L0 Contracts(formal_props 类型)
//! 对应 ADR: ADR-047(M2:属性 #6 decay 衰减一致性)+ ADR-042(R2 解冻阶段①)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` Phase 8.1
//!
//! # M2 属性矩阵(在 M1 五属性基础上新增属性 #6 与 #7)
//!
//! | # | 属性 | 验证器 | 落点 | 里程碑 |
//! |---|------|--------|------|--------|
//! | 1 | GSOE 谱系 DAG 完整性 | LineageChecker | gsoe-evolution | M0 |
//! | 2 | AEGIS Critic 单调性 | CriticMonotonicityChecker | gsoe-evolution | M0 |
//! | 3 | AutoDPO 偏好对一致性 | PreferenceConsistencyChecker | auto-dpo | M1 |
//! | 4 | 事件因果一致性 | CausalConsistencyChecker | event-bus | M1 |
//! | 5 | 学习单调性 | LearningMonotonicityChecker | omega-learner | M1 |
//! | **6** | **能力衰减一致性** | **DecayConsistencyChecker** | **decay-engine** | **M2** |
//! | **7** | **全系统不变量传递闭包** | **InvariantClosureChecker** | **gsoe-evolution** | **M2** |
//!
//! # 测试覆盖
//!
//! 1. 属性 #6 三性质正反路径(单调性/有界性/Freeze 归零)
//! 2. 真实 DecayEngine 轨迹 → 属性 #6 验证(引擎与验证器的真实集成)
//! 3. 属性 #7 三性质正反路径(依赖无环/满足传播/终端锚点)
//! 4. M2 综合管线:属性 #6 + #7 定义 → 各验证器执行 → 汇总全 Satisfied

use decay_engine::formal::{DecayConsistencyChecker, DecayEventKind, LevelTransition};
use decay_engine::{DecayConfig, DecayEngine, DecayEvent};
use gsoe_evolution::formal::invariant_closure::{
    InvariantClosureChecker, InvariantEdge, InvariantNode,
};
use nexus_contracts::formal_props::{
    InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
};

fn tr(event: DecayEventKind, before: f32, after: f32) -> LevelTransition {
    LevelTransition::new(event, before, after)
}

// ============================================================
// 属性 #6:能力衰减一致性(M2 新增)三性质正反路径
// ============================================================

#[test]
fn test_property6_decay_monotonic_satisfied() {
    let checker = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::TimeDecay, 1.0, 0.85),
        tr(DecayEventKind::ViolationPenalty, 0.85, 0.4),
    ];
    assert!(checker.verify_decay_monotonic(&seq).is_satisfied());
}

#[test]
fn test_property6_decay_increase_violated_with_counterexample() {
    let checker = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::TimeDecay, 0.8, 0.7),
        tr(DecayEventKind::ViolationPenalty, 0.7, 0.95), // 权限提升攻击面
    ];
    match checker.verify_decay_monotonic(&seq) {
        VerificationResult::Violated {
            counterexample,
            samples_tested,
        } => {
            assert!(counterexample.contains("上升"));
            assert_eq!(samples_tested, 2);
        }
        other => panic!("期望 Violated,实际: {other:?}"),
    }
}

#[test]
fn test_property6_freeze_zero_irreversible_satisfied() {
    let checker = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::Freeze, 0.6, 0.0),
        tr(DecayEventKind::ViolationPenalty, 0.0, 0.0),
        tr(DecayEventKind::Restore, 0.0, 0.3),
    ];
    assert!(checker.verify_freeze_zero_irreversible(&seq).is_satisfied());
}

#[test]
fn test_property6_frozen_residual_violated() {
    let checker = DecayConsistencyChecker::new();
    // 冻结区间内权限残留(泄漏)
    let seq = [
        tr(DecayEventKind::Freeze, 0.6, 0.0),
        tr(DecayEventKind::TimeDecay, 0.0, 0.15),
    ];
    assert!(matches!(
        checker.verify_freeze_zero_irreversible(&seq),
        VerificationResult::Violated { .. }
    ));
}

// ============================================================
// 真实 DecayEngine 轨迹集成验证
// ============================================================

/// 从真实 DecayEngine 采集迁移轨迹并验证属性 #6
///
/// WHY: 验证器与引擎的真实集成——不用手构造的迁移,而是驱动真实引擎
/// 产生 before/after 快照,确认引擎实际行为满足形式化不变量。
#[test]
fn test_property6_real_engine_trajectory() {
    let engine = DecayEngine::new(DecayConfig::default());
    engine
        .register_capability("shell_exec", "shell 执行", 1.0)
        .expect("注册能力失败");

    let mut transitions: Vec<LevelTransition> = Vec::new();

    // 连续 3 次违规惩罚:每次记录 before/after
    for _ in 0..3 {
        let before = engine.get_level("shell_exec").expect("查询失败").value();
        let after = engine
            .decay(
                "shell_exec",
                DecayEvent::ViolationPenalty {
                    capability_id: "shell_exec".into(),
                    severity: 1.0,
                },
            )
            .expect("衰减失败")
            .value();
        transitions.push(tr(DecayEventKind::ViolationPenalty, before, after));
    }

    // 显式冻结
    let before_freeze = engine.get_level("shell_exec").expect("查询失败").value();
    engine.freeze("shell_exec", "test freeze").ok(); // 可能已自动冻结
    let after_freeze = engine.get_level("shell_exec").expect("查询失败").value();
    transitions.push(tr(DecayEventKind::Freeze, before_freeze, after_freeze));

    let checker = DecayConsistencyChecker::new();
    // 真实引擎轨迹必须满足三性质
    assert!(
        checker.verify_decay_monotonic(&transitions).is_satisfied(),
        "真实引擎衰减轨迹违反单调性: {transitions:?}"
    );
    assert!(
        checker.verify_level_bounded(&transitions).is_satisfied(),
        "真实引擎 level 越界: {transitions:?}"
    );
    assert!(
        checker
            .verify_freeze_zero_irreversible(&transitions)
            .is_satisfied(),
        "真实引擎 Freeze 归零违反: {transitions:?}"
    );
}

// ============================================================
// 属性 #7:全系统不变量传递闭包(M2 新增)三性质正反路径
// ============================================================

fn inv(id: &str, satisfied: bool, terminal: bool) -> InvariantNode {
    InvariantNode::new(id, satisfied, terminal)
}

fn dep(dependent: &str, prerequisite: &str) -> InvariantEdge {
    InvariantEdge::new(dependent, prerequisite)
}

#[test]
fn test_property7_acyclic_satisfied() {
    let checker = InvariantClosureChecker::new();
    // 派生不变量依赖 INV-9(委托无环),INV-9 无进一步依赖 → 无环
    let edges = [dep("derived-evolution", "inv-9")];
    assert!(checker.verify_dependency_acyclic(&edges).is_satisfied());
}

#[test]
fn test_property7_cycle_violated() {
    let checker = InvariantClosureChecker::new();
    let edges = [dep("A", "B"), dep("B", "A")];
    assert!(matches!(
        checker.verify_dependency_acyclic(&edges),
        VerificationResult::Violated { .. }
    ));
}

#[test]
fn test_property7_propagation_broken_foundation_violated() {
    let checker = InvariantClosureChecker::new();
    // 派生不变量满足,但传递前置 INV-9 被违反 → 满足建立在被违反的地基上
    let nodes = [
        inv("derived-evolution", true, false),
        inv("inv-9", false, true),
    ];
    let edges = [dep("derived-evolution", "inv-9")];
    match checker.verify_satisfaction_propagation(&nodes, &edges) {
        VerificationResult::Violated { counterexample, .. } => {
            assert!(counterexample.contains("inv-9"));
        }
        other => panic!("期望 Violated,实际: {other:?}"),
    }
}

#[test]
fn test_property7_terminal_anchor_violated() {
    let checker = InvariantClosureChecker::new();
    // INV-7/8/9 三终端锚点,其中 INV-8 未满足 → 安全地基被违反
    let nodes = [
        inv("inv-7", true, true),
        inv("inv-8", false, true), // 归档单调性被违反
        inv("inv-9", true, true),
    ];
    assert!(matches!(
        checker.verify_terminal_anchored(&nodes),
        VerificationResult::Violated { .. }
    ));
}

// ============================================================
// M2 综合管线:属性 #6 + #7(定义 → 执行 → 汇总)
// ============================================================

#[test]
fn test_m2_pipeline_property6_and_7_satisfied() {
    // Step 1: 属性 #6 + #7 的 InvariantSpec 定义(L0 契约层)
    let specs = [
        InvariantSpec::new(
            "M2-P6",
            "能力衰减单调 + 有界 + Freeze 归零不可逆",
            PropertyCategory::InvariantPreservation,
            "decay-engine",
            VerificationMethod::PropTest,
        ),
        InvariantSpec::new(
            "M2-P7",
            "全系统不变量传递闭包:依赖无环 + 满足传播 + 终端锚点",
            PropertyCategory::InvariantPreservation,
            "gsoe-evolution",
            VerificationMethod::PropTest,
        ),
    ];
    assert_eq!(specs.len(), 2);

    // Step 2: 属性 #6 验证器执行(三性质合法序列)
    let decay = DecayConsistencyChecker::new();
    let seq = [
        tr(DecayEventKind::TimeDecay, 1.0, 0.9),
        tr(DecayEventKind::ViolationPenalty, 0.9, 0.5),
        tr(DecayEventKind::Freeze, 0.5, 0.0),
        tr(DecayEventKind::Restore, 0.0, 0.2),
    ];
    let p6 = [
        decay.verify_decay_monotonic(&seq),
        decay.verify_level_bounded(&seq),
        decay.verify_freeze_zero_irreversible(&seq),
    ];

    // Step 3: 属性 #7 验证器执行(合法不变量系统:INV-7/8/9 终端锚点全满足)
    let closure = InvariantClosureChecker::new();
    let nodes = [
        inv("inv-7", true, true),
        inv("inv-8", true, true),
        inv("inv-9", true, true),
        inv("derived-evolution", true, false),
    ];
    let edges = [
        dep("derived-evolution", "inv-9"),
        dep("derived-evolution", "inv-7"),
    ];
    let p7 = [
        closure.verify_dependency_acyclic(&edges),
        closure.verify_satisfaction_propagation(&nodes, &edges),
        closure.verify_terminal_anchored(&nodes),
    ];

    // Step 4: 汇总(属性 #6 三性质 + 属性 #7 三性质全 Satisfied = M2 两属性达成)
    let all: Vec<&VerificationResult> = p6.iter().chain(p7.iter()).collect();
    let satisfied = all.iter().filter(|r| r.is_satisfied()).count();
    assert_eq!(
        satisfied, 6,
        "M2 属性 #6+#7 要求六性质全通过,实际 {satisfied}/6"
    );
}
