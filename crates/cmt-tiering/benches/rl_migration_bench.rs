//! DQN 记忆迁移决策基准（Milestone D-2e：性能可证伪）
//!
//! 运行: `cargo bench -p cmt-tiering --bench rl_migration_bench`
//! 基线价值: decide_tier 为纳秒级热路径（每记忆块迁移决策），
//! 性能回退红线由 perf 脚本/CI benchmark check 守护。

use cmt_tiering::rl_migration::{DQNMigrationPolicy, MigrationState};
use criterion::{criterion_group, criterion_main, Criterion};

/// 默认专家先验权重 + ε=0.1（确定性为主、少量探索）
fn policy() -> DQNMigrationPolicy {
    DQNMigrationPolicy::new(
        vec![
            [1.0, 0.5, 0.2, -1.0],
            [0.5, 1.0, 0.5, -0.3],
            [0.1, 0.5, 1.0, -0.1],
            [-0.5, -0.2, 0.5, 0.5],
        ],
        0.1,
        4096,
    )
}

/// 典型迁移状态（中等访问频率）
fn state() -> MigrationState {
    MigrationState {
        chunk_id: "bench-chunk".into(),
        access_frequency_1m: 12,
        access_frequency_10m: 80,
        access_frequency_1h: 400,
        last_access_age_ms: 60_000,
    }
}

fn bench_decide_tier(c: &mut Criterion) {
    let policy = policy();
    let state = state();
    let mut group = c.benchmark_group("rl_migration/decide_tier");
    group.bench_function("single_decision", |b| b.iter(|| policy.decide_tier(&state)));
    group.finish();
}

criterion_group!(benches, bench_decide_tier);
criterion_main!(benches);
