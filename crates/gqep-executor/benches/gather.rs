//! GQEP 聚集性能基准 — criterion 基准测试
//!
//! 对应 SubTask 24.6:测量 10/50/100 操作聚集延迟
//!
//! 运行方式:
//! ```powershell
//! cargo bench -p gqep-executor
//! ```

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use gqep_executor::{GqepConfig, GqepExecutor, GqepFuture};

/// 基准测试:不同规模操作聚集的延迟
///
/// 测量 10/50/100 个即时操作(无 sleep)经 GQEP 聚集的耗时,
/// 包含 QEEP entangle 包裹 + FuturesUnordered 流式处理开销。
fn bench_gather(c: &mut Criterion) {
    // 创建 tokio runtime 供异步基准使用
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("gather");
    group.sample_size(30); // 降低样本数以加速基准(默认 100)

    for size in [10usize, 50, 100] {
        group.bench_with_input(format!("{size}_ops"), &size, |b, &size| {
            b.to_async(&rt).iter(|| async {
                let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
                let futures: Vec<GqepFuture<String>> = (0..size)
                    .map(|i| {
                        Box::pin(async move { Ok(format!("result-{i}")) }) as GqepFuture<String>
                    })
                    .collect();
                executor.gather(futures).await
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_gather);
criterion_main!(benches);
