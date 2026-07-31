//! RAG 混合检索融合(RRF)集成测试 — Task 3
//!
//! 对应架构层:L5 Knowledge
//! 对应 spec: optimize-context-sparse-rag / Task 3
//!
//! # 测试覆盖
//! - **公开 API 可访问性**:验证 `hybrid_search` / `rrf_fuse` / `HybridSearchConfig` /
//!   `HybridSearchResult` 从 `repo_wiki` crate 顶层正确导出
//! - **端到端混合检索**:WikiStore(FTS5 sparse)+ HnswStore(dense)→ RRF 融合,
//!   验证两路均命中的文档获得最高 RRF 分数
//! - **配置集成**:WikiConfig.hybrid_search 字段可通过 builder 配置,
//!   serde 反序列化向后兼容(旧配置无此字段时取默认值)
//! - **降级路径**:FTS5 不可用时仅用 dense 结果,HNSW 为空时仅用 sparse 结果
//!
//! # 设计原则
//! - 使用真实 WikiStore + HnswStore,而非 mock,验证真实检索管线的端到端正确性
//! - HnswStore 使用低维向量(4-dim)而非 512-dim,降低测试耗时并使结果可预测
//! - f32 分数比较使用容差(1e-6),避免浮点精度问题

#![forbid(unsafe_code)]

use nexus_contracts::VectorStore;
use repo_wiki::search::{hybrid_search, rrf_fuse, HybridSearchConfig};
use repo_wiki::{HnswStore, WikiConfig, WikiEntry, WikiStore};

// ============================================================
// 公开 API 可访问性测试
// ============================================================

/// 验证混合检索类型从 crate 顶层正确导出
#[test]
fn test_public_api_exports_accessible() {
    let config = HybridSearchConfig::default();
    assert_eq!(config.rrf_k, 60);

    let results = rrf_fuse(&["doc-1".to_string()], &["doc-1".to_string()], &config, 5);
    assert_eq!(results.len(), 1);
    // doc-1 在两路均命中,RRF 分数应 > 0
    assert!(results[0].rrf_score > 0.0);

    // hybrid_search 应等价于 rrf_fuse
    let results2 = hybrid_search(&["doc-1".to_string()], &["doc-1".to_string()], &config, 5);
    assert_eq!(results, results2);
}

// ============================================================
// 端到端混合检索测试
// ============================================================

/// 构建测试 Wiki 条目(512-dim 占位向量,用于 WikiStore 持久化)
fn make_entry(id: &str, title: &str, content: &str) -> WikiEntry {
    WikiEntry::new(id, title, content, vec![], vec![0.0; 512])
}

/// 端到端混合检索:WikiStore(FTS5) + HnswStore(dense) → RRF 融合
///
/// # 场景
/// 插入 5 个文档,内容涵盖 "tokio async" 与 "sqlite database" 两类主题:
/// - e-1: "Tokio runtime"(含 tokio + async 关键词,FTS5 高匹配)
/// - e-2: "Async scheduler"(含 async 关键词,FTS5 中匹配)
/// - e-3: "SQLite database"(不含 tokio/async,FTS5 不匹配)
/// - e-4: "Rust ownership"(不含 tokio/async,FTS5 不匹配)
/// - e-5: "Python asyncio"(含 async,FTS5 中匹配)
///
/// HNSW dense 检索使用 4-dim 向量,e-1 和 e-2 的向量与查询向量最相似。
///
/// # 预期
/// 查询 "tokio async" 时:
/// - FTS5(sparse)返回 e-1, e-2, e-5(含关键词的文档)
/// - HNSW(dense)返回 e-1, e-2, e-4(向量最相似的文档)
/// - RRF 融合后,e-1 和 e-2 在两路均命中,应排在最前
#[tokio::test]
async fn test_end_to_end_hybrid_search() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("hybrid.db")).unwrap();

    // 1. 插入 5 个文档到 WikiStore(用于 FTS5 sparse 检索)
    let entries = [
        make_entry("e-1", "Tokio runtime", "Tokio is an async runtime for Rust"),
        make_entry(
            "e-2",
            "Async scheduler",
            "The async scheduler drives futures in tokio",
        ),
        make_entry(
            "e-3",
            "SQLite database",
            "SQLite is an embedded database engine",
        ),
        make_entry(
            "e-4",
            "Rust ownership",
            "Ownership and borrowing are core to Rust",
        ),
        make_entry(
            "e-5",
            "Python asyncio",
            "Python asyncio provides async concurrency",
        ),
    ];
    for entry in &entries {
        store.insert(entry.clone()).await.unwrap();
    }

    // 2. 构建 HnswStore(4-dim)用于 dense 检索
    //    WHY 4-dim:低维使余弦相似度结果可预测,降低测试耗时
    let hnsw = HnswStore::with_dim(4);
    // e-1 和 e-2 的向量接近查询向量 [1.0, 0.9, 0.0, 0.0]
    hnsw.upsert("e-1", &[1.0, 0.9, 0.0, 0.0], ()).unwrap();
    hnsw.upsert("e-2", &[0.9, 1.0, 0.1, 0.0], ()).unwrap();
    hnsw.upsert("e-3", &[0.0, 0.0, 1.0, 0.9], ()).unwrap();
    hnsw.upsert("e-4", &[0.8, 0.7, 0.0, 0.1], ()).unwrap();
    hnsw.upsert("e-5", &[0.1, 0.0, 0.0, 1.0], ()).unwrap();

    // 3. 执行 FTS5 sparse 检索(查询 "tokio async")
    let fts_results = store
        .search_fulltext("tokio async".to_string())
        .await
        .unwrap();
    let sparse_results: Vec<String> = fts_results.iter().map(|e| e.entry_id.clone()).collect();

    // FTS5 应返回含 "tokio" 或 "async" 的文档(e-1, e-2, e-5)
    assert!(!sparse_results.is_empty(), "FTS5 应返回匹配文档");
    assert!(
        sparse_results.contains(&"e-1".to_string()),
        "e-1 含 tokio+async 应被 FTS5 命中"
    );

    // 4. 执行 HNSW dense 检索(查询向量接近 e-1/e-2)
    let query_vec = &[1.0f32, 0.9, 0.0, 0.0];
    let dense_hits = hnsw.top_k(query_vec, 5, "").unwrap();
    let dense_results: Vec<String> = dense_hits.iter().map(|h| h.id.clone()).collect();

    // HNSW 应返回所有 5 个文档,e-1 应排第一(查询向量与 e-1 完全相同)
    assert_eq!(dense_results.len(), 5);
    assert_eq!(dense_results[0], "e-1", "e-1 向量与查询相同,应排第一");

    // 5. RRF 融合两路结果
    let config = HybridSearchConfig::default();
    let fused = hybrid_search(&dense_results, &sparse_results, &config, 5);

    // 融合结果应包含所有文档(dense 返回全部 5 个)
    assert!(!fused.is_empty(), "融合结果不应为空");

    // e-1 在两路均命中(dense_rank=1, sparse_rank=1),应排第一
    assert_eq!(fused[0].doc_id, "e-1", "e-1 两路均 rank=1,RRF 分数应最高");
    assert!(fused[0].dense_rank.is_some(), "e-1 应有 dense_rank");
    assert!(fused[0].sparse_rank.is_some(), "e-1 应有 sparse_rank");

    // e-1 的 RRF 分数应严格大于仅在一路命中的文档
    let e1_score = fused[0].rrf_score;
    for r in fused.iter().skip(1) {
        assert!(
            e1_score > r.rrf_score,
            "e-1 RRF 分数 {} 应大于 {} 的 {}",
            e1_score,
            r.doc_id,
            r.rrf_score
        );
    }

    // 结果应按 RRF 分数降序排列
    for i in 1..fused.len() {
        assert!(
            fused[i - 1].rrf_score >= fused[i].rrf_score,
            "结果应降序:[{}]={} < [{}]={}",
            i - 1,
            fused[i - 1].rrf_score,
            i,
            fused[i].rrf_score
        );
    }
}

/// 验证两路均命中的文档 RRF 分数 = dense 贡献 + sparse 贡献
#[tokio::test]
async fn test_hybrid_search_score_accumulation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("score.db")).unwrap();

    // 插入 2 个文档:e-shared 含 "tokio"(FTS5 命中),e-dense-only 不含
    store
        .insert(make_entry("e-shared", "Tokio", "Tokio async runtime"))
        .await
        .unwrap();
    store
        .insert(make_entry("e-dense-only", "Other", "Unrelated content"))
        .await
        .unwrap();

    // HNSW:两文档都插入,e-shared 排第一
    let hnsw = HnswStore::with_dim(2);
    hnsw.upsert("e-shared", &[1.0, 0.0], ()).unwrap();
    hnsw.upsert("e-dense-only", &[0.5, 0.5], ()).unwrap();

    // dense 检索:e-shared rank=1, e-dense-only rank=2
    let dense_hits = hnsw.top_k(&[1.0, 0.0], 5, "").unwrap();
    let dense: Vec<String> = dense_hits.iter().map(|h| h.id.clone()).collect();

    // sparse 检索:仅 e-shared 命中(含 "tokio")
    let fts_results = store.search_fulltext("tokio".to_string()).await.unwrap();
    let sparse: Vec<String> = fts_results.iter().map(|e| e.entry_id.clone()).collect();

    let config = HybridSearchConfig::default(); // k=60, 等权 1.0
    let fused = rrf_fuse(&dense, &sparse, &config, 10);

    // e-shared 应同时有 dense_rank 和 sparse_rank
    let shared = fused.iter().find(|r| r.doc_id == "e-shared").unwrap();
    assert_eq!(shared.dense_rank, Some(1));
    assert!(shared.sparse_rank.is_some(), "e-shared 应被 FTS5 命中");

    // e-shared RRF 分数 = 1/(60+1) + 1/(60+sparse_rank)
    let expected = 1.0f32 / (60.0 + 1.0) + 1.0f32 / (60.0 + shared.sparse_rank.unwrap() as f32);
    assert!(
        (shared.rrf_score - expected).abs() < 1e-6,
        "e-shared RRF 分数错误:期望 {},实际 {}",
        expected,
        shared.rrf_score
    );

    // e-dense-only 仅 dense_rank=2,无 sparse_rank
    let dense_only = fused.iter().find(|r| r.doc_id == "e-dense-only").unwrap();
    assert_eq!(dense_only.dense_rank, Some(2));
    assert!(
        dense_only.sparse_rank.is_none(),
        "e-dense-only 不应被 FTS5 命中"
    );

    // e-shared 分数应高于 e-dense-only(两路贡献 vs 一路)
    assert!(
        shared.rrf_score > dense_only.rrf_score,
        "两路命中的 e-shared 分数应高于一路命中的 e-dense-only"
    );
}

// ============================================================
// 降级路径测试
// ============================================================

/// FTS5 不可用时仅用 dense 结果(传入空 sparse_results)
#[tokio::test]
async fn test_degradation_fts_unavailable() {
    let hnsw = HnswStore::with_dim(3);
    hnsw.upsert("d-1", &[1.0, 0.0, 0.0], ()).unwrap();
    hnsw.upsert("d-2", &[0.9, 0.1, 0.0], ()).unwrap();

    let dense_hits = hnsw.top_k(&[1.0, 0.0, 0.0], 5, "").unwrap();
    let dense: Vec<String> = dense_hits.iter().map(|h| h.id.clone()).collect();

    // FTS5 不可用:sparse_results 为空
    let sparse: Vec<String> = Vec::new();
    let config = HybridSearchConfig::default();
    let results = hybrid_search(&dense, &sparse, &config, 10);

    assert_eq!(results.len(), 2);
    // 所有结果应有 dense_rank,无 sparse_rank
    for r in &results {
        assert!(r.dense_rank.is_some(), "{} 应有 dense_rank", r.doc_id);
        assert!(r.sparse_rank.is_none(), "{} 不应有 sparse_rank", r.doc_id);
    }
    // 按 dense 排名顺序(d-1 rank=1 分数最高)
    assert_eq!(results[0].doc_id, "d-1");
    assert_eq!(results[0].dense_rank, Some(1));
}

/// HNSW 为空时仅用 sparse 结果(传入空 dense_results)
#[tokio::test]
async fn test_degradation_hnsw_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("degrade.db")).unwrap();
    store
        .insert(make_entry("s-1", "Rust", "Rust programming language"))
        .await
        .unwrap();
    store
        .insert(make_entry("s-2", "Tokio", "Tokio async runtime for Rust"))
        .await
        .unwrap();

    let fts_results = store.search_fulltext("rust".to_string()).await.unwrap();
    let sparse: Vec<String> = fts_results.iter().map(|e| e.entry_id.clone()).collect();

    // HNSW 为空:dense_results 为空
    let dense: Vec<String> = Vec::new();
    let config = HybridSearchConfig::default();
    let results = hybrid_search(&dense, &sparse, &config, 10);

    assert!(!results.is_empty(), "应有 sparse 结果");
    for r in &results {
        assert!(r.dense_rank.is_none(), "{} 不应有 dense_rank", r.doc_id);
        assert!(r.sparse_rank.is_some(), "{} 应有 sparse_rank", r.doc_id);
    }
}

// ============================================================
// 配置集成测试
// ============================================================

/// WikiConfig.hybrid_search 字段可通过 builder 配置
#[test]
fn test_wiki_config_hybrid_search_builder() {
    let config = WikiConfig::with_path("wiki.db")
        .hybrid_search_config(HybridSearchConfig::new(30, 1.5, 0.8));

    assert_eq!(config.hybrid_search.rrf_k, 30);
    assert!((config.hybrid_search.dense_weight - 1.5).abs() < 1e-6);
    assert!((config.hybrid_search.sparse_weight - 0.8).abs() < 1e-6);
}

/// WikiConfig 默认 hybrid_search 为默认配置(rrf_k=60,等权)
#[test]
fn test_wiki_config_hybrid_search_default() {
    let config = WikiConfig::with_path("wiki.db");
    assert_eq!(config.hybrid_search, HybridSearchConfig::default());
    assert_eq!(config.hybrid_search.rrf_k, 60);
    assert!((config.hybrid_search.dense_weight - 1.0).abs() < 1e-6);
    assert!((config.hybrid_search.sparse_weight - 1.0).abs() < 1e-6);
}

/// 旧配置文件(无 hybrid_search 段)反序列化为默认值(向后兼容)
#[test]
fn test_wiki_config_hybrid_search_backward_compat() {
    let old_json = r#"{
        "db_path": "wiki.db",
        "vector_dim": 512,
        "wal_enabled": true,
        "read_pool_size": 2,
        "fts_enabled": true
    }"#;
    let config: WikiConfig = serde_json::from_str(old_json).unwrap();
    assert_eq!(config.hybrid_search, HybridSearchConfig::default());
}

/// WikiConfig serde 往返保持 hybrid_search 配置
#[test]
fn test_wiki_config_hybrid_search_serde_roundtrip() {
    let config = WikiConfig::with_path("wiki.db")
        .hybrid_search_config(HybridSearchConfig::new(45, 1.2, 0.9));
    let json = serde_json::to_string(&config).unwrap();
    let de: WikiConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(de.hybrid_search, config.hybrid_search);
}
