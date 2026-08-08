//! SkillGraph 安全约束接口测试 — 悬空依赖 / 循环依赖检测（Milestone B-3a）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P2 / §7.2 九层防御 L5 补齐）：
//! SkillGraph 无显式安全约束（仅 Blueprint validate_plan 覆盖）→ 补齐安全约束接口，
//! 防止技能图出现悬空依赖（依赖的技能不存在）与循环依赖（A→B→A 执行死锁）。

#![forbid(unsafe_code)]

use nexus_core::CLV;
use repo_wiki::skill_graph::{SkillGraph, SkillUsagePattern};

/// 构造技能使用模式（sequence 相邻技能建依赖边：后者依赖前者）
fn pattern(skill_id: &str, sequence: Vec<&str>, frequency: u32) -> SkillUsagePattern {
    SkillUsagePattern {
        skill_id: skill_id.into(),
        embedding: CLV::zero(),
        frequency,
        success_rate: 0.8,
        sequence: sequence.into_iter().map(String::from).collect(),
    }
}

/// 入图三技能（频率 ≥3 阈值）
/// 依赖建边规则：sequence 相邻对 [X, Y] → Y 依赖 X（windows(2)）
fn build_graph() -> SkillGraph {
    let mut graph = SkillGraph::new();
    graph.evolve_with_patterns(&[
        pattern("skill-a", vec![], 3),
        pattern("skill-b", vec!["skill-a", "skill-b"], 3), // b 依赖 a
        pattern("skill-c", vec!["skill-b", "skill-c"], 3), // c 依赖 b
    ]);
    graph
}

/// 安全图：无悬空依赖、无循环 → 零违规
#[test]
fn healthy_graph_has_no_violations() {
    let graph = build_graph();
    let violations = graph.check_security();
    assert!(violations.is_empty(), "健康图不应有违规: {violations:?}");
}

/// 悬空依赖：依赖的技能不存在于图中 → 检出
#[test]
fn dangling_dependency_detected() {
    let mut graph = build_graph();
    // 注入悬空依赖：sequence=[ghost-skill, skill-a] → skill-a 依赖 ghost-skill
    graph.evolve_with_patterns(&[pattern("skill-a", vec!["ghost-skill", "skill-a"], 3)]);
    let violations = graph.check_security();
    assert!(
        violations
            .iter()
            .any(|v| v.skill_id == "skill-a" && v.reason.contains("ghost-skill")),
        "应检出 skill-a 的悬空依赖: {violations:?}"
    );
}

/// 循环依赖：a → b → a → 检出
#[test]
fn cyclic_dependency_detected() {
    let mut graph = build_graph();
    // 注入反向依赖：sequence=[skill-c, skill-a] → skill-a 依赖 skill-c（c → b → a → c 成环）
    graph.evolve_with_patterns(&[pattern("skill-a", vec!["skill-c", "skill-a"], 3)]);
    let violations = graph.check_security();
    assert!(
        violations
            .iter()
            .any(|v| v.reason.contains("cycle") || v.reason.contains("循环")),
        "应检出循环依赖: {violations:?}"
    );
}

/// 自依赖：技能依赖自身 → 检出
#[test]
fn self_dependency_detected() {
    let mut graph = build_graph();
    // sequence=[skill-b, skill-b] → skill-b 依赖 skill-b
    graph.evolve_with_patterns(&[pattern("skill-b", vec!["skill-b", "skill-b"], 3)]);
    let violations = graph.check_security();
    assert!(
        violations.iter().any(|v| v.skill_id == "skill-b"),
        "应检出 skill-b 自依赖: {violations:?}"
    );
}

/// 安全约束在推荐前的闸门语义：违规时 recommend 仍可用但调用方可先查 check_security
#[test]
fn security_check_is_queryable_before_recommend() {
    let graph = build_graph();
    // 接口语义：check_security 是纯查询（&self），不改变图状态
    let before = graph.len();
    let _ = graph.check_security();
    assert_eq!(graph.len(), before, "check_security 不应改变图");
}

/// 菱形依赖（a→b, a→c, b→d, c→d）不是环 → 不误报
#[test]
fn diamond_dependency_is_not_cycle() {
    let mut graph = SkillGraph::new();
    graph.evolve_with_patterns(&[
        pattern("d", vec![], 3),
        pattern("b", vec!["d", "b"], 3),
        pattern("c", vec!["d", "c"], 3),
        pattern("a", vec!["b", "a"], 3),
        pattern("a", vec!["c", "a"], 3),
    ]);
    let violations = graph.check_security();
    assert!(
        violations.is_empty(),
        "菱形依赖不应误报为环: {violations:?}"
    );
}
