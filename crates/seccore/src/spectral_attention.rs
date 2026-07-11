//! Spectral Attention 安全审计 — 基于图注意力网络(GAT)的轻量级异常检测
//!
//! 对应架构层: L4 Security 增强层
//! 对应论文: Khalafi et al., Graph Convolutional Attention for Security Audit, 2026
//!
//! 设计决策(WHY):
//! - **轻量级频谱分析**: 不依赖外部线性代数库,纯 Rust 实现幂法/子空间迭代,
//!   满足 < 10ms 延迟要求(图规模 < 1000 节点)。
//! - **命令执行图建模**: 节点 = 命令/文件/网络端点,边 = 依赖/数据流/因果关系。
//!   将离散的命令序列转化为连续的图结构,暴露隐藏的执行模式。
//! - **注意力头分级**: 网络头(network)、文件系统头(filesystem)、权限头(privilege)
//!   分别关注不同安全领域,实现细粒度审计。
//! - **频谱异常检测**: 基于归一化拉普拉斯矩阵特征值分布检测图的结构性异常,
//!   如异常密集连接(可能表示攻击链)或周期性模式(可能表示自动化攻击)。
//!
//! 核心算法:
//! 1. 构建执行图: 从 CommandSpec + ExecutionResult 提取节点与边
//! 2. 归一化拉普拉斯: L = I - D^(-1/2) * A * D^(-1/2)
//! 3. 子空间迭代: 求前 k 个特征值(默认 k=5)
//! 4. 注意力权重: 基于特征值重要性 + 节点风险属性 + 安全头加权
//! 5. 异常检测: 特征值分布统计 + 周期性自相关分析

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::SecCoreError;
use crate::types::{CommandSpec, ExecutionResult, RiskLevel};

/// 节点 ID — 执行图中唯一标识符。
pub type NodeId = u64;

/// 文件访问类型 — 用于区分文件操作的安全影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileAccessType {
    /// 读取 — 低影响
    Read,
    /// 写入 — 中影响,可能覆盖数据
    Write,
    /// 删除 — 高影响,破坏性操作
    Delete,
    /// 执行 — 中影响,可能触发代码执行
    Execute,
}

/// 图节点类型 — 执行图中的实体分类。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    /// 命令节点 — 程序名 + 风险等级
    Command {
        /// 程序名
        program: String,
        /// 风险等级
        risk_level: RiskLevel,
    },
    /// 文件节点 — 路径 + 访问类型
    File {
        /// 文件路径
        path: String,
        /// 访问类型
        access_type: FileAccessType,
    },
    /// 网络节点 — 端点 + 协议
    Network {
        /// 网络端点(如 IP:port 或 URL)
        endpoint: String,
        /// 协议(如 tcp/udp/http)
        protocol: String,
    },
}

/// 图边类型 — 节点间关系的语义分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeType {
    /// 依赖 — 命令间的执行顺序依赖
    DependsOn,
    /// 读取 — 命令从文件读取数据
    ReadsFrom,
    /// 写入 — 命令向文件写入数据
    WritesTo,
    /// 连接 — 命令发起网络连接
    ConnectsTo,
    /// 产生 — 命令产生文件输出
    Produces,
}

/// 执行图节点 — 携带时序与风险信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionNode {
    /// 唯一标识
    pub id: NodeId,
    /// 节点类型
    pub node_type: NodeType,
    /// 创建时间戳(UTC 秒)
    pub timestamp: i64,
    /// 风险权重 ∈ [0.0, 1.0]
    pub risk_weight: f64,
}

/// 执行图边 — 带权重的关系。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEdge {
    /// 源节点 ID
    pub source: NodeId,
    /// 目标节点 ID
    pub target: NodeId,
    /// 边类型
    pub edge_type: EdgeType,
    /// 权重 ∈ [0.0, 1.0]
    pub weight: f64,
}

/// 执行图 — 命令执行序列的图结构表示。
///
/// 将离散命令序列转化为图,使频谱分析能够检测:
/// - 异常密集连接(可能表示攻击链)
/// - 孤立节点(可能表示未授权操作)
/// - 周期性模式(可能表示自动化攻击)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionGraph {
    nodes: Vec<ExecutionNode>,
    edges: Vec<ExecutionEdge>,
    node_index: HashMap<NodeId, usize>,
    next_id: NodeId,
    max_nodes: usize,
    max_edges: usize,
}

/// 警报级别 — 安全关键头检测的分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertLevel {
    /// 信息 — 一般性提示
    Info,
    /// 警告 — 需要关注
    Warning,
    /// 严重 — 需要立即响应
    Critical,
}

/// 安全关键头警报 — 注意力头级别的异常通知。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticalHeadAlert {
    /// 头类型: "network" / "filesystem" / "privilege"
    pub head_type: String,
    /// 警报级别
    pub alert_level: AlertLevel,
    /// 人类可读描述
    pub description: String,
    /// 受影响节点 ID 列表
    pub affected_node_ids: Vec<NodeId>,
    /// 注意力分数
    pub attention_score: f64,
}

/// 异常级别 — 综合异常检测的分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnomalyLevel {
    /// 正常 — 无异常
    Normal,
    /// 可疑 — 轻微异常,需观察
    Suspicious,
    /// 异常 — 明显异常,需告警
    Anomaly,
    /// 严重 — 严重异常,需立即干预
    Critical,
}

/// Spectral Attention 分析结果 — 单次分析的完整输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralAnalysis {
    /// 归一化拉普拉斯矩阵特征值(升序)
    pub eigenvalues: Vec<f64>,
    /// 节点注意力权重(与 nodes 列表一一对应,softmax 归一化)
    pub attention_weights: Vec<f64>,
    /// 综合异常分数 ∈ [0.0, 1.0]
    pub anomaly_score: f64,
    /// 周期性异常分数 ∈ [0.0, 1.0]
    pub periodicity_score: f64,
    /// 安全关键头警报列表
    pub critical_head_alerts: Vec<CriticalHeadAlert>,
    /// 异常级别
    pub anomaly_level: AnomalyLevel,
    /// 分析时间戳(UTC 秒)
    pub timestamp: i64,
}

/// Spectral Attention 配置 — 可调参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralConfig {
    /// 异常检测阈值(默认 0.7)
    /// 异常分数 > 此值 → Anomaly 级别
    pub anomaly_threshold: f64,
    /// 周期性检测阈值(默认 0.6)
    /// 周期性分数 > 此值 → 触发周期性告警
    pub periodicity_threshold: f64,
    /// 网络连接注意力权重(默认 2.0)
    /// 网络节点的注意力基础乘数
    pub network_attention_weight: f64,
    /// 文件删除注意力权重(默认 2.5)
    /// 文件删除节点的注意力基础乘数
    pub file_delete_attention_weight: f64,
    /// 提权操作注意力权重(默认 3.0)
    /// 提权命令节点的注意力基础乘数
    pub privilege_attention_weight: f64,
    /// 最大保留节点数(默认 1000,防止内存无限增长)
    pub max_nodes: usize,
    /// 最大保留边数(默认 5000)
    pub max_edges: usize,
    /// 子空间迭代维度(默认 5,求前 5 个特征值)
    pub subspace_dimension: usize,
    /// 子空间迭代次数(默认 30)
    pub subspace_iterations: usize,
    /// 历史特征值序列最大长度(用于周期性检测,默认 100)
    pub max_eigenvalue_history: usize,
    /// 历史异常分数最大长度(默认 1000)
    pub max_anomaly_history: usize,
}

impl Default for SpectralConfig {
    fn default() -> Self {
        Self {
            anomaly_threshold: 0.7,
            periodicity_threshold: 0.6,
            network_attention_weight: 2.0,
            file_delete_attention_weight: 2.5,
            privilege_attention_weight: 3.0,
            max_nodes: 1000,
            max_edges: 5000,
            subspace_dimension: 5,
            subspace_iterations: 30,
            max_eigenvalue_history: 100,
            max_anomaly_history: 1000,
        }
    }
}

impl ExecutionGraph {
    /// 创建空执行图。
    pub fn new(max_nodes: usize, max_edges: usize) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            node_index: HashMap::new(),
            next_id: 0,
            max_nodes,
            max_edges,
        }
    }

    /// 添加节点,返回节点 ID。
    ///
    /// 若节点数超过 max_nodes,自动移除最旧的节点。
    pub fn add_node(&mut self, node_type: NodeType, risk_weight: f64) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;

        let node = ExecutionNode {
            id,
            node_type,
            timestamp: Utc::now().timestamp(),
            risk_weight: risk_weight.clamp(0.0, 1.0),
        };

        self.node_index.insert(id, self.nodes.len());
        self.nodes.push(node);

        // 容量控制: 移除最旧节点
        if self.nodes.len() > self.max_nodes {
            self.remove_oldest_node();
        }

        id
    }

    /// 添加边。
    ///
    /// 仅当源节点和目标节点都存在时添加。
    /// 若边数超过 max_edges,自动移除最旧的边。
    pub fn add_edge(&mut self, source: NodeId, target: NodeId, edge_type: EdgeType, weight: f64) {
        if self.node_index.contains_key(&source) && self.node_index.contains_key(&target) {
            self.edges.push(ExecutionEdge {
                source,
                target,
                edge_type,
                weight: weight.clamp(0.0, 1.0),
            });

            if self.edges.len() > self.max_edges {
                self.edges.remove(0);
            }
        }
    }

    /// 获取所有节点(只读)。
    pub fn nodes(&self) -> &[ExecutionNode] {
        &self.nodes
    }

    /// 获取所有边(只读)。
    pub fn edges(&self) -> &[ExecutionEdge] {
        &self.edges
    }

    /// 根据 ID 查找节点索引。
    pub fn node_index_by_id(&self, id: NodeId) -> Option<usize> {
        self.node_index.get(&id).copied()
    }

    /// 构建加权邻接矩阵(对称)。
    ///
    /// 返回 n×n 矩阵,其中 n = nodes.len()。
    /// 矩阵元素 adj[i][j] = 节点 i 与节点 j 之间所有边的权重和。
    pub fn adjacency_matrix(&self) -> Vec<Vec<f64>> {
        let n = self.nodes.len();
        let mut adj = vec![vec![0.0; n]; n];

        for edge in &self.edges {
            if let (Some(&si), Some(&ti)) = (
                self.node_index.get(&edge.source),
                self.node_index.get(&edge.target),
            ) {
                adj[si][ti] += edge.weight;
                if si != ti {
                    adj[ti][si] += edge.weight;
                }
            }
        }

        adj
    }

    /// 计算度矩阵(对角线元素为各节点度数)。
    ///
    /// 返回长度为 n 的向量,degrees[i] = 节点 i 的加权度数。
    pub fn degree_matrix(&self) -> Vec<f64> {
        let n = self.nodes.len();
        let mut degrees = vec![0.0; n];

        for edge in &self.edges {
            if let (Some(&si), Some(&ti)) = (
                self.node_index.get(&edge.source),
                self.node_index.get(&edge.target),
            ) {
                degrees[si] += edge.weight;
                if si != ti {
                    degrees[ti] += edge.weight;
                }
            }
        }

        degrees
    }

    /// 移除最旧的节点及其关联边。
    fn remove_oldest_node(&mut self) {
        if self.nodes.is_empty() {
            return;
        }

        let oldest_id = self.nodes[0].id;
        self.nodes.remove(0);
        self.node_index.remove(&oldest_id);
        self.edges
            .retain(|e| e.source != oldest_id && e.target != oldest_id);
        self.rebuild_index();
    }

    /// 重建节点索引映射。
    fn rebuild_index(&mut self) {
        self.node_index.clear();
        for (i, node) in self.nodes.iter().enumerate() {
            self.node_index.insert(node.id, i);
        }
    }
}

/// Spectral Attention 安全审计分析器。
///
/// 基于图注意力网络(GAT)的轻量级实现,通过频谱分析检测命令执行序列中的异常模式。
///
/// 使用方式:
/// ```ignore
/// let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
/// analyzer.add_execution_record(&spec, &result);
/// let analysis = analyzer.analyze()?;
/// if analysis.anomaly_level > AnomalyLevel::Suspicious {
///     warn!("检测到异常执行模式!");
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectralAttentionAnalyzer {
    graph: ExecutionGraph,
    config: SpectralConfig,
    /// 历史特征值序列(用于周期性检测)
    eigenvalue_history: Vec<Vec<f64>>,
    /// 历史异常分数(用于趋势分析)
    anomaly_score_history: Vec<f64>,
}

impl SpectralAttentionAnalyzer {
    /// 创建带指定配置的分析器。
    pub fn new(config: SpectralConfig) -> Self {
        let max_nodes = config.max_nodes;
        let max_edges = config.max_edges;
        Self {
            graph: ExecutionGraph::new(max_nodes, max_edges),
            config,
            eigenvalue_history: Vec::new(),
            anomaly_score_history: Vec::new(),
        }
    }

    /// 创建使用默认配置的分析器。
    pub fn with_default_config() -> Self {
        Self::new(SpectralConfig::default())
    }

    /// 获取当前配置(只读)。
    pub fn config(&self) -> &SpectralConfig {
        &self.config
    }

    /// 获取当前执行图(只读)。
    pub fn graph(&self) -> &ExecutionGraph {
        &self.graph
    }

    /// 获取历史异常分数(只读)。
    pub fn anomaly_history(&self) -> &[f64] {
        &self.anomaly_score_history
    }

    /// 添加执行记录到图,并返回创建的命令节点 ID。
    ///
    /// 此方法从 CommandSpec 和 ExecutionResult 中提取信息:
    /// 1. 创建命令节点
    /// 2. 从参数推断文件节点和网络节点
    /// 3. 建立边(依赖、读取、写入、连接)
    /// 4. 连接前一个命令节点形成序列
    ///
    /// # 参数
    /// - `spec`: 校验通过的命令规格
    /// - `result`: 执行结果(用于推断额外信息,如输出文件)
    pub fn add_execution_record(
        &mut self,
        spec: &CommandSpec,
        _result: &ExecutionResult,
    ) -> NodeId {
        // 1. 创建命令节点
        let risk_weight = risk_level_to_weight(spec.risk_level);
        let command_node = NodeType::Command {
            program: spec.program.clone(),
            risk_level: spec.risk_level,
        };
        let cmd_id = self.graph.add_node(command_node, risk_weight);

        // 2. 从参数推断文件节点和网络节点
        for arg in &spec.allowed_args {
            self.infer_nodes_from_arg(cmd_id, &spec.program, arg);
        }

        // 3. 连接前一个命令节点(形成序列依赖)
        if self.graph.nodes.len() > 1 {
            let prev_node = &self.graph.nodes[self.graph.nodes.len() - 2];
            if prev_node.id != cmd_id {
                self.graph
                    .add_edge(prev_node.id, cmd_id, EdgeType::DependsOn, 0.5);
            }
        }

        cmd_id
    }

    /// 从参数推断并创建相关节点和边。
    fn infer_nodes_from_arg(&mut self, cmd_id: NodeId, program: &str, arg: &str) {
        // 检测文件路径
        let is_path =
            arg.starts_with('/') || arg.starts_with('.') || arg.contains("\\") || arg.contains('/');

        if is_path && !arg.contains("http://") && !arg.contains("https://") {
            let access_type = infer_file_access_type(program, arg);
            let file_weight = match access_type {
                FileAccessType::Delete => 0.9,
                FileAccessType::Write => 0.6,
                FileAccessType::Read => 0.3,
                FileAccessType::Execute => 0.7,
            };

            let file_node = NodeType::File {
                path: arg.to_string(),
                access_type,
            };
            let file_id = self.graph.add_node(file_node, file_weight);

            let edge_type = match access_type {
                FileAccessType::Read => EdgeType::ReadsFrom,
                FileAccessType::Write => EdgeType::WritesTo,
                FileAccessType::Delete => EdgeType::WritesTo,
                FileAccessType::Execute => EdgeType::DependsOn,
            };
            self.graph.add_edge(cmd_id, file_id, edge_type, 1.0);
        }

        // 检测网络端点
        let is_network = arg.starts_with("http://")
            || arg.starts_with("https://")
            || (arg.contains(':') && !arg.starts_with("/") && arg.parse::<u16>().is_ok());

        if is_network {
            let endpoint = if arg.starts_with("http://") || arg.starts_with("https://") {
                arg.to_string()
            } else if let Ok(port) = arg.parse::<u16>() {
                format!("tcp://0.0.0.0:{}", port)
            } else {
                format!("tcp://{}", arg)
            };

            let net_node = NodeType::Network {
                endpoint,
                protocol: if arg.starts_with("http://") || arg.starts_with("https://") {
                    "http".to_string()
                } else {
                    "tcp".to_string()
                },
            };
            let net_id = self.graph.add_node(net_node, 0.8);
            self.graph
                .add_edge(cmd_id, net_id, EdgeType::ConnectsTo, 1.0);
        }
    }

    /// 执行 Spectral Attention 分析。
    ///
    /// 分析流程:
    /// 1. 构建邻接矩阵与度矩阵
    /// 2. 计算归一化拉普拉斯矩阵
    /// 3. 子空间迭代求特征值
    /// 4. 计算注意力权重
    /// 5. 异常检测与周期性检测
    /// 6. 安全关键头检测
    ///
    /// 当图节点数 < 2 时返回默认 Normal 结果。
    pub fn analyze(&mut self) -> Result<SpectralAnalysis, SecCoreError> {
        if self.graph.nodes.len() < 2 {
            info!(
                node_count = self.graph.nodes.len(),
                "Spectral Attention: 图节点数不足,跳过频谱分析"
            );
            return Ok(SpectralAnalysis {
                eigenvalues: vec![],
                attention_weights: vec![],
                anomaly_score: 0.0,
                periodicity_score: 0.0,
                critical_head_alerts: vec![],
                anomaly_level: AnomalyLevel::Normal,
                timestamp: Utc::now().timestamp(),
            });
        }

        // 1. 构建邻接矩阵与度矩阵
        let adj = self.graph.adjacency_matrix();
        let degrees = self.graph.degree_matrix();

        // 2. 计算归一化拉普拉斯矩阵 L = I - D^(-1/2) * A * D^(-1/2)
        let laplacian = compute_normalized_laplacian(&adj, &degrees);

        // 3. 子空间迭代求前 k 个特征值
        let k = self.config.subspace_dimension.min(self.graph.nodes.len());
        let eigenvalues = subspace_iteration(&laplacian, k, self.config.subspace_iterations);

        // 4. 计算注意力权重
        let attention_weights = compute_attention_weights(&self.graph, &eigenvalues, &self.config);

        // 5. 异常检测
        let anomaly_score = detect_anomaly(&eigenvalues, &self.anomaly_score_history);

        // 6. 周期性检测
        let periodicity_score = detect_periodicity(&eigenvalues, &self.eigenvalue_history);

        // 7. 安全关键头检测
        let critical_head_alerts =
            detect_critical_heads(&self.graph, &attention_weights, &self.config);

        // 8. 确定异常级别
        let anomaly_level = classify_anomaly_level(anomaly_score, &self.config);

        // 更新历史
        self.eigenvalue_history.push(eigenvalues.clone());
        if self.eigenvalue_history.len() > self.config.max_eigenvalue_history {
            self.eigenvalue_history.remove(0);
        }
        self.anomaly_score_history.push(anomaly_score);
        if self.anomaly_score_history.len() > self.config.max_anomaly_history {
            self.anomaly_score_history.remove(0);
        }

        info!(
            node_count = self.graph.nodes.len(),
            edge_count = self.graph.edges.len(),
            eigenvalue_count = eigenvalues.len(),
            anomaly_score = anomaly_score,
            periodicity_score = periodicity_score,
            anomaly_level = ?anomaly_level,
            critical_alerts = critical_head_alerts.len(),
            "Spectral Attention 频谱分析完成"
        );

        Ok(SpectralAnalysis {
            eigenvalues,
            attention_weights,
            anomaly_score,
            periodicity_score,
            critical_head_alerts,
            anomaly_level,
            timestamp: Utc::now().timestamp(),
        })
    }
}

// === 辅助函数 ===

/// 将 RiskLevel 映射为风险权重。
fn risk_level_to_weight(risk_level: RiskLevel) -> f64 {
    match risk_level {
        RiskLevel::Low => 0.2,
        RiskLevel::Medium => 0.5,
        RiskLevel::High => 0.8,
        RiskLevel::Critical => 1.0,
        RiskLevel::Unknown => 0.4,
    }
}

/// 从程序名和参数推断文件访问类型。
fn infer_file_access_type(program: &str, arg: &str) -> FileAccessType {
    let p = program.to_lowercase();
    if p == "rm" || p == "del" || p == "shred" || p == "wipe" {
        return FileAccessType::Delete;
    }
    if p == "cat" || p == "type" || p == "head" || p == "tail" || p == "ls" {
        return FileAccessType::Read;
    }
    if p == "echo" && arg.contains('>') {
        return FileAccessType::Write;
    }
    if p == "cp" || p == "mv" || p == "write" {
        return FileAccessType::Write;
    }
    if p == "chmod" || p == "chown" {
        return FileAccessType::Execute;
    }
    FileAccessType::Read
}

/// 计算归一化拉普拉斯矩阵。
///
/// L = I - D^(-1/2) * A * D^(-1/2)
///
/// 对于无向图, L 是实对称半正定矩阵,特征值 ∈ [0, 2]。
/// 连通图的最小特征值 = 0,对应全1特征向量。
fn compute_normalized_laplacian(adj: &[Vec<f64>], degrees: &[f64]) -> Vec<Vec<f64>> {
    let n = adj.len();
    if n == 0 {
        return vec![];
    }

    // D^(-1/2)
    let mut d_inv_sqrt = vec![0.0; n];
    for i in 0..n {
        if degrees[i] > 1e-10 {
            d_inv_sqrt[i] = 1.0 / degrees[i].sqrt();
        }
    }

    // L = I - D^(-1/2) * A * D^(-1/2)
    let mut l = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                l[i][j] = 1.0 - adj[i][j] * d_inv_sqrt[i] * d_inv_sqrt[j];
            } else {
                l[i][j] = -adj[i][j] * d_inv_sqrt[i] * d_inv_sqrt[j];
            }
        }
    }

    l
}

/// 矩阵-向量乘法。
fn matrix_vector_mul(matrix: &[Vec<f64>], vector: &[f64]) -> Vec<f64> {
    let n = matrix.len();
    let mut result = vec![0.0; n];
    for i in 0..n {
        for j in 0..vector.len() {
            result[i] += matrix[i][j] * vector[j];
        }
    }
    result
}

/// 向量点积。
fn dot_product(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// 子空间迭代求实对称矩阵的前 k 个特征值。
///
/// 算法: 初始化 k 个正交向量,反复执行 A*Q → QR 分解,最后 Ritz 值近似特征值。
/// 收敛速度取决于特征值间隙,对安全审计场景(小图、低精度)足够。
fn subspace_iteration(matrix: &[Vec<f64>], k: usize, max_iterations: usize) -> Vec<f64> {
    let n = matrix.len();
    if n == 0 {
        return vec![];
    }
    let k = k.min(n);

    // 初始化 Q 为标准基向量
    let mut q = vec![vec![0.0; n]; k];
    for (i, row) in q.iter_mut().enumerate() {
        row[i] = 1.0;
    }

    for _ in 0..max_iterations {
        // Z = A * Q
        let mut z = vec![vec![0.0; n]; k];
        for i in 0..k {
            z[i] = matrix_vector_mul(matrix, &q[i]);
        }

        // QR 分解(Gram-Schmidt 正交化)
        for i in 0..k {
            for q_j in q.iter().take(i) {
                let proj = dot_product(&z[i], q_j);
                for l in 0..n {
                    z[i][l] -= proj * q_j[l];
                }
            }
            let norm = z[i].iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm > 1e-10 {
                for l in 0..n {
                    q[i][l] = z[i][l] / norm;
                }
            }
        }
    }

    // 计算 Ritz 值: q_i^T * A * q_i
    let mut eigenvalues = Vec::with_capacity(k);
    for q_i in q.iter().take(k) {
        let aq = matrix_vector_mul(matrix, q_i);
        eigenvalues.push(dot_product(q_i, &aq));
    }

    eigenvalues.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    eigenvalues
}

/// 计算注意力权重。
///
/// 注意力权重 = softmax(基础风险权重 × 安全头加权 × (1 + 频谱注意力))
/// 频谱注意力 = tanh(节点度数 / 谱宽度)
fn compute_attention_weights(
    graph: &ExecutionGraph,
    eigenvalues: &[f64],
    config: &SpectralConfig,
) -> Vec<f64> {
    let n = graph.nodes.len();
    let mut weights = vec![0.0; n];

    if eigenvalues.is_empty() || n == 0 {
        return weights;
    }

    let spectral_width = eigenvalues.last().unwrap_or(&1.0) - eigenvalues.first().unwrap_or(&0.0);
    let spectral_width = if spectral_width > 1e-10 {
        spectral_width
    } else {
        1.0
    };

    for (i, node) in graph.nodes.iter().enumerate() {
        let base_weight = node.risk_weight;

        // 安全关键头加权
        let critical_weight = match &node.node_type {
            NodeType::Network { .. } => config.network_attention_weight,
            NodeType::File { access_type, .. } => match access_type {
                FileAccessType::Delete => config.file_delete_attention_weight,
                _ => 1.0,
            },
            NodeType::Command { program, .. } => {
                let p = program.to_lowercase();
                if p == "sudo" || p == "su" || p == "chmod" || p == "chown" {
                    config.privilege_attention_weight
                } else {
                    1.0
                }
            }
        };

        // 频谱注意力: 节点在图中的连接度反映其结构重要性
        let degree = graph
            .edges
            .iter()
            .filter(|e| e.source == node.id || e.target == node.id)
            .map(|e| e.weight)
            .sum::<f64>();
        let spectral_attention = (degree / spectral_width).tanh();

        weights[i] = base_weight * critical_weight * (1.0 + spectral_attention);
    }

    // Softmax 归一化
    softmax(&mut weights);
    weights
}

/// Softmax 归一化。
///
/// 为避免数值溢出,先减去最大值再取指数。
fn softmax(values: &mut [f64]) {
    if values.is_empty() {
        return;
    }
    let max_val = values.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));
    let exps: Vec<f64> = values.iter().map(|&v| (v - max_val).exp()).collect();
    let sum_exp: f64 = exps.iter().sum();
    if sum_exp > 1e-10 {
        for (i, v) in values.iter_mut().enumerate() {
            *v = exps[i] / sum_exp;
        }
    }
}

/// 异常检测 — 基于特征值分布的统计异常。
///
/// 检测指标:
/// 1. 变异系数(CV): 特征值分布的离散程度,CV 高 = 图结构不均匀
/// 2. 谱间隙: λ_2 - λ_1,间隙小 = 图更连通(可能异常密集)
/// 3. 历史偏离: 与近期异常分数均值的偏离程度
fn detect_anomaly(eigenvalues: &[f64], history: &[f64]) -> f64 {
    if eigenvalues.len() < 2 {
        return 0.0;
    }

    // 1. 变异系数
    let mean = eigenvalues.iter().sum::<f64>() / eigenvalues.len() as f64;
    let variance =
        eigenvalues.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / eigenvalues.len() as f64;
    let std_dev = variance.sqrt();
    let cv = if mean.abs() > 1e-10 {
        std_dev / mean.abs()
    } else {
        0.0
    };

    // 2. 谱间隙 (对归一化拉普拉斯,最小特征值 ≈ 0)
    let spectral_gap = eigenvalues.get(1).unwrap_or(&0.0) - eigenvalues.first().unwrap_or(&0.0);
    let spectral_gap_score = 1.0 - (spectral_gap * 5.0).min(1.0); // 间隙小 → 分数高

    // 3. 与历史趋势的偏离
    let history_deviation = if history.len() >= 10 {
        let recent = &history[history.len() - 10..];
        let recent_mean = recent.iter().sum::<f64>() / recent.len() as f64;
        let recent_var = recent
            .iter()
            .map(|&v| (v - recent_mean).powi(2))
            .sum::<f64>()
            / recent.len() as f64;
        let recent_std = recent_var.sqrt();
        if recent_std > 1e-10 {
            ((cv - recent_mean).abs() / recent_std).min(2.0) / 2.0
        } else {
            0.0
        }
    } else {
        0.0
    };

    // 综合异常分数(加权平均)
    (cv.abs() * 0.3 + spectral_gap_score * 0.4 + history_deviation * 0.3).clamp(0.0, 1.0)
}

/// 周期性检测 — 基于特征值序列的自相关分析。
///
/// 检查最近 20 次特征值序列的第一主成分是否存在周期 2-10 的自相关。
/// 高自相关可能表示自动化攻击的周期性行为。
fn detect_periodicity(eigenvalues: &[f64], history: &[Vec<f64>]) -> f64 {
    if history.len() < 20 || eigenvalues.is_empty() {
        return 0.0;
    }

    // 取最近 20 次特征值序列的第一主成分(最大特征值)
    let recent: Vec<f64> = history
        .iter()
        .rev()
        .take(20)
        .map(|e| e.last().copied().unwrap_or(0.0))
        .collect();

    let n = recent.len();
    let mean = recent.iter().sum::<f64>() / n as f64;
    let variance = recent.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;

    if variance < 1e-10 {
        return 0.0;
    }

    // 检查周期 2-10
    let mut max_autocorr: f64 = 0.0;
    let max_lag = 10usize.min(n / 2);
    for lag in 2..=max_lag {
        let mut cov = 0.0;
        for i in 0..n - lag {
            cov += (recent[i] - mean) * (recent[i + lag] - mean);
        }
        cov /= (n - lag) as f64;
        let autocorr = (cov / variance).abs();
        max_autocorr = max_autocorr.max(autocorr);
    }

    max_autocorr.clamp(0.0, 1.0)
}

/// 检测安全关键头异常。
///
/// 对网络、文件删除、提权操作进行注意力头级别的监控。
/// 当某类节点的注意力权重超过阈值时,生成对应警报。
fn detect_critical_heads(
    graph: &ExecutionGraph,
    attention_weights: &[f64],
    config: &SpectralConfig,
) -> Vec<CriticalHeadAlert> {
    let mut alerts = Vec::new();

    for (i, node) in graph.nodes.iter().enumerate() {
        let weight = attention_weights.get(i).copied().unwrap_or(0.0);

        match &node.node_type {
            NodeType::Network { endpoint, .. } => {
                let threshold = config.network_attention_weight / 10.0;
                if weight > threshold {
                    alerts.push(CriticalHeadAlert {
                        head_type: "network".to_string(),
                        alert_level: if weight > config.network_attention_weight / 5.0 {
                            AlertLevel::Critical
                        } else {
                            AlertLevel::Warning
                        },
                        description: format!("网络端点 {} 注意力权重异常高", endpoint),
                        affected_node_ids: vec![node.id],
                        attention_score: weight,
                    });
                }
            }
            NodeType::File {
                path,
                access_type: FileAccessType::Delete,
            } => {
                let threshold = config.file_delete_attention_weight / 10.0;
                if weight > threshold {
                    alerts.push(CriticalHeadAlert {
                        head_type: "filesystem".to_string(),
                        alert_level: if weight > config.file_delete_attention_weight / 5.0 {
                            AlertLevel::Critical
                        } else {
                            AlertLevel::Warning
                        },
                        description: format!("文件删除操作 {} 注意力权重异常高", path),
                        affected_node_ids: vec![node.id],
                        attention_score: weight,
                    });
                }
            }
            NodeType::Command { program, .. } => {
                let p = program.to_lowercase();
                if p == "sudo" || p == "su" || p == "chmod" || p == "chown" {
                    let threshold = config.privilege_attention_weight / 10.0;
                    if weight > threshold {
                        alerts.push(CriticalHeadAlert {
                            head_type: "privilege".to_string(),
                            alert_level: if weight > config.privilege_attention_weight / 5.0 {
                                AlertLevel::Critical
                            } else {
                                AlertLevel::Warning
                            },
                            description: format!("提权操作 {} 注意力权重异常高", program),
                            affected_node_ids: vec![node.id],
                            attention_score: weight,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    alerts
}

/// 根据异常分数分类异常级别。
fn classify_anomaly_level(anomaly_score: f64, config: &SpectralConfig) -> AnomalyLevel {
    if anomaly_score > config.anomaly_threshold * 1.5 {
        AnomalyLevel::Critical
    } else if anomaly_score > config.anomaly_threshold {
        AnomalyLevel::Anomaly
    } else if anomaly_score > config.anomaly_threshold * 0.7 {
        AnomalyLevel::Suspicious
    } else {
        AnomalyLevel::Normal
    }
}

// === 单元测试 ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    fn make_spec(program: &str, args: Vec<&str>, risk: RiskLevel) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            allowed_args: args.iter().map(|s| s.to_string()).collect(),
            env_whitelist: HashMap::new(),
            risk_level: risk,
        }
    }

    fn make_result() -> ExecutionResult {
        ExecutionResult {
            exit_code: 0,
            stdout: "".to_string(),
            stderr: "".to_string(),
            duration: Duration::from_secs(1),
            audit_hash: "0".repeat(64),
        }
    }

    // --- 图构建测试 ---

    #[test]
    fn test_graph_construction() {
        let mut graph = ExecutionGraph::new(100, 500);
        let id = graph.add_node(
            NodeType::Command {
                program: "echo".to_string(),
                risk_level: RiskLevel::Low,
            },
            0.2,
        );
        assert_eq!(graph.nodes().len(), 1);
        assert_eq!(graph.nodes()[0].id, id);
    }

    #[test]
    fn test_graph_add_edge() {
        let mut graph = ExecutionGraph::new(100, 500);
        let id1 = graph.add_node(
            NodeType::Command {
                program: "echo".to_string(),
                risk_level: RiskLevel::Low,
            },
            0.2,
        );
        let id2 = graph.add_node(
            NodeType::File {
                path: "/tmp/test.txt".to_string(),
                access_type: FileAccessType::Read,
            },
            0.3,
        );
        graph.add_edge(id1, id2, EdgeType::ReadsFrom, 1.0);
        assert_eq!(graph.edges().len(), 1);
    }

    #[test]
    fn test_graph_capacity_limit() {
        let mut graph = ExecutionGraph::new(3, 10);
        for i in 0..5 {
            graph.add_node(
                NodeType::Command {
                    program: format!("cmd{}", i),
                    risk_level: RiskLevel::Low,
                },
                0.2,
            );
        }
        assert_eq!(graph.nodes().len(), 3, "节点数应受 max_nodes 限制");
    }

    // --- 节点推断测试 ---

    #[test]
    fn test_file_node_detection() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("cat", vec!["/etc/passwd"], RiskLevel::Medium);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let file_nodes: Vec<_> = analyzer
            .graph()
            .nodes()
            .iter()
            .filter(|n| matches!(&n.node_type, NodeType::File { .. }))
            .collect();
        assert_eq!(file_nodes.len(), 1, "应检测到文件节点");
    }

    #[test]
    fn test_network_node_detection() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("curl", vec!["http://evil.com"], RiskLevel::High);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let net_nodes: Vec<_> = analyzer
            .graph()
            .nodes()
            .iter()
            .filter(|n| matches!(&n.node_type, NodeType::Network { .. }))
            .collect();
        assert_eq!(net_nodes.len(), 1, "应检测到网络节点");
    }

    #[test]
    fn test_delete_node_detection() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("rm", vec!["/tmp/sensitive.txt"], RiskLevel::High);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let delete_nodes: Vec<_> = analyzer
            .graph()
            .nodes()
            .iter()
            .filter(|n| {
                matches!(
                    &n.node_type,
                    NodeType::File {
                        access_type: FileAccessType::Delete,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(delete_nodes.len(), 1, "应检测到删除文件节点");
        assert!(delete_nodes[0].risk_weight > 0.8, "删除节点风险权重应高");
    }

    // --- 矩阵运算测试 ---

    #[test]
    fn test_matrix_vector_mul() {
        let matrix = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let vector = vec![1.0, 0.0];
        let result = matrix_vector_mul(&matrix, &vector);
        assert_eq!(result, vec![1.0, 3.0]);
    }

    #[test]
    fn test_dot_product() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        assert!((dot_product(&a, &b) - 32.0).abs() < 1e-10);
    }

    #[test]
    fn test_softmax() {
        let mut v = vec![1.0, 2.0, 3.0];
        softmax(&mut v);
        let sum: f64 = v.iter().sum();
        assert!((sum - 1.0).abs() < 1e-10, "softmax 后应和为 1");
        assert!(v[0] < v[1] && v[1] < v[2], "softmax 应保持单调性");
    }

    // --- 频谱分析测试 ---

    #[test]
    fn test_subspace_iteration_known_eigenvalues() {
        // 对称矩阵 [2, 1; 1, 2], 特征值 = 1, 3
        let matrix = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        let eigenvalues = subspace_iteration(&matrix, 2, 50);
        assert_eq!(eigenvalues.len(), 2);
        assert!((eigenvalues[0] - 1.0).abs() < 0.01, "最小特征值应约等于 1");
        assert!((eigenvalues[1] - 3.0).abs() < 0.01, "最大特征值应约等于 3");
    }

    #[test]
    fn test_normalized_laplacian_properties() {
        // 完全图 K3 的归一化拉普拉斯特征值 = 0, 1.5, 1.5
        let adj = vec![
            vec![0.0, 1.0, 1.0],
            vec![1.0, 0.0, 1.0],
            vec![1.0, 1.0, 0.0],
        ];
        let degrees = vec![2.0, 2.0, 2.0];
        let l = compute_normalized_laplacian(&adj, &degrees);

        // 检查对称性
        for (i, row) in l.iter().enumerate().take(3) {
            for (j, &val) in row.iter().enumerate().take(3) {
                assert!((val - l[j][i]).abs() < 1e-10, "拉普拉斯矩阵应对称");
            }
        }

        // 检查迹 = n
        let trace: f64 = (0..3).map(|i| l[i][i]).sum();
        assert!((trace - 3.0).abs() < 1e-10, "迹应等于 3");
    }

    // --- Spectral Attention 分析器测试 ---

    #[test]
    fn test_analyze_empty_graph() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let analysis = analyzer.analyze().unwrap();
        assert_eq!(analysis.anomaly_level, AnomalyLevel::Normal);
        assert!(analysis.eigenvalues.is_empty());
    }

    #[test]
    fn test_analyze_single_node() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("echo", vec!["hello"], RiskLevel::Low);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let analysis = analyzer.analyze().unwrap();
        assert_eq!(analysis.anomaly_level, AnomalyLevel::Normal);
    }

    #[test]
    fn test_analyze_multiple_nodes() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        for i in 0..10 {
            let spec = make_spec("echo", vec![&format!("test{}", i)], RiskLevel::Low);
            let result = make_result();
            analyzer.add_execution_record(&spec, &result);
        }

        let analysis = analyzer.analyze().unwrap();
        assert!(!analysis.eigenvalues.is_empty(), "应计算特征值");
        assert!(!analysis.attention_weights.is_empty(), "应计算注意力权重");
        assert_eq!(
            analysis.attention_weights.len(),
            analyzer.graph().nodes().len(),
            "注意力权重应与节点数一致"
        );
    }

    #[test]
    fn test_critical_head_network_alert() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("curl", vec!["http://evil.com"], RiskLevel::High);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let analysis = analyzer.analyze().unwrap();
        let network_alerts: Vec<_> = analysis
            .critical_head_alerts
            .iter()
            .filter(|a| a.head_type == "network")
            .collect();
        assert!(!network_alerts.is_empty(), "网络关键头应触发警报");
    }

    #[test]
    fn test_critical_head_privilege_alert() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        let spec = make_spec("sudo", vec!["rm", "/etc/passwd"], RiskLevel::High);
        let result = make_result();
        analyzer.add_execution_record(&spec, &result);

        let analysis = analyzer.analyze().unwrap();
        let privilege_alerts: Vec<_> = analysis
            .critical_head_alerts
            .iter()
            .filter(|a| a.head_type == "privilege")
            .collect();
        assert!(!privilege_alerts.is_empty(), "提权关键头应触发警报");
    }

    #[test]
    fn test_periodicity_detection() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();

        // 添加周期性执行序列
        for _ in 0..25 {
            let spec = make_spec("curl", vec!["http://evil.com"], RiskLevel::High);
            let result = make_result();
            analyzer.add_execution_record(&spec, &result);
            let _ = analyzer.analyze();
        }

        let analysis = analyzer.analyze().unwrap();
        // 周期性检测需要足够历史数据,主要验证不 panic
        assert!(analysis.periodicity_score >= 0.0);
    }

    #[test]
    fn test_anomaly_score_history() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        // WHY 6 次迭代:首次 analyze() 时图仅 1 个节点(< 2),跳过频谱分析不记录异常分数;
        // 后续 5 次 analyze() 才实际记录,因此需 6 次迭代以积累 5 条历史记录
        for i in 0..6 {
            let spec = make_spec("echo", vec![&format!("test{}", i)], RiskLevel::Low);
            let result = make_result();
            analyzer.add_execution_record(&spec, &result);
            let _ = analyzer.analyze();
        }
        assert_eq!(analyzer.anomaly_history().len(), 5, "应记录 5 次异常分数");
    }

    #[test]
    fn test_config_default() {
        let config = SpectralConfig::default();
        assert_eq!(config.anomaly_threshold, 0.7);
        assert_eq!(config.network_attention_weight, 2.0);
        assert_eq!(config.file_delete_attention_weight, 2.5);
        assert_eq!(config.privilege_attention_weight, 3.0);
    }

    #[test]
    fn test_anomaly_level_classification() {
        let config = SpectralConfig::default();
        assert_eq!(classify_anomaly_level(0.0, &config), AnomalyLevel::Normal);
        assert_eq!(
            classify_anomaly_level(0.5, &config),
            AnomalyLevel::Suspicious
        );
        assert_eq!(classify_anomaly_level(0.8, &config), AnomalyLevel::Anomaly);
        assert_eq!(classify_anomaly_level(1.2, &config), AnomalyLevel::Critical);
    }

    #[test]
    fn test_attention_weights_sum_to_one() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        for i in 0..5 {
            let spec = make_spec("echo", vec![&format!("test{}", i)], RiskLevel::Low);
            let result = make_result();
            analyzer.add_execution_record(&spec, &result);
        }

        let analysis = analyzer.analyze().unwrap();
        let sum: f64 = analysis.attention_weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6, "注意力权重应和为 1");
    }

    #[test]
    fn test_eigenvalue_range() {
        let mut analyzer = SpectralAttentionAnalyzer::with_default_config();
        for i in 0..10 {
            let spec = make_spec("echo", vec![&format!("test{}", i)], RiskLevel::Low);
            let result = make_result();
            analyzer.add_execution_record(&spec, &result);
        }

        let analysis = analyzer.analyze().unwrap();
        for &eigen in &analysis.eigenvalues {
            assert!(
                (-0.1..=2.1).contains(&eigen),
                "归一化拉普拉斯特征值应在 [0, 2] 附近,实际 {}",
                eigen
            );
        }
    }

    #[test]
    fn test_node_index_after_removal() {
        let mut graph = ExecutionGraph::new(2, 10);
        let id1 = graph.add_node(
            NodeType::Command {
                program: "cmd1".to_string(),
                risk_level: RiskLevel::Low,
            },
            0.2,
        );
        let id2 = graph.add_node(
            NodeType::Command {
                program: "cmd2".to_string(),
                risk_level: RiskLevel::Low,
            },
            0.2,
        );
        let id3 = graph.add_node(
            NodeType::Command {
                program: "cmd3".to_string(),
                risk_level: RiskLevel::Low,
            },
            0.2,
        );

        // id1 应被移除
        assert!(graph.node_index_by_id(id1).is_none(), "最旧节点应被移除");
        assert!(graph.node_index_by_id(id2).is_some(), "id2 应存在");
        assert!(graph.node_index_by_id(id3).is_some(), "id3 应存在");
    }
}
