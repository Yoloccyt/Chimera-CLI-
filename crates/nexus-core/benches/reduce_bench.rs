//! DetReduce 位模式归约性能基准(Deterministic 树归约 + Audit 指数分桶)
//!
//! 对应任务:P1-T10(手册 §10.2 / ADR-102 / ADR-106),审计开销 ≤ 30% 门禁基准
//! 对应架构层:L1 Core
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`compute/reduce.rs`) 公共 API 做功能重建。
//!
//! # 基准场景
//! - `reduce_deterministic`:固定分块树归约(`tree_reduce_fixed`)延迟;
//! - `reduce_audit`:ReproBLAS 式指数分桶归约(`repro_reduce`)延迟,
//!   两模式延迟对比即"审计开销"的直接证据。
//!
//! # 参考样板
//! 语法结构参照同目录完整保留的 `clv_bench.rs`。
//!
//! # 数据
//! 固定种子确定性数据(`sin` 映射,平台无关),多规模梯度覆盖 L1/L2 cache 边界。

#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_core::compute::reduce::{reduce, ReduceMode};

/// 多规模梯度(覆盖 cache 边界)
const SIZES: &[usize] = &[4_096, 65_536, 262_144];

/// 生成固定种子可复现数据 — 平台无关(`i.sin()` 由 std 保证确定性)
fn make_data(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let x = (i as f64) * 0.001;
            x.sin() + x.cos() * 0.5
        })
        .collect()
}

/// bench 1:固定分块树归约(Deterministic)
fn reduce_deterministic(c: &mut Criterion) {
    let mut group = c.benchmark_group("reduce_deterministic");
    for size in SIZES {
        group.throughput(Throughput::Elements(*size as u64));
        let data = make_data(*size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, vals| {
            b.iter(|| {
                let r = reduce(vals, ReduceMode::Deterministic);
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

/// bench 2:ReproBLAS 指数分桶归约(Audit)
fn reduce_audit(c: &mut Criterion) {
    let mut group = c.benchmark_group("reduce_audit");
    for size in SIZES {
        group.throughput(Throughput::Elements(*size as u64));
        let data = make_data(*size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, vals| {
            b.iter(|| {
                let r = reduce(vals, ReduceMode::Audit);
                criterion::black_box(r);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, reduce_deterministic, reduce_audit);
criterion_main!(benches);
