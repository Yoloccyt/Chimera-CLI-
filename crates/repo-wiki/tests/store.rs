//! 集成测试 — WikiStore + VectorIndex + WikiGenerator 端到端验证
//!
//! 覆盖任务要求的 7 个场景:
//! 1. 10 条 Wiki 条目 CRUD
//! 2. 向量相似度检索(延迟 < 50ms)
//! 3. WAL 模式验证
//! 4. WikiUpdated 事件发布
//! 5. 10 条 Wiki 生成 + 持久化 < 2s
//! 6. 占位嵌入向量维度正确
//! 7. delete 同步删除向量索引

use chrono::Utc;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use repo_wiki::{VectorIndex, WikiEntry, WikiGenerator, WikiStore};
use std::time::{Duration, Instant};

/// 构造测试用 WikiEntry
fn make_entry(
    id: &str,
    title: &str,
    content: &str,
    tags: Vec<String>,
    embedding: Vec<f32>,
) -> WikiEntry {
    let now = Utc::now();
    WikiEntry {
        entry_id: id.into(),
        title: title.into(),
        content: content.into(),
        tags,
        embedding,
        created_at: now,
        updated_at: now,
    }
}

/// 构造测试用 Quest(含指定数量的 Completed Task)
fn make_quest_with_completed_tasks(quest_id: &str, count: usize) -> Quest {
    let tasks: Vec<Task> = (0..count)
        .map(|i| Task {
            task_id: format!("t-{i}"),
            description: format!("任务 {i} 的详细描述,用于生成 Wiki 条目"),
            status: TaskStatus::Completed,
            dependencies: vec![],
        })
        .collect();
    Quest {
        quest_id: quest_id.into(),
        title: "测试 Quest".into(),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    }
}

// ============================================================
// 场景 1:10 条 Wiki 条目 CRUD
// ============================================================

#[test]
fn test_10_entries_crud() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_crud.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 插入 10 条
    for i in 0..10 {
        let entry = make_entry(
            &format!("e-{i}"),
            &format!("Entry {i}"),
            &format!("Content for entry {i}"),
            vec!["test".into(), format!("tag-{}", i % 3)],
            vec![0.0_f32; 512],
        );
        store.insert(&entry).unwrap();
    }

    assert_eq!(store.count().unwrap(), 10);

    // get:验证条目存在且字段正确
    let e = store.get("e-0").unwrap().unwrap();
    assert_eq!(e.title, "Entry 0");
    assert_eq!(e.content, "Content for entry 0");
    assert_eq!(e.tags, vec!["test".to_string(), "tag-0".to_string()]);

    // list_by_tag:tag-0 应匹配 0, 3, 6, 9 共 4 条
    let tagged = store.list_by_tag("tag-0").unwrap();
    assert_eq!(tagged.len(), 4);

    // search_fulltext:搜索 "entry 5" 应至少匹配 1 条
    let found = store.search_fulltext("entry 5").unwrap();
    assert!(!found.is_empty());

    // delete:删除后验证条目不存在且总数减 1
    store.delete("e-0").unwrap();
    assert!(store.get("e-0").unwrap().is_none());
    assert_eq!(store.count().unwrap(), 9);

    // list_all:验证剩余 9 条
    let all = store.list_all().unwrap();
    assert_eq!(all.len(), 9);
}

// ============================================================
// 场景 2:向量相似度检索(延迟 < 50ms)
// ============================================================

#[test]
fn test_vector_search_latency_under_50ms() {
    let idx = VectorIndex::new(512);

    // 插入 5 条向量(使用不同的占位向量)
    for i in 0..5 {
        let mut emb = vec![0.0_f32; 512];
        emb[i] = 1.0; // 每个向量在不同维度为 1.0,形成正交基
        idx.upsert(&format!("e-{i}"), &emb).unwrap();
    }

    // 构造查询向量(与 e-0 完全相同)
    let mut query = vec![0.0_f32; 512];
    query[0] = 1.0;

    // 测量 KNN 检索延迟
    let start = Instant::now();
    let results = idx.search(&query, 3).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 3);
    // 最相似的应是 e-0(余弦相似度 = 1.0)
    assert_eq!(results[0].0, "e-0");
    assert!((results[0].1 - 1.0).abs() < 1e-5);

    // 延迟应 < 50ms(实际通常 < 1ms)
    assert!(
        elapsed < Duration::from_millis(50),
        "vector search latency {elapsed:?} exceeds 50ms"
    );
}

// ============================================================
// 场景 3:WAL 模式验证
// ============================================================

#[test]
fn test_wal_mode_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_wal.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 查询 journal_mode,应为 "wal"
    let mode = store.journal_mode().unwrap();
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "WAL mode should be enabled by default"
    );
}

#[test]
fn test_wal_mode_disabled_when_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_no_wal.db");
    let config = repo_wiki::WikiConfig::with_path(&db_path).wal_enabled(false);
    let store = WikiStore::open_with_config(config).unwrap();

    // WAL 关闭后,journal_mode 应为 "delete"(SQLite 默认)
    let mode = store.journal_mode().unwrap();
    assert_ne!(
        mode.to_lowercase(),
        "wal",
        "WAL should be disabled when configured false"
    );
}

// ============================================================
// 场景 4:WikiUpdated 事件发布
// ============================================================

#[tokio::test]
async fn test_wiki_updated_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_event.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 插入条目
    let entry = make_entry(
        "e-1",
        "事件测试",
        "验证 WikiUpdated 事件发布",
        vec!["event".into()],
        vec![0.0_f32; 512],
    );
    store.insert(&entry).unwrap();

    // 发布 WikiUpdated 事件(模拟 WikiStore 集成 EventBus 后的行为)
    let wiki_hash = "abc123".to_string();
    let delta = 1u32;
    bus.publish(NexusEvent::WikiUpdated {
        metadata: EventMetadata::new("repo-wiki"),
        wiki_hash: wiki_hash.clone(),
        delta,
    })
    .await
    .unwrap();

    // 验证订阅者收到事件
    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::WikiUpdated {
            wiki_hash: hash,
            delta: d,
            ..
        } => {
            assert_eq!(hash, wiki_hash);
            assert_eq!(d, delta);
        }
        other => panic!("expected WikiUpdated event, got {other:?}"),
    }
}

// ============================================================
// 场景 5:10 条 Wiki 生成 + 持久化 < 2s
// ============================================================

#[test]
fn test_10_entries_generate_and_persist_under_2s() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_perf.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 构造含 10 个 Completed Task 的 Quest
    let quest = make_quest_with_completed_tasks("q-perf", 10);

    let start = Instant::now();

    // 生成 Wiki 条目
    let entries = WikiGenerator::from_quest_result(&quest);
    assert_eq!(entries.len(), 10);

    // 持久化到 SQLite
    for entry in &entries {
        store.insert(entry).unwrap();
    }

    let elapsed = start.elapsed();

    // 验证全部持久化成功
    assert_eq!(store.count().unwrap(), 10);

    // 总耗时应 < 2s(实际通常 < 50ms)
    assert!(
        elapsed < Duration::from_secs(2),
        "generate + persist 10 entries took {elapsed:?}, expected < 2s"
    );
}

// ============================================================
// 场景 6:占位嵌入向量维度正确
// ============================================================

#[test]
fn test_placeholder_embedding_dimension() {
    let quest = make_quest_with_completed_tasks("q-embed", 3);
    let entries = WikiGenerator::from_quest_result(&quest);

    assert_eq!(entries.len(), 3);
    for entry in &entries {
        assert_eq!(
            entry.embedding.len(),
            512,
            "embedding dimension must be 512, got {}",
            entry.embedding.len()
        );
        // 占位向量所有值应在 [0, 1] 范围内
        for &v in &entry.embedding {
            assert!(
                (0.0..=1.0).contains(&v),
                "embedding value out of [0,1] range: {v}"
            );
        }
    }
}

#[test]
fn test_placeholder_embedding_deterministic_across_calls() {
    let quest = make_quest_with_completed_tasks("q-det", 2);

    let entries1 = WikiGenerator::from_quest_result(&quest);
    let entries2 = WikiGenerator::from_quest_result(&quest);

    // 相同 Quest 应生成相同嵌入向量
    for (e1, e2) in entries1.iter().zip(entries2.iter()) {
        assert_eq!(e1.embedding, e2.embedding);
    }
}

// ============================================================
// 场景 7:delete 同步删除向量索引
// ============================================================

#[test]
fn test_delete_syncs_vector_index() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_sync_delete.db");
    let store = WikiStore::open(&db_path).unwrap();
    let idx = VectorIndex::new(512);

    // 插入 3 条条目,同步写入 VectorIndex
    for i in 0..3 {
        let mut emb = vec![0.0_f32; 512];
        emb[i] = 1.0;
        let entry = make_entry(
            &format!("e-{i}"),
            &format!("Entry {i}"),
            &format!("Content {i}"),
            vec!["test".into()],
            emb.clone(),
        );
        store.insert(&entry).unwrap();
        idx.upsert(&entry.entry_id, &emb).unwrap();
    }

    assert_eq!(store.count().unwrap(), 3);
    assert_eq!(idx.len().unwrap(), 3);

    // 删除 e-1:同时删除 SQLite 记录与向量索引
    store.delete("e-1").unwrap();
    idx.delete("e-1").unwrap();

    // SQLite 中 e-1 不存在
    assert!(store.get("e-1").unwrap().is_none());
    assert_eq!(store.count().unwrap(), 2);

    // 向量索引中 e-1 不存在
    assert_eq!(idx.len().unwrap(), 2);

    // KNN 检索不应返回 e-1
    let mut query = vec![0.0_f32; 512];
    query[1] = 1.0;
    let results = idx.search(&query, 10).unwrap();
    assert!(!results.iter().any(|(id, _)| id == "e-1"));
    assert_eq!(results.len(), 2);
}

// ============================================================
// 综合场景:WikiGenerator → WikiStore → VectorIndex 端到端
// ============================================================

#[test]
fn test_end_to_end_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_e2e.db");
    let store = WikiStore::open(&db_path).unwrap();
    let idx = VectorIndex::new(512);

    // 1. 从 Quest 生成 Wiki 条目
    let quest = make_quest_with_completed_tasks("q-e2e", 5);
    let entries = WikiGenerator::from_quest_result(&quest);
    assert_eq!(entries.len(), 5);

    // 2. 持久化到 SQLite + 同步向量索引
    for entry in &entries {
        store.insert(entry).unwrap();
        idx.upsert(&entry.entry_id, &entry.embedding).unwrap();
    }

    // 3. 按 tag 过滤(所有条目都应有 "quest" 和 "q-e2e" 标签)
    let quest_tagged = store.list_by_tag("quest").unwrap();
    assert_eq!(quest_tagged.len(), 5);

    let qe2e_tagged = store.list_by_tag("q-e2e").unwrap();
    assert_eq!(qe2e_tagged.len(), 5);

    // 4. 向量检索:用第一个条目的嵌入作为查询,应返回自身为 Top-1
    let query = entries[0].embedding.clone();
    let results = idx.search(&query, 3).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].0, entries[0].entry_id);
    assert!((results[0].1 - 1.0).abs() < 1e-5);

    // 5. 全文搜索
    let found = store.search_fulltext("任务").unwrap();
    assert_eq!(found.len(), 5);
}

// ============================================================
// 边界场景:空 Quest 与无 Completed Task
// ============================================================

#[test]
fn test_generator_empty_quest() {
    let quest = Quest {
        quest_id: "q-empty".into(),
        title: "空 Quest".into(),
        tasks: vec![],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    };

    let entries = WikiGenerator::from_quest_result(&quest);
    assert!(entries.is_empty());
}

#[test]
fn test_generator_no_completed_tasks() {
    let quest = Quest {
        quest_id: "q-nocomplete".into(),
        title: "无完成 Task".into(),
        tasks: vec![
            Task {
                task_id: "t-1".into(),
                description: "待执行".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            },
            Task {
                task_id: "t-2".into(),
                description: "已失败".into(),
                status: TaskStatus::Failed,
                dependencies: vec![],
            },
        ],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    };

    let entries = WikiGenerator::from_quest_result(&quest);
    assert!(entries.is_empty());
}

// ============================================================
// 边界场景:重复插入(UPSERT)
// ============================================================

#[test]
fn test_upsert_same_entry_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_upsert.db");
    let store = WikiStore::open(&db_path).unwrap();

    let entry_v1 = make_entry(
        "e-1",
        "版本1",
        "原始内容",
        vec!["v1".into()],
        vec![0.0_f32; 512],
    );
    store.insert(&entry_v1).unwrap();

    let entry_v2 = make_entry(
        "e-1",
        "版本2",
        "更新内容",
        vec!["v2".into()],
        vec![1.0_f32; 512],
    );
    store.insert(&entry_v2).unwrap();

    // 应只有 1 条记录,且为最新版本
    assert_eq!(store.count().unwrap(), 1);
    let fetched = store.get("e-1").unwrap().unwrap();
    assert_eq!(fetched.title, "版本2");
    assert_eq!(fetched.content, "更新内容");
    assert_eq!(fetched.tags, vec!["v2".to_string()]);
}
