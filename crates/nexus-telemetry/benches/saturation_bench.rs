//! 饱和判定纯函数 micro-bench（第三轮 R6a 批，登记 ADR-304 测量纪律）
//!
//! WHY 存在：telemetry 的守护不对称病灶（取证：crate 469 行 src 享 4 道机器门
//! 却 bench=0，判定纯函数零性能守护）——判定函数虽是 O(1)，但它们是饱和降级
//! 链的入口判定，回归形态只能是「变慢」而非「变错」，无 bench 即无机器证据。
//!
//! 阈值口径：绝对阈值随文件注册进 CI 运行，初值取 1ms 宽松档（纯函数 ns 级，
//! 4-5 个数量级余量），首跑后按校准批次定档收紧（I4 只减不增棘轮允许收紧，
//! 与 faae_scoring 先例同口径）——【首跑数值待验证：CI bench_check Run 步输出】。

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use nexus_telemetry::{report_needed, saturation_ratio_bp};

fn bench_saturation_ratio_bp(c: &mut Criterion) {
    let mut group = c.benchmark_group("saturation_ratio_bp");
    // 表驱动：常态/饱和/超压/单线程/零活跃 全档（f32 除法 + clamp 的完整输入面）
    for (active, threads) in [
        (0usize, 8usize),
        (1, 8),
        (4, 8),
        (7, 8),
        (8, 8),
        (16, 8),
        (100, 8),
        (1, 1),
    ] {
        group.bench_function(format!("active{active}_threads{threads}"), |b| {
            b.iter(|| saturation_ratio_bp(black_box(active), black_box(threads)))
        });
    }
    group.finish();
}

fn bench_report_needed(c: &mut Criterion) {
    let mut group = c.benchmark_group("report_needed");
    // 表驱动：阈下静默 / 首次超阈（哨兵）/ 同数静默 / 变化上报 / 高占用 / 界下 1bp
    for (ratio, previous, degraded) in [
        (500u32, u64::MAX, 0u64),
        (900, u64::MAX, 0),
        (900, 3, 3),
        (900, 3, 4),
        (2000, 1, 2),
        (899, u64::MAX, 0),
    ] {
        group.bench_function(format!("r{ratio}_p{previous}_d{degraded}"), |b| {
            b.iter(|| {
                report_needed(
                    black_box(ratio),
                    black_box(900),
                    black_box(degraded),
                    black_box(previous),
                )
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_saturation_ratio_bp, bench_report_needed);
criterion_main!(benches);
