//! SCC 预取与访问模式学习器集成测试
//!
//! 覆盖 Phase III P0 性能优化 Task III-3:
//! - 马尔可夫链转移矩阵容量上限
//! - LRU 淘汰策略

#![forbid(unsafe_code)]

use event_bus::EventBus;
use scc_cache::{AccessPatternLearner, ContextId};

/// 验证转移矩阵容量上限与 LRU 淘汰
///
/// 当插入超过容量上限的独立 (previous, current) 对时,最早访问的对应被
/// LRU 淘汰,确保内存占用有界。
#[test]
fn test_pattern_capacity_limit() {
    let capacity = 10_000;
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, capacity);

    let previous = ContextId::new("ctx-previous");

    // 插入 2 倍容量的独立 (previous, current) 对
    for i in 0..(capacity * 2) {
        let current = ContextId::new(format!("ctx-{}", i));
        let next = ContextId::new(format!("next-{}", i));
        learner.record_access(&previous, &current, &next);
    }

    // 验证容量未超过上限
    assert_eq!(
        learner.pattern_count(),
        capacity,
        "转移矩阵容量应被限制在 {}",
        capacity
    );

    // 验证最早插入的 (previous, ctx-0) 已被淘汰
    let evicted = ContextId::new("ctx-0");
    assert!(
        learner.predict_next(&previous, &evicted).is_empty(),
        "最早访问的 (previous, current) 对应被 LRU 淘汰"
    );

    // 验证最近插入的 (previous, ctx-{capacity*2-1}) 仍在
    let recent = ContextId::new(format!("ctx-{}", capacity * 2 - 1));
    assert!(
        !learner.predict_next(&previous, &recent).is_empty(),
        "最近访问的 (previous, current) 对应保留"
    );
}

/// 验证重新访问会更新 LRU 顺序
///
/// 当 capacity=3 时,访问顺序为 (a,b)→(b,c)→(c,a)→(a,b)→(d,a):
/// - (a,b) 被重新访问后变为 MRU
/// - 插入 (d,a) 时应淘汰最旧的 (b,c),而非 (a,b)
#[test]
fn test_lru_order_updated_on_reaccess() {
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, 3);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");
    let ctx_d = ContextId::new("ctx-d");

    // 建立 3 个 (previous, current) 对
    learner.record_access(&ctx_a, &ctx_b, &ctx_c);
    learner.record_access(&ctx_b, &ctx_c, &ctx_a);
    learner.record_access(&ctx_c, &ctx_a, &ctx_d);

    // 重新访问 (a,b),使其变为 MRU
    learner.record_access(&ctx_a, &ctx_b, &ctx_c);

    // 插入新的 (d,a) 对,应淘汰最旧的 (b,c)
    learner.record_access(&ctx_d, &ctx_a, &ctx_b);

    assert_eq!(learner.pattern_count(), 3, "容量应保持在 3");
    assert!(
        !learner.predict_next(&ctx_a, &ctx_b).is_empty(),
        "重新访问的 (a,b) 对应保留"
    );
    assert!(
        learner.predict_next(&ctx_b, &ctx_c).is_empty(),
        "最久未访问的 (b,c) 对应被淘汰"
    );
    assert!(
        !learner.predict_next(&ctx_c, &ctx_a).is_empty(),
        "(c,a) 对应保留"
    );
    assert!(
        !learner.predict_next(&ctx_d, &ctx_a).is_empty(),
        "(d,a) 对应保留"
    );
}

/// 验证 capacity=1 的边界行为
#[test]
fn test_capacity_one_eviction() {
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, 1);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    learner.record_access(&ctx_a, &ctx_b, &ctx_b);
    assert_eq!(learner.pattern_count(), 1);
    assert!(!learner.predict_next(&ctx_a, &ctx_b).is_empty());

    learner.record_access(&ctx_b, &ctx_a, &ctx_a);
    assert_eq!(learner.pattern_count(), 1);
    assert!(
        learner.predict_next(&ctx_a, &ctx_b).is_empty(),
        "(a,b) 对应被淘汰"
    );
    assert!(
        !learner.predict_next(&ctx_b, &ctx_a).is_empty(),
        "(b,a) 对应存在"
    );
}
