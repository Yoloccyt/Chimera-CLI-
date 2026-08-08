//! GTPO + RLVR 基准（Milestone D-2e：性能可证伪）
//!
//! 运行: `cargo bench -p pvl-layer --bench gtpo_rlvr_bench`
//! 覆盖: Turn-Level 优势计算（100 步轨迹）与可验证奖励（100 用例）。

use criterion::{criterion_group, criterion_main, Criterion};
use pvl_layer::gtpo::{TurnTrajectory, GTPO};
use pvl_layer::rlvr::{TestCase, VerifierKind, RLVR};

fn bench_gtpo_advantages(c: &mut Criterion) {
    let gtpo = GTPO::new(0.95);
    let traj = TurnTrajectory {
        rewards: (0..100).map(|i| (i % 7) as f32).collect(),
    };
    let mut group = c.benchmark_group("gtpo/compute_advantages_100_turns");
    group.bench_function("discounted_normalized", |b| {
        b.iter(|| gtpo.compute_advantages(&traj))
    });
    group.finish();
}

fn bench_rlvr_reward(c: &mut Criterion) {
    let rlvr = RLVR::new(vec![
        VerifierKind::Syntax,
        VerifierKind::Logic,
        VerifierKind::Sandbox,
    ]);
    let cases: Vec<TestCase> = (0..100)
        .map(|i| TestCase {
            expected: format!("tok-{i}"),
        })
        .collect();
    let output = (0..100).map(|i| format!("tok-{i}\n")).collect::<String>();
    let mut group = c.benchmark_group("rlvr/compute_reward_100_cases");
    group.bench_function("three_stage", |b| {
        b.iter(|| rlvr.compute_reward(&output, &cases, 42))
    });
    group.finish();
}

criterion_group!(benches, bench_gtpo_advantages, bench_rlvr_reward);
criterion_main!(benches);
