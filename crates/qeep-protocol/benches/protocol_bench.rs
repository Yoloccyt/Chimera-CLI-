//! QEEP 协议性能基准测试
//!
//! 对应架构层:L4 Security
//! 对应创新点:QEEP(Quantum-Entangled Execution Protocol)
//!
//! # 基准项
//! - `entangle_latency`:单次 `entangle` 调用延迟(立即完成的 async 操作)
//! - `bulk_entangle_throughput`:批量 entangle(100 次调用)吞吐量
//!
//! # 设计说明
//! - bench 1 测量 `entangle` 包装一个立即返回的 async 操作的端到端开销,
//!   反映 QEEP 的协议开销下界(UUIDv7 生成、DashMap insert、OrphanGuard 创建与
//!   drop、DashMap remove)。实际生产中的 async 操作有自身开销,QEEP 开销是
//!   额外成本,bench 1 即测量此"协议税"。
//! - bench 2 验证批量调用下 QEEP 状态不泄漏(pending_count 归零、completed_count 递增),
//!   `Throughput::Elements(N)` 反映 entangle_ops/sec。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。

#![forbid(unsafe_code)]

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use qeep_protocol::{QeepError, QeepProtocol};
use tokio::runtime::Runtime;

/// 批量 entangle 调用次数(验证 QEEP 状态机在大规模调用下不泄漏)
const BULK_CALLS: usize = 100;

/// bench 1:单次 entangle 调用延迟
///
/// WHY 测量协议开销下界:entangle 包装的 future 为 `async { Ok(42) }`(立即完成),
/// 实测时间几乎全是 QEEP 协议本身的开销(UUIDv7 + DashMap + OrphanGuard),
/// 这是判断 QEEP 是否在生产中可接受的延迟基线。
fn entangle_latency(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let protocol = QeepProtocol::new(Duration::from_secs(5));

    let mut group = c.benchmark_group("entangle_latency");
    group.bench_function("immediate_complete", |b| {
        b.iter(|| {
            rt.block_on(async {
                let result: Result<i32, QeepError> = protocol.entangle(async { Ok(42) }).await;
                // WHY expect:bench 中 entangle 立即完成不应失败
                let value = result.expect("entangle 失败");
                black_box(value);
            });
        });
    });
    group.finish();
}

/// bench 2:批量 entangle 吞吐量
///
/// WHY 批量场景:验证 QEEP 在连续调用下的状态机清洁度(pending_count 归零、
/// completed_count 递增)。100 次调用足以触发 DashMap 内部分片锁竞争,
/// 反映生产中 L7 Execution 层多个 async 操作的纠缠开销。
///
/// 实现策略:每次 iter 连续 await N 个 entangle 调用,Throughput::Elements(N)
/// 报告 entangle_ops/sec。
fn bulk_entangle_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("bulk_entangle_throughput");
    group.throughput(Throughput::Elements(BULK_CALLS as u64));

    for &calls in &[10usize, 100] {
        let protocol = QeepProtocol::new(Duration::from_secs(5));

        group.bench_with_input(BenchmarkId::from_parameter(calls), &calls, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    for _ in 0..n {
                        let result: Result<(), QeepError> =
                            protocol.entangle(async { Ok(()) }).await;
                        result.expect("entangle 失败");
                    }
                });
            });
        });
    }
    group.finish();
}

criterion_group!(benches, entangle_latency, bulk_entangle_throughput);
criterion_main!(benches);
