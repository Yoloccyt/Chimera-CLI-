//! GEA 门控计算性能基准 — criterion 基准测试
//!
//! 对应 SubTask 23.6
//!
//! # 基准配置
//! - warmup: 10 次迭代
//! - measurement: 100 次采样
//! - 测量 P50/P99 延迟

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventBus;
use gea_activator::{compute_gate_value, ExpertProfile, GeaActivator, GeaConfig, TaskProfile};

fn bench_gate_compute(c: &mut Criterion) {
    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    c.bench_with_input(
        BenchmarkId::new("gate_compute", "64dim"),
        &(&task, &expert, &config),
        |b, &(task, expert, config)| {
            b.iter(|| compute_gate_value(task, expert, config));
        },
    );
}

fn bench_gate_compute_512dim(c: &mut Criterion) {
    let config = GeaConfig::default();
    let expert = ExpertProfile::new("e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()]);
    // 512 维 CLV(与 64 维专家向量不等长,测试维度差异下的性能)
    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 512]);

    c.bench_with_input(
        BenchmarkId::new("gate_compute", "512dim_clv"),
        &(&task, &expert, &config),
        |b, &(task, expert, config)| {
            b.iter(|| compute_gate_value(task, expert, config));
        },
    );
}

fn bench_activate_with_cache(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    // 注册 5 个专家
    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);

    // 预热缓存
    rt.block_on(activator.activate(&task)).unwrap();

    c.bench_function("activate_cached", |b| {
        b.iter(|| {
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

fn bench_activate_no_cache(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();

    // 注册 5 个专家
    for i in 0..5 {
        let mut v = vec![0.0; 64];
        v[i] = 1.0;
        activator.register_expert(ExpertProfile::new(
            format!("e-{i}"),
            v,
            0.8,
            vec!["code-gen".into()],
        ));
    }

    c.bench_function("activate_no_cache", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            // 每次用不同任务,避免缓存命中
            let task = TaskProfile::new(
                0.5 + (idx % 100) as f32 * 0.005,
                "code-gen",
                30,
                vec![0.5; 64],
            );
            idx += 1;
            rt.block_on(activator.activate(&task)).unwrap();
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(std::time::Duration::from_millis(500));
    targets = bench_gate_compute, bench_gate_compute_512dim, bench_activate_with_cache, bench_activate_no_cache
}

criterion_main!(benches);
