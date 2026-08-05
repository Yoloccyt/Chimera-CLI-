//! S9 63 臂场景下查询性能基准
//!
//! 对应架构层: L6 Router(omega-learner)
//! 对应 ADR: ADR-065(MCA M3), ADR-068
//! 对应设计源: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4 路由亲和
//!
//! # 性能红线
//!
//! - `s9_select_63_arms`: 63 臂 select_route p99 < 100μs
//! - `s9_observe_63_arms`: 63 臂 observe p99 < 100μs
//!
//! # 基准场景
//!
//! - **63 臂 / 6 维上下文**: 7 厂商 × 3 模型 × 3 思考档,覆盖 S9 路由臂
//!   完整空间(超过 ~40 臂典型场景,压力测试)。
//! - **select + observe 全周期**: 模拟生产环境的一轮选择+观察操作。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use omega_learner::s9_route::{S9Context, S9Reward, S9RouteLearner};

/// 构造 63 臂路由集(7 厂商 × 3 模型 × 3 思考档)
fn build_63_arms() -> Vec<String> {
    let providers = [
        "zhipu",
        "deep_seek",
        "moonshot",
        "mini_max",
        "volcano_ark",
        "alibaba_cloud",
        "step_fun",
    ];
    let models = ["glm-5.2", "deepseek-v4-flash", "kimi-k3"];
    let modes = ["fast", "standard", "deep"];
    let mut arms = Vec::with_capacity(63);
    for p in &providers {
        for m in &models {
            for mode in &modes {
                arms.push(format!("{p}/{m}/{mode}"));
            }
        }
    }
    arms
}

/// 构造默认 S9 上下文(中等值)
fn default_context() -> S9Context {
    S9Context {
        task_complexity: 0.5,
        budget_water_level: 0.5,
        latency_sensitivity: 0.5,
        cache_hit_history: 0.5,
        risk_level: 0.2,
    }
}

/// 构造默认 S9 奖励(成功高质量,成本/延迟适中)
fn default_reward() -> S9Reward {
    S9Reward {
        success: true,
        quality: 0.8,
        normalized_cost: 0.3,
        normalized_latency: 0.2,
    }
}

fn bench_63_arm_select(c: &mut Criterion) {
    let arms = build_63_arms();
    let learner = S9RouteLearner::new(&arms, 1.0).unwrap();
    let ctx = default_context();

    c.bench_function("s9_select_63_arms", |b| {
        b.iter(|| black_box(learner.select_route(black_box(ctx))).unwrap())
    });
}

fn bench_63_arm_observe(c: &mut Criterion) {
    let arms = build_63_arms();
    let mut learner = S9RouteLearner::new(&arms, 1.0).unwrap();
    let ctx = default_context();
    let reward = default_reward();

    // 先选一个臂用于观察
    let arm = learner.select_route(ctx).unwrap();

    c.bench_function("s9_observe_63_arms", |b| {
        b.iter(|| {
            learner
                .observe(black_box(&arm), black_box(ctx), black_box(reward))
                .unwrap();
            black_box(())
        })
    });
}

fn bench_63_arm_full_cycle(c: &mut Criterion) {
    let arms = build_63_arms();
    let mut learner = S9RouteLearner::new(&arms, 1.0).unwrap();
    let ctx = default_context();
    let reward = default_reward();

    c.bench_function("s9_full_cycle_63_arms", |b| {
        b.iter(|| {
            let arm = learner.select_route(black_box(ctx)).unwrap();
            learner
                .observe(black_box(&arm), black_box(ctx), black_box(reward))
                .unwrap();
        })
    });
}

criterion_group!(
    benches,
    bench_63_arm_select,
    bench_63_arm_observe,
    bench_63_arm_full_cycle,
);
criterion_main!(benches);
