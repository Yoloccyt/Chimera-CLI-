//! AutoDPO 偏好对生成性能基准测试
//!
//! 对应架构层:L5 Knowledge
//!
//! # 基准项
//! - `preference_update_latency`:单次偏好对生成(`generate`)延迟
//! - `bulk_preference_throughput`:批量偏好对生成吞吐量
//!
//! # 设计说明
//! - bench 1 测量 `PreferencePairGenerator::generate` 单次调用延迟,反映 DPO
//!   样本构造的端到端开销(选最高/最低分 + 质量门控 + 事件发布)。
//!   候选数为 2(最小合法值),作为延迟下界。
//! - bench 2 验证批量调用下 generate 的吞吐,`Throughput::Elements(N)` 反映
//!   pairs/sec。每次 iter 重新构造相同候选集,排除历史 pair_id 干扰。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。

#![forbid(unsafe_code)]

use auto_dpo::{AutoDpoConfig, ModelOutput, PreferencePairGenerator};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// 批量偏好对生成次数
const BULK_PAIRS: usize = 100;

/// 构造测试用候选集(2 个候选,chosen + rejected)
///
/// WHY 固定候选:DPO 最小合法输入,避免排序开销干扰测量,反映 generate 纯协议开销
fn make_outputs() -> Vec<ModelOutput> {
    vec![
        ModelOutput::new("good-output", 0.9),
        ModelOutput::new("bad-output", 0.3),
    ]
}

/// bench 1:单次偏好对生成延迟
///
/// WHY 单次调用:测量 generate 路径的端到端开销。候选数固定为 2,
/// 排除 O(n) 遍历的开销干扰,聚焦在"选 chosen/rejected + 质量门控 +
/// EventBus publish"的核心路径。
fn preference_update_latency(c: &mut Criterion) {
    // WHY 关闭事件发布:bench 关注 generate 协议开销,排除 EventBus 路径干扰
    let config = AutoDpoConfig {
        enable_event_publish: false,
        ..Default::default()
    };
    let generator = PreferencePairGenerator::new(config).expect("config valid");
    let outputs = make_outputs();

    let mut group = c.benchmark_group("preference_update_latency");
    group.bench_function("generate_pair", |b| {
        b.iter(|| {
            let pair = generator
                .generate(black_box(&outputs))
                .expect("generate 失败");
            black_box(pair);
        });
    });
    group.finish();
}

/// bench 2:批量偏好对生成吞吐量
///
/// WHY 批量场景:DPO 训练通常需要数千偏好对,Generator 单次调用约 1μs,
/// 100 次调用的批量吞吐反映生产中 GSOE 进化闭环的样本构造效率。
///
/// 实现策略:每次 iter 连续调用 generate N 次,Throughput::Elements(N)
/// 报告 pairs/sec。
fn bulk_preference_throughput(c: &mut Criterion) {
    let config = AutoDpoConfig {
        enable_event_publish: false,
        ..Default::default()
    };
    let generator = PreferencePairGenerator::new(config).expect("config valid");
    let outputs = make_outputs();

    let mut group = c.benchmark_group("bulk_preference_throughput");
    group.throughput(Throughput::Elements(BULK_PAIRS as u64));

    for &n in &[10usize, 100] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &_| {
            b.iter(|| {
                for _ in 0..n {
                    let pair = generator
                        .generate(black_box(&outputs))
                        .expect("generate 失败");
                    black_box(pair);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    preference_update_latency,
    bulk_preference_throughput
);
criterion_main!(benches);
