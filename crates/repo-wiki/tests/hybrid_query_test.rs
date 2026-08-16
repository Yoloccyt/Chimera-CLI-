//! P1-2:WikiStore::hybrid_query 混合检索集成测试
//!
//! 覆盖:
//! - sparse-only 退化模式(query_embedding = None)
//! - dense + sparse RRF 融合
//! - 查询嵌入维度不匹配的显式报错
//! - 惰性 HNSW 索引在写操作后脏重建

use repo_wiki::{WikiEntry, WikiError, WikiStore};

fn make_entry(id: &str, content: &str, embedding: Vec<f32>) -> WikiEntry {
    WikiEntry::new(id, "标题", content, vec!["test".into()], embedding)
}

fn dim_vec() -> Vec<f32> {
    vec![0.0; 512]
}

/// sparse-only 模式:无查询嵌入时走 FTS5 单通道,dense_rank 恒 None
#[tokio::test]
async fn test_hybrid_query_sparse_only_degradation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store
        .insert(make_entry("a", "Rust 异步编程入门", dim_vec()))
        .await
        .unwrap();
    store
        .insert(make_entry("b", "Rust 异步编程进阶", dim_vec()))
        .await
        .unwrap();
    store
        .insert(make_entry("c", "SQLite 调优指南", dim_vec()))
        .await
        .unwrap();

    let results = store.hybrid_query("异步编程", None, 5).await.unwrap();
    assert!(!results.is_empty());
    // 退化模式断言:sparse 路全命中,dense 路全未命中
    assert!(results.iter().all(|r| r.sparse_rank.is_some()));
    assert!(results.iter().all(|r| r.dense_rank.is_none()));
    // 命中条目 a 与 b(内容含"异步编程"),不含 c
    let ids: Vec<&str> = results.iter().map(|r| r.doc_id.as_str()).collect();
    assert!(ids.contains(&"a"));
    assert!(ids.contains(&"b"));
    assert!(!ids.contains(&"c"));
}

/// dense + sparse 融合:双通道命中的条目 RRF 分数最高排最前
#[tokio::test]
async fn test_hybrid_query_dense_sparse_fusion() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();

    // 构造向量:query 用 v_a;v_a 与自身余弦相似度 1.0 最高
    let v_a: Vec<f32> = (0..512)
        .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
        .collect();
    let v_b: Vec<f32> = (0..512)
        .map(|i| if i % 3 == 0 { 1.0 } else { 0.0 })
        .collect();
    let v_c: Vec<f32> = (0..512).map(|_| -1.0).collect();

    store
        .insert(make_entry("a", "向量检索语义召回", v_a.clone()))
        .await
        .unwrap();
    store
        .insert(make_entry("b", "向量检索混合融合", v_b.clone()))
        .await
        .unwrap();
    store
        .insert(make_entry("c", "数据库事务隔离", v_c.clone()))
        .await
        .unwrap();

    let results = store.hybrid_query("向量检索", Some(&v_a), 3).await.unwrap();
    assert!(!results.is_empty());
    // dense 通道命中(索引已惰性构建)
    assert!(results.iter().any(|r| r.dense_rank.is_some()));
    // 条目 a 两路命中(dense 相似度最高 + sparse 关键词命中),应排最前
    assert_eq!(results[0].doc_id, "a");
}

/// 查询嵌入维度与 WikiConfig.vector_dim 不一致时显式报错
#[tokio::test]
async fn test_hybrid_query_dimension_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store
        .insert(make_entry("a", "内容", dim_vec()))
        .await
        .unwrap();

    // 256 维查询向量 vs vector_dim=512
    let bad = vec![0.0; 256];
    let err = store.hybrid_query("内容", Some(&bad), 5).await.unwrap_err();
    assert!(matches!(
        err,
        WikiError::EmbeddingDimensionMismatch {
            expected: 512,
            actual: 256
        }
    ));
}

/// 写操作后惰性索引置脏,下次 dense 检索重建并召回新条目
#[tokio::test]
async fn test_hybrid_query_dirty_rebuild_after_insert() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();

    let v_a: Vec<f32> = (0..512)
        .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
        .collect();
    store
        .insert(make_entry("a", "检索", v_a.clone()))
        .await
        .unwrap();
    // 首次 dense 检索:惰性构建索引
    let r1 = store.hybrid_query("检索", Some(&v_a), 5).await.unwrap();
    assert!(r1.iter().any(|r| r.doc_id == "a"));

    // 新增条目后索引应置脏,下次检索重建并召回新条目
    let v_b: Vec<f32> = (0..512)
        .map(|i| if i % 2 == 0 { 1.0 } else { 0.0 })
        .collect();
    store
        .insert(make_entry("b", "检索", v_b.clone()))
        .await
        .unwrap();
    let r2 = store.hybrid_query("检索", Some(&v_a), 5).await.unwrap();
    assert!(r2.iter().any(|r| r.doc_id == "b"));
}
