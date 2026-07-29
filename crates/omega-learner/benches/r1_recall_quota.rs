//! R1 召回配额离线 RL 性能基准（P4-W16.2.2 步骤 7）
//!
//! 对应 ADR: **ADR-042**（R2 冻结）+ **ADR-043**（R1 影子模式）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.5
//!
//! # 基准场景
//!
//! - **CQL train_step**: 单步 CQL 训练延迟（目标 < 5ms）
//! - **IQL train_step**: 单步 IQL 训练延迟（目标 < 7ms）
//! - **CQL select_quota**: CQL 推理延迟（目标 < 100μs）
//! - **IQL select_quota**: IQL 推理延迟（目标 < 100μs）
//! - **CQL full_train_100_iters**: 100 轮完整训练延迟（目标 < 500ms）

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::RecallQuota;
use omega_learner::r1_recall_quota::{R1Context, RecallQuotaLearner, RecallQuotaTransition};
use omega_learner::replay_pool::ReplayPool;
use omega_learner::s2_memory::TaskPhase;
use rand::thread_rng;

/// 构造填充好的回放池（200 轨迹，满足 min_pool_size）
fn make_filled_pool() -> ReplayPool<RecallQuotaTransition> {
    let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
    let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
    let next_ctx = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
    for _ in 0..200 {
        pool.push(
            RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next_ctx, false, "q-1")
                .unwrap(),
        );
    }
    pool
}

/// 构造样本 R1 上下文
fn make_ctx() -> R1Context {
    R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap()
}

fn bench_cql_train_step(c: &mut Criterion) {
    let pool = make_filled_pool();
    let mut learner = RecallQuotaLearner::default_cql().unwrap();

    c.bench_function("r1_cql_train_step", |b| {
        b.iter(|| {
            let mut rng = thread_rng();
            learner
                .train(black_box(&pool), black_box(&mut rng))
                .unwrap()
        })
    });
}

fn bench_iql_train_step(c: &mut Criterion) {
    let pool = make_filled_pool();
    let mut learner = RecallQuotaLearner::default_iql().unwrap();

    c.bench_function("r1_iql_train_step", |b| {
        b.iter(|| {
            let mut rng = thread_rng();
            learner
                .train(black_box(&pool), black_box(&mut rng))
                .unwrap()
        })
    });
}

fn bench_cql_select_quota(c: &mut Criterion) {
    let pool = make_filled_pool();
    let mut learner = RecallQuotaLearner::default_cql().unwrap();
    let mut rng = thread_rng();
    learner.train(&pool, &mut rng).unwrap();
    let ctx = make_ctx();

    c.bench_function("r1_cql_select_quota", |b| {
        b.iter(|| black_box(learner.select_quota(black_box(&ctx)).unwrap()))
    });
}

fn bench_iql_select_quota(c: &mut Criterion) {
    let pool = make_filled_pool();
    let mut learner = RecallQuotaLearner::default_iql().unwrap();
    let mut rng = thread_rng();
    learner.train(&pool, &mut rng).unwrap();
    let ctx = make_ctx();

    c.bench_function("r1_iql_select_quota", |b| {
        b.iter(|| black_box(learner.select_quota(black_box(&ctx)).unwrap()))
    });
}

fn bench_cql_full_train_100_iters(c: &mut Criterion) {
    // 100 轮完整训练（验证收敛时间在可接受范围）
    c.bench_function("r1_cql_full_train_100_iters", |b| {
        b.iter(|| {
            let pool = make_filled_pool();
            let mut learner = RecallQuotaLearner::default_cql().unwrap();
            let mut rng = thread_rng();
            for _ in 0..100 {
                learner
                    .train(black_box(&pool), black_box(&mut rng))
                    .unwrap();
            }
        })
    });
}

criterion_group!(
    r1_benches,
    bench_cql_train_step,
    bench_iql_train_step,
    bench_cql_select_quota,
    bench_iql_select_quota,
    bench_cql_full_train_100_iters,
);
criterion_main!(r1_benches);
