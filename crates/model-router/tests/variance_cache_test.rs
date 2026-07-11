//! `latency_variance` 缓存优化测试 — v1.5.0-omega 性能优化
//!
//! 对应架构层:L1 Core(model-router)
//!
//! # 测试目标
//! 1. **缓存命中**:连续两次调用 `latency_variance()`,第二次命中缓存(结果一致)
//! 2. **缓存失效**:`record()` 后 `latency_variance()` 应重算并更新缓存
//! 3. **边界**:空 history 的 variance 为 0.0;n=1 时也为 0.0
//! 4. **Store 级缓存**:`InMemoryHistoryStore::latency_variance(model_id)` 操作
//!    stored record(DashMap 内的原始记录),缓存跨 `get()` clone 持久化
//! 5. **Store 级失效**:`store.record()` 后 `store.latency_variance()` 反映新数据
//!
//! # 设计说明
//! 缓存字段位于 `HistoryRecord`(每个记录缓存自己的 variance),而非
//! `InMemoryHistoryStore`(单一缓存槽无法服务多模型 store)。`InMemoryHistoryStore`
//! 通过 trait 方法 `latency_variance(model_id)` override,使用 DashMap `get()`
//! 返回的 `Ref` 操作 stored record,使缓存跨 `get()` clone 持久化。

#![forbid(unsafe_code)]

use model_router::{HistoryRecord, HistoryStore, InMemoryHistoryStore};

// ============================================================
// 测试 1:缓存命中 — 连续两次调用结果一致
// ============================================================

#[test]
fn test_cache_hit_returns_same_value() {
    let mut record = HistoryRecord::new();
    // 注入 10 个样本:100.0 ~ 109.0
    for i in 0..10u32 {
        record.record(100.0 + i as f32, true);
    }

    // 第一次调用:cache miss → 计算 → 写入缓存
    let v1 = record.latency_variance();
    // 第二次调用:cache hit → 直接返回缓存值
    let v2 = record.latency_variance();

    // 两次结果必须完全一致(缓存命中不改变值)
    assert!(
        (v1 - v2).abs() < 1e-7,
        "缓存命中应返回相同值: v1={v1}, v2={v2}"
    );
    // 验证值非零(有方差)
    assert!(v1 > 0.0, "10 个不同样本的方差应 > 0");
}

// ============================================================
// 测试 2:缓存失效 — record() 后 latency_variance() 重算
// ============================================================

#[test]
fn test_cache_invalidation_on_record() {
    let mut record = HistoryRecord::new();
    // 初始 5 个样本:全部 100.0(方差 = 0)
    for _ in 0..5 {
        record.record(100.0, true);
    }
    let v_before = record.latency_variance();
    assert!(
        v_before.abs() < 1e-6,
        "5 个相同样本方差应为 0,实际 {v_before}"
    );

    // record() 新样本:200.0 → 方差应增大,缓存应失效并重算
    record.record(200.0, true);
    let v_after = record.latency_variance();

    // 方差应显著增大(从 0 到 > 0)
    assert!(
        v_after > v_before,
        "record() 后方差应增大: before={v_before}, after={v_after}"
    );
    // 验证重算值正确:6 个样本 [100,100,100,100,100,200]
    // mean = 700/6 ≈ 116.67, sum_sq = 5*(100-116.67)^2 + (200-116.67)^2
    // = 5*277.78 + 6944.44 = 8333.33
    // variance = 8333.33 / 5 ≈ 1666.67
    assert!(
        (v_after - 1666.6667).abs() < 1.0,
        "重算方差应 ≈ 1666.67,实际 {v_after}"
    );
}

// ============================================================
// 测试 3:边界 — 空 history 与单样本
// ============================================================

#[test]
fn test_empty_record_variance_is_zero() {
    let record = HistoryRecord::new();
    // 空记录:无样本,n < 2 → 返回 0.0
    let v = record.latency_variance();
    assert!(v.abs() < 1e-6, "空记录方差应为 0.0,实际 {v}");
}

#[test]
fn test_single_sample_variance_is_zero() {
    let mut record = HistoryRecord::new();
    record.record(42.0, true);
    // 单样本:n=1 < 2 → 返回 0.0
    let v = record.latency_variance();
    assert!(v.abs() < 1e-6, "单样本方差应为 0.0,实际 {v}");
}

// ============================================================
// 测试 4:Store 级缓存 — latency_variance(model_id) 操作 stored record
// ============================================================

#[test]
fn test_store_latency_variance_returns_correct_value() {
    let store = InMemoryHistoryStore::new();
    // 注入 50 个样本:200.0 ± 5.0 抖动
    for i in 0..50u32 {
        let latency = 200.0 + (i as f32 % 10.0) - 5.0;
        store.record("model-a", latency, true);
    }

    // 通过 store 级 trait 方法获取 variance(操作 stored record)
    let v_store = store.latency_variance("model-a");
    assert!(v_store.is_some(), "有记录时应返回 Some");

    // 通过 get() + record.latency_variance() 对比(操作 clone)
    let record = store.get("model-a").expect("应有记录");
    let v_clone = record.latency_variance();

    // 两者应一致(store 级方法与 record 级方法语义等价)
    assert!(
        (v_store.unwrap() - v_clone).abs() < 1e-6,
        "store 级与 record 级 variance 应一致"
    );
}

#[test]
fn test_store_latency_variance_none_for_absent_model() {
    let store = InMemoryHistoryStore::new();
    // 不存在的模型应返回 None
    assert!(
        store.latency_variance("nonexistent").is_none(),
        "不存在的模型应返回 None"
    );
}

// ============================================================
// 测试 5:Store 级缓存持久化 — 多次调用结果一致(缓存跨调用持久)
// ============================================================

#[test]
fn test_store_cache_persists_across_calls() {
    let store = InMemoryHistoryStore::new();
    for i in 0..100u32 {
        store.record("cached-model", 150.0 + i as f32 * 0.1, true);
    }

    // 第一次调用:cache miss → 计算 → 写入 stored record 缓存
    let v1 = store
        .latency_variance("cached-model")
        .expect("应有 variance");
    // 第二次调用:cache hit(stored record 缓存命中)
    let v2 = store
        .latency_variance("cached-model")
        .expect("应有 variance");
    // 第三次调用:cache hit
    let v3 = store
        .latency_variance("cached-model")
        .expect("应有 variance");

    // 三次调用结果完全一致
    assert!(
        (v1 - v2).abs() < 1e-7,
        "第二次调用应命中缓存: v1={v1}, v2={v2}"
    );
    assert!(
        (v2 - v3).abs() < 1e-7,
        "第三次调用应命中缓存: v2={v2}, v3={v3}"
    );
}

// ============================================================
// 测试 6:Store 级缓存失效 — record() 后 variance 更新
// ============================================================

#[test]
fn test_store_cache_invalidation_on_record() {
    let store = InMemoryHistoryStore::new();
    // 初始:全部 100.0(方差 = 0)
    for _ in 0..100 {
        store.record("invalidate-test", 100.0, true);
    }
    let v_before = store
        .latency_variance("invalidate-test")
        .expect("应有 variance");
    assert!(
        v_before.abs() < 1e-6,
        "100 个相同样本方差应为 0,实际 {v_before}"
    );

    // record() 一个极端值 → 缓存失效 → 下次调用重算
    store.record("invalidate-test", 1000.0, true);
    let v_after = store
        .latency_variance("invalidate-test")
        .expect("应有 variance");

    // 方差应显著增大
    assert!(
        v_after > v_before,
        "record() 后方差应增大: before={v_before}, after={v_after}"
    );
    assert!(
        v_after > 100.0,
        "加入 1000.0 后方差应显著增大,实际 {v_after}"
    );
}

// ============================================================
// 测试 7:多模型 store 缓存独立 — 不同模型缓存互不干扰
// ============================================================

#[test]
fn test_store_cache_independent_per_model() {
    let store = InMemoryHistoryStore::new();

    // model-stable:稳定延迟(低方差)
    for i in 0..100u32 {
        store.record("model-stable", 200.0 + (i as f32 % 2.0), true);
    }
    // model-unstable:不稳定延迟(高方差)
    for i in 0..100u32 {
        let latency = if i % 2 == 0 { 10.0 } else { 990.0 };
        store.record("model-unstable", latency, true);
    }

    let v_stable = store
        .latency_variance("model-stable")
        .expect("应有 variance");
    let v_unstable = store
        .latency_variance("model-unstable")
        .expect("应有 variance");

    // 不稳定模型方差应远大于稳定模型
    assert!(
        v_unstable > v_stable,
        "不稳定模型方差应更大: stable={v_stable}, unstable={v_unstable}"
    );
    assert!(
        v_unstable > 100_000.0,
        "10/990 交替方差应极大,实际 {v_unstable}"
    );

    // 再次查询:缓存命中,值不变
    let v_stable_2 = store
        .latency_variance("model-stable")
        .expect("应有 variance");
    assert!(
        (v_stable - v_stable_2).abs() < 1e-7,
        "缓存命中应返回相同值: v1={v_stable}, v2={v_stable_2}"
    );
}

// ============================================================
// 测试 8:缓存命中后 get() clone 继承缓存值
// ============================================================

#[test]
fn test_cache_inherited_by_clone_after_population() {
    let store = InMemoryHistoryStore::new();
    for i in 0..100u32 {
        store.record("clone-test", 200.0 + i as f32 * 0.5, true);
    }

    // 通过 store 级方法填充 stored record 的缓存
    let v_store = store.latency_variance("clone-test").expect("应有 variance");

    // get() 返回 clone,clone 继承 stored record 的缓存值
    let record = store.get("clone-test").expect("应有记录");
    // clone 的 latency_variance() 应命中缓存(继承自 stored record)
    let v_clone = record.latency_variance();

    assert!(
        (v_store - v_clone).abs() < 1e-7,
        "clone 应继承 stored record 缓存值: store={v_store}, clone={v_clone}"
    );
}
