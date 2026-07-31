//! shadow 模块统计热路径 benchmark(§3.4.1 性能可证伪)
//!
//! 覆盖晋级门统计判定的三条路径:
//! - `wilson_lower_bound`:解析式(预期 <100ns,非热点,基线留档)
//! - `moving_block_bootstrap_lower`:B=10000 重采样,晋级门唯一重计算路径
//! - `effective_lower_bound`(哨兵拒绝路径):游程检验 + bootstrap 全链
//!
//! 基线用途:接入 `.github/workflows/bench_check.yml` 阈值断言,
//! 防止统计核重构引入性能回归(bootstrap 在 n=25/B=10000 下应 <50ms)。

use chimera_mas::shadow::{
    effective_lower_bound, moving_block_bootstrap_lower, wilson_lower_bound,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// 构造哨兵必拒的强聚集序列(触发 bootstrap 全链)
fn clustered_outcomes(n: usize) -> Vec<bool> {
    (0..n).map(|i| i < n * 2 / 3).collect()
}

fn bench_wilson(c: &mut Criterion) {
    c.bench_function("shadow_stats/wilson_lower_bound_n25", |b| {
        b.iter(|| wilson_lower_bound(black_box(17), black_box(25)))
    });
}

fn bench_bootstrap(c: &mut Criterion) {
    let outcomes_14 = clustered_outcomes(14);
    let outcomes_25 = clustered_outcomes(25);
    c.bench_function("shadow_stats/bootstrap_n14_b10000", |b| {
        b.iter(|| moving_block_bootstrap_lower(black_box(&outcomes_14), black_box(42)))
    });
    c.bench_function("shadow_stats/bootstrap_n25_b10000", |b| {
        b.iter(|| moving_block_bootstrap_lower(black_box(&outcomes_25), black_box(42)))
    });
}

fn bench_effective_lower_bound(c: &mut Criterion) {
    let outcomes = clustered_outcomes(25);
    c.bench_function(
        "shadow_stats/effective_lower_bound_sentinel_reject_n25",
        |b| b.iter(|| effective_lower_bound(black_box(&outcomes), black_box(42))),
    );
}

criterion_group!(
    benches,
    bench_wilson,
    bench_bootstrap,
    bench_effective_lower_bound
);
criterion_main!(benches);
