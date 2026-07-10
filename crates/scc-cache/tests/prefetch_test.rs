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
/// 当插入超过容量上限的独立 current 上下文时,最早访问的上下文应被
/// LRU 淘汰,确保内存占用有界。
#[test]
fn test_pattern_capacity_limit() {
    let capacity = 10_000;
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, capacity);

    // 插入 2 倍容量的独立 current 上下文
    for i in 0..(capacity * 2) {
        let current = ContextId::new(format!("ctx-{}", i));
        let next = ContextId::new(format!("next-{}", i));
        learner.record_access(&current, &next);
    }

    // 验证容量未超过上限
    assert_eq!(
        learner.pattern_count(),
        capacity,
        "转移矩阵容量应被限制在 {}",
        capacity
    );

    // 验证最早插入的上下文已被淘汰
    let evicted = ContextId::new("ctx-0");
    assert!(
        learner.predict_next(&evicted).is_empty(),
        "最早访问的上下文应被 LRU 淘汰"
    );

    // 验证最近插入的上下文仍在
    let recent = ContextId::new(format!("ctx-{}", capacity * 2 - 1));
    assert!(
        !learner.predict_next(&recent).is_empty(),
        "最近访问的上下文应保留"
    );
}

/// 验证重新访问会更新 LRU 顺序
///
/// 当 capacity=3 时,访问顺序为 A→B→C→A→D:
/// - A 被重新访问后变为 MRU
/// - 插入 D 时应淘汰最旧的 B,而非 A
#[test]
fn test_lru_order_updated_on_reaccess() {
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, 3);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");
    let ctx_d = ContextId::new("ctx-d");

    learner.record_access(&ctx_a, &ctx_b);
    learner.record_access(&ctx_b, &ctx_c);
    learner.record_access(&ctx_c, &ctx_a);
    // 重新访问 A,使其变为 MRU
    learner.record_access(&ctx_a, &ctx_d);
    // 插入 D,应淘汰 B(当前 LRU)
    learner.record_access(&ctx_d, &ctx_a);

    assert_eq!(learner.pattern_count(), 3, "容量应保持在 3");
    assert!(
        !learner.predict_next(&ctx_a).is_empty(),
        "重新访问的 A 应保留"
    );
    assert!(
        learner.predict_next(&ctx_b).is_empty(),
        "最久未访问的 B 应被淘汰"
    );
    assert!(!learner.predict_next(&ctx_c).is_empty(), "C 应保留");
    assert!(!learner.predict_next(&ctx_d).is_empty(), "D 应保留");
}

/// 验证 capacity=1 的边界行为
#[test]
fn test_capacity_one_eviction() {
    let learner = AccessPatternLearner::with_capacity(EventBus::new(), 0.6, 1);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    learner.record_access(&ctx_a, &ctx_b);
    assert_eq!(learner.pattern_count(), 1);
    assert!(!learner.predict_next(&ctx_a).is_empty());

    learner.record_access(&ctx_b, &ctx_a);
    assert_eq!(learner.pattern_count(), 1);
    assert!(learner.predict_next(&ctx_a).is_empty(), "A 应被淘汰");
    assert!(!learner.predict_next(&ctx_b).is_empty(), "B 应存在");
}
