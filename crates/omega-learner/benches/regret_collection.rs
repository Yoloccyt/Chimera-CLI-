//! 后悔率采集管线性能基准 — R2 解冻阶段③ 前置 1
//!
//! 对应架构层: L6 omega-learner
//! 对应 ADR: ADR-052 待办 1 / ADR-049 决策 6(性能可证伪)
//!
//! # SLO 目标
//!
//! 采集管线在影子模式下每步调用,必须低开销:
//! - `record`:单步观测追加 SLO < 1µs(热路径,每学习步调用)
//! - `assess_trend`:窗口趋势评估 SLO < 100µs(周期性调用,非每步)
//!
//! # 运行
//! ```powershell
//! cargo bench -p omega-learner --bench regret_collection
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use omega_learner::regret_pipeline::RegretCollector;

/// 单步 record 开销(热路径,每学习步调用)
fn bench_record(c: &mut Criterion) {
    c.bench_function("regret_collection/record", |b| {
        let mut collector = RegretCollector::new(128, 2, 0.05);
        let mut step = 0u64;
        b.iter(|| {
            step += 1;
            collector.record_regret(black_box(step), black_box(0.5));
        });
    });
}

/// assess_trend 趋势评估开销(不同窗口容量规模)
fn bench_assess_trend(c: &mut Criterion) {
    let mut group = c.benchmark_group("regret_collection/assess_trend");
    for &capacity in &[32usize, 128, 512] {
        // 预填满窗口
        let mut collector = RegretCollector::new(capacity, 2, 0.05);
        for step in 0..capacity {
            collector.record_regret(step as u64 + 1, 1.0 - step as f64 * 0.001);
        }
        group.bench_function(BenchmarkId::from_parameter(capacity), |b| {
            b.iter(|| {
                black_box(collector.assess_trend());
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_record, bench_assess_trend);
criterion_main!(benches);
