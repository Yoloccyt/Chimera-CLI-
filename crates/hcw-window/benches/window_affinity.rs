//! 窗口亲和折减基准测试 — MCA P5 承诺不超发性能验证
//!
//! 对应架构层:L2 Memory(hcw-window)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.2 窗口亲和映射
//!
//! # 测试场景
//! - `fold_1m_l3`:大模型窗口(1M)请求 L3,不折减(最简路径)
//! - `fold_4k_l3`:小模型窗口(4K)请求 L3,折减到 L0(最严折减)
//! - `fold_256k_l3`:中等窗口(256K)请求 L3,折减到 L2 + 分块标记
//! - `max_tier_for_window`:查表性能(3 次调用)
//!
//! # 性能红线
//! `WindowAffinity::fold()` 是 O(1) 查表,不进任何热路径分配。
//! 单次调用应 < 10ns(p99 受 CPU 缓存影响,目标 < 50ns)。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hcw_window::WindowAffinity;
use hcw_window::WindowTier;

fn bench_window_affinity_fold(c: &mut Criterion) {
    let mut group = c.benchmark_group("window_affinity");
    group.sample_size(100);

    // 大模型窗口(1M):不折减
    group.bench_function("fold_1m_l3", |b| {
        b.iter(|| {
            let _ = WindowAffinity::fold(black_box(WindowTier::L3), black_box(1_000_000));
        });
    });

    // 小模型窗口(4K):L3 请求折减到 L0
    group.bench_function("fold_4k_l3", |b| {
        b.iter(|| {
            let _ = WindowAffinity::fold(black_box(WindowTier::L3), black_box(4_096));
        });
    });

    // 中等窗口(256K):L3 请求折减到 L2
    group.bench_function("fold_256k_l3", |b| {
        b.iter(|| {
            let _ = WindowAffinity::fold(black_box(WindowTier::L3), black_box(262_144));
        });
    });

    // max_tier_for_window 查表
    group.bench_function("max_tier_for_window", |b| {
        b.iter(|| {
            let _ = WindowAffinity::max_tier_for_window(black_box(1_000_000));
            let _ = WindowAffinity::max_tier_for_window(black_box(4_096));
            let _ = WindowAffinity::max_tier_for_window(black_box(262_144));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_window_affinity_fold);
criterion_main!(benches);
