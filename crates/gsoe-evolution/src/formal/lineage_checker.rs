//! 谱系图验证器 — DAG 性质 + 回滚可达性 + 变异幅度硬上限
//!
//! 对应架构层: L5 (Knowledge) / L4 (FormalVerifier)
//! 对应不变量类别: `PropertyCategory::LineageIntegrity`
//!
//! # 核心职责
//!
//! 1. **DAG 性质验证**: 谱系图不得含环（DFS 三色标记法）
//! 2. **回滚可达性**: 从当前版本可沿父链到达目标版本
//! 3. **变异幅度硬上限**: 防止奖励欺骗，变异幅度不得超过硬上限
//!
//! # 设计决策(WHY)
//!
//! - **纯函数设计**: `LineageChecker` 不持有状态，所有验证函数接收数据、返回 `VerificationResult`。
//!   这使得验证逻辑可在任意上下文（测试/CI/运行时）中无副作用调用。
//! - **本地图类型**: 定义 `LineageGraph`/`LineageNode`/`LineageEdge` 而非依赖 `SpecRegistry` 内部结构，
//!   因为验证器需独立于注册表实现，且图结构可来自任意数据源。
//! - **DFS 三色标记**: 标准 O(V+E) 环检测算法，白色=未访问、灰色=访问中、黑色=已完成。
//!   灰色节点被再次访问即存在回边（环）。

use std::collections::{HashMap, HashSet};

use nexus_contracts::formal_props::{PropertyCategory, VerificationResult};

// ──────────────────────────────────────────────
// 领域类型定义
// ──────────────────────────────────────────────

/// 谱系节点 — 代表一个版本快照
///
/// WHY 使用 String ID 而非 u32: 谱系节点 ID 需同时编码 harness 名称与版本号
/// （如 "quest-parse@v3"），纯数字无法跨 harness 唯一标识。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LineageNode {
    /// 节点唯一标识（如 "quest-parse@v1"）
    pub id: String,
}

impl LineageNode {
    /// 构造谱系节点
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// 谱系有向边 — 从父版本指向子版本
///
/// WHY 有向边: 谱系关系天然有方向（parent → child），
/// DAG 性质要求不存在有向环，无向环检测无法捕捉语义错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageEdge {
    /// 源节点（父版本）
    pub from: String,
    /// 目标节点（子版本）
    pub to: String,
}

impl LineageEdge {
    /// 构造谱系边
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// 谱系图 — 由节点与有向边组成的有向图
///
/// WHY 邻接表用 HashMap 而非 Vec: 节点 ID 为 String，
/// HashMap 提供 O(1) 查找，避免线性扫描。
#[derive(Debug, Clone)]
pub struct LineageGraph {
    /// 节点集合
    pub nodes: Vec<LineageNode>,
    /// 有向边集合
    pub edges: Vec<LineageEdge>,
}

impl LineageGraph {
    /// 构造空谱系图
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// 构造谱系图（便捷构造器）
    pub fn with_nodes_and_edges(nodes: Vec<LineageNode>, edges: Vec<LineageEdge>) -> Self {
        Self { nodes, edges }
    }
}

impl Default for LineageGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// 变异参数规格 — 描述单个变异操作的幅度
///
/// WHY 独立类型: 变异幅度验证需要知道参数名（用于反例描述）与幅度值，
/// 与 `MutationCandidate` 解耦避免验证器依赖进化引擎内部类型。
#[derive(Debug, Clone, PartialEq)]
pub struct MutationSpec {
    /// 参数名称（人类可读，供反例描述）
    pub parameter_name: String,
    /// 变异幅度（非负浮点数）
    pub amplitude: f64,
}

impl MutationSpec {
    /// 构造变异规格
    pub fn new(parameter_name: impl Into<String>, amplitude: f64) -> Self {
        Self {
            parameter_name: parameter_name.into(),
            amplitude,
        }
    }
}

// ──────────────────────────────────────────────
// DFS 三色标记 — 内部辅助类型
// ──────────────────────────────────────────────

/// DFS 节点颜色 — 三色标记法状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    /// 白色: 尚未访问
    White,
    /// 灰色: 正在访问中（在递归栈中）
    Gray,
    /// 黑色: 已完成所有后代访问
    Black,
}

// ──────────────────────────────────────────────
// LineageChecker — 纯函数验证器
// ──────────────────────────────────────────────

/// 谱系验证器 — 提供谱系图 DAG 性质、回滚可达性、变异幅度硬上限验证
///
/// WHY 零状态结构体: 所有验证方法为纯函数（`&self` 不读取任何字段），
/// 使用 unit struct 模式提供命名空间与文档组织，未来可扩展为持有配置（如超时阈值）。
pub struct LineageChecker;

impl LineageChecker {
    /// 验证谱系图满足 DAG 性质（无有向环）
    ///
    /// # 算法
    ///
    /// DFS 三色标记法: 白色→灰色→黑色。
    /// 若 DFS 过程中遇到灰色邻居，说明存在回边（有向环）。
    ///
    /// # 参数
    ///
    /// - `graph`: 待验证的谱系图
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 图是 DAG（无环）
    /// - `Violated`: 发现环，`counterexample` 包含环路径描述
    #[must_use]
    pub fn verify_dag_property(graph: &LineageGraph) -> VerificationResult {
        // 构建邻接表（from → [to]）
        let adj = Self::build_adjacency_list(graph);
        let mut color: HashMap<&str, Color> = HashMap::new();

        // 初始化所有节点为白色
        for node in &graph.nodes {
            color.insert(&node.id, Color::White);
        }
        // 边中可能出现未在 nodes 中声明的节点 ID，也需纳入着色
        for edge in &graph.edges {
            color.entry(&edge.from).or_insert(Color::White);
            color.entry(&edge.to).or_insert(Color::White);
        }

        // 收集所有需要 DFS 的起始节点（从 graph 数据直接引用，避免借用 color）
        let mut start_nodes: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        for edge in &graph.edges {
            if !start_nodes.contains(&edge.from.as_str()) {
                start_nodes.push(&edge.from);
            }
            if !start_nodes.contains(&edge.to.as_str()) {
                start_nodes.push(&edge.to);
            }
        }

        for start in &start_nodes {
            if color.get(start).copied() == Some(Color::White) {
                if let Some(cycle) = Self::dfs_detect_cycle(start, &adj, &mut color) {
                    return VerificationResult::Violated {
                        counterexample: format!("检测到有向环: {cycle}"),
                        samples_tested: 0,
                    };
                }
            }
        }

        VerificationResult::Satisfied {
            samples_tested: graph.nodes.len() as u64,
        }
    }

    /// 验证从当前版本可通过回滚到达目标版本
    ///
    /// # 语义
    ///
    /// 回滚操作沿谱系边的**反方向**（子→父）进行。
    /// 本函数验证从 `current_version` 出发，沿反向边（to→from）
    /// 是否存在路径到达 `target_version`。
    ///
    /// # 参数
    ///
    /// - `graph`: 谱系图
    /// - `current_version`: 当前活跃版本 ID
    /// - `target_version`: 期望回滚到的目标版本 ID
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 可达
    /// - `Violated`: 不可达，`counterexample` 描述原因
    #[must_use]
    pub fn verify_rollback_reachability(
        graph: &LineageGraph,
        current_version: &str,
        target_version: &str,
    ) -> VerificationResult {
        // 同一版本无需回滚，直接满足
        if current_version == target_version {
            return VerificationResult::Satisfied { samples_tested: 1 };
        }

        // 构建反向邻接表（子→父，即 to→from）
        let reverse_adj = Self::build_reverse_adjacency_list(graph);

        // BFS 从 current_version 沿反向边搜索 target_version
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(current_version.to_string());
        visited.insert(current_version.to_string());

        while let Some(node) = queue.pop_front() {
            if let Some(neighbors) = reverse_adj.get(node.as_str()) {
                for neighbor in neighbors {
                    if *neighbor == target_version {
                        return VerificationResult::Satisfied { samples_tested: 1 };
                    }
                    if visited.insert(neighbor.to_string()) {
                        queue.push_back(neighbor.to_string());
                    }
                }
            }
        }

        VerificationResult::Violated {
            counterexample: format!(
                "从版本 '{current_version}' 无法通过回滚到达目标版本 '{target_version}'"
            ),
            samples_tested: visited.len() as u64,
        }
    }

    /// 验证所有变异幅度不超过硬上限
    ///
    /// # 设计决策(WHY)
    ///
    /// 硬上限是防止奖励欺骗（reward hacking）的关键安全阀。
    /// 即使适应度分数显示改善，若单次变异幅度超限，该变异必须被拒绝。
    /// 这是 ADR-042 R2 冻结机制的形式化保障之一。
    ///
    /// # 参数
    ///
    /// - `mutations`: 待验证的变异规格列表
    /// - `max_amplitude`: 允许的最大变异幅度（非负）
    ///
    /// # 返回
    ///
    /// - `Satisfied`: 所有变异幅度 ≤ max_amplitude
    /// - `Violated`: 存在超限变异，`counterexample` 列出首个违规参数
    #[must_use]
    pub fn verify_mutation_amplitude_limit(
        mutations: &[MutationSpec],
        max_amplitude: f64,
    ) -> VerificationResult {
        let mut checked: u64 = 0;
        for spec in mutations {
            if spec.amplitude > max_amplitude {
                return VerificationResult::Violated {
                    counterexample: format!(
                        "参数 '{}' 变异幅度 {} 超过硬上限 {}",
                        spec.parameter_name, spec.amplitude, max_amplitude
                    ),
                    samples_tested: checked,
                };
            }
            checked += 1;
        }

        VerificationResult::Satisfied {
            samples_tested: checked,
        }
    }

    /// 获取验证器对应的不变量类别（谱系完整性）
    #[must_use]
    pub fn category() -> PropertyCategory {
        PropertyCategory::LineageIntegrity
    }

    // ──────────────────────────────────────────────
    // 内部辅助方法
    // ──────────────────────────────────────────────

    /// 构建正向邻接表（from → [to]）
    fn build_adjacency_list(graph: &LineageGraph) -> HashMap<&str, Vec<&str>> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &graph.edges {
            adj.entry(&edge.from).or_default().push(&edge.to);
        }
        adj
    }

    /// 构建反向邻接表（to → [from]）
    fn build_reverse_adjacency_list(graph: &LineageGraph) -> HashMap<&str, Vec<&str>> {
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &graph.edges {
            adj.entry(&edge.to).or_default().push(&edge.from);
        }
        adj
    }

    /// DFS 递归检测环 — 返回 `Some(cycle_description)` 表示发现环
    ///
    /// 三色标记:
    /// - White → Gray: 进入节点
    /// - Gray → Black: 离开节点（所有后代已处理）
    /// - 遇到 Gray 邻居: 发现回边（环）
    fn dfs_detect_cycle<'a>(
        node: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        color: &mut HashMap<&'a str, Color>,
    ) -> Option<String> {
        color.insert(node, Color::Gray);

        if let Some(neighbors) = adj.get(node) {
            for &neighbor in neighbors {
                match color.get(neighbor).copied().unwrap_or(Color::White) {
                    Color::Gray => {
                        // 发现回边: node → neighbor，且 neighbor 在栈中
                        return Some(format!("{neighbor} -> ... -> {node} -> {neighbor}"));
                    }
                    Color::White => {
                        if let Some(cycle) = Self::dfs_detect_cycle(neighbor, adj, color) {
                            return Some(cycle);
                        }
                    }
                    Color::Black => {
                        // 已完全处理，跳过
                    }
                }
            }
        }

        color.insert(node, Color::Black);
        None
    }
}

// ──────────────────────────────────────────────
// 单元测试
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── DAG 性质验证 ──

    #[test]
    fn test_dag_empty_graph_is_satisfied() {
        let graph = LineageGraph::new();
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_satisfied(), "空图应为 DAG");
    }

    #[test]
    fn test_dag_single_node_is_satisfied() {
        let graph = LineageGraph::with_nodes_and_edges(vec![LineageNode::new("v1")], vec![]);
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_satisfied(), "单节点图应为 DAG");
    }

    #[test]
    fn test_dag_valid_chain_is_satisfied() {
        // v1 → v2 → v3（线性链，无环）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![
                LineageNode::new("v1"),
                LineageNode::new("v2"),
                LineageNode::new("v3"),
            ],
            vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v2", "v3")],
        );
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_satisfied(), "线性链应为 DAG");
    }

    #[test]
    fn test_dag_diamond_is_satisfied() {
        // v1 → v2, v1 → v3, v2 → v4, v3 → v4（菱形，无环）
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
        assert!(result.is_satisfied(), "菱形图应为 DAG");
    }

    #[test]
    fn test_dag_self_loop_is_violated() {
        // v1 → v1（自环）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![LineageNode::new("v1")],
            vec![LineageEdge::new("v1", "v1")],
        );
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_violated(), "自环应违反 DAG 性质");
        if let VerificationResult::Violated { counterexample, .. } = &result {
            assert!(counterexample.contains("v1"), "反例应包含环中节点");
        }
    }

    #[test]
    fn test_dag_two_node_cycle_is_violated() {
        // v1 → v2 → v1（两节点环）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![LineageNode::new("v1"), LineageNode::new("v2")],
            vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v2", "v1")],
        );
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_violated(), "两节点环应违反 DAG 性质");
    }

    #[test]
    fn test_dag_three_node_cycle_is_violated() {
        // v1 → v2 → v3 → v1（三节点环）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![
                LineageNode::new("v1"),
                LineageNode::new("v2"),
                LineageNode::new("v3"),
            ],
            vec![
                LineageEdge::new("v1", "v2"),
                LineageEdge::new("v2", "v3"),
                LineageEdge::new("v3", "v1"),
            ],
        );
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_violated(), "三节点环应违反 DAG 性质");
        if let VerificationResult::Violated { counterexample, .. } = &result {
            assert!(!counterexample.is_empty(), "反例描述不应为空");
        }
    }

    #[test]
    fn test_dag_disconnected_components_no_cycle() {
        // 两个独立组件: v1→v2 和 v3→v4，无环
        let graph = LineageGraph::with_nodes_and_edges(
            vec![
                LineageNode::new("v1"),
                LineageNode::new("v2"),
                LineageNode::new("v3"),
                LineageNode::new("v4"),
            ],
            vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v3", "v4")],
        );
        let result = LineageChecker::verify_dag_property(&graph);
        assert!(result.is_satisfied(), "无环不连通图应为 DAG");
    }

    // ── 回滚可达性验证 ──

    #[test]
    fn test_rollback_same_version_is_satisfied() {
        let graph = LineageGraph::with_nodes_and_edges(vec![LineageNode::new("v1")], vec![]);
        let result = LineageChecker::verify_rollback_reachability(&graph, "v1", "v1");
        assert!(result.is_satisfied(), "同版本应可达");
    }

    #[test]
    fn test_rollback_direct_parent_is_satisfied() {
        // v1 → v2，从 v2 回滚到 v1
        let graph = LineageGraph::with_nodes_and_edges(
            vec![LineageNode::new("v1"), LineageNode::new("v2")],
            vec![LineageEdge::new("v1", "v2")],
        );
        let result = LineageChecker::verify_rollback_reachability(&graph, "v2", "v1");
        assert!(result.is_satisfied(), "直接父版本应可达");
    }

    #[test]
    fn test_rollback_transitive_ancestor_is_satisfied() {
        // v1 → v2 → v3，从 v3 回滚到 v1
        let graph = LineageGraph::with_nodes_and_edges(
            vec![
                LineageNode::new("v1"),
                LineageNode::new("v2"),
                LineageNode::new("v3"),
            ],
            vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v2", "v3")],
        );
        let result = LineageChecker::verify_rollback_reachability(&graph, "v3", "v1");
        assert!(result.is_satisfied(), "传递祖先应可达");
    }

    #[test]
    fn test_rollback_child_not_reachable() {
        // v1 → v2，从 v1 无法回滚到 v2（只能向上）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![LineageNode::new("v1"), LineageNode::new("v2")],
            vec![LineageEdge::new("v1", "v2")],
        );
        let result = LineageChecker::verify_rollback_reachability(&graph, "v1", "v2");
        assert!(result.is_violated(), "子版本不应通过回滚可达");
    }

    #[test]
    fn test_rollback_nonexistent_target_is_violated() {
        let graph = LineageGraph::with_nodes_and_edges(vec![LineageNode::new("v1")], vec![]);
        let result = LineageChecker::verify_rollback_reachability(&graph, "v1", "v999");
        assert!(result.is_violated(), "不存在的目标版本应不可达");
    }

    #[test]
    fn test_rollback_disconnected_is_violated() {
        // v1 → v2, v3 → v4（两独立组件）
        let graph = LineageGraph::with_nodes_and_edges(
            vec![
                LineageNode::new("v1"),
                LineageNode::new("v2"),
                LineageNode::new("v3"),
                LineageNode::new("v4"),
            ],
            vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v3", "v4")],
        );
        let result = LineageChecker::verify_rollback_reachability(&graph, "v4", "v1");
        assert!(result.is_violated(), "不连通组件间应不可达");
    }

    // ── 变异幅度硬上限验证 ──

    #[test]
    fn test_amplitude_empty_mutations_is_satisfied() {
        let result = LineageChecker::verify_mutation_amplitude_limit(&[], 1.0);
        assert!(result.is_satisfied(), "空变异列表应满足");
    }

    #[test]
    fn test_amplitude_within_limit_is_satisfied() {
        let mutations = vec![
            MutationSpec::new("learning_rate", 0.05),
            MutationSpec::new("dropout", 0.1),
        ];
        let result = LineageChecker::verify_mutation_amplitude_limit(&mutations, 0.5);
        assert!(result.is_satisfied(), "幅度在限制内应满足");
        if let VerificationResult::Satisfied { samples_tested } = result {
            assert_eq!(samples_tested, 2, "应检查 2 个变异");
        }
    }

    #[test]
    fn test_amplitude_exactly_at_limit_is_satisfied() {
        // 等于上限不算超限（≤ 而非 <）
        let mutations = vec![MutationSpec::new("weight", 0.5)];
        let result = LineageChecker::verify_mutation_amplitude_limit(&mutations, 0.5);
        assert!(result.is_satisfied(), "幅度等于上限应满足");
    }

    #[test]
    fn test_amplitude_exceeds_limit_is_violated() {
        let mutations = vec![
            MutationSpec::new("learning_rate", 0.05),
            MutationSpec::new("weight_scale", 2.0),
        ];
        let result = LineageChecker::verify_mutation_amplitude_limit(&mutations, 0.5);
        assert!(result.is_violated(), "超限应违反");
        if let VerificationResult::Violated {
            counterexample,
            samples_tested,
        } = result
        {
            assert!(
                counterexample.contains("weight_scale"),
                "反例应指出违规参数名"
            );
            assert_eq!(samples_tested, 1, "应在第二个变异处停止");
        }
    }

    #[test]
    fn test_amplitude_zero_limit_only_zero_passes() {
        let mutations = vec![MutationSpec::new("frozen_param", 0.001)];
        let result = LineageChecker::verify_mutation_amplitude_limit(&mutations, 0.0);
        assert!(result.is_violated(), "零上限不允许任何正幅度");
    }

    #[test]
    fn test_amplitude_zero_amplitude_with_zero_limit() {
        let mutations = vec![MutationSpec::new("frozen_param", 0.0)];
        let result = LineageChecker::verify_mutation_amplitude_limit(&mutations, 0.0);
        assert!(result.is_satisfied(), "零幅度在零上限下应满足");
    }

    // ── 类别查询 ──

    #[test]
    fn test_category_is_lineage_integrity() {
        assert_eq!(
            LineageChecker::category(),
            PropertyCategory::LineageIntegrity
        );
    }
}
