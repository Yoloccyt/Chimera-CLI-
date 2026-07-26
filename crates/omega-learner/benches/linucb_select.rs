//! LinUCB select_arm 性能基准
//!
//! 对应任务: **P4-W13.1**（验证 LinUCB 选择延迟 < 1ms)
//! 对应 ADR: **ADR-031**（omega-learner 不在推理关键路径同步调用的性能边界）
//!
//! # 基准场景
//!
//! - **4 臂 / 3 维上下文**: S1 接缝典型配置
//! - **10 臂 / 8 维上下文**: 中等规模(覆盖 S2/S3 接缝)
//! - **20 臂 / 16 维上下文**: 大规模(压力测试)
//! - **update 性能**: Sherman-Morrison 增量更新延迟

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use omega_learner::arm::{ArmId, ArmIndex, DiscreteArmSet};
use omega_learner::context::SeamContext;
use omega_learner::linucb::LinUCB;

/// 构造样本 LinUCB 实例
fn make_linucb(num_arms: usize, dim: usize) -> LinUCB {
    let arms: Vec<ArmId> = (0..num_arms)
        .map(|i| ArmId::new(format!("arm-{}", i)))
        .collect();
    let arm_set = DiscreteArmSet::new(arms);
    LinUCB::new(dim, &arm_set, 1.0).unwrap()
}

/// 构造样本上下文(归一化)
fn make_context(dim: usize) -> SeamContext {
    let features: Vec<f32> = (0..dim).map(|i| 1.0 / (i as f32 + 2.0)).collect();
    SeamContext::new(features).unwrap()
}

fn bench_select_arm_small(c: &mut Criterion) {
    let linucb = make_linucb(4, 3);
    let ctx = make_context(3);

    c.bench_function("select_arm_4arms_3dim", |b| {
        b.iter(|| black_box(linucb.select_arm(black_box(&ctx)).unwrap()))
    });
}

fn bench_select_arm_medium(c: &mut Criterion) {
    let linucb = make_linucb(10, 8);
    let ctx = make_context(8);

    c.bench_function("select_arm_10arms_8dim", |b| {
        b.iter(|| black_box(linucb.select_arm(black_box(&ctx)).unwrap()))
    });
}

fn bench_select_arm_large(c: &mut Criterion) {
    let linucb = make_linucb(20, 16);
    let ctx = make_context(16);

    c.bench_function("select_arm_20arms_16dim", |b| {
        b.iter(|| black_box(linucb.select_arm(black_box(&ctx)).unwrap()))
    });
}

fn bench_update(c: &mut Criterion) {
    let mut linucb = make_linucb(4, 3);
    let ctx = make_context(3);
    let arm = ArmIndex::new(0);

    c.bench_function("update_4arms_3dim", |b| {
        // update 返回 Result<()>, unwrap 为 unit 类型
        // 不用 black_box 包裹返回值避免 clippy::unit_arg 警告
        b.iter(|| linucb.update(black_box(arm), black_box(&ctx), 0.5).unwrap())
    });
}

fn bench_full_cycle(c: &mut Criterion) {
    let mut linucb = make_linucb(4, 3);
    let ctx = make_context(3);

    c.bench_function("full_cycle_select_then_update", |b| {
        b.iter(|| {
            let arm = linucb.select_arm(black_box(&ctx)).unwrap();
            linucb.update(black_box(arm), black_box(&ctx), 0.5).unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_select_arm_small,
    bench_select_arm_medium,
    bench_select_arm_large,
    bench_update,
    bench_full_cycle,
);
criterion_main!(benches);
