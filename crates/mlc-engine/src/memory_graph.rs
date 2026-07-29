//! 记忆图谱 — 语义边 + 共现边的记忆关系建模(polish-v2.7 P4-3)
//!
//! 对应架构层:L2 Memory(mlc-engine 子模块)
//! 对应 ADR:ADR-049 决策 1(memory-graph 落点 mlc-engine)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §6.2(jcode 记忆图谱)
//!
//! # 设计决策(WHY)
//!
//! - **Top-K 近邻建边替代 O(n²) 全对建边**(ADR-049 决策 6):
//!   方案文档参考实现对每对节点建边(1 万节点 = 5000 万次比较 + 潜在
//!   百万级边)。本实现每节点仅连接 Top-K 最相似近邻中超阈值者,
//!   边数上界 O(n·k);Top-K 选择用 `select_nth_unstable`(§4.1 红线)。
//! - **规模路由:>1K 节点切换 HNSW 候选生成**(Phase 8.3,Stage C 立项落地):
//!   精确路径的相似度打分是 O(n²·512 维),criterion 实测 1K 节点建边
//!   464ms、2K 节点 2.15s(平方增长,docs/performance/closure_stage_c_baseline.md),
//!   10K <1s 门禁实证不可达。现按节点数路由:≤1K 走精确路径(小规模精确
//!   且足够快),>1K 走 `hnsw_rs` 索引候选生成(O(n·log n),复用 repo-wiki
//!   `HnswStore` 生产验证过的参数,DistCosine 距离 = 1 − 余弦相似度)。
//!   HNSW 为近似算法:>1K 规模下召回近邻的微小差异对"建边候选"语义可接受
//!   (边本身就是阈值过滤的近似关系)。
//! - **图谱召回 = 种子 + BFS 扩展**:先向量检索种子节点,再沿边扩展
//!   depth 层,比纯向量检索多召回"语义不相似但共现相关"的记忆

use std::collections::{HashMap, HashSet, VecDeque};

use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::prelude::DistCosine;
use nexus_core::CLV;

/// 每节点语义边的近邻数上界(边数 = O(n·k))
const SEMANTIC_EDGE_TOP_K: usize = 8;

/// 语义建边规模切换阈值:节点数 > 该值时走 HNSW 候选生成
///
/// WHY 1000:criterion 实测精确路径 1K=464ms(可接受)、2K=2.15s(平方增长),
/// 阈值经 Stage C 基线报告从 10K 下修至 1K(性能可证伪,不接受外推乐观)。
const HNSW_SWITCH_THRESHOLD: usize = 1000;

/// HNSW 每层最大连接数(M 参数;与 repo-wiki HnswStore 生产默认一致,
/// Malkov & Yashunin 2016 论文推荐 M ∈ [16, 48])
const HNSW_MAX_NB_CONNECTION: usize = 16;

/// HNSW 最大层级(与 repo-wiki 生产默认一致)
const HNSW_MAX_LAYER: usize = 16;

/// HNSW 构建时 ef 参数(论文推荐 [100, 500],与 repo-wiki 生产默认一致)
const HNSW_EF_CONSTRUCTION: usize = 200;

/// HNSW 搜索宽度(必须 > K;取较大值保证召回,repo-wiki 测试经验:
/// 小簇随机性下 ef=100 可稳定召回)
const HNSW_EF_SEARCH: usize = 100;

/// 记忆节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryNodeType {
    /// 代码片段
    CodeSnippet,
    /// 文档
    Documentation,
    /// 对话
    Conversation,
    /// 错误模式
    ErrorPattern,
    /// 解决方案
    Solution,
}

/// 记忆节点
#[derive(Debug, Clone)]
pub struct MemoryNode {
    /// 节点唯一标识
    pub node_id: String,
    /// 内容摘要
    pub content: String,
    /// CLV 语义嵌入
    pub embedding: CLV,
    /// 节点类型
    pub node_type: MemoryNodeType,
    /// 是否与成功任务关联(Sideagent 验证的正信号)
    pub success_associated: bool,
}

/// 边类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeType {
    /// 语义相似边(余弦相似度超阈值)
    SemanticRelated,
    /// 任务共现边(同一轨迹中共同被访问)
    Cooccurrence,
}

/// 记忆边(有向,from → to)
#[derive(Debug, Clone)]
pub struct MemoryEdge {
    /// 源节点
    pub from: String,
    /// 目标节点
    pub to: String,
    /// 边权重(语义边 = 相似度;共现边 = 共现次数)
    pub weight: f32,
    /// 边类型
    pub edge_type: EdgeType,
}

/// 记忆图谱
#[derive(Debug, Default)]
pub struct MemoryGraph {
    nodes: HashMap<String, MemoryNode>,
    /// 邻接表(from → 出边集合)— O(1) 邻居查询,BFS 扩展的热路径
    adjacency: HashMap<String, Vec<MemoryEdge>>,
}

impl MemoryGraph {
    /// 创建空图谱
    pub fn new() -> Self {
        Self::default()
    }

    /// 节点数
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 边数
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(Vec::len).sum()
    }

    /// 插入节点(同 ID 覆盖)
    pub fn insert_node(&mut self, node: MemoryNode) {
        self.nodes.insert(node.node_id.clone(), node);
    }

    /// 构建语义边 — 规模路由:≤1K 精确 Top-K,>1K HNSW 候选生成
    ///
    /// 对每个节点连接 Top-K 最相似近邻中相似度 > threshold 者(双向语义边)。
    /// 规模路由(Phase 8.3):
    /// - **≤ `HNSW_SWITCH_THRESHOLD`**:精确全对打分 + `select_nth_unstable`
    ///   Top-K(小规模精确且足够快,1K=464ms 实测)
    /// - **> `HNSW_SWITCH_THRESHOLD`**:HNSW 索引查询 Top-K 候选(O(n·log n),
    ///   规避 O(n²·512) 平方增长)
    ///
    /// 两路径产出的边语义等价(均为"每节点连 Top-K 超阈值近邻"),
    /// 差异仅在大规模下 HNSW 近邻的近似性(建边候选可接受,见模块头注释)。
    pub fn build_semantic_edges(&mut self, threshold: f32) {
        let ids: Vec<String> = self.nodes.keys().cloned().collect();
        // 清除旧语义边(重建幂等;保留共现边)
        for edges in self.adjacency.values_mut() {
            edges.retain(|e| e.edge_type != EdgeType::SemanticRelated);
        }

        if ids.len() > HNSW_SWITCH_THRESHOLD {
            self.build_semantic_edges_hnsw(&ids, threshold);
        } else {
            self.build_semantic_edges_exact(&ids, threshold);
        }
    }

    /// 精确路径:全对打分 + `select_nth_unstable_by` Top-K(≤1K 节点)
    ///
    /// 打分 O(n²·512),Top-K 选择 O(n)(§4.1 红线:禁止 sort_by 做 Top-K)。
    fn build_semantic_edges_exact(&mut self, ids: &[String], threshold: f32) {
        for id in ids {
            let source = &self.nodes[id];
            // 打分:与所有其他节点的相似度
            let mut scored: Vec<(String, f32)> = ids
                .iter()
                .filter(|other| *other != id)
                .map(|other| {
                    let sim = source
                        .embedding
                        .cosine_similarity(&self.nodes[other].embedding);
                    (other.clone(), sim)
                })
                .collect();

            // Top-K 用 select_nth_unstable_by(§4.1 红线:禁止 sort_by 做 Top-K)
            let k = SEMANTIC_EDGE_TOP_K.min(scored.len());
            if k == 0 {
                continue;
            }
            if k < scored.len() {
                scored.select_nth_unstable_by(k - 1, |a, b| {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                });
                scored.truncate(k);
            }

            for (other, sim) in scored {
                if sim > threshold {
                    self.push_semantic_edge(id, other, sim);
                }
            }
        }
    }

    /// HNSW 路径:索引查询 Top-K 候选(>1K 节点)
    ///
    /// 1. 将全部节点嵌入插入 HNSW 索引(DistCosine 距离 = 1 − 余弦相似度)
    /// 2. 对每个节点查询 Top-(K+1) 近邻(+1 补偿命中自身),排除自身后
    ///    对相似度 > threshold 者建双向语义边
    ///
    /// WHY dataid 映射:hnsw_rs 用 usize dataid 标识向量,需维护
    /// dataid ↔ node_id 双向映射以回填边的字符串 ID。
    fn build_semantic_edges_hnsw(&mut self, ids: &[String], threshold: f32) {
        let n = ids.len();
        let hnsw: Hnsw<'_, f32, DistCosine> = Hnsw::new(
            HNSW_MAX_NB_CONNECTION,
            n,
            HNSW_MAX_LAYER,
            HNSW_EF_CONSTRUCTION,
            DistCosine,
        );

        // dataid = ids 中的索引下标;并行插入全部节点嵌入
        // WHY parallel_insert:hnsw_rs 内置 rayon 多核并行建索引,10K 规模下
        // 相比串行 insert 有显著加速(建索引是本路径主要耗时)。需物化 Vec<f32>
        // (parallel_insert 要求 &Vec 引用),10K×512×4B≈20MB 可接受。
        let vectors: Vec<Vec<f32>> = ids
            .iter()
            .map(|id| self.nodes[id].embedding.as_slice().to_vec())
            .collect();
        let insert_data: Vec<(&Vec<f32>, usize)> =
            vectors.iter().enumerate().map(|(i, v)| (v, i)).collect();
        hnsw.parallel_insert(&insert_data);

        // 查询 Top-(K+1) 补偿自身命中;搜索宽度 ef_search
        let fetch_k = (SEMANTIC_EDGE_TOP_K + 1).min(n);
        let ef = HNSW_EF_SEARCH.max(fetch_k);

        // 并行查询全部节点近邻(复用已物化的 vectors,rayon 多核加速)。
        // 返回顺序与 vectors 一致:results[dataid] = 节点 dataid 的近邻表。
        let results = hnsw.parallel_search(&vectors, fetch_k, ef);

        // 先收集边(避免查询借用与 self 可变借用冲突)
        let mut pending: Vec<(String, String, f32)> = Vec::new();
        for (dataid, neighbours) in results.into_iter().enumerate() {
            let id = &ids[dataid];
            let mut kept = 0usize;
            for nb in neighbours {
                if nb.d_id == dataid {
                    continue; // 排除自身
                }
                if kept >= SEMANTIC_EDGE_TOP_K {
                    break;
                }
                kept += 1;
                // DistCosine 距离 = 1 − 余弦相似度 → 还原相似度
                let sim = 1.0 - nb.distance;
                if sim > threshold {
                    pending.push((id.clone(), ids[nb.d_id].clone(), sim));
                }
            }
        }

        for (from, to, sim) in pending {
            self.push_semantic_edge(&from, to, sim);
        }
    }

    /// 追加一条语义边(两建边路径共享,避免逻辑漂移)
    fn push_semantic_edge(&mut self, from: &str, to: String, weight: f32) {
        self.adjacency
            .entry(from.to_string())
            .or_default()
            .push(MemoryEdge {
                from: from.to_string(),
                to,
                weight,
                edge_type: EdgeType::SemanticRelated,
            });
    }

    /// 构建共现边 — 同一轨迹访问的记忆两两相连
    ///
    /// `trajectories`:每条轨迹访问的记忆 ID 列表。
    /// 同轨迹内两两建双向共现边(轨迹内记忆数通常 <10,组合数可控)。
    pub fn build_cooccurrence_edges(&mut self, trajectories: &[Vec<String>]) {
        for accessed in trajectories {
            for i in 0..accessed.len() {
                for j in (i + 1)..accessed.len() {
                    // 仅为图谱内存在的节点建边(防御悬空引用)
                    if !self.nodes.contains_key(&accessed[i])
                        || !self.nodes.contains_key(&accessed[j])
                    {
                        continue;
                    }
                    for (from, to) in [(&accessed[i], &accessed[j]), (&accessed[j], &accessed[i])] {
                        self.adjacency
                            .entry(from.clone())
                            .or_default()
                            .push(MemoryEdge {
                                from: from.clone(),
                                to: to.clone(),
                                weight: 1.0,
                                edge_type: EdgeType::Cooccurrence,
                            });
                    }
                }
            }
        }
    }

    /// 图谱召回 — 种子向量检索 + BFS 边扩展(方案 §6.2)
    ///
    /// 1. 找与 query 最相似且相似度 >0.5 的种子节点
    /// 2. 沿邻接边 BFS 扩展至 depth 层或 max_results 上限
    ///
    /// WHY 优于纯向量检索:共现边可召回"语义不相似但任务相关"的记忆
    /// (如报错信息与其修复方案的嵌入相距甚远,但共现边直连)。
    pub fn recall_with_graph(
        &self,
        query: &CLV,
        depth: u32,
        max_results: usize,
    ) -> Vec<&MemoryNode> {
        // 种子:全库线性扫描取最相似(内存 KNN,规模 <10K 可接受)
        let seed = self
            .nodes
            .values()
            .map(|n| (n, n.embedding.cosine_similarity(query)))
            .filter(|(_, sim)| *sim > 0.5)
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        let Some((seed_node, _)) = seed else {
            return Vec::new();
        };

        // BFS 扩展
        let mut results: Vec<&MemoryNode> = vec![seed_node];
        let mut visited: HashSet<&str> = HashSet::from([seed_node.node_id.as_str()]);
        let mut frontier: VecDeque<(&str, u32)> = VecDeque::from([(seed_node.node_id.as_str(), 0)]);

        while let Some((current, level)) = frontier.pop_front() {
            if level >= depth || results.len() >= max_results {
                break;
            }
            if let Some(edges) = self.adjacency.get(current) {
                for edge in edges {
                    if results.len() >= max_results {
                        break;
                    }
                    if visited.insert(edge.to.as_str()) {
                        if let Some(node) = self.nodes.get(&edge.to) {
                            results.push(node);
                            frontier.push_back((node.node_id.as_str(), level + 1));
                        }
                    }
                }
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造单位向量 CLV:维度 dim 置 1
    fn unit_clv(dim: usize) -> CLV {
        let mut v = vec![0.0f32; CLV::DIMENSION];
        v[dim] = 1.0;
        CLV::from_vec(v).expect("512 维合法")
    }

    fn node(id: &str, clv: CLV, success: bool) -> MemoryNode {
        MemoryNode {
            node_id: id.into(),
            content: format!("content-{id}"),
            embedding: clv,
            node_type: MemoryNodeType::Solution,
            success_associated: success,
        }
    }

    #[test]
    fn test_semantic_edges_connect_similar_nodes_only() {
        let mut graph = MemoryGraph::new();
        // a 与 b 同向(相似度 1.0),c 正交(相似度 0.0)
        graph.insert_node(node("a", unit_clv(0), true));
        graph.insert_node(node("b", unit_clv(0), true));
        graph.insert_node(node("c", unit_clv(1), true));
        graph.build_semantic_edges(0.7);

        // a↔b 双向语义边;c 无边
        assert_eq!(graph.edge_count(), 2);
        assert!(!graph.adjacency.contains_key("c"));
    }

    #[test]
    fn test_semantic_edges_rebuild_is_idempotent() {
        let mut graph = MemoryGraph::new();
        graph.insert_node(node("a", unit_clv(0), true));
        graph.insert_node(node("b", unit_clv(0), true));
        graph.build_semantic_edges(0.7);
        graph.build_semantic_edges(0.7); // 重建不应翻倍
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn test_cooccurrence_edges_bridge_dissimilar_nodes() {
        let mut graph = MemoryGraph::new();
        graph.insert_node(node("error", unit_clv(0), false));
        graph.insert_node(node("fix", unit_clv(1), true)); // 语义正交
        graph.build_cooccurrence_edges(&[vec!["error".into(), "fix".into()]]);
        assert_eq!(graph.edge_count(), 2); // 双向共现边
    }

    #[test]
    fn test_graph_recall_expands_via_cooccurrence() {
        let mut graph = MemoryGraph::new();
        graph.insert_node(node("error", unit_clv(0), false));
        graph.insert_node(node("fix", unit_clv(1), true));
        graph.build_cooccurrence_edges(&[vec!["error".into(), "fix".into()]]);

        // query 只与 error 相似,但图谱召回经共现边带出 fix
        let results = graph.recall_with_graph(&unit_clv(0), 2, 10);
        let ids: Vec<&str> = results.iter().map(|n| n.node_id.as_str()).collect();
        assert!(ids.contains(&"error"));
        assert!(ids.contains(&"fix"), "共现边应召回语义不相似的关联记忆");
    }

    #[test]
    fn test_recall_empty_when_no_seed_above_threshold() {
        let mut graph = MemoryGraph::new();
        graph.insert_node(node("a", unit_clv(0), true));
        // query 与 a 正交,种子相似度 0 < 0.5 → 空召回
        let results = graph.recall_with_graph(&unit_clv(1), 2, 10);
        assert!(results.is_empty());
    }

    /// Phase 8.3:>1K 节点触发 HNSW 路径,相同向量簇应建立语义边
    ///
    /// 构造 1200 节点(> HNSW_SWITCH_THRESHOLD=1000),嵌入落在 8 个正交
    /// 方向上形成簇。同簇节点相似度 1.0,应经 HNSW 候选建立语义边。
    #[test]
    fn test_hnsw_path_builds_edges_above_threshold() {
        let mut graph = MemoryGraph::new();
        // 刚过 HNSW_SWITCH_THRESHOLD=1000 即触发 HNSW 路径(最小化 debug 建索引开销)
        let n = 1001;
        for i in 0..n {
            // 8 个簇:同簇同向(余弦 1.0),跨簇正交(余弦 0.0)
            graph.insert_node(node(&format!("n{i}"), unit_clv(i % 8), i % 2 == 0));
        }
        graph.build_semantic_edges(0.7);

        // HNSW 路径应建立可观数量的边(同簇成员互为近邻)
        assert!(
            graph.edge_count() > 0,
            "HNSW 路径应为同簇节点建立语义边,实际边数 0"
        );
        // 所有边权重必须 > threshold(阈值过滤生效)
        for edges in graph.adjacency.values() {
            for e in edges {
                assert!(e.weight > 0.7, "语义边权重 {} 应 > 阈值 0.7", e.weight);
            }
        }
    }

    /// Phase 8.3:HNSW 路径同样满足重建幂等(不翻倍)
    #[test]
    fn test_hnsw_path_rebuild_idempotent() {
        let mut graph = MemoryGraph::new();
        for i in 0..1001 {
            graph.insert_node(node(&format!("n{i}"), unit_clv(i % 4), true));
        }
        graph.build_semantic_edges(0.7);
        let first = graph.edge_count();
        graph.build_semantic_edges(0.7); // 重建
        assert_eq!(graph.edge_count(), first, "HNSW 路径重建不应翻倍边数");
    }
}
