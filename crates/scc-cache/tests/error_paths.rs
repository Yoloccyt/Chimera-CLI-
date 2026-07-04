//! SCC 推测上下文缓存错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 scc-cache 补充错误路径测试
//!
//! # 测试覆盖
//! 1. 缓存未命中:get_or_prefetch 返回 None
//! 2. 预取失败:预取的上下文不在缓存中(静默处理,不 panic)
//! 3. 模式不存在:predict_next 对未知上下文返回空 Vec
//! 4. 容量为 1 的 LRU 边界:插入 2 个条目,第一个被驱逐
//! 5. 并发驱逐:多线程并发 insert,验证不 panic

#![forbid(unsafe_code)]

use std::sync::Arc;

use event_bus::EventBus;
use scc_cache::{AccessPatternLearner, ContextEntry, ContextId, SccCache, SccConfig};

/// 缓存未命中:get_or_prefetch 返回 None
///
/// WHY:缓存未命中是常见场景,必须返回 None 而非 panic。
/// 此测试验证空缓存的 get_or_prefetch 返回 None,且 access_count 递增。
#[tokio::test]
async fn test_cache_miss_returns_none() {
    let cache = SccCache::new(SccConfig::default(), EventBus::new());
    let id = ContextId::new("ctx-nonexistent");

    let result = cache.get_or_prefetch(&id);
    assert!(result.is_none(), "空缓存应返回 None");
    assert_eq!(cache.access_count(), 1, "未命中也应递增 access_count");
    assert_eq!(cache.hit_count(), 0, "未命中不应递增 hit_count");
}

/// 预取失败:预取的上下文不在缓存中(静默处理,不 panic)
///
/// WHY:预取的上下文可能尚未加载到缓存(无后端存储可加载),
/// 此场景应静默处理(仅 warn 日志),不 panic、不返回错误。
/// 此测试验证预取缺失条目时不 panic。
#[tokio::test]
async fn test_prefetch_missing_entry_silent_no_panic() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.5);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 训练模式但不插入 ctx-b 到缓存
    learner.record_access(&ctx_a, &ctx_b);

    // 预取:ctx-b 不在缓存中,应静默失败
    let prefetched = learner.prefetch(&ctx_a, &cache);
    assert_eq!(prefetched.len(), 1, "应返回预测 ID(不管是否在缓存中)");
    assert_eq!(prefetched[0].as_str(), "ctx-b");

    // 等待后台任务完成(不应 panic)
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // ctx-b 仍不在缓存中
    assert!(!cache.contains(&ctx_b), "ctx-b 不应被插入缓存");
}

/// 模式不存在:predict_next 对未知上下文返回空 Vec
///
/// WHY:未知上下文无转移记录,应返回空 Vec 而非 panic 或返回错误。
/// 此测试验证边界条件的健壮性。
#[tokio::test]
async fn test_predict_next_unknown_returns_empty() {
    let learner = AccessPatternLearner::new(EventBus::new(), 0.6);
    let unknown = ContextId::new("ctx-unknown");

    let predictions = learner.predict_next(&unknown);
    assert!(predictions.is_empty(), "未知上下文应返回空预测列表");

    // get_pattern 也应返回 None
    let pattern = learner.get_pattern(&unknown);
    assert!(pattern.is_none(), "未知上下文应返回 None 模式");
}

/// 容量为 1 的 LRU 边界:插入 2 个条目,第一个被驱逐
///
/// WHY:容量为 1 是 LRU 的最小边界,验证驱逐逻辑在极端容量下正确工作。
/// 此测试确保 LRU 驱逐在容量=1 时不 panic 且正确驱逐。
#[tokio::test]
async fn test_lru_capacity_one_eviction() {
    let cache = SccCache::new(SccConfig::default().with_capacity(1), EventBus::new());

    // 插入第一个条目
    cache.insert(ContextEntry::new("ctx-a", "content-a"));
    assert_eq!(cache.len(), 1);
    assert!(cache.contains(&ContextId::new("ctx-a")));

    // 插入第二个条目,应驱逐 ctx-a
    cache.insert(ContextEntry::new("ctx-b", "content-b"));
    assert_eq!(cache.len(), 1, "容量 1 时应只有 1 个条目");
    assert!(!cache.contains(&ContextId::new("ctx-a")), "ctx-a 应被驱逐");
    assert!(cache.contains(&ContextId::new("ctx-b")), "ctx-b 应存在");
    assert_eq!(cache.evictions(), 1, "应发生 1 次驱逐");
}

/// 并发驱逐:多线程并发 insert,验证不 panic
///
/// WHY:SCC 是并发缓存,多线程并发 insert 时 LRU 驱逐必须线程安全。
/// 此测试验证并发场景下无数据竞争、无 panic。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_insert_no_panic() {
    let cache = Arc::new(SccCache::new(
        SccConfig::default().with_capacity(10),
        EventBus::new(),
    ));

    let mut handles = Vec::new();

    // 4 线程并发插入,每线程插入 20 个条目
    for thread_id in 0..4 {
        let cache_clone = Arc::clone(&cache);
        handles.push(tokio::spawn(async move {
            for i in 0..20 {
                let id = format!("ctx-t{thread_id}-{i}");
                cache_clone.insert(ContextEntry::new(id, format!("content-{i}")));
            }
        }));
    }

    // 等待所有线程完成,任一 panic 则测试失败
    for handle in handles {
        handle.await.expect("并发 insert 任务 panic");
    }

    // 缓存条目数应 <= 容量(10)
    assert!(
        cache.len() <= 10,
        "并发插入后条目数 {} 应 <= 容量 10",
        cache.len()
    );

    // 驱逐数应为 max(0, 80 - 10) = 70
    assert_eq!(cache.evictions(), 70, "应驱逐 70 个条目(80 插入 - 10 容量)");
}
