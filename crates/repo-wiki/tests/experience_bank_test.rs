//! P1-3(计划 Task 5):双层经验库测试(案例级 + 全局蒸馏,MemoHarness,文档 §10.2 问题 4)
//!
//! 覆盖:
//! - 规则式蒸馏:高频标签 → 全局洞察(支持度门控)
//! - 标签共现对 → 共现洞察
//! - 案例级检索(case_level_search 复用 WikiStore)
//! - 全局蒸馏检索(global_search)
//! - 蒸馏幂等(UPSERT 语义)

use std::sync::Arc;

use repo_wiki::experience_bank::DualExperienceBank;
use repo_wiki::{WikiEntry, WikiStore};

fn entry(id: &str, tags: Vec<&str>) -> WikiEntry {
    // WHY content 含标签词:FTS5 检索索引 title/content,不含标签词的条目
    // 无法通过关键词命中标签语义。
    let tag_text = tags.join(" ");
    WikiEntry::new(
        id,
        format!("title-{id}"),
        format!("content of {id} {tag_text}"),
        tags.iter().map(|s| s.to_string()).collect(),
        vec![0.0; 512],
    )
}

/// 高频标签产生全局洞察,低频标签被支持度门控过滤
#[tokio::test]
async fn test_distill_single_tag_insight() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store
        .insert(entry("a", vec!["rust", "async"]))
        .await
        .unwrap();
    store.insert(entry("b", vec!["rust"])).await.unwrap();
    store.insert(entry("c", vec!["sqlite"])).await.unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));

    let insights = bank.distill_from_entries(2).await.unwrap();
    // "rust" 支持度 2 ≥ min_support=2 → 单标签洞察
    let rust_insight = insights
        .iter()
        .find(|i| i.content.contains("rust") && !i.content.contains("async"))
        .expect("rust 高频标签应产生洞察");
    assert_eq!(rust_insight.source_count, 2);
    assert!((rust_insight.confidence - 2.0 / 3.0).abs() < 1e-6);
    // "sqlite" 支持度 1 < 2 → 不产生洞察
    assert!(
        !insights
            .iter()
            .any(|i| i.content.contains("sqlite") && i.source_count == 1),
        "低频标签不应产生洞察"
    );
}

/// 标签共现对(满足支持度)产生共现洞察
#[tokio::test]
async fn test_distill_cooccurrence_insight() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store
        .insert(entry("a", vec!["rust", "async"]))
        .await
        .unwrap();
    store
        .insert(entry("b", vec!["rust", "async"]))
        .await
        .unwrap();
    store.insert(entry("c", vec!["rust"])).await.unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));

    let insights = bank.distill_from_entries(2).await.unwrap();
    // (async, rust) 共现 2 次 → 共现洞察
    let pair = insights
        .iter()
        .find(|i| i.content.contains("async") && i.content.contains("共现"))
        .expect("高频共现对应产生洞察");
    assert_eq!(pair.source_count, 2);
    // 共现洞察携带两个标签
    assert!(pair.tags.contains(&"rust".to_string()));
    assert!(pair.tags.contains(&"async".to_string()));
}

/// 案例级检索复用 WikiStore 返回 WikiEntry
#[tokio::test]
async fn test_case_level_search_returns_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store.insert(entry("a", vec!["rust"])).await.unwrap();
    store.insert(entry("b", vec!["sqlite"])).await.unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));

    let entries = bank.case_level_search("rust", 5).await.unwrap();
    // 条目 a 携带 rust 标签且 content 含 rust → 命中
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].entry_id, "a");
}

/// 全局蒸馏检索按标签/内容匹配返回洞察
#[tokio::test]
async fn test_global_search_returns_insights() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store.insert(entry("a", vec!["rust"])).await.unwrap();
    store.insert(entry("b", vec!["rust"])).await.unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));

    bank.distill_from_entries(2).await.unwrap();
    let insights = bank.global_search("rust", 5).await.unwrap();
    assert!(!insights.is_empty());
    assert!(insights.iter().all(|i| i.content.contains("rust")));
}

/// 蒸馏幂等:重复蒸馏不产生重复洞察(UPSERT 语义)
#[tokio::test]
async fn test_distill_upsert_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store.insert(entry("a", vec!["rust"])).await.unwrap();
    store.insert(entry("b", vec!["rust"])).await.unwrap();
    // WHY clone:WikiStore::clone 共享同一写线程与读连接池,测试末尾仍需
    // 用 store 直接查询蒸馏表,故以 clone 进入 Arc。
    let bank = DualExperienceBank::new(Arc::new(store.clone()));

    bank.distill_from_entries(2).await.unwrap();
    bank.distill_from_entries(2).await.unwrap();

    let insights = store.list_distilled_insights().await.unwrap();
    let rust_count = insights
        .iter()
        .filter(|i| i.content.contains("rust"))
        .count();
    assert_eq!(rust_count, 1, "重复蒸馏应覆盖而非追加");
}
