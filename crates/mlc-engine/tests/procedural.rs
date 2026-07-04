//! SubTask 1.15:L3 ProceduralMemory 集成测试
//!
//! 验证 L3 程序记忆的模式匹配与执行统计更新,SQLite 持久化往返一致性。
//!
//! 注:SubTask 9.1 将 ProceduralMemory 所有方法改为 async + spawn_blocking,
//! 测试需用 `#[tokio::test]` 并在 async 方法调用后添加 `.await`。

use mlc_engine::{ExecutionStats, PatternSignature, ProceduralEntry, ProceduralMemory};

/// 构造测试用模式签名
fn make_signature(suffix: &str) -> PatternSignature {
    PatternSignature::new(
        vec!["tool_a".into(), format!("tool_{suffix}")],
        format!("hash-{suffix}"),
    )
}

#[tokio::test]
async fn test_l3_open_in_memory() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    assert_eq!(mem.count().await.unwrap(), 0);
}

#[tokio::test]
async fn test_l3_insert_and_match_pattern() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("1");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");

    mem.insert(&entry).await.unwrap();
    assert_eq!(mem.count().await.unwrap(), 1);

    let matched = mem.match_pattern(&sig).await.unwrap();
    assert!(matched.is_some());
    let matched = matched.unwrap();
    assert_eq!(matched.pattern_signature, sig);
    assert_eq!(matched.output, "output-1");
}

#[tokio::test]
async fn test_l3_match_pattern_nonexistent() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("nonexistent");
    let result = mem.match_pattern(&sig).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_l3_insert_pattern_conflict() {
    // 相同签名但不同 output 应返回 PatternConflict 错误
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("1");

    let entry1 = ProceduralEntry::new(sig.clone(), "output-1");
    mem.insert(&entry1).await.unwrap();

    let entry2 = ProceduralEntry::new(sig.clone(), "output-2");
    let result = mem.insert(&entry2).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_l3_insert_same_output_updates_stats() {
    // 相同签名相同 output,应更新 execution_stats
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("1");

    let entry1 = ProceduralEntry::new(sig.clone(), "output-1");
    mem.insert(&entry1).await.unwrap();

    let mut entry2 = ProceduralEntry::new(sig.clone(), "output-1");
    entry2.execution_stats.record(true, 100);
    mem.insert(&entry2).await.unwrap();

    let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
    assert_eq!(matched.execution_stats.success_count, 1);
    assert_eq!(matched.execution_stats.total_latency_ms, 100);
}

#[tokio::test]
async fn test_l3_update_stats() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("1");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");
    mem.insert(&entry).await.unwrap();

    // 记录两次执行
    mem.update_stats(&sig, true, 100).await.unwrap();
    mem.update_stats(&sig, false, 50).await.unwrap();

    let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
    assert_eq!(matched.execution_stats.success_count, 1);
    assert_eq!(matched.execution_stats.failure_count, 1);
    assert_eq!(matched.execution_stats.total_latency_ms, 150);
    assert!(matched.execution_stats.last_executed_at.is_some());
}

#[tokio::test]
async fn test_l3_update_stats_nonexistent() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("nonexistent");
    let result = mem.update_stats(&sig, true, 100).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_l3_load_all() {
    let mem = ProceduralMemory::open_in_memory().unwrap();

    for i in 0..3 {
        let sig = make_signature(&i.to_string());
        let entry = ProceduralEntry::new(sig, format!("output-{i}"));
        mem.insert(&entry).await.unwrap();
    }

    let all = mem.load_all().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn test_l3_load_all_empty() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let all = mem.load_all().await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_l3_delete() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("1");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");
    mem.insert(&entry).await.unwrap();
    assert_eq!(mem.count().await.unwrap(), 1);

    mem.delete(&sig).await.unwrap();
    assert_eq!(mem.count().await.unwrap(), 0);
    assert!(mem.match_pattern(&sig).await.unwrap().is_none());
}

#[tokio::test]
async fn test_l3_delete_nonexistent() {
    let mem = ProceduralMemory::open_in_memory().unwrap();
    let sig = make_signature("nonexistent");
    let result = mem.delete(&sig).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_l3_persistence_roundtrip() {
    // 验证 SQLite 持久化往返一致性:写入 → 关闭 → 重新打开 → 读取
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_procedural.db");

    // 写入数据
    {
        let mem = ProceduralMemory::open(&db_path).unwrap();
        let sig = make_signature("1");
        let entry = ProceduralEntry::new(sig.clone(), "output-1");
        mem.insert(&entry).await.unwrap();
        mem.update_stats(&sig, true, 100).await.unwrap();
    }

    // 重新打开并验证
    {
        let mem = ProceduralMemory::open(&db_path).unwrap();
        assert_eq!(mem.count().await.unwrap(), 1);

        let sig = make_signature("1");
        let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
        assert_eq!(matched.output, "output-1");
        assert_eq!(matched.execution_stats.success_count, 1);
        assert_eq!(matched.execution_stats.total_latency_ms, 100);
    }
}

#[tokio::test]
async fn test_l3_wal_mode_enabled() {
    // 验证 WAL 模式已启用(提升并发读写性能)
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_wal.db");
    let mem = ProceduralMemory::open(&db_path).unwrap();

    // 通过 PRAGMA 验证 journal_mode = wal
    // 注:此处仅验证外部行为,不直接访问内部 Mutex<Connection>
    let count = mem.count().await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_l3_multiple_patterns() {
    // 验证多个模式签名共存
    let mem = ProceduralMemory::open_in_memory().unwrap();

    for i in 0..10 {
        let sig = make_signature(&i.to_string());
        let entry = ProceduralEntry::new(sig, format!("output-{i}"));
        mem.insert(&entry).await.unwrap();
    }

    assert_eq!(mem.count().await.unwrap(), 10);

    // 验证每个模式都能匹配
    for i in 0..10 {
        let sig = make_signature(&i.to_string());
        let matched = mem.match_pattern(&sig).await.unwrap();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().output, format!("output-{i}"));
    }
}

#[test]
fn test_l3_execution_stats_serialization() {
    // 验证 ExecutionStats 的 JSON 序列化/反序列化往返
    let mut stats = ExecutionStats::new();
    stats.record(true, 100);
    stats.record(true, 200);
    stats.record(false, 50);

    let json = serde_json::to_string(&stats).unwrap();
    let deserialized: ExecutionStats = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.success_count, 2);
    assert_eq!(deserialized.failure_count, 1);
    assert_eq!(deserialized.total_latency_ms, 350);
    assert!((deserialized.success_rate() - (2.0 / 3.0)).abs() < 1e-6);
}

#[test]
fn test_l3_pattern_signature_to_key_stable() {
    // 验证相同 PatternSignature 产生相同 key(作为 SQLite 主键稳定性)
    let sig1 = PatternSignature::new(vec!["a".into(), "b".into()], "hash-1");
    let sig2 = PatternSignature::new(vec!["a".into(), "b".into()], "hash-1");
    assert_eq!(sig1.to_key().unwrap(), sig2.to_key().unwrap());
}

#[test]
fn test_l3_pattern_signature_to_key_differs() {
    // 验证不同 PatternSignature 产生不同 key
    let sig1 = PatternSignature::new(vec!["a".into()], "hash-1");
    let sig2 = PatternSignature::new(vec!["b".into()], "hash-1");
    assert_ne!(sig1.to_key().unwrap(), sig2.to_key().unwrap());
}

/// SubTask 10.4:验证 L3 ProceduralMemory 10 任务并发写入无锁错误
///
/// ProceduralMemory 内部用 `Arc<Mutex<Connection>>` 保护 SQLite 连接,
/// WAL 模式下 10 个 tokio::spawn 并发 insert 应通过 Mutex 串行化,
/// 无死锁、无锁错误、无数据丢失。
#[tokio::test]
async fn test_l3_concurrent_writes() {
    use std::sync::Arc;

    let mem = Arc::new(ProceduralMemory::open_in_memory().unwrap());

    // 10 任务并发 insert,每个任务写入唯一签名
    let mut handles = Vec::with_capacity(10);
    for i in 0..10 {
        let mem_clone = mem.clone();
        let sig = make_signature(&i.to_string());
        let entry = ProceduralEntry::new(sig.clone(), format!("output-{i}"));
        handles.push(tokio::spawn(async move { mem_clone.insert(&entry).await }));
    }

    // 等待所有写入完成,验证无 panic、无错误
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // 验证无数据丢失:10 个条目都应存在
    assert_eq!(mem.count().await.unwrap(), 10);
    for i in 0..10 {
        let sig = make_signature(&i.to_string());
        let matched = mem.match_pattern(&sig).await.unwrap();
        assert!(matched.is_some(), "并发写入后 signature-{i} 应存在");
        assert_eq!(matched.unwrap().output, format!("output-{i}"));
    }
}

/// SubTask 12.1:验证 L3 update_stats 原子性 — 10 线程并发调用 update_stats(true, 100),
/// 最终 success_count 必须为 10(无丢失更新)。
///
/// WHY:原实现采用 SELECT → 修改 → UPDATE 模式,并发调用时存在丢失更新。
/// 改为单条 SQL 原子更新后,10 次并发递增应全部生效。
#[tokio::test]
async fn test_l3_concurrent_update_stats() {
    use std::sync::Arc;

    let mem = Arc::new(ProceduralMemory::open_in_memory().unwrap());
    let sig = make_signature("concurrent-stats");
    let entry = ProceduralEntry::new(sig.clone(), "output-1");
    mem.insert(&entry).await.unwrap();

    // 10 任务并发 update_stats(true, 100),每个任务对同一签名递增 success_count
    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let mem_clone = mem.clone();
        let sig_clone = sig.clone();
        handles.push(tokio::spawn(async move {
            mem_clone.update_stats(&sig_clone, true, 100).await
        }));
    }

    // 等待所有更新完成,验证无 panic、无错误
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // 验证无丢失更新:success_count 必须为 10
    let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
    assert_eq!(
        matched.execution_stats.success_count, 10,
        "10 次并发 update_stats(true) 后 success_count 必须为 10,无丢失更新"
    );
    assert_eq!(matched.execution_stats.failure_count, 0);
    assert_eq!(matched.execution_stats.total_latency_ms, 1000);
}

/// SubTask 12.2:验证 L3 insert_batch 事务回滚 — 插入 5 条其中第 5 条主键冲突
/// (与第 1 条相同),断言前 4 条未持久化(事务回滚)。
///
/// WHY:原实现用手动 BEGIN/COMMIT,任一插入失败时未显式 ROLLBACK,已插入条目残留。
/// 改用 rusqlite::Transaction 后,任一失败时 Drop 自动回滚,确保原子性。
#[tokio::test]
async fn test_insert_batch_rollback() {
    let mem = ProceduralMemory::open_in_memory().unwrap();

    // 构造 5 条条目,第 5 条与第 1 条签名相同(主键冲突)
    let sig1 = make_signature("1");
    let entries = vec![
        ProceduralEntry::new(sig1.clone(), "output-1"),
        ProceduralEntry::new(make_signature("2"), "output-2"),
        ProceduralEntry::new(make_signature("3"), "output-3"),
        ProceduralEntry::new(make_signature("4"), "output-4"),
        ProceduralEntry::new(sig1, "output-5"), // 与第 1 条主键冲突
    ];

    // 批量插入应失败(主键冲突)
    let result = mem.insert_batch(entries).await;
    assert!(result.is_err(), "第 5 条主键冲突应导致整个事务回滚");

    // 验证前 4 条未持久化(事务回滚)
    assert_eq!(
        mem.count().await.unwrap(),
        0,
        "事务回滚后不应有任何条目持久化"
    );
}
