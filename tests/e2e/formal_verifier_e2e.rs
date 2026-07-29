//! T6-7: FormalVerifier 端到端闭环测试
//!
//! 对应架构层: L4 FormalVerifier + L0 Contracts(formal_props 类型)
//! 对应 ADR: ADR-050(AEGIS-lite)/ ADR-051(变体治理)/ ADR-044(谱系追踪)
//!
//! # 测试覆盖(属性定义 → 验证执行 → 结果报告 全链路闭环)
//!
//! 1. **属性定义闭环**: InvariantSpec → FormalProperty → 验证 → 结果更新
//! 2. **GSOE 谱系 DAG 验证**: 有效 DAG → LineageChecker → Satisfied
//! 3. **GSOE 谱系环检测**: 含环图 → LineageChecker → Violated(含反例)
//! 4. **AEGIS Critic 单调性**: 正常递增序列 → CriticMonotonicityChecker → Satisfied
//! 5. **AEGIS 奖励欺骗检测**: 异常序列 → CriticMonotonicityChecker → Violated
//! 6. **Parliament Security 否决不可覆盖**: 否决 + 全票 → ConsensusSafetyChecker → Violated
//! 7. **Parliament 2/3 阈值边界**: 恰好 2/3 vs 不足 2/3 → 阈值正确性
//! 8. **综合管线**: 三个验证器同时运行 → 汇总 VerificationReport → 全部 Satisfied

use gsoe_evolution::formal::critic_monotonicity::CriticMonotonicityChecker;
use gsoe_evolution::formal::lineage_checker::{
    LineageChecker, LineageEdge, LineageGraph, LineageNode,
};
use nexus_contracts::formal_props::{
    FormalProperty, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
};
use parliament::formal::consensus_safety::{
    ConsensusOutcome, ConsensusSafetyChecker, SecurityVote,
};

// ────────────────────────────────────────────────────────────
// (a) 属性定义闭环
// ────────────────────────────────────────────────────────────

/// 属性定义生命周期闭环: InvariantSpec → FormalProperty → 验证执行 → 结果报告
///
/// WHY 这是最基础的闭环: 所有形式化验证都遵循 "定义属性 → 执行验证 → 记录结果"
/// 的三步流程。此测试确保 L0 契约层的类型能正确串联整个流程。
#[test]
fn test_formal_property_lifecycle() {
    // Step 1: 定义不变量规格
    let spec = InvariantSpec::new(
        "inv-lineage-dag-e2e",
        "谱系图必须保持 DAG 性质（无有向环）",
        PropertyCategory::LineageIntegrity,
        "gsoe-evolution",
        VerificationMethod::ManualProof,
    );

    // Step 2: 包装为形式化属性（初始无验证结果）
    let mut prop = FormalProperty::new(spec);
    assert!(!prop.is_verified(), "未执行验证前 is_verified 应为 false");
    assert!(prop.last_result.is_none(), "初始 last_result 应为 None");

    // Step 3: 执行验证（构造有效 DAG 并用 LineageChecker 验证）
    let dag = LineageGraph::with_nodes_and_edges(
        vec![
            LineageNode::new("spec@v1"),
            LineageNode::new("spec@v2"),
            LineageNode::new("spec@v3"),
        ],
        vec![
            LineageEdge::new("spec@v1", "spec@v2"),
            LineageEdge::new("spec@v2", "spec@v3"),
        ],
    );
    let result = LineageChecker::verify_dag_property(&dag);
    assert!(result.is_satisfied(), "有效 DAG 应通过验证");

    // Step 4: 更新验证结果
    prop = prop.with_result(result);
    assert!(prop.is_verified(), "验证通过后 is_verified 应为 true");
    assert_eq!(prop.spec.category, PropertyCategory::LineageIntegrity);
}

// ────────────────────────────────────────────────────────────
// (b) GSOE 谱系验证闭环
// ────────────────────────────────────────────────────────────

/// 谱系 DAG 验证闭环: 构造有效 DAG → LineageChecker → Satisfied
///
/// WHY DAG 性质是谱系追踪的核心安全保证(ADR-044):
/// 若谱系图含环，SpecRegistry 的回滚操作可能陷入无限循环，
/// 版本晋升/回滚的语义将完全崩溃。
#[test]
fn test_lineage_dag_verification() {
    // 构造菱形 DAG: v1 → v2, v1 → v3, v2 → v4, v3 → v4
    let graph = LineageGraph::with_nodes_and_edges(
        vec![
            LineageNode::new("v1"),
            LineageNode::new("v2"),
            LineageNode::new("v3"),
            LineageNode::new("v4"),
        ],
        vec![
            LineageEdge::new("v1", "v2"),
            LineageEdge::new("v1", "v3"),
            LineageEdge::new("v2", "v4"),
            LineageEdge::new("v3", "v4"),
        ],
    );

    let result = LineageChecker::verify_dag_property(&graph);
    assert!(result.is_satisfied(), "菱形 DAG 应通过验证");

    // 同时验证回滚可达性: v4 应可回滚到 v1
    let rollback = LineageChecker::verify_rollback_reachability(&graph, "v4", "v1");
    assert!(rollback.is_satisfied(), "v4 应可通过回滚到达 v1");
}

/// 谱系环检测闭环: 构造含环图 → LineageChecker → Violated(含反例)
///
/// WHY 环检测是谱系完整性的底线保证:
/// 一旦谱系图出现环（如 v1 → v2 → v3 → v1），版本管理将完全失效，
/// 回滚操作会在环中无限循环。
#[test]
fn test_lineage_cycle_detection() {
    // 构造含环图: v1 → v2 → v3 → v1
    let graph = LineageGraph::with_nodes_and_edges(
        vec![
            LineageNode::new("v1"),
            LineageNode::new("v2"),
            LineageNode::new("v3"),
        ],
        vec![
            LineageEdge::new("v1", "v2"),
            LineageEdge::new("v2", "v3"),
            LineageEdge::new("v3", "v1"), // 形成环
        ],
    );

    let result = LineageChecker::verify_dag_property(&graph);
    assert!(result.is_violated(), "含环图应返回 Violated");

    // 反例应包含环路径描述
    if let VerificationResult::Violated { counterexample, .. } = &result {
        assert!(
            counterexample.contains("有向环"),
            "反例应描述检测到的有向环，实际: {counterexample}"
        );
    }
}

// ────────────────────────────────────────────────────────────
// (c) AEGIS Critic 单调性闭环
// ────────────────────────────────────────────────────────────

/// Critic 单调性满足闭环: 正常递增序列 → CriticMonotonicityChecker → Satisfied
///
/// WHY 单调性是 AEGIS Critic 的核心安全属性(ADR-050 决策 4):
/// 适应度提升时 Critic 评分不能下降，否则进化方向将产生矛盾信号，
/// 导致 GSOE 进化引擎在"改善"与"退化"之间振荡。
#[test]
fn test_critic_monotonicity_satisfied() {
    let checker = CriticMonotonicityChecker::new();

    // 适应度单调递增，评分也单调递增
    let fitness = vec![0.1, 0.3, 0.5, 0.7, 0.9];
    let scores = vec![0.05, 0.15, 0.25, 0.35, 0.45];

    let result = checker.verify_monotonicity(&fitness, &scores);
    assert!(
        result.is_satisfied(),
        "适应度与评分均单调递增，应返回 Satisfied"
    );

    // 同时验证评分有界性
    let bounded = checker.verify_score_bounded(&scores, 0.0, 1.0);
    assert!(bounded.is_satisfied(), "评分应在 [0, 1] 范围内");

    // 验证无奖励黑客: tolerance = 2.0，评分增量/适应度增量 ≤ 1.0 < 2.0
    let anti_hack = checker.verify_no_reward_hacking(&fitness, &scores, 2.0);
    assert!(
        anti_hack.is_satisfied(),
        "评分提升不应超过适应度提升的 tolerance 倍"
    );
}

/// 奖励欺骗检测闭环: 异常序列 → CriticMonotonicityChecker → Violated
///
/// WHY 奖励欺骗检测是防止进化系统被"游戏化"的最后防线:
/// 若 Critic 评分可以被微小适应度提升操纵为巨大暴涨，
/// 进化引擎将学会"刷分"而非真正改善系统行为。
#[test]
fn test_critic_reward_hacking_detected() {
    let checker = CriticMonotonicityChecker::new();

    // 适应度微增（每步 +0.01），但评分暴涨（每步 +5.0）
    let fitness = vec![0.50, 0.51, 0.52, 0.53];
    let scores = vec![0.10, 5.10, 10.10, 15.10];

    // 单调性验证: 适应度不减但评分递增 → 单调性本身满足
    let mono = checker.verify_monotonicity(&fitness, &scores);
    assert!(mono.is_satisfied(), "适应度和评分均递增，单调性应满足");

    // 奖励黑客验证: 评分增量远超过适应度增量的 tolerance 倍 → 违反
    let anti_hack = checker.verify_no_reward_hacking(&fitness, &scores, 2.0);
    assert!(anti_hack.is_violated(), "评分暴涨应被奖励黑客检测捕获");

    // 反例应包含具体位置信息
    if let VerificationResult::Violated { counterexample, .. } = &anti_hack {
        assert!(
            counterexample.contains("位置"),
            "反例应包含违反位置描述，实际: {counterexample}"
        );
    }
}

// ────────────────────────────────────────────────────────────
// (d) Parliament 一致性闭环
// ────────────────────────────────────────────────────────────

/// Security 否决不可覆盖闭环: Security 否决 + 全票通过 → 验证否决必须生效
///
/// WHY 这是 Parliament 最核心的安全阀属性(ADR-051 决策 3):
/// Security 角色拥有"一票否决"权力，即使其他所有角色全票通过，
/// 只要 Security 识别到安全风险，决议就不能生效。
/// 这防止了"多数暴政"——多数角色联合通过危险决策。
#[test]
fn test_security_veto_immutable() {
    // Security 否决 + 全票通过 → 结果必须为 Vetoed（不可为 Reached）
    // verify_security_veto_immutable 检查: 若 Security=Veto 且 outcome=Reached → Violated
    let violated_result = ConsensusSafetyChecker::verify_security_veto_immutable(
        100, // 其他 100 票全部赞成
        SecurityVote::Veto,
        ConsensusOutcome::Reached, // 若结果为 Reached → 违反属性
    );
    assert!(
        violated_result.is_violated(),
        "Security 否决 + 结果为 Reached → 应检测到属性违反"
    );

    // 反例应包含否决信息
    if let VerificationResult::Violated { counterexample, .. } = &violated_result {
        assert!(
            counterexample.contains("Security") || counterexample.contains("否决"),
            "反例应提及 Security 否决，实际: {counterexample}"
        );
    }

    // 正确行为: Security 否决 → 结果为 Vetoed → Satisfied
    let satisfied_result = ConsensusSafetyChecker::verify_security_veto_immutable(
        100,
        SecurityVote::Veto,
        ConsensusOutcome::Vetoed,
    );
    assert!(
        satisfied_result.is_satisfied(),
        "Security 否决 + 结果为 Vetoed → 属性满足"
    );

    // 对照: Security 未否决 → 属性不适用（Skipped）
    let skipped_result = ConsensusSafetyChecker::verify_security_veto_immutable(
        10,
        SecurityVote::Approve,
        ConsensusOutcome::Reached,
    );
    assert!(
        skipped_result.is_skipped(),
        "Security 未否决时属性应 Skipped"
    );
}

/// 2/3 阈值边界闭环: 恰好 2/3 vs 不足 2/3 → 验证阈值精确性
///
/// WHY 2/3 超级多数阈值(ADR-051 决策 3)确保决议具有广泛共识:
/// 简单多数(>50%)可能通过争议性决策，2/3 阈值要求至少三分之二的角色
/// 达成一致，降低"刚好过半"的冒险决策风险。
#[test]
fn test_two_thirds_threshold_boundary() {
    // 恰好 2/3: 4/6 = 66.67% → 结果 Reached → Satisfied
    let exact_result =
        ConsensusSafetyChecker::verify_two_thirds_threshold(4, 6, ConsensusOutcome::Reached);
    assert!(
        exact_result.is_satisfied(),
        "恰好 2/3 (4/6) + Reached → 应 Satisfied"
    );

    // 不足 2/3: 3/5 = 60% → 结果 Reached → Violated（不应通过却通过了）
    let below_result =
        ConsensusSafetyChecker::verify_two_thirds_threshold(3, 5, ConsensusOutcome::Reached);
    assert!(
        below_result.is_violated(),
        "不足 2/3 (3/5 = 60%) + Reached → 应 Violated"
    );

    // 反例应包含阈值信息
    if let VerificationResult::Violated { counterexample, .. } = &below_result {
        assert!(
            counterexample.contains("2/3") || counterexample.contains("阈值"),
            "反例应包含阈值信息，实际: {counterexample}"
        );
    }

    // 边界: 99/150 = 66% < 2/3 → 结果 Reached → Violated
    let large_below =
        ConsensusSafetyChecker::verify_two_thirds_threshold(99, 150, ConsensusOutcome::Reached);
    assert!(
        large_below.is_violated(),
        "99/150 = 66% < 2/3 + Reached → 应 Violated"
    );

    // 边界: 100/150 = 66.67% = 2/3 → 结果 Reached → Satisfied
    let large_exact =
        ConsensusSafetyChecker::verify_two_thirds_threshold(100, 150, ConsensusOutcome::Reached);
    assert!(
        large_exact.is_satisfied(),
        "100/150 = 2/3 + Reached → 应 Satisfied"
    );

    // 不足 2/3 + 正确拒绝 → Satisfied
    let correct_reject =
        ConsensusSafetyChecker::verify_two_thirds_threshold(3, 5, ConsensusOutcome::Rejected);
    assert!(
        correct_reject.is_satisfied(),
        "不足 2/3 + Rejected → 正确拒绝应 Satisfied"
    );
}

// ────────────────────────────────────────────────────────────
// (e) 综合闭环
// ────────────────────────────────────────────────────────────

/// 综合验证管线闭环: 三个验证器同时运行 → 汇总报告 → 全部 Satisfied
///
/// WHY 综合管线测试模拟真实 FormalVerifier 的运行场景:
/// 实际部署中，FormalVerifier 需要同时运行所有验证器（谱系、单调性、共识），
/// 并将结果汇总为统一报告。此测试验证三个验证器可以无冲突地并行运行，
/// 且所有属性同时满足时报告为全绿。
#[test]
fn test_full_verification_pipeline() {
    // ── 定义三个形式化属性 ──
    let lineage_prop = FormalProperty::new(InvariantSpec::new(
        "inv-lineage-dag",
        "谱系图必须保持 DAG 性质",
        PropertyCategory::LineageIntegrity,
        "gsoe-evolution",
        VerificationMethod::Hybrid,
    ));
    let monotonicity_prop = FormalProperty::new(InvariantSpec::new(
        "inv-score-monotone",
        "Critic 评分单调不减",
        PropertyCategory::ScoreMonotonicity,
        "gsoe-evolution",
        VerificationMethod::PropTest,
    ));
    let consensus_prop = FormalProperty::new(InvariantSpec::new(
        "inv-consensus-threshold",
        "共识决议满足 2/3 阈值",
        PropertyCategory::ConsensusSafety,
        "parliament",
        VerificationMethod::ManualProof,
    ));

    // ── 执行三个验证器 ──

    // 1. 谱系验证: 有效 DAG
    let dag = LineageGraph::with_nodes_and_edges(
        vec![
            LineageNode::new("a@v1"),
            LineageNode::new("a@v2"),
            LineageNode::new("a@v3"),
        ],
        vec![
            LineageEdge::new("a@v1", "a@v2"),
            LineageEdge::new("a@v2", "a@v3"),
        ],
    );
    let lineage_result = LineageChecker::verify_dag_property(&dag);

    // 2. 单调性验证: 正常递增序列
    let critic_checker = CriticMonotonicityChecker::new();
    let fitness = vec![0.2, 0.4, 0.6, 0.8];
    let scores = vec![0.1, 0.2, 0.3, 0.4];
    let mono_result = critic_checker.verify_monotonicity(&fitness, &scores);

    // 3. 共识安全性验证: 满足 2/3 阈值 + 正确通过
    let consensus_result = ConsensusSafetyChecker::verify_two_thirds_threshold(
        7, // yes_votes
        9, // total_votes (7/9 = 77.8% > 2/3)
        ConsensusOutcome::Reached,
    );

    // ── 汇总为验证报告 ──
    let lineage_prop = lineage_prop.with_result(lineage_result);
    let monotonicity_prop = monotonicity_prop.with_result(mono_result);
    let consensus_prop = consensus_prop.with_result(consensus_result);

    let all_properties = vec![&lineage_prop, &monotonicity_prop, &consensus_prop];

    // 所有属性应验证通过
    for prop in &all_properties {
        assert!(prop.is_verified(), "属性 '{}' 应验证通过", prop.spec.id);
    }

    // 序列化验证报告（确保可持久化/传输）
    let report_json = serde_json::to_string(
        &all_properties
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.spec.id,
                    "category": p.spec.category,
                    "verified": p.is_verified(),
                })
            })
            .collect::<Vec<_>>(),
    )
    .expect("报告序列化不应失败");

    assert!(report_json.contains("inv-lineage-dag"));
    assert!(report_json.contains("inv-score-monotone"));
    assert!(report_json.contains("inv-consensus-threshold"));
}
