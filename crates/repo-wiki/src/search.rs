//! RAG 混合检索融合模块 — Reciprocal Rank Fusion (RRF)
//!
//! 对应架构层:L5 Knowledge
//!
//! 融合 HNSW(dense 向量检索)与 FTS5(sparse 全文检索)结果,
//! 提升整体召回率。RRF 算法无需训练,对分数尺度不敏感,
//! 适合融合不同检索系统的排名列表。
//!
//! # 算法
//! RRF 公式: `score(d) = Σ w_i × 1/(k + rank_i(d))`
//! - `k`: 平滑参数(默认 60),k 越大对低排名越宽容
//! - `w_i`: 第 i 路检索的权重(默认 dense 与 sparse 各 1.0)
//! - `rank_i(d)`: 文档 d 在第 i 路检索中的排名(1-based)
//!
//! # 降级路径
//! - FTS5 不可用时,调用方传入空 `sparse_results`,`rrf_fuse` 仅用 dense 结果
//!   (sparse_rank = None,不贡献分数),等效于返回 dense 排名列表
//! - HNSW 不可用时,调用方传入空 `dense_results`,同理
//!
//! # WHY RRF 而非加权分数融合
//! HNSW 返回余弦相似度 ∈ [0,1],FTS5 返回 BM25 分数(无上界),
//! 两者尺度不可比。RRF 基于排名(1/(k+rank))而非原始分数,
//! 天然消除尺度差异,无需归一化或训练权重。
//!
//! # WHY k=60
//! 原论文(Cormack et al. 2009 "Reciprocal Rank Fusion outperforms Condorcet
//! and individual Rank Learning Methods")在 TREC 数据集上经验值,
//! 对 k ∈ [1, 100] 不敏感(性能差异 < 2%),60 是稳健默认值。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================
// 结果与配置类型
// ============================================================

/// RRF 混合检索结果条目
///
/// 每条结果记录文档 ID、RRF 融合分数,以及该文档在两路检索中的排名。
/// `dense_rank` / `sparse_rank` 为 `None` 表示该路未命中(不贡献 RRF 分数)。
#[derive(Debug, Clone, PartialEq)]
pub struct HybridSearchResult {
    /// 文档 ID(`WikiEntry::entry_id` 的字符串形式)
    pub doc_id: String,
    /// RRF 融合分数(越高越相关)
    ///
    /// 全程保持 f32(工程约定:Top-K 选择与分数比较禁止 f32→f64 隐式转换,
    /// 避免 sesa-router 那种 0.4f32 as f64 精度膨胀问题)。
    pub rrf_score: f32,
    /// 在 dense 检索中的排名(1-based,None = 未命中)
    pub dense_rank: Option<usize>,
    /// 在 sparse 检索中的排名(1-based,None = 未命中)
    pub sparse_rank: Option<usize>,
}

/// 混合检索配置 — 控制 RRF 融合参数
///
/// # 默认值
/// - `rrf_k`: 60(原论文经验值)
/// - `dense_weight`: 1.0(dense 与 sparse 等权)
/// - `sparse_weight`: 1.0
///
/// # 调优建议
/// - 偏向语义召回(向量检索强):提高 `dense_weight` 至 1.5-2.0
/// - 偏向精确匹配(关键词检索强):提高 `sparse_weight` 至 1.5-2.0
/// - 大规模索引(100K+)低排名文档噪声多:降低 `rrf_k` 至 30(增强头部优势)
/// - 小规模索引(< 1K)召回率不足:提高 `rrf_k` 至 100(给低排名更多机会)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HybridSearchConfig {
    /// RRF 参数 k(默认 60)— k 越大对低排名越宽容
    ///
    /// WHY 60:原论文(Cormack et al. 2009)经验值,在 TREC 数据集上
    /// 表现稳健。k 越大,低排名文档的分数衰减越慢,更多低排名文档
    /// 有机会进入 Top-K;k 越小,高分排名优势更明显。
    #[serde(default = "default_rrf_k")]
    pub rrf_k: usize,
    /// dense 检索权重(默认 1.0)
    #[serde(default = "default_dense_weight")]
    pub dense_weight: f32,
    /// sparse 检索权重(默认 1.0)
    #[serde(default = "default_sparse_weight")]
    pub sparse_weight: f32,
}

/// 默认 RRF k 参数 — 与 `HybridSearchConfig::default` 保持一致
const fn default_rrf_k() -> usize {
    60
}

/// 默认 dense 权重 — 与 `HybridSearchConfig::default` 保持一致
const fn default_dense_weight() -> f32 {
    1.0
}

/// 默认 sparse 权重 — 与 `HybridSearchConfig::default` 保持一致
const fn default_sparse_weight() -> f32 {
    1.0
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            rrf_k: default_rrf_k(),
            dense_weight: default_dense_weight(),
            sparse_weight: default_sparse_weight(),
        }
    }
}

impl HybridSearchConfig {
    /// 创建自定义 RRF 配置
    ///
    /// # 参数
    /// - `rrf_k`: RRF 平滑参数(推荐 60,大规模可降至 30)
    /// - `dense_weight`: dense 检索权重(默认 1.0)
    /// - `sparse_weight`: sparse 检索权重(默认 1.0)
    pub fn new(rrf_k: usize, dense_weight: f32, sparse_weight: f32) -> Self {
        Self {
            rrf_k,
            dense_weight,
            sparse_weight,
        }
    }
}

// ============================================================
// RRF 融合核心算法
// ============================================================

/// 使用 RRF 算法融合 dense 和 sparse 检索结果(纯函数)
///
/// RRF 公式: `score(d) = Σ w_i × 1/(k + rank_i(d))`
///
/// # 参数
/// - `dense_results`: HNSW 检索结果(已按相关性降序排列,前述为最佳匹配)
/// - `sparse_results`: FTS5 检索结果(已按相关性降序排列)
/// - `config`: 混合检索配置
/// - `top_k`: 返回的 Top-K 数量
///
/// # 返回
/// 融合后的 Top-K 结果,按 RRF 分数降序排列。每条结果记录该文档在
/// 两路检索中的排名(1-based,`None` 表示该路未命中)。
///
/// # 降级行为
/// - `dense_results` 为空:仅用 sparse 排名计算 RRF 分数(`dense_rank = None`)
/// - `sparse_results` 为空:仅用 dense 排名计算 RRF 分数(`sparse_rank = None`)
/// - 两路均空:返回空 Vec
/// - 同一 `doc_id` 在两路中出现:RRF 分数累加(去重,`dense_rank` 与 `sparse_rank` 均填充)
/// - `top_k = 0`:返回空 Vec
///
/// # Top-K 选择
/// 用 `select_nth_unstable_by`(O(n))做 Top-K 选择,禁止 `sort_by`(O(n log n))
/// 做 Top-K(工程约定,见 `VectorIndex::search`)。最终对前 K 元素做
/// `sort_by`(K log K)降序排列(此为最终排序,非 Top-K 选择)。
///
/// # 示例
/// ```
/// use repo_wiki::search::{rrf_fuse, HybridSearchConfig};
///
/// let dense = vec!["doc-1".to_string(), "doc-2".to_string()];
/// let sparse = vec!["doc-2".to_string(), "doc-3".to_string()];
/// let config = HybridSearchConfig::default();
///
/// let results = rrf_fuse(&dense, &sparse, &config, 5);
/// // doc-2 在两路均命中(dense_rank=2, sparse_rank=1),RRF 分数最高
/// assert_eq!(results[0].doc_id, "doc-2");
/// assert_eq!(results[0].dense_rank, Some(2));
/// assert_eq!(results[0].sparse_rank, Some(1));
/// ```
pub fn rrf_fuse(
    dense_results: &[String],
    sparse_results: &[String],
    config: &HybridSearchConfig,
    top_k: usize,
) -> Vec<HybridSearchResult> {
    // 边界:top_k = 0 直接返回空,避免 select_nth_unstable_by(0, ...) 的语义歧义
    // 与无意义计算
    if top_k == 0 {
        return Vec::new();
    }

    let mut scores: HashMap<String, HybridSearchResult> = HashMap::new();

    // dense 路径:按相关性降序遍历,rank 从 1 开始(1-based)
    // RRF 贡献 = dense_weight / (rrf_k + rank)
    for (idx, doc_id) in dense_results.iter().enumerate() {
        let rank = idx + 1; // 1-based
                            // f32 全程保持(禁止 as f64 精度膨胀)
        let rrf = config.dense_weight / (config.rrf_k as f32 + rank as f32);
        scores
            .entry(doc_id.clone())
            .and_modify(|e| {
                // 文档已在 sparse 路径出现过:累加 RRF 分数,记录 dense_rank
                e.rrf_score += rrf;
                e.dense_rank = Some(rank);
            })
            .or_insert(HybridSearchResult {
                doc_id: doc_id.clone(),
                rrf_score: rrf,
                dense_rank: Some(rank),
                sparse_rank: None,
            });
    }

    // sparse 路径:按相关性降序遍历,rank 从 1 开始(1-based)
    // RRF 贡献 = sparse_weight / (rrf_k + rank)
    for (idx, doc_id) in sparse_results.iter().enumerate() {
        let rank = idx + 1; // 1-based
        let rrf = config.sparse_weight / (config.rrf_k as f32 + rank as f32);
        scores
            .entry(doc_id.clone())
            .and_modify(|e| {
                // 文档已在 dense 路径出现过:累加 RRF 分数,记录 sparse_rank
                e.rrf_score += rrf;
                e.sparse_rank = Some(rank);
            })
            .or_insert(HybridSearchResult {
                doc_id: doc_id.clone(),
                rrf_score: rrf,
                dense_rank: None,
                sparse_rank: Some(rank),
            });
    }

    // 收集所有结果并按 RRF 分数降序取 Top-K
    let mut results: Vec<HybridSearchResult> = scores.into_values().collect();

    // Top-K 选择用 select_nth_unstable_by(O(n)),禁止 sort_by 做 Top-K(O(n log n))
    // WHY:工程约定 Top-K 必须用 select_nth_unstable(O(n)) 替代 O(n log n) sort_by。
    // select_nth_unstable_by(k, cmp) 后,前 k 元素是无序的 Top-K 集合,
    // 第 k 元素是 pivot(已就位),后 n-k 元素均 ≤ pivot。
    if top_k < results.len() {
        results.select_nth_unstable_by(top_k, |a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // 前 K 元素已是无序的 Top-K 集合,做最终降序排序(K log K,非 Top-K 选择)
    // WHY 此处 sort_by 合法:仅对前 K 元素排序(K log K),不是用 sort_by 做
    // Top-K 选择(那是 O(n log n))。与 VectorIndex::search 模式一致。
    let k = top_k.min(results.len());
    results[..k].sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(k);
    results
}

/// 混合检索高层接口 — 接受已检索的两路 doc_id 列表,内部调用 `rrf_fuse` 融合
///
/// 这是 `rrf_fuse` 的语义化包装,供调用方在已通过 HNSW `top_k` 与 FTS5
/// `search_fts` 获取 doc_id 列表后调用。两路结果均需按相关性降序排列
/// (最佳匹配在前),`rrf_fuse` 据此计算 1-based 排名。
///
/// # 参数
/// - `dense_results`: HNSW 检索的 doc_id 列表(按相似度降序)
/// - `sparse_results`: FTS5 检索的 doc_id 列表(按相关度降序)
/// - `config`: 混合检索配置
/// - `top_k`: 返回的 Top-K 数量
///
/// # 降级路径
/// - FTS5 不可用:调用方传入空 `sparse_results`,仅用 dense 结果
///   (每条结果的 `sparse_rank = None`)
/// - HNSW 不可用:调用方传入空 `dense_results`,仅用 sparse 结果
///   (每条结果的 `dense_rank = None`)
///
/// # 示例
/// ```
/// use repo_wiki::search::{hybrid_search, HybridSearchConfig};
///
/// let dense = vec!["doc-1".to_string(), "doc-2".to_string()];
/// let sparse = vec!["doc-2".to_string(), "doc-3".to_string()];
/// let config = HybridSearchConfig::default();
///
/// let results = hybrid_search(&dense, &sparse, &config, 5);
/// // doc-2 在两路均命中,RRF 分数最高
/// assert_eq!(results[0].doc_id, "doc-2");
/// ```
pub fn hybrid_search(
    dense_results: &[String],
    sparse_results: &[String],
    config: &HybridSearchConfig,
    top_k: usize,
) -> Vec<HybridSearchResult> {
    rrf_fuse(dense_results, sparse_results, config, top_k)
}
