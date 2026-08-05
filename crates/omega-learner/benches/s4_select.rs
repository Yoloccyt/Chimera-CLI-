//! S4 探针配比臂选择性能基准（PROBE P2.3）
//!
//! # 基准场景
//!
//! - **29 臂 / 8 维上下文**: S4 接缝扩展后配置（5 原权重臂 + 24 探针配比臂）
//! - 红线: `select_arm` p99 < 50μs（对齐 `P99_TARGET_US`，40 臂 s9 先例）
//! - `update_last` 全链路: 探针臂非唯一权重经 last_arm_idx 回溯（O(1)，无查找开销）
//!
//! # WHY 独立 bench
//!
//! 既有 `linucb_select.rs` 40 臂红线覆盖 s9 通路；本文件提供 S4 29 臂
//! 精确证据（臂数更小延迟更低，但需可证伪数据——性能可证伪原则）。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use omega_learner::arm::{ArmId, ArmIndex, DiscreteArmSet};
use omega_learner::context::SeamContext;
use omega_learner::linucb::LinUCB;
use omega_learner::s4_selector::{probe_arm_params, BlockType, S4Context, S4Reward};

/// 29 臂 S4 选择延迟红线(μs)——对齐 s9 40 臂先例
pub const P99_TARGET_US: u64 = 50;

/// 构造 S4 29 臂 LinUCB（s4_arm_set 同构：5 原权重臂 + 24 探针配比臂）
fn make_s4_linucb() -> LinUCB {
    let mut arm_ids: Vec<ArmId> = Vec::with_capacity(29);
    // 原 5 权重臂（与 s4_arm_set 一致的 ID 格式）
    for i in 0..5 {
        let (r, f, rel) = omega_learner::s4_selector::arm_index_to_weights(i).as_tuple();
        arm_ids.push(ArmId::new(format!("w=({r},{f},{rel})")));
    }
    // 24 探针配比臂（probe- 命名空间）
    for idx in 5..29 {
        let params = probe_arm_params(idx).expect("探针臂参数");
        arm_ids.push(ArmId::new(format!(
            "probe-a{}-g{}-k{}",
            params.alpha, params.grain, params.k
        )));
    }
    let arm_set = DiscreteArmSet::new(arm_ids);
    LinUCB::new(8, &arm_set, 1.0).unwrap()
}

/// 构造样本 S4 上下文（8 维：块类型 one-hot 4 + 3 数值 + bias）
fn make_s4_context() -> SeamContext {
    let ctx = S4Context::new(BlockType::Code, 0.8, 0.5, 0.1).unwrap();
    ctx.to_seam_context().unwrap()
}

fn bench_s4_select_29arm(c: &mut Criterion) {
    let linucb = make_s4_linucb();
    let ctx = make_s4_context();

    c.bench_function("select_arm_29arms_8dim_s4", |b| {
        b.iter(|| black_box(linucb.select_arm(black_box(&ctx)).unwrap()))
    });
}

fn bench_s4_probe_cycle(c: &mut Criterion) {
    // 探针全链路: select_probe → update_last（含 LinUCB update O(d²)）
    let mut linucb = make_s4_linucb();
    let ctx = make_s4_context();
    let reward = S4Reward::new(0.2).unwrap();
    let arm = ArmIndex::new(5); // 探针臂代表

    c.bench_function("probe_select_then_update_last", |b| {
        b.iter(|| {
            black_box(linucb.select_arm(black_box(&ctx)).unwrap());
            linucb
                .update(black_box(arm), black_box(&ctx), reward.reward())
                .unwrap();
        })
    });
}

criterion_group!(benches, bench_s4_select_29arm, bench_s4_probe_cycle,);
criterion_main!(benches);
