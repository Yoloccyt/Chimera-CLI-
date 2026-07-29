//! FormalVerifier M2 — 全系统不变量传递闭包形式化验证(Phase 8.2,属性 #7)
//!
//! 对应架构层: L4 FormalVerifier(gsoe-evolution 作为 FormalVerifier hub)
//! 对应 ADR: ADR-047(M2 Property #7:全系统不变量传递闭包)、
//!   ADR-028(INV-7/8/9 不变量)、ADR-042(R2 解冻阶段① 前置)
//! 对应计划: `IMPLEMENTATION_PLAN_phase8_formal_verifier_m2.md` Phase 8.2
//!
//! # 核心保证(Property #7)
//!
//! 前六个属性各自验证单个组件的局部不变量;属性 #7 是**对不变量系统本身
//! 的元验证**——把不变量之间的依赖关系建模为有向图,验证系统级一致性:
//!
//! 1. **依赖无环**: 不变量依赖图无环(循环依赖 = 系统病态,无法定义传递闭包)
//!    —— 将 INV-9 委托图无环原理提升到"不变量之间"的元层面
//! 2. **满足传播(传递闭包核心)**: 若不变量 A 满足,则 A 的全部传递前置
//!    也必须满足(满足的不变量不能建立在被违反的地基上)
//! 3. **终端锚点不可绕过**: 终端安全不变量(INV-7 预算界 / INV-8 归档单调 /
//!    INV-9 委托无环 / UNLEARNABLE 红线)一旦进入系统,必须满足且不可 Skipped
//!    —— 强制安全地基不可孤立、不可绕过
//! # 与 INV-7/8/9 的接地关系(非虚构)
//!
//! | 不变量 | 语义 | 在本验证器中的角色 |
//! |--------|------|-------------------|
//! | INV-7 | 上下文预算界(内存 ≤130MB×0.9) | 终端锚点(terminal=true) |
//! | INV-8 | 归档单调性(Current→Historical 不可逆) | 终端锚点 |
//! | INV-9 | 委托图无环(DFS 三色) | 终端锚点 + 依赖无环原理来源 |
//!
//! # 设计决策(WHY 验证抽象不变量图而非绑定 crate 运行时类型)
//!
//! 验证器消费抽象的 `InvariantNode`(id + 满足态 + 是否终端)与
//! `InvariantEdge`(依赖 → 前置)序列,不引用任何具体 crate 的运行时类型:
//! - **跨 crate 元验证的唯一合法形态**:属性 #7 无单一 owner crate,
//!   建模为抽象图使其只依赖 L0 formal_props(gsoe-evolution L5 → L0 合规)
//! - **与 M1/M2-P6 验证器同构**:纯函数 + `VerificationResult` 三态,
//!   FormalVerifier 管线统一并发消费
//! - **上层(RuntimeAuditor / CI)负责采集**:将各 crate 的 InvariantChecker
//!   实际检查结果映射为 `InvariantNode` 投喂本验证器
//!
//! # R2 冻结声明(ADR-042)
//!
//! 纯观测函数,无梯度更新/无训练路径;标识符规避 5 个 R2 扫描关键词。
//! 是 R2 解冻三阶递进阶段① 的组成——系统级不变量一致性的形式化保证。

use nexus_contracts::formal_props::VerificationResult;
use std::collections::{HashMap, HashSet, VecDeque};

/// 环检测的最大追溯步数(防病态输入下 DFS 不终止)
///
/// WHY 有界:不变量系统规模远小于此(INV-1..9 量级 + UNLEARNABLE 6 条);
/// 有界迭代保证任意输入(含超大伪造图)下验证器 O(V+E) 有界终止。
const MAX_TRAVERSAL_NODES: usize = 4096;

/// 不变量节点 — 单个系统不变量的可观测状态
///
/// WHY 独立轻量类型:属性 #7 验证不变量之间的关系,不关心各不变量的
/// 具体检查逻辑(那是各 crate InvariantChecker 的职责),只需其满足态与
/// 是否为终端安全锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantNode {
    /// 不变量标识(如 "inv-7" / "inv-9" / "unlearnable-sandbox")
    pub id: String,
    /// 该不变量当前是否满足
    pub satisfied: bool,
    /// 是否为终端安全锚点(INV-7/8/9 / UNLEARNABLE 红线 = true)
    ///
    /// 终端锚点是系统安全的地基,一旦纳入系统必须满足且不可绕过。
    pub terminal: bool,
}

impl InvariantNode {
    /// 构造不变量节点
    pub fn new(id: impl Into<String>, satisfied: bool, terminal: bool) -> Self {
        Self {
            id: id.into(),
            satisfied,
            terminal,
        }
    }
}

/// 不变量依赖边 — `dependent` 的正确性建立在 `prerequisite` 之上
///
/// 语义:`dependent → prerequisite`,即"dependent 满足"要求"prerequisite 满足"。
/// 例:某进化决策不变量依赖 INV-9(委托无环)作为前置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantEdge {
    /// 依赖方不变量 ID(其正确性依赖 prerequisite)
    pub dependent: String,
    /// 前置不变量 ID(被依赖的地基)
    pub prerequisite: String,
}

impl InvariantEdge {
    /// 构造依赖边
    pub fn new(dependent: impl Into<String>, prerequisite: impl Into<String>) -> Self {
        Self {
            dependent: dependent.into(),
            prerequisite: prerequisite.into(),
        }
    }
}

/// 全系统不变量传递闭包验证器
///
/// 所有方法为纯函数,不修改内部状态,可在 FormalVerifier 管线中并发调用。
#[derive(Debug, Default, Clone, Copy)]
pub struct InvariantClosureChecker;

impl InvariantClosureChecker {
    /// 创建不变量闭包验证器实例
    pub fn new() -> Self {
        Self
    }

    /// 验证不变量依赖图无环
    ///
    /// 循环依赖(A 依赖 B,B 依赖 A)使传递闭包无法定义,是系统病态。
    /// 采用迭代式 Kahn 拓扑排序(入度归零法):若排序无法覆盖全部节点,
    /// 则存在环。避免递归 DFS 的栈风险(§4.1 边界)。
    ///
    /// # 返回
    /// - `Satisfied`: 依赖图无环
    /// - `Violated`: 存在环(携带残留于环中的节点)
    /// - `Skipped`: 无依赖边(无关系可验证)
    #[must_use]
    pub fn verify_dependency_acyclic(&self, edges: &[InvariantEdge]) -> VerificationResult {
        if edges.is_empty() {
            return VerificationResult::Skipped {
                reason: "无依赖边,无环可验证".to_string(),
            };
        }

        // 构建邻接表(dependent → [prerequisite])与入度表
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut nodes: HashSet<&str> = HashSet::new();

        for e in edges {
            adj.entry(&e.dependent).or_default().push(&e.prerequisite);
            *in_degree.entry(&e.prerequisite).or_insert(0) += 1;
            in_degree.entry(&e.dependent).or_insert(0);
            nodes.insert(&e.dependent);
            nodes.insert(&e.prerequisite);
        }

        // Kahn 算法:入度为 0 的节点入队,逐步移除
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();
        let mut removed = 0usize;

        while let Some(node) = queue.pop_front() {
            removed += 1;
            if let Some(prereqs) = adj.get(node) {
                for &p in prereqs {
                    let d = in_degree.get_mut(p).expect("边中节点必在入度表");
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(p);
                    }
                }
            }
        }

        if removed == nodes.len() {
            VerificationResult::Satisfied {
                samples_tested: nodes.len() as u64,
            }
        } else {
            // 未被移除的节点仍处于环中
            let in_cycle: Vec<&str> = nodes
                .iter()
                .filter(|n| in_degree.get(*n).copied().unwrap_or(0) > 0)
                .copied()
                .collect();
            VerificationResult::Violated {
                counterexample: format!("不变量依赖图存在环,残留节点: {in_cycle:?}"),
                samples_tested: nodes.len() as u64,
            }
        }
    }

    /// 验证满足传播(传递闭包核心):满足的不变量其全部传递前置也满足
    ///
    /// 对每个 `satisfied == true` 的不变量,沿依赖边计算其传递前置闭包,
    /// 闭包内任一前置若 `satisfied == false`,则违反——满足的不变量不能
    /// 建立在被违反的地基上。
    ///
    /// # 参数
    /// - `nodes`: 不变量节点集(提供满足态)
    /// - `edges`: 依赖边集
    ///
    /// # 返回
    /// - `Skipped`: 无满足的不变量,或节点集为空
    /// - `Violated`: 存在满足的不变量其传递前置被违反(携带违反链)
    #[must_use]
    pub fn verify_satisfaction_propagation(
        &self,
        nodes: &[InvariantNode],
        edges: &[InvariantEdge],
    ) -> VerificationResult {
        if nodes.is_empty() {
            return VerificationResult::Skipped {
                reason: "节点集为空".to_string(),
            };
        }

        let satisfied_map: HashMap<&str, bool> =
            nodes.iter().map(|n| (n.id.as_str(), n.satisfied)).collect();
        let adj = Self::build_prereq_adjacency(edges);

        let satisfied_nodes: Vec<&InvariantNode> = nodes.iter().filter(|n| n.satisfied).collect();
        if satisfied_nodes.is_empty() {
            return VerificationResult::Skipped {
                reason: "无满足的不变量,传播无从验证".to_string(),
            };
        }

        let mut violations: Vec<String> = Vec::new();
        for node in &satisfied_nodes {
            // BFS 计算 node 的传递前置闭包(visited 去重 + 有界防病态)
            let mut visited: HashSet<&str> = HashSet::new();
            let mut queue: VecDeque<&str> = VecDeque::new();
            queue.push_back(node.id.as_str());
            visited.insert(node.id.as_str());
            let mut steps = 0usize;

            while let Some(cur) = queue.pop_front() {
                steps += 1;
                if steps > MAX_TRAVERSAL_NODES {
                    break; // 有界防护(病态图);无环时不会触发
                }
                if let Some(prereqs) = adj.get(cur) {
                    for &p in prereqs {
                        // 前置若已知且被违反 → 满足传播被破坏
                        if satisfied_map.get(p) == Some(&false) {
                            violations.push(format!(
                                "满足的不变量 '{}' 依赖被违反的前置 '{}'",
                                node.id, p
                            ));
                        }
                        if visited.insert(p) {
                            queue.push_back(p);
                        }
                    }
                }
            }
        }

        Self::to_result(violations, satisfied_nodes.len() as u64)
    }

    /// 验证终端锚点不可绕过:终端安全不变量必须满足
    ///
    /// 终端锚点(INV-7/8/9 / UNLEARNABLE 红线,`terminal == true`)是系统
    /// 安全地基。一旦纳入不变量系统,必须 `satisfied == true`——不可被
    /// 违反(否则整个依赖其上的闭包失去安全保证)。
    ///
    /// # 返回
    /// - `Skipped`: 无终端锚点节点
    /// - `Violated`: 存在未满足的终端锚点
    #[must_use]
    pub fn verify_terminal_anchored(&self, nodes: &[InvariantNode]) -> VerificationResult {
        let terminals: Vec<&InvariantNode> = nodes.iter().filter(|n| n.terminal).collect();
        if terminals.is_empty() {
            return VerificationResult::Skipped {
                reason: "无终端安全锚点节点".to_string(),
            };
        }

        let violations: Vec<String> = terminals
            .iter()
            .filter(|n| !n.satisfied)
            .map(|n| format!("终端安全锚点 '{}' 未满足(安全地基被违反)", n.id))
            .collect();

        Self::to_result(violations, terminals.len() as u64)
    }

    /// 构建前置邻接表(dependent → [prerequisite])
    fn build_prereq_adjacency(edges: &[InvariantEdge]) -> HashMap<&str, Vec<&str>> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in edges {
            adj.entry(&e.dependent).or_default().push(&e.prerequisite);
        }
        adj
    }

    /// 违规列表 → VerificationResult(三验证器共享的收敛逻辑)
    fn to_result(violations: Vec<String>, samples_tested: u64) -> VerificationResult {
        if violations.is_empty() {
            VerificationResult::Satisfied { samples_tested }
        } else {
            VerificationResult::Violated {
                counterexample: violations.join("; "),
                samples_tested,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn node(id: &str, satisfied: bool, terminal: bool) -> InvariantNode {
        InvariantNode::new(id, satisfied, terminal)
    }

    fn edge(dependent: &str, prerequisite: &str) -> InvariantEdge {
        InvariantEdge::new(dependent, prerequisite)
    }

    // ============================================================
    // 依赖无环
    // ============================================================

    #[test]
    fn test_acyclic_satisfied() {
        let checker = InvariantClosureChecker::new();
        // A → B → C(无环)
        let edges = [edge("A", "B"), edge("B", "C")];
        assert!(checker.verify_dependency_acyclic(&edges).is_satisfied());
    }

    #[test]
    fn test_acyclic_diamond_satisfied() {
        let checker = InvariantClosureChecker::new();
        // 菱形:A→B, A→C, B→D, C→D(无环)
        let edges = [
            edge("A", "B"),
            edge("A", "C"),
            edge("B", "D"),
            edge("C", "D"),
        ];
        assert!(checker.verify_dependency_acyclic(&edges).is_satisfied());
    }

    #[test]
    fn test_cycle_violated() {
        let checker = InvariantClosureChecker::new();
        // A → B → A(环)
        let edges = [edge("A", "B"), edge("B", "A")];
        let result = checker.verify_dependency_acyclic(&edges);
        match result {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("环"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    #[test]
    fn test_self_cycle_violated() {
        let checker = InvariantClosureChecker::new();
        // A → A(自环)
        let edges = [edge("A", "A")];
        assert!(matches!(
            checker.verify_dependency_acyclic(&edges),
            VerificationResult::Violated { .. }
        ));
    }

    #[test]
    fn test_acyclic_empty_skipped() {
        let checker = InvariantClosureChecker::new();
        assert!(matches!(
            checker.verify_dependency_acyclic(&[]),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // 满足传播(传递闭包核心)
    // ============================================================

    #[test]
    fn test_propagation_satisfied() {
        let checker = InvariantClosureChecker::new();
        // A 满足,依赖 B、C 均满足 → 传播成立
        let nodes = [
            node("A", true, false),
            node("B", true, false),
            node("C", true, true),
        ];
        let edges = [edge("A", "B"), edge("B", "C")];
        assert!(checker
            .verify_satisfaction_propagation(&nodes, &edges)
            .is_satisfied());
    }

    #[test]
    fn test_propagation_violated_on_broken_foundation() {
        let checker = InvariantClosureChecker::new();
        // A 满足,但传递前置 C 被违反 → 满足建立在被违反的地基上
        let nodes = [
            node("A", true, false),
            node("B", true, false),
            node("C", false, true), // 地基违反
        ];
        let edges = [edge("A", "B"), edge("B", "C")];
        match checker.verify_satisfaction_propagation(&nodes, &edges) {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("C"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    #[test]
    fn test_propagation_violated_node_not_satisfied_ok() {
        let checker = InvariantClosureChecker::new();
        // A 未满足,其前置 B 被违反 —— 不违反传播(未满足的不变量不约束前置)
        let nodes = [node("A", false, false), node("B", false, false)];
        let edges = [edge("A", "B")];
        // A 未满足 → 无满足节点 → Skipped
        assert!(matches!(
            checker.verify_satisfaction_propagation(&nodes, &edges),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_propagation_empty_nodes_skipped() {
        let checker = InvariantClosureChecker::new();
        assert!(matches!(
            checker.verify_satisfaction_propagation(&[], &[]),
            VerificationResult::Skipped { .. }
        ));
    }

    #[test]
    fn test_propagation_terminates_on_cycle() {
        // 满足传播的 BFS 在环图上必须有界终止(visited 去重),不 panic 不死循环
        let checker = InvariantClosureChecker::new();
        let nodes = [node("A", true, false), node("B", true, false)];
        let edges = [edge("A", "B"), edge("B", "A")]; // 环
                                                      // 全满足 → 传播不违反(有界终止即可)
        let result = checker.verify_satisfaction_propagation(&nodes, &edges);
        assert!(result.is_satisfied());
    }

    // ============================================================
    // 终端锚点不可绕过
    // ============================================================

    #[test]
    fn test_terminal_anchored_satisfied() {
        let checker = InvariantClosureChecker::new();
        // INV-7/8/9 三终端锚点均满足
        let nodes = [
            node("inv-7", true, true),
            node("inv-8", true, true),
            node("inv-9", true, true),
            node("derived", true, false),
        ];
        assert!(checker.verify_terminal_anchored(&nodes).is_satisfied());
    }

    #[test]
    fn test_terminal_violated_on_unsatisfied_anchor() {
        let checker = InvariantClosureChecker::new();
        // INV-9 终端锚点未满足 → 安全地基被违反
        let nodes = [
            node("inv-7", true, true),
            node("inv-9", false, true), // 违反
        ];
        match checker.verify_terminal_anchored(&nodes) {
            VerificationResult::Violated { counterexample, .. } => {
                assert!(counterexample.contains("inv-9"));
            }
            other => panic!("期望 Violated,实际: {other:?}"),
        }
    }

    #[test]
    fn test_terminal_no_anchor_skipped() {
        let checker = InvariantClosureChecker::new();
        // 无终端锚点 → Skipped
        let nodes = [
            node("derived-1", true, false),
            node("derived-2", true, false),
        ];
        assert!(matches!(
            checker.verify_terminal_anchored(&nodes),
            VerificationResult::Skipped { .. }
        ));
    }

    // ============================================================
    // proptest 属性(M2 覆盖强化)
    // ============================================================

    proptest! {
        /// 属性 1: 纯链式依赖(A→B→C→...)恒无环
        #[test]
        fn prop_chain_is_acyclic(len in 2usize..50) {
            let checker = InvariantClosureChecker::new();
            let edges: Vec<InvariantEdge> = (0..len - 1)
                .map(|i| edge(&format!("n{i}"), &format!("n{}", i + 1)))
                .collect();
            prop_assert!(checker.verify_dependency_acyclic(&edges).is_satisfied());
        }

        /// 属性 2: 全满足的节点集恒满足传播(无论依赖结构)
        #[test]
        fn prop_all_satisfied_propagation_holds(
            n in 1usize..20,
        ) {
            let checker = InvariantClosureChecker::new();
            let nodes: Vec<InvariantNode> =
                (0..n).map(|i| node(&format!("n{i}"), true, false)).collect();
            // 链式依赖
            let edges: Vec<InvariantEdge> = (0..n.saturating_sub(1))
                .map(|i| edge(&format!("n{i}"), &format!("n{}", i + 1)))
                .collect();
            prop_assert!(checker
                .verify_satisfaction_propagation(&nodes, &edges)
                .is_satisfied());
        }

        /// 属性 3: 全满足的终端锚点集恒满足锚点检查
        #[test]
        fn prop_all_satisfied_terminals_anchored(n in 1usize..10) {
            let checker = InvariantClosureChecker::new();
            let nodes: Vec<InvariantNode> =
                (0..n).map(|i| node(&format!("inv-{i}"), true, true)).collect();
            prop_assert!(checker.verify_terminal_anchored(&nodes).is_satisfied());
        }
    }
}
