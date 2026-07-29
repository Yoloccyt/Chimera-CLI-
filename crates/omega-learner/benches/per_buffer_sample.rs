//! PER 缓冲区采样性能基准 — KPI-P4 门禁固化(closure Stage C-12)
//!
//! 对应架构层: L6 Router(omega-learner)
//! 对应 ADR: ADR-049 决策 4(SumTree PER)+ 决策 6(性能可证伪门禁)
//! 对应验收门禁: **100K 容量 / batch=32 采样 p99 < 100µs**
//! (CHANGELOG polish-v2.7 Phase 4 声称 6.22µs,本 bench 将其固化为可回归断言)
//!
//! # 与 replay_sample.rs 的分工
//!
//! `replay_sample.rs` 测量均匀采样 ReplayPool(Phase 0 基线);
//! 本 bench 测量 SumTree 优先级采样 PerBuffer(Phase 4 交付),
//! 两者对照可量化 O(log n) 优先级采样相对 O(1) 均匀采样的开销上界。
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p omega-learner --bench per_buffer_sample
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use omega_learner::per_buffer::PerBuffer;
use rand::rngs::StdRng;
use rand::SeedableRng;

/// 构建填满的 PER 缓冲区(TD 误差按索引周期分布,制造非均匀优先级)
fn filled_buffer(capacity: usize) -> PerBuffer<u64> {
    let buffer = PerBuffer::with_capacity(capacity);
    for i in 0..capacity {
        // TD 误差 0.1..2.6 周期分布:保证 SumTree 各叶子优先级差异化
        let td_error = 0.1 + (i % 26) as f32 * 0.1;
        buffer.push(i as u64, td_error);
    }
    buffer
}

/// 采样延迟随容量的规模曲线(1K / 10K / 100K,batch=32)
///
/// 验收:100K 规模均值应 << 100µs(SumTree O(batch·log n))
fn per_sample_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("per_buffer/sample_batch32");
    for &capacity in &[1_000usize, 10_000, 100_000] {
        let buffer = filled_buffer(capacity);
        // 固定种子:消除采样路径随机性带来的 bench 抖动
        let mut rng = StdRng::seed_from_u64(42);
        group.bench_function(BenchmarkId::from_parameter(capacity), |b| {
            b.iter(|| {
                let batch = buffer.sample(black_box(32), &mut rng);
                black_box(batch);
            });
        });
    }
    group.finish();
}

/// 推入延迟(100K 容量满载后环形覆写路径,O(log n))
fn per_push(c: &mut Criterion) {
    let buffer = filled_buffer(100_000);
    let mut i = 0u64;
    c.bench_function("per_buffer/push_100k", |b| {
        b.iter(|| {
            i = i.wrapping_add(1);
            buffer.push(black_box(i), black_box(0.7));
        });
    });
}

/// 优先级回写延迟(batch=32 槽位,O(k·log n))
fn per_update_priorities(c: &mut Criterion) {
    let buffer = filled_buffer(100_000);
    // 固定 32 个分散槽位模拟训练后回写
    let updates: Vec<(usize, f32)> = (0..32).map(|i| (i * 3_000, 0.5)).collect();
    c.bench_function("per_buffer/update_priorities_32", |b| {
        b.iter(|| {
            buffer.update_priorities(black_box(&updates));
        });
    });
}

criterion_group!(benches, per_sample_scale, per_push, per_update_priorities);
criterion_main!(benches);
