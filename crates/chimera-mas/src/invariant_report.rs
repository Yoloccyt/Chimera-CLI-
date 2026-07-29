//! 系统不变量健康报告器 — INV-7/8/9 采集接线 → FormalVerifier M2 属性 #7(Phase 8.4)
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 对应 ADR: ADR-047(M2 属性 #7 闭环)+ ADR-028(INV-7/8/9)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` Phase 8.4
//!
//! # 职责:闭合属性 #7 的采集接线
//!
//! Phase 8.2 交付了 `gsoe_evolution` 的 `InvariantClosureChecker`(属性 #7),
//! 但它验证的是**抽象不变量图**;M2 报告明确遗留"上层负责将各 crate
//! InvariantChecker 实际结果映射为节点投喂"。本模块闭合该接线:
//!
//! 1. 采集 chimera-mas `InvariantChecker` 对 INV-7/8/9 的**真实检查结果**
//! 2. 映射为 `InvariantNode`(id=inv-7/8/9,satisfied=检查通过,terminal=true)
//! 3. 构建不变量依赖边(派生不变量 → 终端锚点)
//! 4. 喂给 `InvariantClosureChecker` 三验证器(依赖无环/满足传播/终端锚点)
//! 5. 汇总为 `InvariantHealthReport`,供 RuntimeAuditor / CI 消费
//!
//! # 架构依赖(WHY L9→L5 合规)
//!
//! 本模块依赖 `gsoe_evolution`(L5)的 `InvariantClosureChecker`。L9→L5 是
//! 向下依赖(§2.2 铁律允许);gsoe-evolution 不反向依赖 chimera-mas(见
//! `invariants.rs` §P2-7 双实现说明),无循环依赖。
//!
//! # 设计原则
//!
//! - **纯函数采集**:`build_report` 接收观测输入(预算/归档/委托快照),
//!   不持有状态、不发布事件(§6.2:Critical 事件由调用方发布)
//! - **终端锚点语义**:INV-7/8/9 是系统安全地基,映射为 `terminal=true`
//!   节点——属性 #7 的"终端锚点不可绕过"验证器据此判定安全地基是否被违反

use gsoe_evolution::formal::invariant_closure::{
    InvariantClosureChecker, InvariantEdge, InvariantNode,
};
use nexus_contracts::formal_props::VerificationResult;

use crate::invariants::{ArchiveTier, DelegationEdge, InvariantChecker};

/// 单个不变量的检查输入 — INV-7 上下文预算界观测
///
/// 对应 `InvariantChecker::check_inv7_context_budget` 的四参数快照。
#[derive(Debug, Clone, Copy)]
pub struct Inv7Observation {
    /// 该 Agent 当前驻留 Token 数(密集工作集)
    pub agent_resident: usize,
    /// 该 Agent tier 有效容量上限
    pub effective_capacity: usize,
    /// 全 Agent 池聚合内存(MB)
    pub m_total: usize,
    /// 全局内存预算上限(MB,通常 130)
    pub m_budget: usize,
}

/// INV-8 归档单调性观测 — 一次归档层级迁移
#[derive(Debug, Clone, Copy)]
pub struct Inv8Observation {
    /// 源归档层级
    pub from_tier: ArchiveTier,
    /// 目标归档层级
    pub to_tier: ArchiveTier,
}

/// 系统不变量健康快照 — 报告器的完整观测输入
///
/// 汇集 INV-7/8/9 三个不变量的当前观测,由调用方(派生准入闸 / 归档降级点 /
/// 委托编排器)从运行时状态采集后构造。
#[derive(Debug, Clone)]
pub struct SystemInvariantSnapshot {
    /// INV-7 上下文预算观测(None = 本次不检查预算)
    pub inv7: Option<Inv7Observation>,
    /// INV-8 归档迁移观测(None = 本次无归档操作)
    pub inv8: Option<Inv8Observation>,
    /// INV-9 委托图(空 = 无委托关系)
    pub inv9_edges: Vec<DelegationEdge>,
}

impl SystemInvariantSnapshot {
    /// 创建空快照(三不变量均不检查)
    pub fn empty() -> Self {
        Self {
            inv7: None,
            inv8: None,
            inv9_edges: Vec::new(),
        }
    }

    /// 设置 INV-7 观测(builder 风格)
    pub fn with_inv7(mut self, obs: Inv7Observation) -> Self {
        self.inv7 = Some(obs);
        self
    }

    /// 设置 INV-8 观测(builder 风格)
    pub fn with_inv8(mut self, obs: Inv8Observation) -> Self {
        self.inv8 = Some(obs);
        self
    }

    /// 设置 INV-9 委托边(builder 风格)
    pub fn with_inv9_edges(mut self, edges: Vec<DelegationEdge>) -> Self {
        self.inv9_edges = edges;
        self
    }
}

/// 系统不变量健康报告 — 属性 #7 三验证器结果 + 整体健康判定
///
/// 由 `InvariantHealthReporter::build_report` 产出,供 RuntimeAuditor / CI 消费。
#[derive(Debug, Clone)]
pub struct InvariantHealthReport {
    /// 采集到的终端锚点节点(INV-7/8/9 各自的满足态)
    pub nodes: Vec<InvariantNode>,
    /// 依赖无环验证结果(属性 #7 性质 1)
    pub acyclic: VerificationResult,
    /// 满足传播验证结果(属性 #7 性质 2)
    pub propagation: VerificationResult,
    /// 终端锚点验证结果(属性 #7 性质 3)
    pub terminal_anchored: VerificationResult,
}

impl InvariantHealthReport {
    /// 系统不变量是否整体健康
    ///
    /// 定义:三验证器均**未违反**(Satisfied 或 Skipped 都算健康——Skipped
    /// 表示该性质本次无可验证数据,非失败)。任一 Violated 即不健康。
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !self.acyclic.is_violated()
            && !self.propagation.is_violated()
            && !self.terminal_anchored.is_violated()
    }
}

/// 系统不变量健康报告器 — INV-7/8/9 → 属性 #7 采集桥接
#[derive(Debug, Default, Clone, Copy)]
pub struct InvariantHealthReporter;

impl InvariantHealthReporter {
    /// 创建报告器实例
    pub fn new() -> Self {
        Self
    }

    /// 采集快照 → 运行属性 #7 三验证器 → 汇总健康报告
    ///
    /// # 流程
    /// 1. 对快照中每个存在的不变量运行 chimera-mas `InvariantChecker` 真实检查
    /// 2. 将 `Result<()>` 映射为终端锚点 `InvariantNode`(satisfied=is_ok)
    /// 3. 构建依赖边:派生系统健康不变量 → 各终端锚点(inv-system → inv-7/8/9)
    /// 4. 运行 `InvariantClosureChecker` 三验证器
    ///
    /// # 参数
    /// - `snapshot`: 系统不变量观测快照
    ///
    /// # 返回
    /// `InvariantHealthReport`,含三验证结果与采集节点
    #[must_use]
    pub fn build_report(&self, snapshot: &SystemInvariantSnapshot) -> InvariantHealthReport {
        let mut nodes: Vec<InvariantNode> = Vec::new();
        let mut edges: Vec<InvariantEdge> = Vec::new();

        // 派生系统健康不变量:其正确性建立在 INV-7/8/9 终端锚点之上
        const SYSTEM_NODE: &str = "inv-system-health";
        let mut has_terminal = false;

        // INV-7:上下文预算界
        if let Some(obs) = snapshot.inv7 {
            let ok = InvariantChecker::check_inv7_context_budget(
                obs.agent_resident,
                obs.effective_capacity,
                obs.m_total,
                obs.m_budget,
            )
            .is_ok();
            nodes.push(InvariantNode::new("inv-7", ok, true));
            edges.push(InvariantEdge::new(SYSTEM_NODE, "inv-7"));
            has_terminal = true;
        }

        // INV-8:归档单调性
        if let Some(obs) = snapshot.inv8 {
            let ok = InvariantChecker::check_inv8_archive_monotonicity(obs.from_tier, obs.to_tier)
                .is_ok();
            nodes.push(InvariantNode::new("inv-8", ok, true));
            edges.push(InvariantEdge::new(SYSTEM_NODE, "inv-8"));
            has_terminal = true;
        }

        // INV-9:委托图无环
        if !snapshot.inv9_edges.is_empty() {
            let ok = InvariantChecker::check_inv9_delegation_acyclic(&snapshot.inv9_edges).is_ok();
            nodes.push(InvariantNode::new("inv-9", ok, true));
            edges.push(InvariantEdge::new(SYSTEM_NODE, "inv-9"));
            has_terminal = true;
        }

        // 派生系统健康节点:仅当至少有一个终端锚点时加入(其满足态 = 全部锚点满足)。
        // WHY:满足传播验证器要求"满足的不变量其传递前置也满足",派生节点满足
        // 当且仅当所有依赖的终端锚点满足——这精确表达"系统健康依赖安全地基全绿"。
        if has_terminal {
            let all_terminals_ok = nodes.iter().all(|n| n.satisfied);
            nodes.push(InvariantNode::new(SYSTEM_NODE, all_terminals_ok, false));
        }

        let checker = InvariantClosureChecker::new();
        InvariantHealthReport {
            acyclic: checker.verify_dependency_acyclic(&edges),
            propagation: checker.verify_satisfaction_propagation(&nodes, &edges),
            terminal_anchored: checker.verify_terminal_anchored(&nodes),
            nodes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全绿快照:INV-7/8/9 均满足 → 报告健康
    #[test]
    fn test_all_invariants_healthy() {
        let snapshot = SystemInvariantSnapshot::empty()
            .with_inv7(Inv7Observation {
                agent_resident: 64_000,
                effective_capacity: 128_000,
                m_total: 80,
                m_budget: 130,
            })
            .with_inv8(Inv8Observation {
                from_tier: ArchiveTier::Hot,
                to_tier: ArchiveTier::Warm,
            })
            .with_inv9_edges(vec![
                DelegationEdge::new("root", "agent-a"),
                DelegationEdge::new("agent-a", "agent-b"),
            ]);

        let report = InvariantHealthReporter::new().build_report(&snapshot);
        assert!(report.is_healthy(), "全绿快照应报告健康");
        assert!(report.terminal_anchored.is_satisfied());
        assert!(report.acyclic.is_satisfied());
        assert!(report.propagation.is_satisfied());
        // 4 节点:inv-7 / inv-8 / inv-9 / inv-system-health
        assert_eq!(report.nodes.len(), 4);
    }

    /// INV-7 违反(全局超预算)→ 终端锚点验证失败 → 不健康
    #[test]
    fn test_inv7_violation_makes_unhealthy() {
        let snapshot = SystemInvariantSnapshot::empty().with_inv7(Inv7Observation {
            agent_resident: 0,
            effective_capacity: 128_000,
            m_total: 200, // > 130×0.9=117
            m_budget: 130,
        });
        let report = InvariantHealthReporter::new().build_report(&snapshot);
        assert!(!report.is_healthy(), "INV-7 超预算应报告不健康");
        assert!(report.terminal_anchored.is_violated());
    }

    /// INV-9 委托环 → 终端锚点验证失败 → 不健康
    #[test]
    fn test_inv9_cycle_makes_unhealthy() {
        let snapshot = SystemInvariantSnapshot::empty().with_inv9_edges(vec![
            DelegationEdge::new("agent-a", "agent-b"),
            DelegationEdge::new("agent-b", "agent-a"), // 环
        ]);
        let report = InvariantHealthReporter::new().build_report(&snapshot);
        assert!(!report.is_healthy(), "INV-9 委托环应报告不健康");
        assert!(report.terminal_anchored.is_violated());
    }

    /// INV-8 反向归档 → 不健康,且满足传播捕获派生节点不能建立在违反锚点上
    #[test]
    fn test_inv8_reverse_makes_unhealthy() {
        let snapshot = SystemInvariantSnapshot::empty().with_inv8(Inv8Observation {
            from_tier: ArchiveTier::Ice,
            to_tier: ArchiveTier::Hot, // 反向膨胀
        });
        let report = InvariantHealthReporter::new().build_report(&snapshot);
        assert!(!report.is_healthy());
        assert!(report.terminal_anchored.is_violated());
        // inv-8 违反 → 派生系统节点 satisfied=false → 传播不报违反(前置本就未满足)
        // 但终端锚点验证器捕获 inv-8 未满足
    }

    /// 空快照:无不变量可检查 → 三验证器均 Skipped → 健康(无违反)
    #[test]
    fn test_empty_snapshot_is_healthy_vacuously() {
        let report = InvariantHealthReporter::new().build_report(&SystemInvariantSnapshot::empty());
        assert!(report.is_healthy(), "空快照无违反,应判定健康(空真)");
        assert!(report.terminal_anchored.is_skipped());
        assert!(report.acyclic.is_skipped());
        assert!(report.nodes.is_empty());
    }

    /// 混合:INV-7 满足但 INV-9 环 → 整体不健康(任一违反即不健康)
    #[test]
    fn test_partial_violation_makes_unhealthy() {
        let snapshot = SystemInvariantSnapshot::empty()
            .with_inv7(Inv7Observation {
                agent_resident: 1000,
                effective_capacity: 128_000,
                m_total: 50,
                m_budget: 130,
            })
            .with_inv9_edges(vec![DelegationEdge::new("x", "x")]); // 自环
        let report = InvariantHealthReporter::new().build_report(&snapshot);
        assert!(!report.is_healthy(), "任一不变量违反即整体不健康");
        // inv-7 满足、inv-9 违反 → 终端锚点验证器报违反
        assert!(report.terminal_anchored.is_violated());
    }
}
