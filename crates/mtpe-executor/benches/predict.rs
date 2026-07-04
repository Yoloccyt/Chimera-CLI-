//! MTPE 预测延迟基准 — 使用 criterion 测量不同 N 值的预测延迟
//!
//! 对应架构层:L7 Execution
//!
//! # 运行方式
//! ```powershell
//! cargo bench -p mtpe-executor
//! ```
//!
//! # 基准内容
//! - N=1 预测延迟(单步基准)
//! - N=5 预测延迟(典型多步)
//! - N=10 预测延迟(上限)

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use mtpe_executor::{MtpeConfig, MtpeExecutor, PredictionContext};
use tokio::runtime::Runtime;

/// 构造基准测试上下文
fn bench_context() -> PredictionContext {
    PredictionContext {
        quest_id: "q-bench".into(),
        history: vec!["benchmark context".into()],
        clv: vec![0.1; 8],
    }
}

/// 基准:N=1 预测延迟
fn bench_predict_n1(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = bench_context();

    c.bench_function("predict_n1", |b| {
        b.to_async(&runtime).iter(|| {
            let executor = &executor;
            let ctx = &ctx;
            async move { executor.predict(ctx, 1).await.unwrap() }
        });
    });
}

/// 基准:N=5 预测延迟
fn bench_predict_n5(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = bench_context();

    c.bench_function("predict_n5", |b| {
        b.to_async(&runtime).iter(|| {
            let executor = &executor;
            let ctx = &ctx;
            async move { executor.predict(ctx, 5).await.unwrap() }
        });
    });
}

/// 基准:N=10 预测延迟
fn bench_predict_n10(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = bench_context();

    c.bench_function("predict_n10", |b| {
        b.to_async(&runtime).iter(|| {
            let executor = &executor;
            let ctx = &ctx;
            async move { executor.predict(ctx, 10).await.unwrap() }
        });
    });
}

criterion_group!(
    benches,
    bench_predict_n1,
    bench_predict_n5,
    bench_predict_n10
);
criterion_main!(benches);
