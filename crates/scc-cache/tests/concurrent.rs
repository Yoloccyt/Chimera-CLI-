//! SCC 并发测试 — 验证多线程并发访问下无 panic、无数据竞争
//!
//! 对应架构层:L3 Storage
//!
//! # 测试策略
//! - 10 线程并发 get_or_prefetch + insert,验证无 panic
//! - 混合读写场景:部分线程读,部分线程写
//! - 命中率性能断言(标记 #[ignore],需手动运行)

use std::sync::Arc;
use std::thread;

use event_bus::EventBus;
use scc_cache::{ContextEntry, ContextId, SccCache, SccConfig};

/// 10 线程并发访问,无 panic、无数据竞争
#[test]
fn test_concurrent_access_no_panic() {
    let bus = EventBus::new();
    let cache = Arc::new(SccCache::new(SccConfig::default(), bus));

    // 预填充 50 个条目
    for i in 0..50 {
        let id = ContextId::new(format!("ctx-{i}"));
        cache.insert(ContextEntry::new(id, format!("content-{i}")));
    }

    let mut handles = Vec::new();
    for thread_id in 0..10 {
        let cache = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            // 每个线程执行 200 次操作:读已有条目 + 读不存在的条目
            for i in 0..200 {
                let idx = (i + thread_id) % 50;
                let id = ContextId::new(format!("ctx-{idx}"));
                // 读操作(命中)
                let _ = cache.get_or_prefetch(&id);

                // 读不存在的条目(未命中)
                let miss_id = ContextId::new(format!("ctx-miss-{thread_id}-{i}"));
                let _ = cache.get_or_prefetch(&miss_id);
            }
        }));
    }

    // 所有线程应正常完成,无 panic
    for (i, handle) in handles.into_iter().enumerate() {
        handle.join().unwrap_or_else(|e| {
            panic!("线程 {i} panic: {e:?}");
        });
    }

    // 验证缓存仍可正常工作
    let id = ContextId::new("ctx-0");
    assert!(cache.get_or_prefetch(&id).is_some());
}

/// 10 线程并发混合读写(读 + 插入新条目),无 panic
#[test]
fn test_concurrent_mixed_read_write() {
    let bus = EventBus::new();
    let cache = Arc::new(SccCache::new(SccConfig::default().with_capacity(512), bus));

    // 预填充 100 个条目
    for i in 0..100 {
        cache.insert(ContextEntry::new(
            format!("ctx-{i}"),
            format!("content-{i}"),
        ));
    }

    let mut handles = Vec::new();
    for thread_id in 0..10 {
        let cache = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                if i % 3 == 0 {
                    // 写操作:插入新条目
                    let id = ContextId::new(format!("ctx-new-{thread_id}-{i}"));
                    cache.insert(ContextEntry::new(
                        id,
                        format!("new-content-{thread_id}-{i}"),
                    ));
                } else {
                    // 读操作:访问已有条目
                    let idx = i % 100;
                    let id = ContextId::new(format!("ctx-{idx}"));
                    let _ = cache.get_or_prefetch(&id);
                }
            }
        }));
    }

    for (i, handle) in handles.into_iter().enumerate() {
        handle.join().unwrap_or_else(|e| {
            panic!("线程 {i} panic: {e:?}");
        });
    }

    // 验证缓存条目数不超过容量上限(可能因驱逐略少于总数)
    assert!(cache.len() <= 512);
}

/// 并发 Arc 共享验证:多线程获取同一条目,Arc 指向同一分配
#[test]
fn test_concurrent_arc_sharing() {
    let bus = EventBus::new();
    let cache = Arc::new(SccCache::new(SccConfig::default(), bus));

    let target_id = ContextId::new("ctx-shared");
    cache.insert(ContextEntry::new("ctx-shared", "shared content"));

    let cache_clone = Arc::clone(&cache);
    let target_id_clone = target_id.clone();

    let handle = thread::spawn(move || {
        cache_clone
            .get_or_prefetch(&target_id_clone)
            .expect("条目应存在")
    });

    let entry1 = cache.get_or_prefetch(&target_id).expect("条目应存在");
    let entry2 = handle.join().expect("线程应正常完成");

    // 两个线程获取的 Arc 指向同一分配
    assert!(Arc::ptr_eq(&entry1, &entry2));
}

/// 命中率性能断言 — 稳定访问模式下命中率 > 70%
///
/// 标记 #[ignore],需手动运行:`cargo test -p scc-cache -- --ignored test_hit_rate_above_70_percent`
#[test]
#[ignore]
fn test_hit_rate_above_70_percent() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus);

    // 插入 20 个条目
    for i in 0..20 {
        cache.insert(ContextEntry::new(
            format!("ctx-{i}"),
            format!("content-{i}"),
        ));
    }

    // 稳定访问模式:80% 访问 ctx-0..ctx-15(命中),20% 访问不存在的条目(未命中)
    for _ in 0..800 {
        for i in 0..16 {
            let id = ContextId::new(format!("ctx-{i}"));
            assert!(cache.get_or_prefetch(&id).is_some());
        }
    }
    for _ in 0..200 {
        let id = ContextId::new("ctx-nonexistent");
        assert!(cache.get_or_prefetch(&id).is_none());
    }

    // 总访问 800 + 200 = 1000,命中 800
    // hit_rate = 800 / 1000 = 0.8 > 0.7
    let stats = cache.stats();
    assert!(stats.hit_rate > 0.7, "命中率 {} 应 > 0.7", stats.hit_rate);
}

/// LRU 驱逐在并发环境下正确工作
#[test]
fn test_concurrent_lru_eviction() {
    let bus = EventBus::new();
    let cache = Arc::new(SccCache::new(SccConfig::default().with_capacity(50), bus));

    // 预填充到容量上限
    for i in 0..50 {
        cache.insert(ContextEntry::new(
            format!("ctx-{i}"),
            format!("content-{i}"),
        ));
    }
    assert_eq!(cache.len(), 50);

    // 并发插入新条目,触发 LRU 驱逐
    let mut handles = Vec::new();
    for thread_id in 0..5 {
        let cache = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            for i in 0..20 {
                let id = ContextId::new(format!("ctx-new-{thread_id}-{i}"));
                cache.insert(ContextEntry::new(id, format!("new-{thread_id}-{i}")));
            }
        }));
    }

    for handle in handles {
        handle.join().expect("线程应正常完成");
    }

    // 缓存条目数不应超过容量上限(允许因 Arc 引用保护临时超容,
    // 但最终应回到容量附近)
    assert!(
        cache.len() <= 60,
        "缓存条目数 {} 应接近容量上限 50",
        cache.len()
    );

    // 驱逐次数应 > 0
    assert!(cache.evictions() > 0, "应发生 LRU 驱逐");
}
