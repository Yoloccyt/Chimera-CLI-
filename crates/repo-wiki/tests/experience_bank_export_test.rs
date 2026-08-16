//! 双层经验库 L3 协同导出接口集成测试（Phase 5 Wave 4，D-6）
//!
//! 覆盖: export_distilled_insights 导出接口 — 空库边界 / 蒸馏后导出 /
//! 支持度降序 / 导出与 global_search 一致性（职责边界: 不引入 cmt-tiering 依赖）

#![forbid(unsafe_code)]

use std::sync::Arc;

use repo_wiki::{DualExperienceBank, WikiEntry, WikiStore};

fn entry(id: &str, tags: Vec<&str>) -> WikiEntry {
    let tag_text = tags.join(" ");
    WikiEntry::new(
        id,
        format!("title-{id}"),
        format!("content of {id} {tag_text}"),
        tags.iter().map(|s| s.to_string()).collect(),
        vec![0.0; 512],
    )
}

/// 空库导出返回空 Vec（边界）
#[tokio::test]
async fn export_empty_bank_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));
    let exported = bank.export_distilled_insights().await.unwrap();
    assert!(exported.is_empty(), "空库导出应为空");
}

/// 蒸馏后导出全部洞察（D-6: 调用方据此持久化到 L3）
#[tokio::test]
async fn export_returns_all_distilled_insights() {
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

    let distilled = bank.distill_from_entries(2).await.unwrap();
    let exported = bank.export_distilled_insights().await.unwrap();
    // 导出集合与蒸馏持久化集合一致（数量与 ID 全覆盖）
    assert_eq!(exported.len(), distilled.len());
    for insight in &distilled {
        assert!(
            exported.iter().any(|e| e.insight_id == insight.insight_id),
            "导出应包含洞察 {}",
            insight.insight_id
        );
    }
}

/// 导出按支持度降序（高支持度洞察优先持久化）
#[tokio::test]
async fn export_sorted_by_source_count_desc() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    // rust 支持度 3，async 支持度 2
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

    bank.distill_from_entries(2).await.unwrap();
    let exported = bank.export_distilled_insights().await.unwrap();
    assert!(exported.len() >= 2);
    // 支持度降序不变量
    for pair in exported.windows(2) {
        assert!(
            pair[0].source_count >= pair[1].source_count,
            "导出应按 source_count 降序"
        );
    }
    assert_eq!(exported[0].source_count, 3, "最高支持度洞察排首位");
}

/// 重复蒸馏后导出不重复（UPSERT 幂等传导到导出接口）
#[tokio::test]
async fn export_idempotent_after_repeated_distill() {
    let tmp = tempfile::tempdir().unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    store.insert(entry("a", vec!["rust"])).await.unwrap();
    store.insert(entry("b", vec!["rust"])).await.unwrap();
    let bank = DualExperienceBank::new(Arc::new(store));

    bank.distill_from_entries(2).await.unwrap();
    let first = bank.export_distilled_insights().await.unwrap();
    // 重复蒸馏（UPSERT 幂等）
    bank.distill_from_entries(2).await.unwrap();
    let second = bank.export_distilled_insights().await.unwrap();
    assert_eq!(first.len(), second.len(), "重复蒸馏不应产生重复洞察");
}
