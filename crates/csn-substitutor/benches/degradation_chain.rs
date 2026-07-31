//! CSN 降级链推进性能基准 — 测量 advance_degradation 与 degradation_level 延迟
//!
//! 对应 SubTask 0.5.11:验证降级链推进延迟 < 1µs
//!
//! # 验收标准
//! - `advance_degradation` 单次推进延迟 < 1µs(目标)
//! - `degradation_level` 查询延迟 < 1µs(目标)
//!
//! # 运行
//! ```bash
//! cargo bench -p csn-substitutor --bench degradation_chain
//! ```
//!
//! # 设计说明
//! - `advance_degradation` 是同步方法,直接测量 DashMap get_mut + next_level
//! - 基准不含 EventBus 发布开销(那是异步路径,由集成测试覆盖)
//! - 使用 `black_box` 防止编译器优化掉关键操作

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use csn_substitutor::{CapabilityDescriptor, CsnConfig, CsnSubstitutor};

/// 构建带 N 个能力 + 1 条预创建降级链的替代器
///
/// 注册 N 个能力后,触发一次 `trigger_substitution` 创建降级链,
/// 后续 `advance_degradation` 测量基于此链。
fn make_substitutor_with_chain(n: usize) -> CsnSubstitutor {
    let config = CsnConfig::default();
    let sub = CsnSubstitutor::new(config);
    for i in 0..n {
        let id = format!("cap-{i}");
        // 生成 50 维向量,每个能力有微小差异
        let vector: Vec<f32> = (0..50)
            .map(|j| (i as f32 + j as f32 * 0.01) * 0.1)
            .collect();
        let cap = CapabilityDescriptor::new(id, vector);
        let _ = sub.register_capability(cap);
    }
    // 预创建降级链(使用 block_on 避免 tokio runtime 嵌套)
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    rt.block_on(sub.trigger_substitution("cap-0"))
        .expect("预创建降级链失败");
    sub
}

/// 基准:测量 `advance_degradation` 单次推进延迟
///
/// 验收标准:< 1µs
///
/// # 测量逻辑
/// 由于 `advance_degradation` 推进到末端会返回 ChainExhausted 并移除链,
/// 本基准在每次迭代前重置链到 level 0,确保推进路径一致。
/// 重置开销不计入测量(在 `iter` 闭包外完成)。
fn bench_advance_degradation(c: &mut Criterion) {
    let sizes: &[usize] = &[10, 50, 100];

    let mut group = c.benchmark_group("csn_advance_degradation");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in sizes {
        let sub = make_substitutor_with_chain(n);

        group.bench_with_input(BenchmarkId::new("n_caps", n), &n, |b, &_| {
            b.iter_batched(
                // 每次迭代前重置链到 level 0(不计入测量)
                || {
                    let _ = sub.reset_chain("cap-0");
                },
                // 测量目标:advance_degradation 单次推进
                // WHY 参数 _:iter_batched 的 routine 接收 setup 返回值(此处为 ())
                |_| {
                    let result = sub.advance_degradation(black_box("cap-0"));
                    // 防止编译器优化掉 result(WHY let _:Result 可能是 Err,bench 场景忽略)
                    let _ = black_box(result);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

/// 基准:测量 `degradation_level` 查询延迟
///
/// 验收标准:< 1µs(DashMap get + mapref,预期 < 100ns)
fn bench_degradation_level(c: &mut Criterion) {
    let sizes: &[usize] = &[10, 50, 100];

    let mut group = c.benchmark_group("csn_degradation_level");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    for &n in sizes {
        let sub = make_substitutor_with_chain(n);

        group.bench_with_input(BenchmarkId::new("n_caps", n), &n, |b, &_| {
            b.iter(|| {
                let level = sub.degradation_level(black_box("cap-0"));
                black_box(level);
            });
        });
    }

    group.finish();
}

/// 基准:测量 `cleanup_chains` 延迟(空操作场景,验证遍历开销)
///
/// 验收标准:TTL 清理遍历 < 10µs(100 chain 场景)
fn bench_cleanup_chains(c: &mut Criterion) {
    let mut group = c.benchmark_group("csn_cleanup_chains");
    group.sample_size(100);
    group.measurement_time(std::time::Duration::from_secs(5));

    // 单 chain 场景:基础开销
    let sub_single = make_substitutor_with_chain(10);
    group.bench_function("single_chain", |b| {
        b.iter(|| {
            // TTL=1小时,所有 chain 都未过期(空清理)
            let removed =
                sub_single.cleanup_chains(black_box(std::time::Duration::from_secs(3600)));
            black_box(removed);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_advance_degradation,
    bench_degradation_level,
    bench_cleanup_chains
);
criterion_main!(benches);
