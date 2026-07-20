//! 精英专家团队注册表集成测试 (§6 / ADR-027 决策 5)
//!
//! 覆盖:
//! - 注册表恰 8 位专家, 编号 E01-E08 唯一且有序
//! - 象限映射与设计文档 §6.1 完全一致
//! - `by_quadrant` 分组正确且并集覆盖全部专家
//! - 工具白名单非空、权限分级(L0/L1/L2)合法
//! - `has_tool` / `highest_tier` 行为

use chimera_mas::experts::{ExpertRegistry, PermissionTier};
use chimera_mas::quadrant::Quadrant;

// ============================================================
// 注册表规模与编号
// ============================================================

#[test]
fn test_registry_has_exactly_eight_experts() {
    let reg = ExpertRegistry::new();
    assert_eq!(reg.len(), 8);
    assert!(!reg.is_empty());
    assert_eq!(reg.all().len(), 8);
}

#[test]
fn test_expert_ids_e01_to_e08_ordered() {
    let reg = ExpertRegistry::new();
    let ids: Vec<&str> = reg.all().iter().map(|e| e.id).collect();
    assert_eq!(
        ids,
        ["E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08"]
    );
}

#[test]
fn test_default_equals_new() {
    assert_eq!(ExpertRegistry::default().len(), ExpertRegistry::new().len());
}

// ============================================================
// 象限映射 (§6.1)
// ============================================================

#[test]
fn test_quadrant_mapping_matches_design_6_1() {
    let reg = ExpertRegistry::new();
    let q = |id: &str| reg.get(id).unwrap().primary_quadrant;
    assert_eq!(q("E01"), Quadrant::Hardening);
    assert_eq!(q("E02"), Quadrant::Integration);
    assert_eq!(q("E03"), Quadrant::Implementation);
    assert_eq!(q("E04"), Quadrant::Verification);
    assert_eq!(q("E05"), Quadrant::Verification);
    assert_eq!(q("E06"), Quadrant::Hardening);
    assert_eq!(q("E07"), Quadrant::Hardening);
    assert_eq!(q("E08"), Quadrant::Hardening);
}

#[test]
fn test_by_quadrant_grouping() {
    let reg = ExpertRegistry::new();

    let ids = |q: Quadrant| -> Vec<&str> {
        let mut v: Vec<&str> = reg.by_quadrant(q).iter().map(|e| e.id).collect();
        v.sort_unstable();
        v
    };

    assert_eq!(ids(Quadrant::Implementation), ["E03"]);
    assert_eq!(ids(Quadrant::Integration), ["E02"]);
    assert_eq!(ids(Quadrant::Verification), ["E04", "E05"]);
    assert_eq!(ids(Quadrant::Hardening), ["E01", "E06", "E07", "E08"]);
}

#[test]
fn test_by_quadrant_union_covers_all_experts() {
    let reg = ExpertRegistry::new();
    let total: usize = Quadrant::ALL
        .iter()
        .map(|q| reg.by_quadrant(*q).len())
        .sum();
    assert_eq!(total, 8, "四象限分组并集应恰好覆盖 8 位专家");
}

// ============================================================
// 子代理类型标识 (§6.1)
// ============================================================

#[test]
fn test_sub_agent_types() {
    let reg = ExpertRegistry::new();
    assert_eq!(
        reg.get("E01").unwrap().sub_agent_type,
        "chimera-release-analyst"
    );
    assert_eq!(
        reg.get("E02").unwrap().sub_agent_type,
        "architecture-optimization-analyst"
    );
    assert_eq!(
        reg.get("E03").unwrap().sub_agent_type,
        "rust-architecture-expert"
    );
    assert_eq!(
        reg.get("E04").unwrap().sub_agent_type,
        "code-review-refactor-expert"
    );
}

#[test]
fn test_get_unknown_returns_none() {
    let reg = ExpertRegistry::new();
    assert!(reg.get("E99").is_none());
    assert!(reg.get("").is_none());
}

// ============================================================
// 工具白名单与权限分级 (§6.2 / §11.2)
// ============================================================

#[test]
fn test_every_expert_has_non_empty_toolset() {
    let reg = ExpertRegistry::new();
    for e in reg.all() {
        assert!(!e.tools.is_empty(), "{} 工具白名单不应为空", e.id);
    }
}

#[test]
fn test_permission_tier_ordering_and_label() {
    assert!(PermissionTier::HighRiskApproval > PermissionTier::LimitedWrite);
    assert!(PermissionTier::LimitedWrite > PermissionTier::ReadOnly);
    assert_eq!(PermissionTier::ReadOnly.label(), "L0");
    assert_eq!(PermissionTier::LimitedWrite.label(), "L1");
    assert_eq!(PermissionTier::HighRiskApproval.label(), "L2");
}

#[test]
fn test_highest_tier_reflects_whitelist() {
    let reg = ExpertRegistry::new();
    // E01 发布分析: 全只读 → L0
    assert_eq!(
        reg.get("E01").unwrap().highest_tier(),
        PermissionTier::ReadOnly
    );
    // E03 Rust 架构: 含 Edit / Bash(cargo) → L1
    assert_eq!(
        reg.get("E03").unwrap().highest_tier(),
        PermissionTier::LimitedWrite
    );
    // E08 DevOps: 含 Bash(docker/scripts) → L2
    assert_eq!(
        reg.get("E08").unwrap().highest_tier(),
        PermissionTier::HighRiskApproval
    );
}

#[test]
fn test_has_tool_prefix_match() {
    let reg = ExpertRegistry::new();
    let e03 = reg.get("E03").unwrap();
    assert!(e03.has_tool("Read"));
    assert!(e03.has_tool("Bash")); // 前缀匹配 "Bash(cargo)"
    assert!(!e03.has_tool("Agent(CodeReview)"));
}

#[test]
fn test_least_privilege_readonly_experts_have_no_write() {
    // E01/E02/E04 为分析/审查型, 不应含任何写权限(最小权限原则)
    let reg = ExpertRegistry::new();
    for id in ["E01", "E02", "E04"] {
        let e = reg.get(id).unwrap();
        assert_eq!(
            e.highest_tier(),
            PermissionTier::ReadOnly,
            "{id} 应为纯只读专家"
        );
    }
}
