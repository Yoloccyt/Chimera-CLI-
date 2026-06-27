//! SCC 缓存性能基准 — 命中/未命中延迟测量
//!
//! 对应架构层:L3 Storage
//!
//! # 基准项
//! - `cache_hit`:缓存命中延迟(get_or_prefetch 命中路径)
//! - `cache_miss`:缓存未命中延迟(get_or_prefetch 未命中路径)
//! - `cache_insert`:插入延迟(insert 含 LRU 驱逐检查)
//!
//! 运行:`cargo bench -p scc-cache`

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use scc_cache::{ContextEntry, ContextId, SccCache, SccConfig};

/// 缓存命中延迟基准
fn bench_cache_hit(c: &mut Criterion) {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus);

    // 预填充条目
    let id = ContextId::new("ctx-hit");
    cache.insert(ContextEntry::new("ctx-hit", "benchmark content"));

    c.bench_function("cache_hit", |b| {
        b.iter(|| {
            black_box(cache.get_or_prefetch(black_box(&id)));
        });
    });
}

/// 缓存未命中延迟基准
fn bench_cache_miss(c: &mut Criterion) {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus);

    // 使用不存在的 ID,确保未命中
    let id = ContextId::new("ctx-miss");

    c.bench_function("cache_miss", |b| {
        b.iter(|| {
            black_box(cache.get_or_prefetch(black_box(&id)));
        });
    });
}

/// 缓存插入延迟基准(含 LRU 驱逐检查)
fn bench_cache_insert(c: &mut Criterion) {
    let bus = EventBus::new();
    let cache = SccCache::new(SccConfig::default(), bus);

    let mut counter = 0u64;

    c.bench_function("cache_insert", |b| {
        b.iter(|| {
            let id = ContextId::new(format!("ctx-bench-{counter}"));
            counter = counter.wrapping_add(1);
            cache.insert(ContextEntry::new(id, "insert benchmark content"));
        });
    });
}

criterion_group!(
    benches,
    bench_cache_hit,
    bench_cache_miss,
    bench_cache_insert
);
criterion_main!(benches);
