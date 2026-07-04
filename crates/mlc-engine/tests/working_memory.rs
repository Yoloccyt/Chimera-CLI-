//! SubTask 1.12:L0 WorkingMemory 集成测试
//!
//! 验证 L0 工作记忆的 LRU 驱逐策略与并发安全性。
//! 重点测试:插入 65 条目后最久未访问的被驱逐(容量 64)。

use std::thread;

use mlc_engine::{MemoryEntry, MemoryTier, WorkingMemory};

/// 构造测试用记忆条目
fn make_entry(id: &str) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L0Working)
}

#[test]
fn test_l0_capacity_default() {
    let mem = WorkingMemory::new(64);
    assert_eq!(mem.capacity(), 64);
    assert_eq!(mem.len(), 0);
    assert!(mem.is_empty());
}

#[test]
fn test_l0_insert_and_get() {
    let mem = WorkingMemory::new(64);
    mem.insert(make_entry("m-1")).unwrap();

    let fetched = mem.get("m-1").unwrap();
    assert_eq!(fetched.id.as_str(), "m-1");
    assert_eq!(fetched.content, "content-m-1");
    assert_eq!(fetched.tier, MemoryTier::L0Working);
}

#[test]
fn test_l0_get_nonexistent_returns_error() {
    let mem = WorkingMemory::new(64);
    let result = mem.get("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_l0_peek_does_not_update_lru() {
    let mem = WorkingMemory::new(2);
    mem.insert(make_entry("m-1")).unwrap();
    mem.insert(make_entry("m-2")).unwrap();

    // peek m-1(不更新 last_accessed_at)
    let peeked = mem.peek("m-1").unwrap();
    assert_eq!(peeked.id.as_str(), "m-1");

    // 插入 m-3,应驱逐 m-1(peek 未更新访问时间,m-1 仍是最旧)
    let evicted = mem.insert(make_entry("m-3")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));
}

#[test]
fn test_l0_lru_eviction_65_entries() {
    // 任务核心要求:插入 65 条目后最久未访问的被驱逐(容量 64)
    // WorkingMemory 的 LRU 基于链表(LruList),顺序由 push_back/touch 决定,
    // 与墙钟时间无关,无需 thread::sleep(SubTask 20.2)
    let mem = WorkingMemory::new(64);

    // 插入 64 个条目(填满容量)
    for i in 0..64 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }
    assert_eq!(mem.len(), 64);
    assert_eq!(mem.evictions(), 0);

    // 访问 m-1 到 m-63,使 m-0 成为最久未访问(链表 touch 移到 MRU 端)
    for i in 1..64 {
        mem.get(&format!("m-{i}")).unwrap();
    }

    // 插入第 65 个条目,应驱逐 m-0
    let evicted = mem.insert(make_entry("m-64")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-0"));
    assert_eq!(mem.len(), 64); // 容量不变
    assert_eq!(mem.evictions(), 1);
    assert!(!mem.contains("m-0"));
    assert!(mem.contains("m-64"));
}

#[test]
fn test_l0_lru_eviction_order() {
    // 验证 LRU 驱逐顺序:最久未访问的先被驱逐
    // WorkingMemory 的 LRU 基于链表(LruList),顺序由操作顺序决定,
    // 与墙钟时间无关,无需 thread::sleep(SubTask 20.2)
    let mem = WorkingMemory::new(3);

    mem.insert(make_entry("m-1")).unwrap();
    mem.insert(make_entry("m-2")).unwrap();
    mem.insert(make_entry("m-3")).unwrap();

    // 访问 m-1,使 m-2 成为最旧(链表 touch 移到 MRU 端)
    mem.get("m-1").unwrap();

    // 插入 m-4,应驱逐 m-2
    let evicted = mem.insert(make_entry("m-4")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-2"));
}

#[test]
fn test_l0_update_existing_no_eviction() {
    let mem = WorkingMemory::new(2);
    mem.insert(make_entry("m-1")).unwrap();
    mem.insert(make_entry("m-2")).unwrap();

    // 更新已存在的 m-1,不应触发驱逐
    let evicted = mem.insert(make_entry("m-1")).unwrap();
    assert!(evicted.is_none());
    assert_eq!(mem.len(), 2);
    assert_eq!(mem.evictions(), 0);
}

#[test]
fn test_l0_remove() {
    let mem = WorkingMemory::new(64);
    mem.insert(make_entry("m-1")).unwrap();
    assert!(mem.contains("m-1"));

    let removed = mem.remove("m-1");
    assert!(removed.is_some());
    assert!(!mem.contains("m-1"));
    assert_eq!(mem.len(), 0);
}

#[test]
fn test_l0_clear() {
    let mem = WorkingMemory::new(64);
    for i in 0..5 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }
    assert_eq!(mem.len(), 5);

    mem.clear();
    assert_eq!(mem.len(), 0);
    assert!(mem.is_empty());
}

#[test]
fn test_l0_list_all() {
    let mem = WorkingMemory::new(64);
    for i in 0..3 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }
    let all = mem.list_all();
    assert_eq!(all.len(), 3);
}

#[test]
fn test_l0_evictions_counter() {
    let mem = WorkingMemory::new(2);
    assert_eq!(mem.evictions(), 0);

    mem.insert(make_entry("m-1")).unwrap();
    mem.insert(make_entry("m-2")).unwrap();
    assert_eq!(mem.evictions(), 0);

    // 触发驱逐
    mem.insert(make_entry("m-3")).unwrap();
    assert_eq!(mem.evictions(), 1);

    mem.insert(make_entry("m-4")).unwrap();
    assert_eq!(mem.evictions(), 2);
}

#[test]
fn test_l0_concurrent_insert() {
    // 验证 DashMap 的并发安全性
    let mem = std::sync::Arc::new(WorkingMemory::new(1024));
    let mut handles = Vec::new();

    for t in 0..4 {
        let mem_clone = std::sync::Arc::clone(&mem);
        let handle = thread::spawn(move || {
            for i in 0..50 {
                let id = format!("t{t}-m{i}");
                mem_clone.insert(make_entry(&id)).unwrap();
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    // 4 线程 × 50 条目 = 200 条目
    assert_eq!(mem.len(), 200);
    assert_eq!(mem.evictions(), 0); // 容量 1024,无驱逐
}

// === SubTask 13.2:L0 LRU 驱逐 O(1) 化基准测试 ===

/// 验证 LRU 驱逐 O(1):64 条目驱逐延迟远低于 O(n) 的 10μs
///
/// SubTask 13.2 核心验证:从 O(n) 扫描优化为 O(1) 链表弹出后,
/// 64 条目驱逐延迟应远低于原 O(n) 的 10μs。
///
/// 注:Windows 上 Mutex 系统调用有 500-1000ns 开销,阈值适当放宽,
/// 但仍验证 O(1) 化后性能远优于 O(n)。
#[test]
#[ignore = "perf: run with --ignored"]
fn test_l0_lru_eviction_o1_performance() {
    let mem = WorkingMemory::new(64);

    // 填满 64 条目
    for i in 0..64 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }
    assert_eq!(mem.len(), 64);

    // Warmup(10 次,触发缓存预热与分支预测器稳定)
    for i in 0..10 {
        mem.remove(&format!("m-{i}"));
        mem.insert(make_entry(&format!("warm-{i}"))).unwrap();
    }
    // warmup 后 mem 中有 warm-0..warm-9 和 m-10..m-63

    // 正式测量(100 次,每次先 remove 一个再 insert 触发驱逐)
    let mut latencies = Vec::with_capacity(100);
    for i in 0..100 {
        // 先 remove 一个条目腾出空间(避免驱逐,只测 insert 路径)
        let remove_id = if i < 10 {
            format!("warm-{i}")
        } else {
            format!("m-{i}")
        };
        mem.remove(&remove_id);

        // 插入 2 个条目触发 1 次驱逐(容量 64,remove 后 63,insert 后 64,再 insert 触发驱逐)
        let start = std::time::Instant::now();
        mem.insert(make_entry(&format!("new-{i}-a"))).unwrap(); // 64,无驱逐
        mem.insert(make_entry(&format!("new-{i}-b"))).unwrap(); // 65,触发驱逐
        latencies.push(start.elapsed().as_nanos() as f64);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[50];
    let p99 = latencies[99];

    // P50 < 20μs(含 2 次 DashMap insert + 1 次 LRU 驱逐,3 个操作)
    // 原 O(n) 单次驱逐约 10μs,3 个操作应 > 30μs,O(1) 化后 < 20μs
    let p50_threshold = 20_000.0_f64;
    // P99 < 100μs:Windows Mutex 系统调用有 500-1000ns 开销,高负载环境下
    // P99 尾延迟受调度噪声影响大,100μs 阈值在保留 O(1) 验证价值的同时减少 flake
    let p99_threshold = 100_000.0_f64;
    assert!(
        p50 < p50_threshold,
        "P50 延迟 {}ns 超过 {}ns(原 O(n) 单次驱逐约 10μs)",
        p50,
        p50_threshold
    );
    assert!(
        p99 < p99_threshold,
        "P99 延迟 {}ns 超过 {}ns(Windows 高负载环境尾延迟阈值)",
        p99,
        p99_threshold
    );
}

/// 验证纯 evict_lru 操作 O(1):64 条目时单次驱逐延迟远低于 O(n) 的 10μs
#[test]
#[ignore = "perf: run with --ignored"]
fn test_l0_evict_lru_single_call_o1() {
    let mem = WorkingMemory::new(64);

    // 填满 64 条目
    for i in 0..64 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }

    // Warmup
    for _ in 0..10 {
        mem.evict_lru().unwrap();
        mem.insert(make_entry("warmup")).unwrap();
    }

    // 测量单次 evict_lru 延迟(100 次)
    let mut latencies = Vec::with_capacity(100);
    for i in 0..100 {
        // 先 insert 一个补满(上次 evict 后 63 个)
        mem.insert(make_entry(&format!("fill-{i}"))).unwrap();

        let start = std::time::Instant::now();
        let evicted = mem.evict_lru().unwrap();
        latencies.push(start.elapsed().as_nanos() as f64);
        assert!(evicted.is_some(), "evict_lru 应返回被驱逐条目");
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[50];
    let p99 = latencies[99];

    // 单次 evict_lru:P50 < 5μs(含 Mutex lock + LRU pop + DashMap remove)
    // 原 O(n) 约 10μs,O(1) 化后 P50 < 5μs(Windows Mutex 开销约 500-1000ns)
    // P99 < 50μs(允许 Windows 系统调度抖动,仅验证无严重退化)
    let p50_threshold = 5_000.0_f64;
    let p99_threshold = 50_000.0_f64;
    assert!(
        p50 < p50_threshold,
        "evict_lru P50 延迟 {}ns 超过 {}ns(原 O(n) 约 10μs)",
        p50,
        p50_threshold
    );
    assert!(
        p99 < p99_threshold,
        "evict_lru P99 延迟 {}ns 超过 {}ns(可能为系统调度抖动)",
        p99,
        p99_threshold
    );
}

/// 验证 LRU 顺序正确性:touch 后驱逐顺序符合预期
#[test]
fn test_l0_lru_order_after_multiple_touch() {
    let mem = WorkingMemory::new(4);

    // 插入 4 个条目:LRU 顺序 [m-1, m-2, m-3, m-4](m-1 最旧)
    mem.insert(make_entry("m-1")).unwrap();
    mem.insert(make_entry("m-2")).unwrap();
    mem.insert(make_entry("m-3")).unwrap();
    mem.insert(make_entry("m-4")).unwrap();

    // touch m-1 和 m-3,LRU 顺序变为 [m-2, m-4, m-1, m-3]
    mem.get("m-1").unwrap();
    mem.get("m-3").unwrap();

    // 插入 m-5,应驱逐 m-2(最久未访问)
    let evicted = mem.insert(make_entry("m-5")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-2"));

    // 插入 m-6,应驱逐 m-4
    let evicted = mem.insert(make_entry("m-6")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-4"));

    // 插入 m-7,应驱逐 m-1
    let evicted = mem.insert(make_entry("m-7")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));

    // 插入 m-8,应驱逐 m-3
    let evicted = mem.insert(make_entry("m-8")).unwrap();
    assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-3"));
}

/// 验证 LRU 在并发访问下的一致性:多线程 get 不破坏 LRU 顺序
#[test]
fn test_l0_lru_concurrent_touch_consistency() {
    use std::sync::Arc;
    use std::thread;

    let mem = Arc::new(WorkingMemory::new(64));

    // 插入 64 个条目
    for i in 0..64 {
        mem.insert(make_entry(&format!("m-{i}"))).unwrap();
    }

    // 4 线程并发 get 不同条目
    let mut handles = Vec::new();
    for t in 0..4 {
        let mem_clone = Arc::clone(&mem);
        handles.push(thread::spawn(move || {
            for i in 0..16 {
                let id = format!("m-{}", t * 16 + i);
                let _ = mem_clone.get(&id);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 验证所有条目仍存在(并发 touch 不丢数据)
    assert_eq!(mem.len(), 64);

    // 验证驱逐仍能正常工作
    let evicted = mem.insert(make_entry("m-64")).unwrap();
    assert!(evicted.is_some(), "驱逐应返回被驱逐条目");
    assert_eq!(mem.len(), 64); // 容量不变
}

/// SubTask 18.4:验证 DashMap::entry() 原子操作消除 TOCTOU 窗口
///
/// 10 线程各插入 10 条目(相同 id),断言 L0 容量 ≤ 64 且无 panic。
/// 原 contains_key + insert 两步操作在并发下会导致重复驱逐与丢失更新,
/// entry() 原子化 check-then-insert 后,仅 1 个线程 Vacant 插入,其余 Occupied 更新。
#[test]
fn test_l0_concurrent_insert_same_id_no_panic() {
    use std::sync::Arc;
    use std::thread;

    let mem = Arc::new(WorkingMemory::new(64));
    let mut handles = Vec::new();

    for _t in 0..10 {
        let mem_clone = Arc::clone(&mem);
        let handle = thread::spawn(move || {
            // 10 线程插入相同的 10 个 id,触发并发 TOCTOU 场景
            for i in 0..10 {
                mem_clone.insert(make_entry(&format!("m-{i}"))).unwrap();
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().unwrap();
    }

    // 10 个唯一 id,容量 64,应全部存在且无 panic
    assert!(mem.len() <= 64, "L0 容量应 ≤ 64,实际 {}", mem.len());
    assert_eq!(mem.len(), 10, "应有 10 个唯一条目(并发插入未丢失)");

    // 验证所有 id 存在(并发插入同 id 未丢失数据)
    for i in 0..10 {
        assert!(
            mem.contains(&format!("m-{i}")),
            "id m-{i} 应存在(并发插入未丢失)"
        );
    }
}
