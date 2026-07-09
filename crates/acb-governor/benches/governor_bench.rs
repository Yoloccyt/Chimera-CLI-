//! ACB 治理器性能基准测试
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! # 基准项
//! - `allocation_latency`:BudgetAllocation 分配延迟(`allocation_for` + `token_limit_for`)
//! - `concurrent_consumption_throughput`:多线程并发 `record_consumption` 吞吐量
//!
//! # 设计说明
//! - bench 1 测量纯函数路径(`allocation_for` 仅 match + 结构体构造),作为 check_budget
//!   路径的延迟下界。check_budget 内部包含 publish_blocking 事件发布,延迟更高,
//!   `allocation_for` 是其成本下界,便于回归测试判断是否有性能退化。
//! - bench 2 测量 record_consumption 在并发场景的吞吐。record_consumption 内部使用
//!   AtomicU64 无锁累加,理论上无争抢;但 adjust_budget 持有 Mutex 锁,可能成为瓶颈。
//!   `Throughput::Elements(N)` 反映 ops/sec。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::thread;

use acb_governor::{AcbGovernor, AcbGovernorConfig, BudgetTier};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// 每 iter 每线程 record_consumption 调用次数
const OPS_PER_THREAD: usize = 1000;

/// bench 1:BudgetAllocation 分配延迟
///
/// WHY 选 `allocation_for`:这是 check_budget 路径的核心子操作,
/// 仅做 match + 结构体构造,作为 check_budget 延迟下界。
/// 通过 1/10/100 三种规模验证延迟稳定性(应基本恒定,因为纯函数无状态依赖)。
fn allocation_latency(c: &mut Criterion) {
    let config = AcbGovernorConfig::default();

    let mut group = c.benchmark_group("allocation_latency");
    for &tier_level in &[0u8, 1, 2, 3] {
        let tier = BudgetTier::from_level(tier_level).expect("level in [0,3]");
        group.bench_with_input(
            BenchmarkId::from_parameter(tier.as_str()),
            &tier,
            |b, &tier| {
                b.iter(|| {
                    let alloc = config.allocation_for(black_box(tier));
                    black_box(alloc);
                });
            },
        );
    }
    group.finish();
}

/// bench 2:多线程并发 record_consumption 吞吐量
///
/// WHY 并发场景:ACB 治理器在生产中被 L9 Quest Engine 多个 Quest 并行调用,
/// record_consumption 是高频写路径(每次 token 消耗都调用),必须验证并发性能。
/// - `total_consumption` 用 AtomicU64 无锁累加,理论无争抢
/// - `adjust_budget` 用 Mutex,可能成为瓶颈(每次切换需要锁)
///
/// 实现策略:每个线程数对应独立 benchmark_group,Throughput 准确反映该输入的 ops/sec。
/// N 个线程各调用 record_consumption M 次(每次消耗 1 token,避免触发降级),
/// 总操作数 = N × M,ops/sec = (N × M) / iter_time。
fn concurrent_consumption_throughput(c: &mut Criterion) {
    for &threads in &[1usize, 2, 4] {
        let total_ops = (threads * OPS_PER_THREAD) as u64;
        let mut group = c.benchmark_group(format!(
            "concurrent_consumption_throughput/threads={threads}"
        ));
        // WHY 每个线程数独立设置 throughput:不同输入下总操作数不同,
        // 共享 group 会让 throughput 错位。独立 group 让 ops/sec 准确反映该线程数下的吞吐
        group.throughput(Throughput::Elements(total_ops));

        group.bench_function("record_consumption", |b| {
            b.iter(|| {
                // 每次 iter 创建新 governor,重置状态
                let governor =
                    Arc::new(AcbGovernor::new(AcbGovernorConfig::default()).expect("config valid"));

                let handles: Vec<_> = (0..threads)
                    .map(|_| {
                        let gov = Arc::clone(&governor);
                        thread::spawn(move || {
                            // WHY 每次 1 token:避免触发降级(默认 total_budget=1M,
                            // 1000 ops × 1 token = 1000 token 远低于阈值)
                            for _ in 0..OPS_PER_THREAD {
                                gov.record_consumption(black_box(1))
                                    .expect("record_consumption 失败");
                            }
                        })
                    })
                    .collect();

                for h in handles {
                    h.join().expect("worker thread panic");
                }
            });
        });
        group.finish();
    }
}

criterion_group!(
    benches,
    allocation_latency,
    concurrent_consumption_throughput
);
criterion_main!(benches);
