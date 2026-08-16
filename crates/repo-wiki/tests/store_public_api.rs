//! WikiStore 公共 API 集成测试(从 src/store.rs 内嵌测试外移,P1-3 计划 Task 7)
//!
//! 覆盖:open/journal_mode/memory 拒绝/insert/get/delete/list_by_tag/
//! search_fulltext/count/list_all/upsert/读写并发/写序列化/clone 共享/
//! spawn_blocking 非阻塞/并发正确性。
//!
//! 私有依赖测试(embedding blob 编解码)保留在 src/store.rs(依赖私有函数)。

use std::time::Duration;

use repo_wiki::{WikiConfig, WikiEntry, WikiStore};

#[tokio::test]
async fn test_open_and_journal_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();
    let mode = store.journal_mode().await.unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
}

/// 验证 `:memory:` 数据库被彻底拒绝。
///
/// WHY:SQLite `:memory:` 每个 Connection 是独立实例,读连接池无法
/// 看到写线程的数据;即使 read_pool_size=0,后续逻辑也会创建至少 1 个
/// 读连接,导致读操作看到的是空库。彻底拒绝可避免静默的数据"丢失"。
#[test]
fn test_open_memory_db_rejected() {
    let config = WikiConfig {
        db_path: std::path::PathBuf::from(":memory:"),
        vector_dim: 512,
        wal_enabled: false,
        read_pool_size: 0,
        fts_enabled: false,
        hnsw: repo_wiki::HnswConfig::default(),
        hybrid_search: repo_wiki::search::HybridSearchConfig::default(),
    };
    match WikiStore::open_with_config(config) {
        Err(err) => assert!(err.to_string().contains(":memory:")),
        Ok(_) => panic!(":memory: should be rejected; use a file path"),
    }
}

#[tokio::test]
async fn test_insert_and_get() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.5; 512]);
    store.insert(entry).await.unwrap();

    let fetched = store.get("e-1".to_string()).await.unwrap().unwrap();
    assert_eq!(fetched.entry_id, "e-1");
    assert_eq!(fetched.title, "标题");
    assert_eq!(fetched.tags, vec!["t".to_string()]);
    assert_eq!(fetched.embedding.len(), 512);
    assert!((fetched.embedding[0] - 0.5).abs() < 1e-6);
}

#[tokio::test]
async fn test_get_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();
    let result = store.get("nonexistent".to_string()).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    let entry = WikiEntry::new("e-1", "标题", "内容", vec![], vec![0.0; 512]);
    store.insert(entry).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 1);

    store.delete("e-1".to_string()).await.unwrap();
    assert_eq!(store.count().await.unwrap(), 0);
    assert!(store.get("e-1".to_string()).await.unwrap().is_none());
}

#[tokio::test]
async fn test_list_by_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    for i in 0..6 {
        let entry = WikiEntry::new(
            format!("e-{i}"),
            format!("Entry {i}"),
            "content",
            vec!["tag-0".into(), format!("tag-{i}")],
            vec![0.0; 512],
        );
        store.insert(entry).await.unwrap();
    }

    let tagged = store.list_by_tag("tag-0".to_string()).await.unwrap();
    assert_eq!(tagged.len(), 6);

    let tagged_1 = store.list_by_tag("tag-1".to_string()).await.unwrap();
    assert_eq!(tagged_1.len(), 1);
}

#[tokio::test]
async fn test_search_fulltext() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    let entry = WikiEntry::new(
        "e-1",
        "Rust 编程",
        "Rust 是一门系统级编程语言",
        vec![],
        vec![0.0; 512],
    );
    store.insert(entry).await.unwrap();

    let found = store.search_fulltext("Rust".to_string()).await.unwrap();
    assert!(!found.is_empty());

    let not_found = store
        .search_fulltext("nonexistent".to_string())
        .await
        .unwrap();
    assert!(not_found.is_empty());
}

#[tokio::test]
async fn test_count() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    assert_eq!(store.count().await.unwrap(), 0);
    for i in 0..5 {
        let entry = WikiEntry::new(format!("e-{i}"), "t", "c", vec![], vec![0.0; 512]);
        store.insert(entry).await.unwrap();
    }
    assert_eq!(store.count().await.unwrap(), 5);
}

#[tokio::test]
async fn test_list_all() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    for i in 0..3 {
        let entry = WikiEntry::new(
            format!("e-{i}"),
            format!("Entry {i}"),
            "content",
            vec![],
            vec![0.0; 512],
        );
        store.insert(entry).await.unwrap();
    }

    let all = store.list_all().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_upsert_replaces() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test.db");
    let store = WikiStore::open(&db_path).unwrap();

    let entry_v1 = WikiEntry::new("e-1", "v1", "c1", vec![], vec![0.0; 512]);
    store.insert(entry_v1).await.unwrap();

    let entry_v2 = WikiEntry::new("e-1", "v2", "c2", vec![], vec![0.0; 512]);
    store.insert(entry_v2).await.unwrap();

    assert_eq!(store.count().await.unwrap(), 1);
    let fetched = store.get("e-1".to_string()).await.unwrap().unwrap();
    assert_eq!(fetched.title, "v2");
}

/// 验证读操作可在写操作进行时并发完成,不被阻塞。
///
/// WHY:旧实现使用单 `Mutex<Connection>` 串行化所有读写;
/// 本测试一个任务持续写入,另一个任务持续读取,
/// 若读仍被写阻塞,`timeout` 会触发。
#[tokio::test]
async fn test_read_during_write_not_blocked() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_rw.db");
    let store = WikiStore::open(&db_path).unwrap();

    let writer = store.clone();
    let write_handle = tokio::spawn(async move {
        for i in 0..100 {
            let entry = WikiEntry::new(
                format!("e-{i}"),
                format!("Entry {i}"),
                "content",
                vec![],
                vec![0.0; 512],
            );
            writer.insert(entry).await.unwrap();
        }
    });

    let reader = store.clone();
    let read_handle = tokio::spawn(async move {
        for _ in 0..50 {
            tokio::time::timeout(Duration::from_millis(500), reader.count())
                .await
                .expect("读取在写期间被阻塞,超时")
                .expect("count 失败");
        }
    });

    // 两者应同时完成,读取不会因为写入而超时
    write_handle.await.unwrap();
    read_handle.await.unwrap();
}

/// 验证多个并发写入同一 entry_id 最终被序列化,状态一致。
///
/// WHY:写入线程序列化所有写操作,UPSERT 不会产生重复记录;
/// 本测试确保并发写不会破坏该不变量。
#[tokio::test]
async fn test_write_serializes() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_serial.db");
    let store = WikiStore::open(&db_path).unwrap();

    let mut handles = Vec::new();
    for i in 0..10 {
        let store_clone = store.clone();
        handles.push(tokio::spawn(async move {
            let entry = WikiEntry::new(
                "e-same",
                format!("title-{i}"),
                format!("content-{i}"),
                vec![],
                vec![0.0; 512],
            );
            store_clone.insert(entry).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(store.count().await.unwrap(), 1);
    let fetched = store.get("e-same".to_string()).await.unwrap().unwrap();
    assert!(fetched.title.starts_with("title-"));
}

/// 验证 `WikiStore::clone` 共享同一个写入线程与读连接池。
///
/// WHY:clone 不能创建新连接,否则跨 clone 的数据不可见且资源泄漏。
#[tokio::test]
async fn test_clone_shares_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_clone.db");
    let store = WikiStore::open(&db_path).unwrap();
    let cloned = store.clone();

    let entry = WikiEntry::new("e-clone", "clone-title", "content", vec![], vec![0.0; 512]);
    cloned.insert(entry).await.unwrap();

    let fetched = store.get("e-clone".to_string()).await.unwrap().unwrap();
    assert_eq!(fetched.title, "clone-title");
}

/// 回归测试:验证 SQLite 操作不阻塞 async runtime
///
/// WHY:若 SQLite 操作未用 spawn_blocking 包装,直接在 async 上下文中
/// 执行同步阻塞 I/O,会卡住 Tokio 工作线程,导致并发的 async 任务
/// 无法被调度。此测试在执行 list_all(可能较慢)的同时,并发运行
/// 一个轻量 async 任务,验证轻量任务能在超时时间内完成(说明
/// runtime 未被阻塞)。
#[tokio::test]
async fn test_spawn_blocking_does_not_block_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_blocking.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 预置数据(5 条)
    for i in 0..5 {
        let entry = WikiEntry::new(
            format!("e-{i}"),
            format!("Entry {i}"),
            format!("Content {i}"),
            vec![],
            vec![0.0; 512],
        );
        store.insert(entry).await.unwrap();
    }

    // 并发执行:WikiStore 操作 + 轻量 async 计时任务
    // 轻量任务仅做 yield + 简单计算,正常情况下应在 1ms 内完成
    // 若 SQLite 操作阻塞了 runtime,轻量任务会被拖延,触发超时
    let store_clone = store.clone();
    let db_task = tokio::spawn(async move {
        // 执行可能较慢的 SQLite 查询
        store_clone.list_all().await
    });

    // 轻量 async 任务:多次 yield 让出执行权
    // WHY:若 runtime 被阻塞,yield_now 无法被调度,任务无法完成
    let lightweight_task = tokio::time::timeout(Duration::from_millis(100), async {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        42
    })
    .await;

    // 轻量任务应在超时前完成(实际通常 < 1ms)
    assert!(
        lightweight_task.is_ok(),
        "轻量 async 任务超时 — SQLite 操作可能阻塞了 runtime"
    );
    assert_eq!(lightweight_task.unwrap(), 42);

    // 等待 DB 任务完成,验证功能正确性
    let entries = db_task
        .await
        .expect("db task join 失败")
        .expect("list_all 失败");
    assert_eq!(entries.len(), 5, "应列出 5 条条目");
}

/// 回归测试:验证并发场景下 spawn_blocking 的功能正确性
///
/// WHY:多个 spawn_blocking 任务并发执行时,读连接池可并行,
/// 写入线程串行化写操作,整体不应死锁或丢数据。
#[tokio::test]
async fn test_concurrent_operations_correctness() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_concurrent.db");
    let store = WikiStore::open(&db_path).unwrap();

    // 并发插入 10 条(每个任务独立 insert)
    let mut handles = Vec::new();
    for i in 0..10 {
        let store_clone = store.clone();
        handles.push(tokio::spawn(async move {
            let entry = WikiEntry::new(
                format!("e-{i}"),
                format!("Entry {i}"),
                format!("Content {i}"),
                vec![format!("tag-{}", i % 3)],
                vec![0.0; 512],
            );
            store_clone.insert(entry).await
        }));
    }

    // 等待所有插入完成
    for handle in handles {
        handle
            .await
            .expect("insert task join 失败")
            .expect("insert 失败");
    }

    // 验证最终一致性
    assert_eq!(store.count().await.unwrap(), 10, "应持久化 10 条");
    let all = store.list_all().await.unwrap();
    assert_eq!(all.len(), 10, "list_all 应返回 10 条");

    // 按 tag 验证(0,3,6,9 → tag-0;1,4,7 → tag-1;2,5,8 → tag-2)
    let tag0 = store.list_by_tag("tag-0".to_string()).await.unwrap();
    let tag1 = store.list_by_tag("tag-1".to_string()).await.unwrap();
    let tag2 = store.list_by_tag("tag-2".to_string()).await.unwrap();
    assert_eq!(tag0.len(), 4, "tag-0 应有 4 条");
    assert_eq!(tag1.len(), 3, "tag-1 应有 3 条");
    assert_eq!(tag2.len(), 3, "tag-2 应有 3 条");
}
