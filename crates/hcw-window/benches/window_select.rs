//! HCW 窗口选择 SLO 基准测试
//!
//! 对应 T7-1: window_select SLO benchmark
//!
//! # SLO 目标
//! 所有四级窗口选择延迟 < 1ms。
//!
//! # 测试场景
//! `WindowSelector::select` 是纯 O(1) 函数,按 complexity 阈值(0.25/0.5/0.75)
//! 选择窗口层级。本基准测试四个复杂度档位,分别对应四级窗口:
//! - L0(4K):complexity = 0.1(< 0.25,快速响应)
//! - L1(32K):complexity = 0.3([0.25, 0.5),常规任务)
//! - L2(128K):complexity = 0.6([0.5, 0.75),复杂任务)
//! - L3(1M 等效):complexity = 0.8(≥ 0.75,超复杂任务)
//!
//! WHY: 选择各档位中间值而非边界值,避免分支预测器对边界条件的特殊优化,
//! 更真实地反映生产环境中的选择延迟。

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use hcw_window::WindowSelector;

/// 四级窗口选择的代表性复杂度输入
///
/// WHY: 每个值对应一个窗口层级的典型区间,覆盖全部分支路径:
/// - 0.1 → L0(4K)
/// - 0.3 → L1(32K)
/// - 0.6 → L2(128K)
/// - 0.8 → L3(1M 等效)
const TIER_COMPLEXITIES: &[(f32, &str)] = &[
    (0.1, "L0_4K"),
    (0.3, "L1_32K"),
    (0.6, "L2_128K"),
    (0.8, "L3_1M"),
];

/// 基准:四级窗口选择(纯函数 O(1) 决策)
///
/// 测量 `WindowSelector::select` 在各复杂度档位的决策延迟。
/// SLO 目标:所有档位 < 1ms。
fn bench_window_select(c: &mut Criterion) {
    let mut group = c.benchmark_group("window_select");

    for &(complexity, label) in TIER_COMPLEXITIES {
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let tier = WindowSelector::select(complexity);
                // WHY: black_box 防止编译器常量折叠(所有输入是编译期已知)
                criterion::black_box(tier)
            });
        });
    }

    group.finish();
}

/// 基准:边界值窗口选择(阈值边界 0.25/0.5/0.75)
///
/// WHY: 边界值是分支条件判断的关键路径,与中间值的行为可能不同
/// (CPU 分支预测器对重复出现的边界值可能有不同命中率)。
fn bench_window_select_boundaries(c: &mut Criterion) {
    let mut group = c.benchmark_group("window_select_boundary");

    for &(complexity, label) in &[
        (0.25, "boundary_L1"),
        (0.5, "boundary_L2"),
        (0.75, "boundary_L3"),
    ] {
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let tier = WindowSelector::select(complexity);
                criterion::black_box(tier)
            });
        });
    }

    group.finish();
}

/// 基准:特殊输入(NaN / 负值 / 超范围)
///
/// WHY: 确保异常输入不会导致显著性能退化(如 NaN 处理分支)。
fn bench_window_select_edge_cases(c: &mut Criterion) {
    let mut group = c.benchmark_group("window_select_edge");

    for &(complexity, label) in &[(f32::NAN, "nan"), (-0.1, "negative"), (1.5, "overflow")] {
        group.bench_function(BenchmarkId::from_parameter(label), |b| {
            b.iter(|| {
                let tier = WindowSelector::select(complexity);
                criterion::black_box(tier)
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_window_select,
    bench_window_select_boundaries,
    bench_window_select_edge_cases,
);
criterion_main!(benches);
