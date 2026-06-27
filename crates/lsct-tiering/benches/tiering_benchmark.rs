//! LSCT 分层性能基准 — 升降温延迟与吞吐量测量
//!
//! 对应架构层:L3 Storage
//!
//! # 验收指标
//! - tick() p95 ≤ 50ms(1000 能力)
//! - apply_decision() p95 ≤ 50ms
//! - handle_quest_created() p95 ≤ 50ms
//!
//! # 运行方式
//! ```bash
//! cargo bench -p lsct-tiering
//! ```

use cmt_tiering::Tier;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use lsct_tiering::{LsctConfig, LsctCoordinator, TaskLoadProfile, TaskType};

/// 构建 n 个能力的 coordinator
fn build_coordinator(n: usize) -> LsctCoordinator {
    let coordinator = LsctCoordinator::new(LsctConfig::default());
    for i in 0..n {
        // 交替注册到不同层级
        let tier = match i % 4 {
            0 => Tier::Hot,
            1 => Tier::Warm,
            2 => Tier::Cold,
            _ => Tier::Ice,
        };
        coordinator.register_capability(&format!("cap-{i}"), tier);
    }
    coordinator
}

/// 基准 1:tick() 决策生成延迟(10/100/1000 能力)
fn bench_tick(c: &mut Criterion) {
    let mut group = c.benchmark_group("tick");

    for n in [10usize, 100, 1000] {
        let coordinator = build_coordinator(n);
        let profile = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let decisions = black_box(&coordinator).tick(black_box(&profile));
                black_box(decisions);
            });
        });
    }

    group.finish();
}

/// 基准 2:apply_decision() 单次升降温延迟
fn bench_apply_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_decision");

    let coordinator = LsctCoordinator::new(LsctConfig::default());
    coordinator.register_capability("cap-promote", Tier::Warm);
    coordinator.register_capability("cap-demote", Tier::Hot);

    let promote_decision = lsct_tiering::TierSwitchDecision::Promote {
        capability_id: "cap-promote".into(),
        from: Tier::Warm,
        to: Tier::Hot,
        reason: "bench".into(),
    };

    // WHY 使用 to_async:apply_decision 是 async fn,需要 tokio runtime
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
        panic!("创建 tokio runtime 失败: {e}");
    });

    group.bench_function("promote", |b| {
        b.iter(|| {
            // 每次重置 promoter 以允许重复升温
            black_box(&coordinator).tick(&TaskLoadProfile::new(TaskType::Run, 0.9, 1));
            rt.block_on(black_box(&coordinator).apply_decision(black_box(&promote_decision)))
                .ok();
        });
    });

    group.finish();
}

/// 基准 3:handle_quest_created() 完整流程延迟
fn bench_handle_quest_created(c: &mut Criterion) {
    let mut group = c.benchmark_group("handle_quest_created");

    for n in [10usize, 100, 1000] {
        let coordinator = build_coordinator(n);

        let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
            panic!("创建 tokio runtime 失败: {e}");
        });

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    black_box(&coordinator)
                        .handle_quest_created("compile production release")
                        .await
                        .ok();
                });
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_tick,
    bench_apply_decision,
    bench_handle_quest_created
);
criterion_main!(benches);
