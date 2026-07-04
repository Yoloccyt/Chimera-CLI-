//! 进化引擎性能基准 — criterion 基准测试
//!
//! 对应 SubTask 3.5
//!
//! # 基准配置
//! - warmup: 500ms
//! - sample_size: 20
//! - 验收:单轮进化 ≤ 500ms(criterion 自动计算 min-of-N)
//!
//! # 基准场景
//! - `evolve_once`:单轮进化(无 EventBus)
//! - `evolve_once_with_bus`:单轮进化(带 EventBus,含事件发布)
//! - `evolve_5_generations`:5 代连续进化

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use gsoe_evolution::{GsoeConfig, GsoeEvolutionEngine};
use std::time::Duration;

/// 基准:单轮进化延迟(无 EventBus)
///
/// 验收标准:≤ 500ms(目标 ≤ 300ms 留余量)
fn bench_evolve_once(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("evolve_once", |b| {
        b.iter(|| {
            let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
            rt.block_on(engine.evolve_once()).unwrap()
        });
    });
}

/// 基准:单轮进化延迟(带 EventBus,含事件发布开销)
fn bench_evolve_once_with_bus(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("evolve_once_with_bus", |b| {
        b.iter(|| {
            let bus = EventBus::new();
            let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);
            rt.block_on(engine.evolve_once()).unwrap()
        });
    });
}

/// 基准:多代进化(5 代连续)
fn bench_evolve_multi_generation(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("evolve_5_generations", |b| {
        b.iter(|| {
            let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
            rt.block_on(async {
                for _ in 0..5 {
                    let _ = engine.evolve_once().await;
                }
            });
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_evolve_once, bench_evolve_once_with_bus, bench_evolve_multi_generation
}

criterion_main!(benches);
