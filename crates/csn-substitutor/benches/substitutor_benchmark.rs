//! CSN 替代查询性能基准 — 测量 10/50/100 能力的替代查询延迟
//!
//! 对应 SubTask 2.7:性能基准测试
//!
//! # 验收标准
//! - p95 ≤ 30ms(设计目标)
//! - 目标 ≤ 20ms(留 33% 余量)
//!
//! # 运行
//! ```bash
//! cargo bench -p csn-substitutor
//! ```
//!
//! # 注意
//! Criterion 基准不使用 `#[ignore]` 属性(harness=false)。
//! 若需通过 `cargo test --ignored` 运行性能验证,参见
//! `tests/integration.rs` 中的 `test_perf_substitution_latency` 测试。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use csn_substitutor::{
    CapabilityDescriptor, CsnConfig, CsnSubstitutor, SubstitutionCandidateRegistry,
};

/// 构建带 N 个能力的替代器(无 EventBus,纯查询测量)
fn make_substitutor(n: usize) -> CsnSubstitutor {
    let config = CsnConfig::default();
    let sub = CsnSubstitutor::new(config);
    for i in 0..n {
        let id = format!("cap-{i}");
        // 生成 50 维向量,每个能力有微小差异
        let vector: Vec<f32> = (0..50)
            .map(|j| (i as f32 + j as f32 * 0.01) * 0.1)
            .collect();
        let cap = CapabilityDescriptor::new(id, vector);
        let _ = sub.register_capability(cap);
    }
    sub
}

/// 构建带 N 个能力的注册表(直接测量注册表层,绕过 CsnSubstitutor)
fn make_registry(n: usize) -> SubstitutionCandidateRegistry {
    let registry = SubstitutionCandidateRegistry::new(200);
    for i in 0..n {
        let id = format!("cap-{i}");
        let vector: Vec<f32> = (0..50)
            .map(|j| (i as f32 + j as f32 * 0.01) * 0.1)
            .collect();
        let cap = CapabilityDescriptor::new(id, vector);
        let _ = registry.register(cap);
    }
    registry
}

/// 基准:测量不同能力数量下的替代查询延迟(注册表层)
fn bench_find_substitutes(c: &mut Criterion) {
    let sizes: &[usize] = &[10, 50, 100];

    let mut group = c.benchmark_group("csn_find_substitutes");
    group.sample_size(10); // 降低 sample_size 加速(默认 100)
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in sizes {
        let registry = make_registry(n);

        group.bench_with_input(BenchmarkId::new("registry", n), &n, |b, &_| {
            b.iter(|| {
                registry.find_substitutes(black_box("cap-0"), black_box(5));
            });
        });
    }

    group.finish();
}

/// 基准:测量 CsnSubstitutor::find_substitutes 端到端延迟
fn bench_substitutor_find(c: &mut Criterion) {
    let sizes: &[usize] = &[10, 50, 100];

    let mut group = c.benchmark_group("csn_substitutor_find");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in sizes {
        let sub = make_substitutor(n);

        group.bench_with_input(BenchmarkId::new("substitutor", n), &n, |b, &_| {
            b.iter(|| {
                sub.find_substitutes(black_box("cap-0"), black_box(5));
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_find_substitutes, bench_substitutor_find);
criterion_main!(benches);
