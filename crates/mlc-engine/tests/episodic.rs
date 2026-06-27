//! SubTask 1.13:L1 EpisodicMemory 集成测试
//!
//! 验证 L1 情节记忆的时间范围查询与 Quest 关联查询。

use chrono::{DateTime, Duration, Utc};

use mlc_engine::{EpisodicMemory, MemoryEntry, MemoryTier};

/// 构造测试用情节记忆条目(指定时间戳)
fn make_entry(id: &str, ts: DateTime<Utc>) -> MemoryEntry {
    let mut entry = MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L1Episodic);
    entry.created_at = ts;
    entry.last_accessed_at = ts;
    entry
}

/// 构造带 Quest 关联的情节记忆条目
fn make_quest_entry(id: &str, ts: DateTime<Utc>, quest_id: &str) -> MemoryEntry {
    make_entry(id, ts).with_quest(quest_id)
}

#[test]
fn test_l1_capacity_default() {
    let mem = EpisodicMemory::new(1024);
    assert_eq!(mem.capacity(), 1024);
    assert_eq!(mem.len().unwrap(), 0);
    assert!(mem.is_empty().unwrap());
}

#[test]
fn test_l1_insert_and_get() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();
    let entry = make_entry("m-1", now);
    mem.insert(entry.clone()).unwrap();

    let fetched = mem.get("m-1").unwrap();
    assert_eq!(fetched.id.as_str(), "m-1");
    assert_eq!(fetched.tier, MemoryTier::L1Episodic);
}

#[test]
fn test_l1_get_nonexistent_returns_error() {
    let mem = EpisodicMemory::new(1024);
    let result = mem.get("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_l1_query_range() {
    let mem = EpisodicMemory::new(1024);
    let base = Utc::now();

    // 插入 3 个条目,时间戳分别为 base, base+1s, base+2s
    mem.insert(make_entry("m-1", base)).unwrap();
    mem.insert(make_entry("m-2", base + Duration::seconds(1)))
        .unwrap();
    mem.insert(make_entry("m-3", base + Duration::seconds(2)))
        .unwrap();

    // 查询 [base, base+2s) 应返回 m-1 和 m-2
    let results = mem.query_range(base, base + Duration::seconds(2)).unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|e| e.id.as_str() == "m-1"));
    assert!(results.iter().any(|e| e.id.as_str() == "m-2"));
    assert!(!results.iter().any(|e| e.id.as_str() == "m-3"));
}

#[test]
fn test_l1_query_range_empty() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();
    let results = mem.query_range(now, now + Duration::seconds(60)).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_l1_query_range_full() {
    let mem = EpisodicMemory::new(1024);
    let base = Utc::now();

    mem.insert(make_entry("m-1", base)).unwrap();
    mem.insert(make_entry("m-2", base + Duration::seconds(1)))
        .unwrap();
    mem.insert(make_entry("m-3", base + Duration::seconds(2)))
        .unwrap();

    // 查询 [base, base+60s) 应返回全部 3 个
    let results = mem.query_range(base, base + Duration::seconds(60)).unwrap();
    assert_eq!(results.len(), 3);
}

#[test]
fn test_l1_query_by_quest() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();

    // 插入 3 个条目,2 个属于 quest-A,1 个属于 quest-B
    mem.insert(make_quest_entry("m-1", now, "quest-A")).unwrap();
    mem.insert(make_quest_entry("m-2", now, "quest-A")).unwrap();
    mem.insert(make_quest_entry("m-3", now, "quest-B")).unwrap();

    let quest_a = mem.query_by_quest("quest-A").unwrap();
    assert_eq!(quest_a.len(), 2);
    assert!(quest_a
        .iter()
        .all(|e| e.quest_id.as_deref() == Some("quest-A")));

    let quest_b = mem.query_by_quest("quest-B").unwrap();
    assert_eq!(quest_b.len(), 1);

    // 不存在的 Quest 返回空
    let quest_c = mem.query_by_quest("quest-C").unwrap();
    assert!(quest_c.is_empty());
}

#[test]
fn test_l1_fifo_eviction_on_overflow() {
    let mem = EpisodicMemory::new(2);
    let base = Utc::now();

    mem.insert(make_entry("m-1", base)).unwrap();
    mem.insert(make_entry("m-2", base + Duration::seconds(1)))
        .unwrap();
    assert_eq!(mem.len().unwrap(), 2);

    // 插入第 3 个条目,应驱逐 m-1(最旧)
    let evicted = mem
        .insert(make_entry("m-3", base + Duration::seconds(2)))
        .unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));
    assert_eq!(mem.evictions(), 1);
    assert_eq!(mem.len().unwrap(), 2);

    assert!(mem.get("m-1").is_err());
    assert!(mem.get("m-2").is_ok());
    assert!(mem.get("m-3").is_ok());
}

#[test]
fn test_l1_fifo_eviction_preserves_quest_index() {
    // 验证 FIFO 驱逐后 Quest 索引正确清理
    let mem = EpisodicMemory::new(2);
    let base = Utc::now();

    mem.insert(make_quest_entry("m-1", base, "quest-A"))
        .unwrap();
    mem.insert(make_quest_entry(
        "m-2",
        base + Duration::seconds(1),
        "quest-A",
    ))
    .unwrap();

    // 插入第 3 个条目,驱逐 m-1
    mem.insert(make_entry("m-3", base + Duration::seconds(2)))
        .unwrap();

    // quest-A 应只剩 m-2
    let quest_a = mem.query_by_quest("quest-A").unwrap();
    assert_eq!(quest_a.len(), 1);
    assert_eq!(quest_a[0].id.as_str(), "m-2");
}

#[test]
fn test_l1_remove() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();
    mem.insert(make_quest_entry("m-1", now, "quest-A")).unwrap();

    let removed = mem.remove("m-1").unwrap();
    assert!(removed.is_some());

    // 移除后 Quest 索引也应清理
    let quest_a = mem.query_by_quest("quest-A").unwrap();
    assert!(quest_a.is_empty());
}

#[test]
fn test_l1_clear() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();
    for i in 0..5 {
        mem.insert(make_entry(&format!("m-{i}"), now)).unwrap();
    }
    assert_eq!(mem.len().unwrap(), 5);

    mem.clear().unwrap();
    assert_eq!(mem.len().unwrap(), 0);
}

#[test]
fn test_l1_list_all() {
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();
    for i in 0..3 {
        mem.insert(make_entry(&format!("m-{i}"), now)).unwrap();
    }
    let all = mem.list_all().unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn test_l1_update_existing_no_eviction() {
    let mem = EpisodicMemory::new(2);
    let now = Utc::now();
    mem.insert(make_entry("m-1", now)).unwrap();
    mem.insert(make_entry("m-2", now)).unwrap();

    // 更新已存在的 m-1,不应触发驱逐
    let evicted = mem.insert(make_entry("m-1", now)).unwrap();
    assert!(evicted.is_none());
    assert_eq!(mem.len().unwrap(), 2);
}

#[test]
fn test_l1_multiple_quests_same_entry() {
    // 验证同一时间戳可插入多个条目(BTreeMap 时间索引用 Vec 聚合)
    let mem = EpisodicMemory::new(1024);
    let now = Utc::now();

    mem.insert(make_entry("m-1", now)).unwrap();
    mem.insert(make_entry("m-2", now)).unwrap();
    mem.insert(make_entry("m-3", now)).unwrap();

    // 查询 [now, now+1s) 应返回全部 3 个
    let results = mem.query_range(now, now + Duration::seconds(1)).unwrap();
    assert_eq!(results.len(), 3);
}

/// SubTask 10.6:验证 L1 EpisodicMemory 4 线程并发 insert + query_range 无 panic
///
/// EpisodicMemory 内部用 `RwLock` 保护三索引(entries/time_index/quest_index),
/// 读操作用 `read()`(允许多并发),写操作用 `write()`(独占)。
/// 4 线程并发 insert + query_range 应无 panic、无死锁。
/// 使用 `std::thread::spawn`(非 async,因为 L1 方法是同步的)。
#[test]
fn test_l1_concurrent_insert_and_query() {
    use std::sync::Arc;
    use std::thread;

    let mem = Arc::new(EpisodicMemory::new(4096));
    let base = Utc::now();

    // 4 线程并发:2 个 insert + 2 个 query_range
    let mut handles = Vec::with_capacity(4);

    // 2 个 insert 线程,每个插入 25 个条目
    for tid in 0..2 {
        let mem_clone = mem.clone();
        let ts = base + Duration::seconds(tid as i64);
        handles.push(thread::spawn(move || {
            for i in 0..25 {
                let id = format!("t{tid}-m{i}");
                mem_clone.insert(make_entry(&id, ts)).unwrap();
            }
        }));
    }

    // 2 个 query_range 线程,每个查询 10 次
    for _ in 0..2 {
        let mem_clone = mem.clone();
        let start = base;
        let end = base + Duration::seconds(60);
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                // query_range 可能返回部分结果(若 insert 尚未完成),但不应 panic
                let _ = mem_clone.query_range(start, end).unwrap();
            }
        }));
    }

    // 等待所有线程完成,验证无 panic
    for handle in handles {
        handle.join().expect("L1 并发线程不应 panic");
    }

    // 验证所有插入的条目都存在(无数据丢失)
    assert_eq!(mem.len().unwrap(), 50, "2 线程 × 25 条目 = 50 个条目");
}
