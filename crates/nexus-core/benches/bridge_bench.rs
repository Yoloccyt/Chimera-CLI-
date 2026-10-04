//! ComputeBridge 桥接性能基准(route 三态派发 + reduce 双模式归约)
//!
//! 对应任务:P1-T8(手册 §8.3 L-f / §11.1 契约),bridge roundtrip 门禁基准
//! 对应架构层:L1 Core
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`compute/bridge.rs`) 公共 API 做功能重建。
//!
//! # 测量口径（ADR-198 D3）
//! 基准必须固定 `CHIMERA_COMPUTE_CALIBRATION=off`。原因：校准开启后会在被采样的
//! 1/64 调用上探测式改走 Inline 路径，属路径变更；开着校准测量会让同一机器
//! 同一代码得出不可复现的数字（本仓实测同机跨轮方差可达 ±40%，不该再叠一个可变开关）。
//!
//! # 基准场景
//! - `bridge_route`:全登记类（`TaskKind::ALL`）经 [`ComputeBridge::route`] 的三态派发延迟
//!   (纳秒级查表,ADR-127);
//! - `bridge_reduce`:经桥委托 [`reduce`](nexus_core::compute::reduce::reduce)
//!   的 Deterministic / Audit 双模式归约延迟。
//! - `bridge_spawn`:经 [`spawn_compute_batch`](ComputeBridge::spawn_compute_batch)
//!   的批量卸载往返延迟（ADR-197 新增 —— 该路径上多了一对按类型计数与
//!   一次饱和判定，旧 bench 完全测不到，故补此组作为现势基线）。
//!
//! # 参考样板
//! 语法结构参照同目录完整保留的 `clv_bench.rs`。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::compute::reduce::{reduce, ReduceMode};
use nexus_core::compute::{bridge, ComputeBridge, TaskKind};

/// bench 1:全登记类任务经 `bridge().route(kind, n)` 的路由派发延迟
///
/// WHY 覆盖全登记类(遍历 `TaskKind::ALL` 单源):route 查表命中不同数组槽位,
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

/// bench 3:批量卸载往返延迟（ADR-197 新增）
///
/// WHY 必须单独一组：`bridge_route` 只测查表，`bridge_reduce` 只测归约，
/// 而饱和判定与计数都落在 `spawn_compute*` 路径上——若不测，本批改动的
/// 性能影响就是“无数据可辩”。本组不新建 SLO 阈值（避免未实测先登记，
/// ADR-190 反幽灵门），仅作为现势基线与后续 Phase B/D 的对照起点。
fn bridge_spawn(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_spawn");
    for n in [1usize, 16, 128] {
        group.bench_function(BenchmarkId::new("batch", n), |be| {
            be.iter(|| {
                // 每次迭代重建闭包集:连同 Vec 分配一起计入（真实调用方的形状）
                let closures = (0..n).map(|i| move || criterion::black_box(i) as u64 + 1);
                let results = b.spawn_compute_batch(TaskKind::ClvSimilarity, closures);
                criterion::black_box(results.len());
            });
        });
    }
    group.finish();
}

/// bench 3b:饱和判定本身的开销上界（ADR-197）
///
/// WHY 单独测：batch 往返由 rayon scope + 槽位互斥量主导，若只报往返数字，
/// 就无法回答“你新加的判定/计数到底贵不贵”。本组只测
/// [`is_saturated`](ComputeBridge::is_saturated)（一次原子读 + 一次千分比
/// 运算 + 一次阈值读），即本次改动在每调用上的成本上界。
fn saturation_check(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_saturation_check");
    group.bench_function("is_saturated", |be| {
        be.iter(|| criterion::black_box(b.is_saturated()));
    });
    group.finish();
}

criterion_group!(
    benches,
    bridge_route,
    bridge_reduce,
    bridge_spawn,
    saturation_check
);
criterion_main!(benches);
