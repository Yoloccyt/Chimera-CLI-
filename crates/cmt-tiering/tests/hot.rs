//! Hot 层单元测试 — 验证 DashMap + LRU 驱逐策略
//!
//! 对应 SubTask 3.12:验证 Hot 层 LRU 驱逐(插入 257 条目后最久未访问的被驱逐)
//!
//! # 测试覆盖
//! - 基础 CRUD:insert / get / peek / remove / contains
//! - LRU 驱逐:容量满时驱逐最久未访问的条目
//! - 257 条目驱逐基准:插入 257 条目后最久未访问的被驱逐(任务要求)
//! - peek 不更新访问时间(与 get 区分)
//! - 更新已存在条目不触发驱逐
//! - 计数与清空操作

use cmt_tiering::{CapabilityEntry, CmtError, HotTier, Tier};

/// 构造测试用能力条目
fn make_entry(id: &str) -> CapabilityEntry {
    CapabilityEntry::new(id, format!("content-{id}"), Tier::Hot)
}

#[test]
fn test_hot_capacity_and_len() {
    let tier = HotTier::new(256);
    assert_eq!(tier.capacity(), 256);
    assert_eq!(tier.len(), 0);
    assert!(tier.is_empty());

    tier.insert(make_entry("cap-1")).unwrap();
    assert_eq!(tier.len(), 1);
    assert!(!tier.is_empty());
}

#[test]
fn test_hot_insert_and_get() {
    let tier = HotTier::new(256);
    let entry = make_entry("cap-1");
    tier.insert(entry).unwrap();

    let fetched = tier.get("cap-1").unwrap();
    assert_eq!(fetched.id.as_str(), "cap-1");
    assert_eq!(fetched.content, "content-cap-1");
    assert_eq!(fetched.tier, Tier::Hot);
    // insert 时 touch 一次,get 时 touch 一次,共 2 次
    assert_eq!(fetched.access_count, 2);
}

#[test]
fn test_hot_get_nonexistent_returns_error() {
    let tier = HotTier::new(256);
    let result = tier.get("nonexistent");
    assert!(matches!(result, Err(CmtError::EntryNotFound(_))));
}

#[test]
fn test_hot_peek_does_not_update_access() {
    let tier = HotTier::new(256);
    tier.insert(make_entry("cap-1")).unwrap();

    // 记录 insert 后的 last_accessed_at(insert 时 touch 一次)
    let after_insert = tier.peek("cap-1").unwrap().last_accessed_at;

    // 逻辑时钟替代 thread::sleep:peek 不更新 last_accessed_at 与 access_count,
    // 直接验证字段不变即可,无需等待墙钟时间流逝(SubTask 20.2)
    let peeked = tier.peek("cap-1").unwrap();
    // peek 不更新 last_accessed_at(与 get 区分)
    assert_eq!(peeked.last_accessed_at, after_insert);
    // insert 时 touch 一次,peek 不增加 access_count
    assert_eq!(peeked.access_count, 1);
}

#[test]
fn test_hot_lru_eviction_on_overflow() {
    // 容量 2,插入 3 个条目,最久未访问的应被驱逐
    // 逻辑时钟保证 LRU 顺序确定性,无需 thread::sleep(SubTask 20.2)
    let tier = HotTier::new(2);
    tier.insert(make_entry("cap-1")).unwrap();
    tier.insert(make_entry("cap-2")).unwrap();

    // 访问 cap-1,使 cap-2 成为最久未访问(逻辑时钟递增)
    tier.get("cap-1").unwrap();

    // 插入 cap-3,应驱逐 cap-2
    let evicted = tier.insert(make_entry("cap-3")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("cap-2"));
    assert_eq!(tier.evictions(), 1);

    // cap-1 和 cap-3 应存在,cap-2 应被驱逐
    assert!(tier.contains("cap-1"));
    assert!(!tier.contains("cap-2"));
    assert!(tier.contains("cap-3"));
}

#[test]
fn test_hot_lru_eviction_257_entries() {
    // 任务要求:插入 257 条目后最久未访问的被驱逐(容量 256)
    // 逻辑时钟保证 LRU 顺序确定性,无需 thread::sleep(SubTask 20.2)
    let tier = HotTier::new(256);

    // 插入 256 个条目
    for i in 0..256 {
        tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
    }
    assert_eq!(tier.len(), 256);
    assert_eq!(tier.evictions(), 0);

    // 访问 cap-1 到 cap-255,使 cap-0 成为最久未访问(逻辑时钟递增)
    for i in 1..256 {
        tier.get(&format!("cap-{i}")).unwrap();
    }

    // 插入第 257 个条目,应驱逐 cap-0
    let evicted = tier.insert(make_entry("cap-256")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("cap-0"));
    assert_eq!(tier.len(), 256); // 容量不变
    assert_eq!(tier.evictions(), 1);
    assert!(!tier.contains("cap-0"));
    assert!(tier.contains("cap-256"));
}

#[test]
fn test_hot_update_existing_no_eviction() {
    let tier = HotTier::new(2);
    tier.insert(make_entry("cap-1")).unwrap();
    tier.insert(make_entry("cap-2")).unwrap();

    // 更新已存在的 cap-1,不应触发驱逐
    let evicted = tier.insert(make_entry("cap-1")).unwrap();
    assert!(evicted.is_none());
    assert_eq!(tier.len(), 2);
    assert_eq!(tier.evictions(), 0);
}

#[test]
fn test_hot_remove() {
    let tier = HotTier::new(256);
    tier.insert(make_entry("cap-1")).unwrap();
    assert!(tier.contains("cap-1"));

    let removed = tier.remove("cap-1");
    assert!(removed.is_some());
    assert!(!tier.contains("cap-1"));
    assert_eq!(tier.len(), 0);

    // 移除不存在的条目返回 None
    let removed_again = tier.remove("nonexistent");
    assert!(removed_again.is_none());
}

#[test]
fn test_hot_clear() {
    let tier = HotTier::new(256);
    for i in 0..5 {
        tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
    }
    assert_eq!(tier.len(), 5);

    tier.clear();
    assert_eq!(tier.len(), 0);
    assert!(tier.is_empty());
}

#[test]
fn test_hot_list_all() {
    let tier = HotTier::new(256);
    for i in 0..3 {
        tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
    }
    let all = tier.list_all();
    assert_eq!(all.len(), 3);

    // 验证所有条目都是 Hot 层
    for entry in &all {
        assert_eq!(entry.tier, Tier::Hot);
    }
}

#[test]
fn test_hot_evict_lru_empty_returns_none() {
    let tier = HotTier::new(256);
    let result = tier.evict_lru().unwrap();
    assert!(result.is_none());
}

#[test]
fn test_hot_evict_lru_returns_oldest() {
    // 逻辑时钟保证 LRU 顺序确定性,无需 thread::sleep(SubTask 20.2)
    let tier = HotTier::new(256);
    tier.insert(make_entry("cap-1")).unwrap();
    tier.insert(make_entry("cap-2")).unwrap();

    // 驱逐最久未访问的(cap-1,逻辑时钟值更小)
    let evicted = tier.evict_lru().unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("cap-1"));
    assert!(!tier.contains("cap-1"));
    assert!(tier.contains("cap-2"));
}

#[test]
fn test_hot_evictions_counter() {
    let tier = HotTier::new(2);
    assert_eq!(tier.evictions(), 0);

    // 触发 3 次驱逐
    tier.insert(make_entry("cap-1")).unwrap();
    tier.insert(make_entry("cap-2")).unwrap();
    tier.insert(make_entry("cap-3")).unwrap();
    assert_eq!(tier.evictions(), 1);

    tier.insert(make_entry("cap-4")).unwrap();
    assert_eq!(tier.evictions(), 2);

    tier.insert(make_entry("cap-5")).unwrap();
    assert_eq!(tier.evictions(), 3);
}

/// SubTask 13.3:验证 HotTier 并发插入不超容(check-then-act 竞态修复)
///
/// 10 个线程各插入 30 个条目(共 300 个不同 ID),Hot 层容量 256。
/// 修复前:并发场景下多个线程可能同时通过 `len() < capacity` 检查,
/// 导致最终条目数 > 256(超容)。
/// 修复后:`Mutex<()>` 保护"检查 → 驱逐 → 插入"临界区,保证原子性,
/// 最终条目数 ≤ 256。
#[test]
fn test_hot_concurrent_insert_respects_capacity() {
    use std::sync::Arc;
    use std::thread;

    let tier = Arc::new(HotTier::new(256));
    let mut handles = Vec::with_capacity(10);

    // 10 个线程,每个插入 30 个条目(共 300 个不同 ID)
    for thread_id in 0..10 {
        let tier_clone = tier.clone();
        handles.push(thread::spawn(move || {
            for i in 0..30 {
                let cap_id = format!("t{thread_id}-cap-{i}");
                let entry =
                    CapabilityEntry::new(cap_id, format!("content-{thread_id}-{i}"), Tier::Hot);
                // insert 失败不应发生(除非 mutex poisoned)
                tier_clone.insert(entry).expect("并发插入应成功");
            }
        }));
    }

    // 等待所有线程完成
    for handle in handles {
        handle.join().expect("线程不应 panic");
    }

    // 关键断言:最终条目数不超过容量上限 256
    let final_len = tier.len();
    assert!(
        final_len <= 256,
        "Hot 层条目数 {final_len} 超过容量上限 256(并发竞态未修复)"
    );

    // 验证驱逐计数 > 0(300 个条目插入容量 256,至少驱逐 44 次)
    assert!(
        tier.evictions() >= 44,
        "应至少驱逐 44 次(300 - 256),实际 {}",
        tier.evictions()
    );
}

/// SubTask 13.3:验证 HotTier 并发插入与读取不 panic、不数据竞争
///
/// 多线程混合 insert + get + remove 操作,验证 DashMap + Mutex 组合
/// 在并发场景下的线程安全性。
#[test]
fn test_hot_concurrent_mixed_operations() {
    use std::sync::Arc;
    use std::thread;

    let tier = Arc::new(HotTier::new(256));

    // 预填充一些条目
    for i in 0..100 {
        tier.insert(make_entry(&format!("pre-{i}"))).unwrap();
    }

    let mut handles = Vec::new();

    // 线程 1:并发插入
    {
        let tier_clone = tier.clone();
        handles.push(thread::spawn(move || {
            for i in 0..50 {
                let entry = make_entry(&format!("insert-{i}"));
                let _ = tier_clone.insert(entry);
            }
        }));
    }

    // 线程 2:并发读取
    {
        let tier_clone = tier.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                let _ = tier_clone.get(&format!("pre-{i}"));
            }
        }));
    }

    // 线程 3:并发删除
    {
        let tier_clone = tier.clone();
        handles.push(thread::spawn(move || {
            for i in 0..50 {
                let _ = tier_clone.remove(&format!("pre-{i}"));
            }
        }));
    }

    // 等待所有线程完成,验证无 panic
    for handle in handles {
        handle.join().expect("并发操作不应 panic");
    }

    // 最终容量仍应 ≤ 256
    assert!(
        tier.len() <= 256,
        "并发混合操作后条目数 {} 超过容量上限 256",
        tier.len()
    );
}
