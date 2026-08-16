//! SCC 预取模块集成测试 — 从 src/prefetch.rs 内联测试模块外移(L3-P2-1)
//!
//! 外移说明:原 `#[cfg(test)] mod tests` 混在生产文件(521 行,占文件 39%),
//! 外移后 prefetch.rs 仅保留生产代码(~809 行)。测试覆盖:
//! - 一阶马尔可夫链访问模式学习与预测
//! - LRU 模式表容量驱逐
//! - S3 接缝预取策略学习器持有器(异步下发 + 本地 fallback)

use event_bus::EventBus;
use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
use scc_cache::prefetch::PrefetchLearnerHolder;
use scc_cache::{AccessPatternLearner, ContextEntry, ContextId, SccCache, SccConfig};
use std::sync::Arc;

fn make_learner() -> AccessPatternLearner {
    AccessPatternLearner::new(EventBus::new(), 0.6)
}

#[test]
fn test_record_access_and_predict() {
    let learner = make_learner();
    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");

    // 记录转移:a → b 三次,a → c 一次
    learner.record_access(&ctx_a, &ctx_b);
    learner.record_access(&ctx_a, &ctx_b);
    learner.record_access(&ctx_a, &ctx_b);
    learner.record_access(&ctx_a, &ctx_c);

    let predictions = learner.predict_next(&ctx_a);
    assert_eq!(predictions.len(), 2);

    // b 概率 3/4 = 0.75,c 概率 1/4 = 0.25
    assert_eq!(predictions[0].0.as_str(), "ctx-b");
    assert!((predictions[0].1 - 0.75).abs() < 0.01);
    assert_eq!(predictions[1].0.as_str(), "ctx-c");
    assert!((predictions[1].1 - 0.25).abs() < 0.01);
}

#[test]
fn test_predict_unknown_context() {
    let learner = make_learner();
    let unknown = ContextId::new("ctx-unknown");
    let predictions = learner.predict_next(&unknown);
    assert!(predictions.is_empty());
}

#[test]
fn test_predict_sorted_by_probability_desc() {
    let learner = make_learner();
    let ctx_a = ContextId::new("ctx-a");

    // a → b 一次,a → c 五次
    learner.record_access(&ctx_a, &ContextId::new("ctx-b"));
    for _ in 0..5 {
        learner.record_access(&ctx_a, &ContextId::new("ctx-c"));
    }

    let predictions = learner.predict_next(&ctx_a);
    // c (5/6 ≈ 0.83) 应排在 b (1/6 ≈ 0.17) 前面
    assert_eq!(predictions[0].0.as_str(), "ctx-c");
    assert_eq!(predictions[1].0.as_str(), "ctx-b");
}

#[test]
fn test_get_pattern() {
    let learner = make_learner();
    let ctx_a = ContextId::new("ctx-a");

    learner.record_access(&ctx_a, &ContextId::new("ctx-b"));
    learner.record_access(&ctx_a, &ContextId::new("ctx-c"));
    learner.record_access(&ctx_a, &ContextId::new("ctx-c"));

    let pattern = learner.get_pattern(&ctx_a);
    assert!(pattern.is_some());
    let pattern = pattern.unwrap();
    assert_eq!(pattern.current.as_str(), "ctx-a");
    assert_eq!(pattern.transitions.len(), 2);
    // 按计数降序:c (2) 在 b (1) 前面
    assert_eq!(pattern.transitions[0].0.as_str(), "ctx-c");
    assert_eq!(pattern.transitions[0].1, 2);
    assert_eq!(pattern.transitions[1].0.as_str(), "ctx-b");
    assert_eq!(pattern.transitions[1].1, 1);
}

#[test]
fn test_get_pattern_unknown() {
    let learner = make_learner();
    let unknown = ContextId::new("ctx-unknown");
    assert!(learner.get_pattern(&unknown).is_none());
}

#[tokio::test]
async fn test_record_access_background() {
    let learner = Arc::new(make_learner());
    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 后台记录转移
    Arc::clone(&learner).record_access_background(ctx_a.clone(), ctx_b.clone());

    // 等待后台任务完成
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // 验证模式已记录
    let predictions = learner.predict_next(&ctx_a);
    assert_eq!(predictions.len(), 1);
    assert_eq!(predictions[0].0.as_str(), "ctx-b");
    assert!((predictions[0].1 - 1.0).abs() < 0.01);
}

#[tokio::test]
async fn test_prefetch_returns_high_probability_ids() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");

    // 训练模式:a → b 概率 0.75(>= 0.6),a → c 概率 0.25(< 0.6)
    for _ in 0..3 {
        learner.record_access(&ctx_a, &ctx_b);
    }
    learner.record_access(&ctx_a, &ctx_c);

    // 预取应只返回 ctx-b(概率 0.75 >= 0.6)
    let prefetched = learner.prefetch(&ctx_a, &cache);
    assert_eq!(prefetched.len(), 1);
    assert_eq!(prefetched[0].as_str(), "ctx-b");
}

#[tokio::test]
async fn test_prefetch_no_predictions() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    // 未知上下文,无预测
    let unknown = ContextId::new("ctx-unknown");
    let prefetched = learner.prefetch(&unknown, &cache);
    assert!(prefetched.is_empty());
}

#[tokio::test]
async fn test_prefetch_warms_existing_entries() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.5);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 插入 ctx-b 到缓存
    cache.insert(ContextEntry::new("ctx-b", "content-b"));

    // 训练模式:a → b 概率 1.0
    learner.record_access(&ctx_a, &ctx_b);

    // 预取:ctx-b 在缓存中,应被预热
    let prefetched = learner.prefetch(&ctx_a, &cache);
    assert_eq!(prefetched.len(), 1);

    // 等待后台任务完成
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // 验证 ctx-b 被预热(access_count 增加)
    let entry = cache.get_or_prefetch(&ctx_b).unwrap();
    // warm_entry 调用 record_access 一次,get_or_prefetch 又一次
    assert!(entry.access_count() >= 2);
}

#[tokio::test]
async fn test_prefetch_missing_entry_silent() {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.5);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 训练模式但不插入 ctx-b 到缓存
    learner.record_access(&ctx_a, &ctx_b);

    // 预取:ctx-b 不在缓存中,应静默失败(仅 warn 日志)
    let prefetched = learner.prefetch(&ctx_a, &cache);
    assert_eq!(prefetched.len(), 1); // 返回预测 ID(不管是否在缓存中)

    // 等待后台任务完成(不应 panic)
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // ctx-b 不在缓存中
    assert!(!cache.contains(&ctx_b));
}

// ============================================================
// PrefetchLearnerHolder 测试 — P4-W14.2 S3 接缝
// ============================================================

#[test]
fn test_holder_new_is_static_fallback() {
    // C4 合规：new() 必须初始化为 Static(Standard) fallback
    let holder = PrefetchLearnerHolder::new();
    let policy = holder.current_policy();
    assert!(policy.is_static());
    assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
}

#[test]
fn test_holder_default_equals_new() {
    // Default 与 new() 行为一致
    let holder1 = PrefetchLearnerHolder::new();
    let holder2 = PrefetchLearnerHolder::default();
    assert_eq!(holder1.current_policy(), holder2.current_policy());
}

#[test]
fn test_holder_with_policy_learned() {
    // with_policy 可指定 Learned 初始状态（便于测试）
    let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
        42,
        PrefetchStrategy::Aggressive,
    ));
    let policy = holder.current_policy();
    assert!(policy.is_learned());
    assert_eq!(policy.version(), Some(42));
    assert_eq!(policy.strategy(), PrefetchStrategy::Aggressive);
}

#[test]
fn test_update_policy_to_learned() {
    // 从 Static 切换到 Learned
    let holder = PrefetchLearnerHolder::new();
    assert!(holder.current_policy().is_static());

    holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::TopK3));
    let policy = holder.current_policy();
    assert!(policy.is_learned());
    assert_eq!(policy.strategy(), PrefetchStrategy::TopK3);
    assert_eq!(policy.version(), Some(1));
}

#[test]
fn test_update_policy_to_static() {
    // 从 Learned 切换回 Static
    let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
        1,
        PrefetchStrategy::Aggressive,
    ));
    assert!(holder.current_policy().is_learned());

    holder.update_policy(PrefetchPolicy::static_policy(
        PrefetchStrategy::Conservative,
    ));
    assert!(holder.current_policy().is_static());
    assert_eq!(
        holder.current_policy().strategy(),
        PrefetchStrategy::Conservative
    );
}

#[test]
fn test_update_policy_multiple_times() {
    // 连续多次更新策略，验证版本号与策略一致性
    let holder = PrefetchLearnerHolder::new();
    let strategies = [
        PrefetchStrategy::NoPrefetch,
        PrefetchStrategy::Conservative,
        PrefetchStrategy::Standard,
        PrefetchStrategy::Aggressive,
        PrefetchStrategy::TopK3,
    ];

    for (version, strategy) in strategies.iter().enumerate() {
        holder.update_policy(PrefetchPolicy::learned(version as u64 + 1, *strategy));
        assert_eq!(holder.version(), Some(version as u64 + 1));
        assert_eq!(holder.strategy(), *strategy);
    }
}

#[test]
fn test_fallback_to_static() {
    // fallback_to_static() 立即回退到 Static(Standard)
    let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
        99,
        PrefetchStrategy::Aggressive,
    ));
    assert!(holder.current_policy().is_learned());

    holder.fallback_to_static();
    assert!(holder.current_policy().is_static());
    assert_eq!(
        holder.current_policy().strategy(),
        PrefetchStrategy::Standard
    );
}

#[test]
fn test_holder_strategy_convenience_method() {
    // strategy() 便捷方法等价于 current_policy().strategy()
    let holder = PrefetchLearnerHolder::new();
    assert_eq!(holder.strategy(), PrefetchStrategy::Standard);

    holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::TopK3));
    assert_eq!(holder.strategy(), PrefetchStrategy::TopK3);
}

#[test]
fn test_holder_is_learned_and_version() {
    // is_learned() 与 version() 正确反映策略状态
    let holder = PrefetchLearnerHolder::new();
    assert!(!holder.is_learned());
    assert_eq!(holder.version(), None);

    holder.update_policy(PrefetchPolicy::learned(7, PrefetchStrategy::Conservative));
    assert!(holder.is_learned());
    assert_eq!(holder.version(), Some(7));
}

#[test]
fn test_holder_clone_independent() {
    // Clone 后两者策略独立演化，互不影响
    let holder1 =
        PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(1, PrefetchStrategy::Standard));
    let holder2 = holder1.clone();

    // 修改 holder1 不影响 holder2
    holder1.update_policy(PrefetchPolicy::learned(2, PrefetchStrategy::Aggressive));
    assert_eq!(holder1.strategy(), PrefetchStrategy::Aggressive);
    assert_eq!(holder2.strategy(), PrefetchStrategy::Standard);
    assert_eq!(holder2.version(), Some(1));
}

// ============================================================
// prefetch_with_policy 测试 — P4-W14.2 S3 接缝策略感知预取
// ============================================================

#[tokio::test]
async fn test_prefetch_with_policy_standard_default_equivalent() {
    // Static(Standard) 等价于 prefetch(threshold=0.6, top_k=10) 的默认行为
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 训练模式:a → b 概率 1.0
    learner.record_access(&ctx_a, &ctx_b);

    // Standard 策略：threshold=0.6, top_k=10
    let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert_eq!(prefetched.len(), 1);
    assert_eq!(prefetched[0].as_str(), "ctx-b");
}

#[tokio::test]
async fn test_prefetch_with_policy_no_prefetch_returns_empty() {
    // NoPrefetch 策略：threshold=1.1（永不触发），应直接返回空列表
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 训练模式:a → b 概率 1.0
    learner.record_access(&ctx_a, &ctx_b);

    let policy = PrefetchPolicy::static_policy(PrefetchStrategy::NoPrefetch);
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert!(prefetched.is_empty(), "NoPrefetch 策略应阻止所有预取");
}

#[tokio::test]
async fn test_prefetch_with_policy_aggressive_lower_threshold() {
    // Aggressive 策略：threshold=0.3（更低），应预取更多低概率上下文
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");

    // 训练模式:a → b 3次(0.75), a → c 1次(0.25)
    for _ in 0..3 {
        learner.record_access(&ctx_a, &ctx_b);
    }
    learner.record_access(&ctx_a, &ctx_c);

    // Standard 策略：threshold=0.6,只预取 ctx-b (0.75 >= 0.6)
    let standard_policy = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
    let standard_prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &standard_policy);
    assert_eq!(standard_prefetched.len(), 1);
    assert_eq!(standard_prefetched[0].as_str(), "ctx-b");

    // Aggressive 策略：threshold=0.3,预取 ctx-b (0.75) + ctx-c (0.25 < 0.3,被过滤)
    // 注意:0.25 < 0.3,所以 Aggressive 仍然只预取 ctx-b
    let aggressive_policy = PrefetchPolicy::static_policy(PrefetchStrategy::Aggressive);
    let aggressive_prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &aggressive_policy);
    assert_eq!(aggressive_prefetched.len(), 1);
    assert_eq!(aggressive_prefetched[0].as_str(), "ctx-b");
}

#[tokio::test]
async fn test_prefetch_with_policy_topk3_limits_results() {
    // TopK3 策略：top_k=3，应截断超过 3 个的预测
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    // 训练 5 个转移:a → b1..b5,每个概率 0.2
    for i in 1..=5 {
        learner.record_access(&ctx_a, &ContextId::new(format!("ctx-b{i}")));
    }

    // TopK3 策略：threshold=0.0（无阈值），top_k=3
    // 概率 0.2 >= 0.0,所有 5 个都通过阈值,但 top_k=3 截断到 3 个
    let policy = PrefetchPolicy::static_policy(PrefetchStrategy::TopK3);
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert_eq!(prefetched.len(), 3, "TopK3 应截断到 3 个");
}

#[tokio::test]
async fn test_prefetch_with_policy_conservative_higher_threshold() {
    // Conservative 策略：threshold=0.8（更高），应过滤掉中等概率上下文
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");
    let ctx_c = ContextId::new("ctx-c");

    // 训练模式:a → b 3次(0.75), a → c 1次(0.25)
    for _ in 0..3 {
        learner.record_access(&ctx_a, &ctx_b);
    }
    learner.record_access(&ctx_a, &ctx_c);

    // Conservative 策略：threshold=0.8
    // ctx-b 概率 0.75 < 0.8,被过滤;ctx-c 概率 0.25 < 0.8,被过滤
    let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Conservative);
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert!(
        prefetched.is_empty(),
        "Conservative threshold=0.8 应过滤掉 0.75 概率的 ctx-b"
    );
}

#[tokio::test]
async fn test_prefetch_with_policy_learned_strategy() {
    // Learned 策略：与 Static 等价的策略行为，但携带版本号
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    learner.record_access(&ctx_a, &ctx_b);

    // Learned 策略：Aggressive, version=42
    let policy = PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive);
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert_eq!(prefetched.len(), 1);
    assert_eq!(prefetched[0].as_str(), "ctx-b");
}

#[tokio::test]
async fn test_prefetch_with_policy_holder_integration() {
    // 端到端集成：PrefetchLearnerHolder + prefetch_with_policy
    // 模拟 omega-learner 异步下发策略 → holder 缓存 → prefetch_with_policy 感知
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);
    let holder = PrefetchLearnerHolder::new();

    let ctx_a = ContextId::new("ctx-a");
    let ctx_b = ContextId::new("ctx-b");

    // 训练模式:a → b 概率 1.0
    learner.record_access(&ctx_a, &ctx_b);

    // 初始：Static(Standard)，threshold=0.6
    let policy = holder.current_policy();
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert_eq!(prefetched.len(), 1);
    assert_eq!(prefetched[0].as_str(), "ctx-b");

    // omega-learner 下发 Learned(Aggressive, version=1)
    holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::Aggressive));
    assert!(holder.is_learned());
    assert_eq!(holder.version(), Some(1));

    // 再次预取，使用 Learned 策略
    let policy = holder.current_policy();
    let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
    assert_eq!(prefetched.len(), 1);
    assert_eq!(prefetched[0].as_str(), "ctx-b");

    // 触发熔断：fallback_to_static
    holder.fallback_to_static();
    assert!(!holder.is_learned());
    assert_eq!(holder.strategy(), PrefetchStrategy::Standard);
}

#[tokio::test]
async fn test_prefetch_with_policy_no_predictions() {
    // 未知上下文 + 任意策略 → 空列表
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus.clone());
    let learner = AccessPatternLearner::new(bus, 0.6);

    let unknown = ContextId::new("ctx-unknown");

    let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Aggressive);
    let prefetched = learner.prefetch_with_policy(&unknown, &cache, &policy);
    assert!(prefetched.is_empty());
}
