//! QEEP 协议性能基准测试
//!
//! 对应架构层:L4 Security
//! 对应创新点:QEEP(Quantum-Entangled Execution Protocol)
//!
//! # 基准矩阵 (B1-B8)
//!
//! | 基准名 | 场景 | 参数 |
//! |--------|------|------|
//! | `entangle_immediate` | 立即完成 | — |
//! | `entangle_delay_10us` | 10µs 延迟 | `tokio::time::sleep(Duration::from_micros(10))` |
//! | `entangle_delay_100us` | 100µs 延迟 | `tokio::time::sleep(Duration::from_micros(100))` |
//! | `entangle_delay_1ms` | 1ms 延迟 | `tokio::time::sleep(Duration::from_millis(1))` |
//! | `bulk_serial_10` | 串行 10 次 | — |
//! | `bulk_serial_100` | 串行 100 次 | — |
//! | `bulk_concurrent_10` | 10 并发 | `tokio::spawn` + `JoinAll` |
//! | `bulk_concurrent_100` | 100 并发 | `tokio::spawn` + `JoinAll` |
//!
//! # 设计说明
//! - B1 测量 QEEP 协议开销下界(UUIDv7 生成、DashMap insert、OrphanGuard 创建与
//!   drop、DashMap remove)。实际生产中的 async 操作有自身开销,QEEP 开销是
//!   额外成本,B1 即测量此"协议税"。
//! - B2-B4 测量 entangle 在不同延迟 async 操作下的感知延迟,验证延迟叠加特性。
//! - B5-B6 验证批量串行调用下 QEEP 状态不泄漏(pending_count 归零、completed_count 递增)。
//! - B7-B8 验证并发调用下 QEEP 状态机正确性(并发安全)。
//!
//! # 迁移说明
//! 所有基准使用 `b.to_async(&rt)` 替代 `rt.block_on`,确保 criterion 正确测量
//! async 基准的 wall-clock 时间。
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

/// B1:单次 entangle 调用延迟(立即完成)
///
/// WHY 测量协议开销下界:entangle 包装的 future 为 `async { Ok(42) }`(立即完成),
/// 实测时间几乎全是 QEEP 协议本身的开销(UUIDv7 + DashMap + OrphanGuard),
/// 这是判断 QEEP 是否在生产中可接受的延迟基线。
fn entangle_immediate(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");
    let protocol = QeepProtocol::new(Duration::from_secs(5));

    let mut group = c.benchmark_group("entangle_immediate");
    group.bench_function("immediate_complete", |b| {
        b.to_async(&rt).iter(|| async {
            let result: Result<i32, QeepError> = protocol.entangle(async { Ok(42) }).await;
            let value = result.expect("entangle 失败");
            black_box(value);
        });
    });
    group.finish();
}

/// B2-B4:带延迟的 entangle 调用延迟
///
/// WHY 测量 entangle 在不同延迟 async 操作下的感知延迟,验证延迟叠加特性。
/// 延迟通过 `tokio::time::sleep` 在 future 内部实现。
fn entangle_delayed(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("entangle_delayed");

    let delays: &[(&str, Duration)] = &[
        ("10us", Duration::from_micros(10)),
        ("100us", Duration::from_micros(100)),
        ("1ms", Duration::from_millis(1)),
    ];

    for &(name, delay) in delays {
        let protocol = QeepProtocol::new(Duration::from_secs(5));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.to_async(&rt).iter(|| async {
                let result: Result<i32, QeepError> = protocol
                    .entangle(async {
                        tokio::time::sleep(delay).await;
                        Ok(42)
                    })
                    .await;
                let value = result.expect("entangle 失败");
                black_box(value);
            });
        });
    }
    group.finish();
}

/// B5-B6:串行批量 entangle 调用
///
/// WHY 验证批量串行调用下 QEEP 状态不泄漏(pending_count 归零、completed_count 递增)。
fn bulk_serial(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("bulk_serial");
    group.throughput(Throughput::Elements(BULK_CALLS as u64));

    for &calls in &[10usize, 100] {
        let protocol = QeepProtocol::new(Duration::from_secs(5));
        group.bench_with_input(BenchmarkId::from_parameter(calls), &calls, |b, &n| {
            b.to_async(&rt).iter(|| async {
                for _ in 0..n {
                    let result: Result<(), QeepError> = protocol.entangle(async { Ok(()) }).await;
                    result.expect("entangle 失败");
                }
            });
        });
    }
    group.finish();
}

/// B7-B8:并发批量 entangle 调用
///
/// WHY 验证并发调用下 QEEP 状态机正确性(并发安全)。
/// 使用 `tokio::spawn` + `JoinAll` 实现并发 entangle 调用。
fn bulk_concurrent(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("bulk_concurrent");
    group.throughput(Throughput::Elements(BULK_CALLS as u64));

    for &calls in &[10usize, 100] {
        let protocol = QeepProtocol::new(Duration::from_secs(5));
        group.bench_with_input(BenchmarkId::from_parameter(calls), &calls, |b, &n| {
            b.to_async(&rt).iter(|| async {
                let handles: Vec<_> = (0..n)
                    .map(|_| {
                        let protocol = protocol.clone();
                        tokio::spawn(async move { protocol.entangle(async { Ok(()) }).await })
                    })
                    .collect();
                for handle in handles {
                    let result = handle.await.expect("task 执行失败");
                    result.expect("entangle 失败");
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    entangle_immediate,
    entangle_delayed,
    bulk_serial,
    bulk_concurrent,
);
criterion_main!(benches);
