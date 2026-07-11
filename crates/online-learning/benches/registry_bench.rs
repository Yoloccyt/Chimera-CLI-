//! ParameterRegistry 性能基准测试
//!
//! 对应架构层:L1 Core
//! 对应创新点:P2-1 全crate算法参数在线学习
//!
//! # 基准项
//! - `register`:单个参数注册延迟(含 registry 构造,反映 register 路径成本)
//! - `get_value`:参数查询延迟(预填充 registry,测量 DashMap 读路径)
//!
//! # 设计说明
//! - register bench 用 `iter_batched` 每次 iter 创建新 registry,避免同名 register 冲突;
//!   DashMap::new() 成本极低(仅分配),bench 主要反映 DashMap insert 成本。
//! - get_value bench 预填充一个参数,测量纯读路径延迟。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use online_learning::{LearnableParameter, ParameterRegistry, ParameterValue};

/// bench 1:单个参数 register 延迟
///
/// WHY 每次 iter 创建新 registry:register 同名参数会返回错误,
/// 必须用新 registry 隔离。DashMap::new() 成本极低,不影响 register 测量。
fn register_latency(c: &mut Criterion) {
    c.bench_function("register", |b| {
        b.iter_batched(
            ParameterRegistry::new,
            |registry| {
                let param = LearnableParameter::new(
                    "bench-param",
                    "name",
                    "bench-crate",
                    ParameterValue::scalar(0.5),
                );
                registry
                    .register(black_box(param))
                    .expect("register must succeed");
            },
            BatchSize::SmallInput,
        );
    });
}

/// bench 2:参数 get_value 查询延迟
///
/// 预填充 registry 后,测量纯读路径(DashMap shared read)延迟。
fn get_value_latency(c: &mut Criterion) {
    let registry = ParameterRegistry::new();
    registry
        .register(LearnableParameter::new(
            "bench-param",
            "name",
            "bench-crate",
            ParameterValue::scalar(0.5),
        ))
        .expect("register must succeed");

    c.bench_function("get_value", |b| {
        b.iter(|| {
            registry
                .get_value(black_box("bench-param"))
                .expect("get_value must succeed");
        });
    });
}

criterion_group!(benches, register_latency, get_value_latency);
criterion_main!(benches);
