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

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 配置测试 ---

    #[test]
    fn test_hybrid_search_config_default() {
        let config = HybridSearchConfig::default();
        assert_eq!(config.rrf_k, 60);
        assert!((config.dense_weight - 1.0).abs() < 1e-6);
        assert!((config.sparse_weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_hybrid_search_config_new() {
        let config = HybridSearchConfig::new(30, 1.5, 0.8);
        assert_eq!(config.rrf_k, 30);
        assert!((config.dense_weight - 1.5).abs() < 1e-6);
        assert!((config.sparse_weight - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_hybrid_search_config_serde_roundtrip() {
        let config = HybridSearchConfig::new(45, 1.2, 0.9);
        let json = serde_json::to_string(&config).unwrap();
        let restored: HybridSearchConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, config);
    }

    /// 验证缺失字段的 JSON 反序列化为默认值(向后兼容)
    #[test]
    fn test_hybrid_search_config_serde_missing_fields() {
        let json = r#"{}"#;
        let config: HybridSearchConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config, HybridSearchConfig::default());
    }

    // --- rrf_fuse 核心算法测试 ---

    /// 两路均有结果时 RRF 融合正确
    #[test]
    fn test_rrf_fuse_both_paths_present() {
        let dense = vec![
            "doc-1".to_string(),
            "doc-2".to_string(),
            "doc-3".to_string(),
        ];
        let sparse = vec!["doc-2".to_string(), "doc-4".to_string()];
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 10);

        // 应有 4 个不同文档(doc-1/2/3/4)
        assert_eq!(results.len(), 4);

        // doc-2 在两路均命中(dense_rank=2, sparse_rank=1),RRF 分数最高
        // RRF(doc-2) = 1/(60+2) + 1/(60+1) = 1/62 + 1/61 ≈ 0.01613 + 0.01639 ≈ 0.03252
        // RRF(doc-1) = 1/(60+1) = 1/61 ≈ 0.01639(仅 dense_rank=1)
        // RRF(doc-4) = 1/(60+1) = 1/61 ≈ 0.01639(仅 sparse_rank=1)
        // RRF(doc-3) = 1/(60+3) = 1/63 ≈ 0.01587(仅 dense_rank=3)
        assert_eq!(results[0].doc_id, "doc-2");
        assert_eq!(results[0].dense_rank, Some(2));
        assert_eq!(results[0].sparse_rank, Some(1));

        // doc-2 的 RRF 分数应严格大于仅在一路命中的文档
        let doc2_score = results[0].rrf_score;
        for r in results.iter().skip(1) {
            assert!(doc2_score > r.rrf_score, "doc-2 分数应大于 {}", r.doc_id);
        }

        // 结果应按 RRF 分数降序排列
        for i in 1..results.len() {
            assert!(
                results[i - 1].rrf_score >= results[i].rrf_score,
                "结果应降序:[{}]={} < [{}]={}",
                i - 1,
                results[i - 1].rrf_score,
                i,
                results[i].rrf_score
            );
        }
    }

    /// 仅 dense 结果(sparse 为空)— 验证降级路径
    #[test]
    fn test_rrf_fuse_dense_only() {
        let dense = vec![
            "doc-1".to_string(),
            "doc-2".to_string(),
            "doc-3".to_string(),
        ];
        let sparse: Vec<String> = Vec::new();
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 10);

        // 应返回 3 个文档(仅 dense 路径)
        assert_eq!(results.len(), 3);

        // 所有结果的 sparse_rank 应为 None(未命中 sparse 路径)
        for r in &results {
            assert!(
                r.sparse_rank.is_none(),
                "{} 的 sparse_rank 应为 None",
                r.doc_id
            );
            assert!(
                r.dense_rank.is_some(),
                "{} 的 dense_rank 应为 Some",
                r.doc_id
            );
        }

        // 按 dense 排名顺序(doc-1 > doc-2 > doc-3,因 RRF 分数随 rank 递减)
        assert_eq!(results[0].doc_id, "doc-1");
        assert_eq!(results[0].dense_rank, Some(1));
        assert_eq!(results[1].doc_id, "doc-2");
        assert_eq!(results[1].dense_rank, Some(2));
        assert_eq!(results[2].doc_id, "doc-3");
        assert_eq!(results[2].dense_rank, Some(3));
    }

    /// 仅 sparse 结果(dense 为空)— 验证降级路径
    #[test]
    fn test_rrf_fuse_sparse_only() {
        let dense: Vec<String> = Vec::new();
        let sparse = vec!["doc-a".to_string(), "doc-b".to_string()];
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 10);

        assert_eq!(results.len(), 2);

        // 所有结果的 dense_rank 应为 None
        for r in &results {
            assert!(
                r.dense_rank.is_none(),
                "{} 的 dense_rank 应为 None",
                r.doc_id
            );
            assert!(
                r.sparse_rank.is_some(),
                "{} 的 sparse_rank 应为 Some",
                r.doc_id
            );
        }

        // 按 sparse 排名顺序
        assert_eq!(results[0].doc_id, "doc-a");
        assert_eq!(results[0].sparse_rank, Some(1));
        assert_eq!(results[1].doc_id, "doc-b");
        assert_eq!(results[1].sparse_rank, Some(2));
    }

    /// 两路均空返回空 Vec
    #[test]
    fn test_rrf_fuse_both_empty() {
        let dense: Vec<String> = Vec::new();
        let sparse: Vec<String> = Vec::new();
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 10);
        assert!(results.is_empty());
    }

    /// 重复文档去重 — 同一 doc_id 在两路中出现时 RRF 分数合并
    #[test]
    fn test_rrf_fuse_duplicate_doc_id_merges_scores() {
        // doc-shared 在 dense rank=1, sparse rank=2
        let dense = vec!["doc-shared".to_string(), "doc-only-dense".to_string()];
        let sparse = vec!["doc-only-sparse".to_string(), "doc-shared".to_string()];
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 10);

        // 应有 3 个不同文档(doc-shared / doc-only-dense / doc-only-sparse)
        assert_eq!(results.len(), 3);

        // doc-shared 应同时有 dense_rank 和 sparse_rank
        let shared = results
            .iter()
            .find(|r| r.doc_id == "doc-shared")
            .expect("doc-shared 应存在");
        assert_eq!(shared.dense_rank, Some(1));
        assert_eq!(shared.sparse_rank, Some(2));

        // doc-shared 的 RRF 分数 = 1/(60+1) + 1/(60+2)
        let expected = 1.0f32 / (60.0 + 1.0) + 1.0f32 / (60.0 + 2.0);
        assert!(
            (shared.rrf_score - expected).abs() < 1e-6,
            "doc-shared RRF 分数错误:期望 {},实际 {}",
            expected,
            shared.rrf_score
        );

        // doc-only-dense 仅 dense_rank=2
        let only_dense = results
            .iter()
            .find(|r| r.doc_id == "doc-only-dense")
            .expect("doc-only-dense 应存在");
        assert_eq!(only_dense.dense_rank, Some(2));
        assert!(only_dense.sparse_rank.is_none());

        // doc-only-sparse 仅 sparse_rank=1
        let only_sparse = results
            .iter()
            .find(|r| r.doc_id == "doc-only-sparse")
            .expect("doc-only-sparse 应存在");
        assert!(only_sparse.dense_rank.is_none());
        assert_eq!(only_sparse.sparse_rank, Some(1));
    }

    /// Top-K 截断正确
    #[test]
    fn test_rrf_fuse_top_k_truncation() {
        // 5 个 dense 文档 + 5 个 sparse 文档(无重叠),共 10 个文档
        let dense: Vec<String> = (0..5).map(|i| format!("dense-{i}")).collect();
        let sparse: Vec<String> = (0..5).map(|i| format!("sparse-{i}")).collect();
        let config = HybridSearchConfig::default();

        // top_k = 3 应只返回 3 个文档(分数最高的 3 个)
        let results = rrf_fuse(&dense, &sparse, &config, 3);
        assert_eq!(results.len(), 3);

        // 结果应按 RRF 分数降序排列
        for i in 1..results.len() {
            assert!(
                results[i - 1].rrf_score >= results[i].rrf_score,
                "Top-K 结果应降序"
            );
        }

        // 前 3 应是 rank=1 的文档(dense-0 和 sparse-0 分数最高,二者相等;
        // 第 3 名是 rank=2 的文档之一)
        // dense-0: 1/(60+1) ≈ 0.01639
        // sparse-0: 1/(60+1) ≈ 0.01639
        // dense-1 / sparse-1: 1/(60+2) ≈ 0.01613
        let top_scores: Vec<f32> = results.iter().map(|r| r.rrf_score).collect();
        assert!(top_scores[0] >= top_scores[1], "Top-1 分数应 ≥ Top-2");
    }

    /// top_k = 0 返回空 Vec(边界条件)
    #[test]
    fn test_rrf_fuse_top_k_zero() {
        let dense = vec!["doc-1".to_string()];
        let sparse = vec!["doc-2".to_string()];
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 0);
        assert!(results.is_empty());
    }

    /// top_k 大于结果总数时返回全部(按降序)
    #[test]
    fn test_rrf_fuse_top_k_larger_than_results() {
        let dense = vec!["doc-1".to_string(), "doc-2".to_string()];
        let sparse: Vec<String> = Vec::new();
        let config = HybridSearchConfig::default();

        let results = rrf_fuse(&dense, &sparse, &config, 100);
        // 应返回全部 2 个文档
        assert_eq!(results.len(), 2);
        // 按降序排列
        assert_eq!(results[0].doc_id, "doc-1");
        assert_eq!(results[1].doc_id, "doc-2");
    }

    /// 验证 RRF 分数计算公式正确
    #[test]
    fn test_rrf_fuse_score_formula() {
        // 单文档在 dense rank=1,验证 RRF 分数 = 1/(k+1)
        let dense = vec!["doc-1".to_string()];
        let sparse: Vec<String> = Vec::new();
        let config = HybridSearchConfig::default(); // k=60

        let results = rrf_fuse(&dense, &sparse, &config, 10);
        assert_eq!(results.len(), 1);

        let expected = 1.0f32 / (60.0 + 1.0);
        assert!(
            (results[0].rrf_score - expected).abs() < 1e-6,
            "RRF 分数错误:期望 {},实际 {}",
            expected,
            results[0].rrf_score
        );
    }

    /// 验证权重配置影响 RRF 分数
    #[test]
    fn test_rrf_fuse_weights_affect_score() {
        let dense = vec!["doc-1".to_string()];
        let sparse = vec!["doc-1".to_string()];

        // 等权配置:dense_weight=1, sparse_weight=1
        let config_equal = HybridSearchConfig::default();
        let results_equal = rrf_fuse(&dense, &sparse, &config_equal, 10);
        let score_equal = results_equal[0].rrf_score;

        // dense 权重加倍:dense_weight=2, sparse_weight=1
        let config_dense_heavy = HybridSearchConfig::new(60, 2.0, 1.0);
        let results_dense_heavy = rrf_fuse(&dense, &sparse, &config_dense_heavy, 10);
        let score_dense_heavy = results_dense_heavy[0].rrf_score;

        // dense 权重加倍后,总分应更高
        assert!(
            score_dense_heavy > score_equal,
            "dense 权重加倍后 RRF 分数应更高:{} vs {}",
            score_dense_heavy,
            score_equal
        );
    }

    /// 验证 rrf_k 参数影响排名宽容度
    #[test]
    fn test_rrf_fuse_k_affects_rank_tolerance() {
        // doc-low 在 dense rank=100(低排名),doc-high 在 dense rank=1(高排名)
        let mut dense: Vec<String> = (1..=100).map(|i| format!("doc-{i}")).collect();
        // 确保 doc-low 在 rank=100
        // dense 已是 doc-1..doc-100,doc-1 是 rank=1,doc-100 是 rank=100

        // 小 k(10):低排名文档分数衰减快,doc-100 几乎无贡献
        let config_small_k = HybridSearchConfig::new(10, 1.0, 1.0);
        let results_small_k = rrf_fuse(&dense, &[], &config_small_k, 100);
        let score_high_small_k = results_small_k
            .iter()
            .find(|r| r.doc_id == "doc-1")
            .unwrap()
            .rrf_score;
        let score_low_small_k = results_small_k
            .iter()
            .find(|r| r.doc_id == "doc-100")
            .unwrap()
            .rrf_score;

        // 大 k(1000):低排名文档分数衰减慢,doc-100 相对分数更高
        let config_large_k = HybridSearchConfig::new(1000, 1.0, 1.0);
        let results_large_k = rrf_fuse(&dense, &[], &config_large_k, 100);
        let score_high_large_k = results_large_k
            .iter()
            .find(|r| r.doc_id == "doc-1")
            .unwrap()
            .rrf_score;
        let score_low_large_k = results_large_k
            .iter()
            .find(|r| r.doc_id == "doc-100")
            .unwrap()
            .rrf_score;

        // 比值:大 k 时 low/high 比值应大于小 k 时(低排名相对更接近高排名)
        let ratio_small_k = score_low_small_k / score_high_small_k;
        let ratio_large_k = score_low_large_k / score_high_large_k;
        assert!(
            ratio_large_k > ratio_small_k,
            "大 k 时低排名相对分数应更高:ratio_large={} vs ratio_small={}",
            ratio_large_k,
            ratio_small_k
        );

        // 防止 unused mut warning(dense 已用,无需额外处理)
        let _ = &mut dense;
    }

    /// 验证 hybrid_search 等价于 rrf_fuse
    #[test]
    fn test_hybrid_search_equivalent_to_rrf_fuse() {
        let dense = vec!["doc-1".to_string(), "doc-2".to_string()];
        let sparse = vec!["doc-2".to_string(), "doc-3".to_string()];
        let config = HybridSearchConfig::default();

        let r1 = rrf_fuse(&dense, &sparse, &config, 10);
        let r2 = hybrid_search(&dense, &sparse, &config, 10);

        assert_eq!(r1, r2);
    }
}
