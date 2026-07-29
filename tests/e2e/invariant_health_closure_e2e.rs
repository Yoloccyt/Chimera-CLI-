//! FormalVerifier M2 属性 #7 采集闭环端到端测试(Phase 8.4)
//!
//! 对应架构层: L9 chimera-mas(采集) × L5 gsoe-evolution(属性 #7 验证器) × L0 契约
//! 对应 ADR: ADR-047(M2 属性 #7 闭环)+ ADR-028(INV-7/8/9)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` Phase 8.4
//!
//! # 闭环验证:真实组件协同(用户要求 1 — 功能闭环)
//!
//! Phase 8.2 的属性 #7 验证器只验证抽象图;本 E2E 验证**真实采集接线**:
//! chimera-mas `InvariantChecker`(INV-7/8/9 真实检查)→ `InvariantHealthReporter`
//! 映射为节点 → gsoe-evolution `InvariantClosureChecker`(属性 #7)→ 健康判定。
//!
//! # 三路径覆盖(用户要求 1 — 正常/边界/异常)
//!
//! - **正常路径**:INV-7/8/9 全满足 → 系统健康
//! - **边界条件**:预算恰好达阈值 / 空快照(空真健康)
//! - **异常场景**:INV-7 超预算 / INV-8 反向归档 / INV-9 委托环 → 不健康

use chimera_mas::invariant_report::{
    Inv7Observation, Inv8Observation, InvariantHealthReporter, SystemInvariantSnapshot,
};
use chimera_mas::invariants::{ArchiveTier, DelegationEdge};

// ============================================================
// 正常路径:三不变量全满足 → 系统健康,属性 #7 三性质全 Satisfied
// ============================================================

#[test]
fn test_normal_path_all_invariants_satisfied() {
    let snapshot = SystemInvariantSnapshot::empty()
        .with_inv7(Inv7Observation {
            agent_resident: 100_000,
            effective_capacity: 128_000,
            m_total: 100,
            m_budget: 130,
        })
        .with_inv8(Inv8Observation {
            from_tier: ArchiveTier::Warm,
            to_tier: ArchiveTier::Cold,
        })
        .with_inv9_edges(vec![
            DelegationEdge::new("root", "main-1"),
            DelegationEdge::new("main-1", "sub-1"),
            DelegationEdge::new("main-1", "sub-2"),
        ]);

    let report = InvariantHealthReporter::new().build_report(&snapshot);

    assert!(report.is_healthy(), "全满足快照应健康");
    assert!(report.acyclic.is_satisfied(), "依赖图应无环");
    assert!(report.propagation.is_satisfied(), "满足传播应成立");
    assert!(report.terminal_anchored.is_satisfied(), "终端锚点应全满足");
    // inv-7/8/9 + inv-system-health 四节点,全 satisfied
    assert_eq!(report.nodes.len(), 4);
    assert!(report.nodes.iter().all(|n| n.satisfied));
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:INV-7 全局内存恰好达阈值(130×0.9=117)→ 等号允许 → 健康
#[test]
fn test_boundary_budget_exactly_at_threshold() {
    let snapshot = SystemInvariantSnapshot::empty().with_inv7(Inv7Observation {
        agent_resident: 128_000, // 恰好等于容量
        effective_capacity: 128_000,
        m_total: 117, // 恰好等于 130×0.9
        m_budget: 130,
    });
    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(report.is_healthy(), "恰好达阈值(等号)应允许");
    assert!(report.terminal_anchored.is_satisfied());
}

/// 边界:空快照 → 无可验证不变量 → 三验证器 Skipped → 空真健康
#[test]
fn test_boundary_empty_snapshot_vacuously_healthy() {
    let report = InvariantHealthReporter::new().build_report(&SystemInvariantSnapshot::empty());
    assert!(report.is_healthy(), "空快照空真健康");
    assert!(report.acyclic.is_skipped());
    assert!(report.propagation.is_skipped());
    assert!(report.terminal_anchored.is_skipped());
    assert!(report.nodes.is_empty());
}

/// 边界:仅单个不变量(INV-9 深度 5 链,匹配 MAX_AGENT_DEPTH)→ 健康
#[test]
fn test_boundary_single_invariant_depth5_chain() {
    let snapshot = SystemInvariantSnapshot::empty().with_inv9_edges(vec![
        DelegationEdge::new("root", "d1"),
        DelegationEdge::new("d1", "d2"),
        DelegationEdge::new("d2", "d3"),
        DelegationEdge::new("d3", "d4"),
        DelegationEdge::new("d4", "d5"),
    ]);
    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(report.is_healthy(), "深度 5 委托链应健康");
    // inv-9 + inv-system-health 两节点
    assert_eq!(report.nodes.len(), 2);
}

// ============================================================
// 异常场景:各不变量违反 → 不健康,终端锚点验证器捕获
// ============================================================

/// 异常:INV-7 全局超预算(200 > 117)→ 终端锚点违反 → 不健康
#[test]
fn test_exception_inv7_budget_exceeded() {
    let snapshot = SystemInvariantSnapshot::empty().with_inv7(Inv7Observation {
        agent_resident: 0,
        effective_capacity: 128_000,
        m_total: 200,
        m_budget: 130,
    });
    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(!report.is_healthy(), "超预算应不健康");
    assert!(report.terminal_anchored.is_violated());
}

/// 异常:INV-8 反向归档(Ice→Hot)→ 终端锚点违反 → 不健康
#[test]
fn test_exception_inv8_reverse_archive() {
    let snapshot = SystemInvariantSnapshot::empty().with_inv8(Inv8Observation {
        from_tier: ArchiveTier::Ice,
        to_tier: ArchiveTier::Hot,
    });
    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(!report.is_healthy(), "反向归档应不健康");
    assert!(report.terminal_anchored.is_violated());
}

/// 异常:INV-9 委托环(A→B→A)→ 终端锚点违反 → 不健康
#[test]
fn test_exception_inv9_delegation_cycle() {
    let snapshot = SystemInvariantSnapshot::empty().with_inv9_edges(vec![
        DelegationEdge::new("agent-a", "agent-b"),
        DelegationEdge::new("agent-b", "agent-a"),
    ]);
    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(!report.is_healthy(), "委托环应不健康");
    assert!(report.terminal_anchored.is_violated());
}

/// 异常:多不变量部分违反(INV-7 满足 + INV-8 反向 + INV-9 满足)
/// → 整体不健康(任一违反即不健康)
#[test]
fn test_exception_partial_violation_overall_unhealthy() {
    let snapshot = SystemInvariantSnapshot::empty()
        .with_inv7(Inv7Observation {
            agent_resident: 50_000,
            effective_capacity: 128_000,
            m_total: 60,
            m_budget: 130,
        })
        .with_inv8(Inv8Observation {
            from_tier: ArchiveTier::Cold,
            to_tier: ArchiveTier::Warm, // 反向
        })
        .with_inv9_edges(vec![DelegationEdge::new("root", "a")]);

    let report = InvariantHealthReporter::new().build_report(&snapshot);
    assert!(!report.is_healthy(), "任一不变量违反即整体不健康");
    assert!(report.terminal_anchored.is_violated());
    // inv-8 违反,但 inv-7/inv-9 满足;派生系统节点因 inv-8 不满足而 satisfied=false
    let inv8_node = report.nodes.iter().find(|n| n.id == "inv-8").unwrap();
    assert!(!inv8_node.satisfied, "inv-8 应标记为未满足");
    let inv7_node = report.nodes.iter().find(|n| n.id == "inv-7").unwrap();
    assert!(inv7_node.satisfied, "inv-7 应标记为满足");
}
