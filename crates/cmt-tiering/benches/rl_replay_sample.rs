//! 分层回放池采样性能基准(closure Stage C-12)
//!
//! 对应架构层: L3 Storage(cmt-tiering)
//! 对应 ADR: ADR-049 决策 1(分层回放池落点 cmt-tiering)+ 决策 6(性能可证伪)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §7.1(Hot/Warm/Cold 分层采样)
//!
//! # 测量目标
//!
//! 分层采样(0.25/0.25/0.5 三层配比)相对单层均匀采样的额外开销——
//! 分层是"失败经验占半"的学习价值设计,本 bench 固化其性能成本上界,
//! 供 omega-learner 均匀 ReplayPool 与本池的选型对照。
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p cmt-tiering --bench rl_replay_sample
//! ```

use cmt_tiering::rl_replay_pool::{ReplayExperience, TieredReplayPool};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::rngs::StdRng;
use rand::SeedableRng;

/// 构建三层均有数据的回放池
///
/// 奖励分布设计:1/3 高价值失败(reward<-5 → Cold),
/// 2/3 常规经验(近期 → Hot,溢出迁移 Warm),覆盖三层采样路径。
fn filled_pool(n: usize) -> TieredReplayPool {
    let pool = TieredReplayPool::new();
    for i in 0..n {
        let (reward, success) = if i.is_multiple_of(3) {
            (-8.0, false) // 高价值失败 → Cold 层
        } else {
            (1.0, true) // 常规经验 → Hot/Warm 层
        };
        pool.store(ReplayExperience {
            experience_id: format!("exp-{i}"),
            reward,
            success,
            payload: vec![0u8; 64], // 64B 负载模拟降维后的轨迹摘要
        });
    }
    pool
}

/// 分层采样规模曲线(1K / 10K 条经验,batch=32)
fn tiered_sample_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("rl_replay_pool/sample_batch32");
    for &n in &[1_000usize, 10_000] {
        let pool = filled_pool(n);
        let mut rng = StdRng::seed_from_u64(42);
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| {
                let batch = pool.sample(black_box(32), &mut rng);
                black_box(batch);
            });
        });
    }
    group.finish();
}

/// 存储路由延迟(分层判定 + 入层,含 Cold 失败路由分支)
fn tiered_store(c: &mut Criterion) {
    let pool = filled_pool(10_000);
    let mut i = 0usize;
    c.bench_function("rl_replay_pool/store_10k", |b| {
        b.iter(|| {
            i = i.wrapping_add(1);
            pool.store(black_box(ReplayExperience {
                experience_id: format!("bench-{i}"),
                reward: if i.is_multiple_of(3) { -8.0 } else { 1.0 },
                success: !i.is_multiple_of(3),
                payload: vec![0u8; 64],
            }));
        });
    });
}

criterion_group!(benches, tiered_sample_scale, tiered_store);
criterion_main!(benches);
