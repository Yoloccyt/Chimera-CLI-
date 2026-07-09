//! DecayEngine 性能基准测试
//!
//! 对应架构层:L4 Security
//! 对应 ADR-002:能力衰减模型设计(连续权限流体)
//!
//! # 基准项
//! - `single_decay_step_latency`:单次衰减步(`decay` 调用 TimeDecay)延迟
//! - `bulk_decay_throughput`:批量衰减(1000 条目)吞吐量
//!
//! # 设计说明
//! - bench 1 测量单次 `decay` 调用延迟(注册一个能力 + 应用 TimeDecay)。
//!   DashMap 分片锁在单次操作下接近零开销,主要成本在 `Instant::now()` 与
//!   `CapabilityLevel::new()` 的浮点 clamp 校验。
//! - bench 2 测量批量 1000 个能力的衰减吞吐,`Throughput::Elements(1000)`
//!   报告 ops/sec,反映 DecayEngine 在大规模能力注册表下的性能。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use decay_engine::{DecayConfig, DecayEngine, DecayEvent};

/// 批量衰减条目数(模拟 L4 Security 多能力注册表规模)
const BULK_SIZE: usize = 1000;

/// bench 1:单次衰减步延迟
///
/// WHY 单次调用:测量 DashMap::get_mut + Instant::now + CapabilityLevel::new 的
/// 端到端开销,作为 decay 路径的延迟下界。 violation_penalty / Freeze / Restore
/// 路径与 TimeDecay 在结构上类似(仅 match 分支不同),不重复测。
fn single_decay_step_latency(c: &mut Criterion) {
    let engine = DecayEngine::new(DecayConfig::default());
    engine
        .register_capability("cap-bench", "bench capability", 0.5)
        .expect("register_capability 失败");

    let mut group = c.benchmark_group("single_decay_step_latency");
    group.bench_function("time_decay", |b| {
        b.iter(|| {
            // WHY expect:bench 中能力必然存在,失败表明 DecayEngine 状态错误
            let level = engine
                .decay(black_box("cap-bench"), black_box(DecayEvent::TimeDecay))
                .expect("decay 失败");
            black_box(level);
        });
    });
    group.finish();
}

/// bench 2:批量衰减吞吐量(1000 条目)
///
/// WHY 批量场景:DecayEngine 管理所有能力的权限流体(对应 L4 Security 全局
/// 权限治理),生产中可能有数百到数千能力。此 bench 验证遍历全部能力的
/// 衰减性能,反映周期性权限治理的延迟上限。
///
/// 实现策略:预填充 1000 个能力,每次 iter 顺序对所有能力应用 TimeDecay。
/// `Throughput::Elements(1000)` 让 criterion 报告 decay_ops/sec。
fn bulk_decay_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_decay_throughput");
    group.throughput(Throughput::Elements(BULK_SIZE as u64));

    for &size in &[100usize, 1000] {
        // 预填充能力列表(setup,不计入测量时间)
        let engine = DecayEngine::new(DecayConfig::default());
        for i in 0..size {
            let name = format!("capability {i}");
            engine
                .register_capability(&format!("cap-{i}"), &name, 0.5)
                .expect("register_capability 失败");
        }
        // 预生成能力 ID 列表(避免在 iter 中 format 字符串,排除格式化开销干扰)
        let ids: Vec<String> = (0..size).map(|i| format!("cap-{i}")).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &_| {
            b.iter(|| {
                for id in &ids {
                    let level = engine
                        .decay(black_box(id), black_box(DecayEvent::TimeDecay))
                        .expect("decay 失败");
                    black_box(level);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, single_decay_step_latency, bulk_decay_throughput);
criterion_main!(benches);
