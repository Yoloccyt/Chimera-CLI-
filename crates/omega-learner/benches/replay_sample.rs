//! 经验回放池采样性能基线基准 — polish v2.7 Phase 0
//!
//! 对应任务: **P0-5**(建立性能基线,ADR-049 决策 6"性能可证伪门禁")
//! 对应 ADR: **ADR-049**(PER 落点 omega-learner)/ **ADR-042**(数据仅供 R1 路径)
//!
//! # 基准目的
//!
//! 记录既有均匀采样 `ReplayPool`(Mutex + VecDeque,FIFO 淘汰,有放回均匀采样)
//! 在 1K / 10K / 100K 规模下的 push 与 sample 延迟曲线,作为 Phase 4
//! `per_buffer.rs`(SumTree 优先级采样)的对比基线:
//!
//! - **验收门禁**(ADR-049 决策 4):PER 在 10 万规模采样 p99 < 100µs
//! - **对比维度**:均匀采样 O(1)/条 vs SumTree O(log n)/条,量化优先级采样的
//!   额外开销是否在可接受范围
//!
//! # 基准场景
//!
//! - `push_{1k,10k,100k}`: 池已满时的 push 延迟(含 FIFO 淘汰路径)
//! - `sample_batch32_{1k,10k,100k}`: batch=32 的采样延迟(off-policy 训练典型 batch)
//! - `sample_batch256_100k`: 大 batch 压力场景

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use omega_learner::per_buffer::PerBuffer;
use omega_learner::replay_pool::ReplayPool;
use rand::rngs::StdRng;
use rand::SeedableRng;

/// 样本类型:模拟 ~200 字节的轨迹条目(与 replay_pool.rs 容量规划注释一致)
///
/// WHY 固定大小数组而非 String:消除堆分配噪声,基准聚焦池本身的
/// 锁竞争与采样算法开销,而非样本构造成本。
#[derive(Clone)]
struct FakeTrajectory {
    #[allow(dead_code)]
    payload: [u8; 200],
}

impl FakeTrajectory {
    fn new(seed: u8) -> Self {
        Self {
            payload: [seed; 200],
        }
    }
}

/// 构造已填满至指定规模的回放池
fn make_filled_pool(size: usize) -> ReplayPool<FakeTrajectory> {
    let pool = ReplayPool::with_capacity(size);
    for i in 0..size {
        pool.push(FakeTrajectory::new((i % 251) as u8));
    }
    pool
}

/// push 延迟基准 — 池满状态下包含 FIFO 淘汰路径
fn bench_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_pool_push");
    for &size in &[1_000usize, 10_000, 100_000] {
        let pool = make_filled_pool(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| pool.push(black_box(FakeTrajectory::new(42))))
        });
    }
    group.finish();
}

/// batch=32 采样延迟基准 — off-policy 训练典型 mini-batch
fn bench_sample_batch32(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay_pool_sample_batch32");
    for &size in &[1_000usize, 10_000, 100_000] {
        let pool = make_filled_pool(size);
        // 固定种子保证跨运行可比性(基线对比要求确定性)
        let mut rng = StdRng::seed_from_u64(0xC0FFEE);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| black_box(pool.sample(black_box(32), &mut rng)))
        });
    }
    group.finish();
}

/// batch=256 大批量采样压力基准 — 100K 规模
fn bench_sample_batch256_100k(c: &mut Criterion) {
    let pool = make_filled_pool(100_000);
    let mut rng = StdRng::seed_from_u64(0xC0FFEE);
    c.bench_function("replay_pool_sample_batch256_100k", |b| {
        b.iter(|| black_box(pool.sample(black_box(256), &mut rng)))
    });
}

// ============================================================
// PER(SumTree)对比基准 — polish-v2.7 P4-1 验收(KPI-P4)
//
// 验收门禁(ADR-049 决策 4):100K 规模 batch 采样 p99 < 100µs。
// 与上方均匀采样基准同机同 profile 对比,量化 SumTree O(log n)
// 优先级采样相对均匀采样的额外开销。
// ============================================================

/// 构造已填满至指定规模的 PER 缓冲区(TD 误差均匀分布)
fn make_filled_per(size: usize) -> PerBuffer<u64> {
    let buffer = PerBuffer::with_capacity(size);
    for i in 0..size {
        // TD 误差在 [0.1, 2.1) 分布,模拟真实训练的优先级离散
        let td = 0.1 + (i % 20) as f32 * 0.1;
        buffer.push(i as u64, td);
    }
    buffer
}

/// PER push 延迟基准 — O(log n) 环形覆写 + 树修正
fn bench_per_push(c: &mut Criterion) {
    let mut group = c.benchmark_group("per_push");
    for &size in &[1_000usize, 10_000, 100_000] {
        let buffer = make_filled_per(size);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| buffer.push(black_box(42), black_box(1.0)))
        });
    }
    group.finish();
}

/// PER batch=32 优先级采样延迟基准(KPI-P4 主验收场景)
fn bench_per_sample_batch32(c: &mut Criterion) {
    let mut group = c.benchmark_group("per_sample_batch32");
    for &size in &[1_000usize, 10_000, 100_000] {
        let buffer = make_filled_per(size);
        let mut rng = StdRng::seed_from_u64(0xC0FFEE);
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| black_box(buffer.sample(black_box(32), &mut rng)))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_push,
    bench_sample_batch32,
    bench_sample_batch256_100k,
    bench_per_push,
    bench_per_sample_batch32
);
criterion_main!(benches);
