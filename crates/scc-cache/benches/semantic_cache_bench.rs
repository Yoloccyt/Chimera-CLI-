//! 语义缓存基准测试 — ADR-069 Token 效率优化性能验证
//!
//! 红线指标:
//! - semantic_query_256_entries < 50μs (目标 < 10μs)
//! - semantic_insert < 20μs (目标 < 5μs)
//! - cache_key_compute (SHA-256 x2) < 5μs (目标 < 1μs)

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::affinity::{ThinkingPreference, TokenCacheKey};
use scc_cache::semantic_cache::SemanticResponseCache;

/// 构造测试用 TokenCacheKey
fn test_key() -> TokenCacheKey {
    TokenCacheKey {
        model: "glm-5.2".into(),
        model_version: "2026-07".into(),
        tool_schema_hash: [1u8; 32],
        system_prompt_hash: [2u8; 32],
        thinking_tier: ThinkingPreference::Standard,
    }
}

/// 构造 512 维 CLV 向量
fn test_clv(seed: f32) -> Vec<f32> {
    vec![seed; 512]
}

/// 预填充 256 条目的缓存
fn prefilled_cache() -> SemanticResponseCache {
    let cache = SemanticResponseCache::default();
    let base_key = test_key();
    for i in 0..256 {
        let mut key = base_key.clone();
        key.model = format!("model-{i}").into();
        let clv: Vec<f32> = (0..512).map(|j| (i as f32 + j as f32) * 0.001).collect();
        cache.insert(
            "bench-ns",
            key,
            clv,
            &format!("response-{i}"),
            [0u8; 32],
            i as u64,
        );
    }
    cache
}

fn bench_semantic_query(c: &mut Criterion) {
    let cache = prefilled_cache();
    let key = test_key();
    let query_clv = test_clv(0.5);

    c.bench_function("semantic_query_256_entries", |b| {
        b.iter(|| {
            black_box(cache.lookup(
                black_box("bench-ns"),
                black_box(&key),
                black_box(&query_clv),
            ))
        })
    });
}

fn bench_semantic_insert(c: &mut Criterion) {
    let cache = SemanticResponseCache::default();
    let key = test_key();
    let clv = test_clv(0.5);

    c.bench_function("semantic_insert", |b| {
        b.iter(|| {
            cache.insert(
                black_box("bench-ns"),
                black_box(key.clone()),
                black_box(clv.clone()),
                black_box("response"),
                black_box([0u8; 32]),
                black_box(1000),
            )
        })
    });
}

fn bench_cache_key_hash(c: &mut Criterion) {
    use sha2::{Digest, Sha256};

    let system_prompt = "You are Chimera, an AI coding assistant. Follow instructions carefully.";
    let tools_json = r#"[{"name":"read_file","description":"Read a file","parameters_schema":"{\"type\":\"object\"}"}]"#;

    c.bench_function("cache_key_compute_sha256x2", |b| {
        b.iter(|| {
            let mut h1 = Sha256::new();
            h1.update(black_box(system_prompt).as_bytes());
            let hash1: [u8; 32] = h1.finalize().into();

            let mut h2 = Sha256::new();
            h2.update(black_box(tools_json).as_bytes());
            let hash2: [u8; 32] = h2.finalize().into();

            black_box((hash1, hash2))
        })
    });
}

criterion_group!(
    benches,
    bench_semantic_query,
    bench_semantic_insert,
    bench_cache_key_hash,
);
criterion_main!(benches);
