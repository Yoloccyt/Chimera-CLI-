//! ComputeBridge 桥接性能基准(route 三态派发 + reduce 双模式归约)
//!
//! 对应任务:P1-T8(手册 §8.3 L-f / §11.1 契约),bridge roundtrip 门禁基准
//! 对应架构层:L1 Core
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`compute/bridge.rs`) 公共 API 做功能重建。
//!
//! # 基准场景
//! - `bridge_route`:六类 `TaskKind` 经 [`ComputeBridge::route`] 的三态派发延迟
//!   (纳秒级查表,ADR-127);
//! - `bridge_reduce`:经桥委托 [`reduce`](nexus_core::compute::reduce::reduce)
//!   的 Deterministic / Audit 双模式归约延迟。
//!
//! # 参考样板
//! 语法结构参照同目录完整保留的 `clv_bench.rs`。

#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::compute::reduce::{reduce, ReduceMode};
use nexus_core::compute::{bridge, ComputeBridge, TaskKind};

/// bench 1:六类任务经 `bridge().route(kind, n)` 的路由派发延迟
///
/// WHY 覆盖六类登记:`TaskKind` 穷举六类,route 查表命中不同数组槽位,
/// 验证 arc-swap RCU 快照 + 常数数组索引在每类上保持纳秒级下限。
fn bridge_route(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_route");
    for kind in TaskKind::ALL {
        group.bench_function(BenchmarkId::new("kind", format!("{kind:?}")), |be| {
            be.iter(|| {
                let plan = b.route(kind, 64);
                criterion::black_box(plan);
            });
        });
    }
    group.finish();
}

/// bench 2:经桥委托的 Deterministic / Audit 双模式归约
///
/// WHY 桥 `reduce(&vals, mode)` 委托 `reduce` 模块纯函数;两模式共享
/// 同输入,验证桥接层零额外开销(返回 f64)。
fn bridge_reduce(c: &mut Criterion) {
    let vals: Vec<f64> = (0..16_384).map(|i| (i as f64).sin()).collect();
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_reduce");
    for mode in [ReduceMode::Deterministic, ReduceMode::Audit] {
        group.bench_function(BenchmarkId::new("mode", format!("{mode:?}")), |be| {
            be.iter(|| {
                let r = b.reduce(&vals, mode);
                criterion::black_box(r);
            });
        });
    }
    // 对照:直调 reduce 模块(基准桥委托的等价性依据)
    group.bench_function("direct_reduce_deterministic", |be| {
        be.iter(|| {
            let r = reduce(&vals, ReduceMode::Deterministic);
            criterion::black_box(r);
        });
    });
    group.finish();
}

criterion_group!(benches, bridge_route, bridge_reduce);
criterion_main!(benches);
