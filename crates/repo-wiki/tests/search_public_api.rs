//! RRF 混合检索公共 API 单元测试(从 src/search.rs 内嵌测试外移,P1-3 计划 Task 7)
//!
//! 覆盖:HybridSearchConfig 默认值/构造/serde 兼容、rrf_fuse 全路径
//! (双路融合/dense-only/sparse-only/空输入/去重/Top-K 截断/分数公式/
//! 权重/k 宽容度)、hybrid_search 等价性。

use repo_wiki::search::{hybrid_search, rrf_fuse, HybridSearchConfig};

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
    let dense: Vec<String> = (1..=100).map(|i| format!("doc-{i}")).collect();

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
