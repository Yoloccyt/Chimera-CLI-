//! SESA Router 性能基准 — 256-bit 掩码激活延迟测量
//!
//! 对应 SubTask 3.7:性能基准测试
//!
//! # 验收标准
//! - 256-bit 掩码激活延迟 p95 ≤ 5ms
//! - mask 操作(set_bit/popcount/to_indices)延迟 < 100μs
//! - enforce_sparsity 256 专家规模 < 1ms
//!
//! # 运行
//! ```bash
//! # criterion 基准(同步操作)
//! cargo bench -p sesa-router
//!
//! # 忽略的性能测试(含 async 激活延迟)
//! cargo test -p sesa-router --bench router_benchmark -- --ignored
//! ```
//!
//! 注意:`#[ignore]` 性能测试标记为需显式运行,因为它们包含
//! async 运行时创建与多次采样,不适合常规 `cargo test`。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use sesa_router::{enforce_sparsity, SesaMask};
use std::time::Duration;
// WHY:以下 import 仅在下方 `#[test] #[ignore]` 性能测试函数中使用。
// bench 目标以 `harness = false` + `criterion_main!` 为入口,clippy 在 bench 模式下
// 不会编译 `#[test]` 函数,导致这些 import 被误报为 unused。
// `cargo test --benches` 运行时这些 #[test] 函数会被编译并使用它们,因此 import 必须保留。
#[allow(unused_imports)]
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaRouter};
#[allow(unused_imports)]
use std::time::Instant;

/// 构造带 N 个专家的路由器(无 EventBus,纯计算开销)
#[allow(dead_code)]
fn make_router(n: usize) -> SesaRouter {
    let router = SesaRouter::new(SesaConfig::default());
    for i in 0..n {
        let v = vec![(i as f32) * 0.01; 64];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        let _ = router.register_expert(expert);
    }
    router
}

/// 基准:测量 SesaMask 原子操作延迟
///
/// 验收标准:单次操作 < 100μs
fn bench_mask_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("mask_ops");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(3));

    // set_bit:256 位全置位
    group.bench_function("set_bit_256", |b| {
        b.iter(|| {
            let mut mask = SesaMask::new();
            for i in 0..256 {
                mask.set_bit(black_box(i));
            }
            mask
        });
    });

    // popcount:256 位全置位后计数
    group.bench_function("popcount_256", |b| {
        let mut mask = SesaMask::new();
        for i in 0..256 {
            mask.set_bit(i);
        }
        b.iter(|| black_box(mask.popcount()))
    });

    // to_indices:256 位全置位后转索引
    group.bench_function("to_indices_256", |b| {
        let mut mask = SesaMask::new();
        for i in 0..256 {
            mask.set_bit(i);
        }
        b.iter(|| black_box(mask.to_indices()))
    });

    group.finish();
}

/// 基准:测量 enforce_sparsity 在 256 专家规模下的延迟
///
/// 验收标准:< 1ms(256 专家,200 位激活裁剪到 102 位)
fn bench_enforce_sparsity(c: &mut Criterion) {
    let mut group = c.benchmark_group("enforce_sparsity");
    group.sample_size(200);
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("256_experts_200_active", |b| {
        let scores: Vec<f32> = (0..256).map(|i| (255 - i) as f32).collect();
        b.iter(|| {
            let mut mask = SesaMask::new();
            for i in 0..200 {
                mask.set_bit(i);
            }
            enforce_sparsity(black_box(&mut mask), black_box(&scores), 256, 0.4);
            mask
        });
    });

    group.finish();
}

/// 忽略测试:手动测量 256 专家激活延迟 p95
///
/// 运行:`cargo test -p sesa-router --bench router_benchmark -- --ignored`
#[test]
#[ignore = "perf: run with --ignored"]
fn test_activate_256_experts_latency_p95_under_5ms() {
    let router = make_router(256);

    // 50 次采样
    let mut latencies_us: Vec<u64> = Vec::with_capacity(50);
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio 运行时失败");
    for _ in 0..50 {
        let req = ActivationRequest::new("latency", vec![0.5; 64], 8, 100);
        let start = Instant::now();
        rt.block_on(router.activate(req)).expect("激活失败");
        latencies_us.push(start.elapsed().as_micros() as u64);
    }

    latencies_us.sort_unstable();
    let p95_idx = (latencies_us.len() as f32 * 0.95) as usize;
    let p95_us = latencies_us[p95_idx.min(latencies_us.len() - 1)];
    let p95_ms = p95_us as f64 / 1000.0;

    println!("256 专家激活延迟 p95: {p95_ms:.3}ms ({p95_us}μs)");

    assert!(
        p95_ms <= 5.0,
        "p95 延迟应 ≤ 5ms, got {p95_ms}ms ({p95_us}μs)"
    );
}

/// 忽略测试:测量 mask 操作延迟
#[test]
#[ignore = "perf: run with --ignored"]
fn test_mask_ops_latency_under_100us() {
    // set_bit 256 次
    let start = Instant::now();
    let mut mask = SesaMask::new();
    for i in 0..256 {
        mask.set_bit(i);
    }
    let set_bit_elapsed = start.elapsed();
    println!("set_bit × 256: {set_bit_elapsed:?}");

    // popcount
    let start = Instant::now();
    let count = mask.popcount();
    let popcount_elapsed = start.elapsed();
    println!("popcount (count={count}): {popcount_elapsed:?}");

    // to_indices
    let start = Instant::now();
    let indices = mask.to_indices();
    let to_indices_elapsed = start.elapsed();
    println!("to_indices (len={}): {to_indices_elapsed:?}", indices.len());

    assert!(
        set_bit_elapsed < Duration::from_micros(100),
        "set_bit × 256 延迟 {set_bit_elapsed:?} 超过 100μs"
    );
    assert!(
        popcount_elapsed < Duration::from_micros(100),
        "popcount 延迟 {popcount_elapsed:?} 超过 100μs"
    );
    assert!(
        to_indices_elapsed < Duration::from_micros(100),
        "to_indices 延迟 {to_indices_elapsed:?} 超过 100μs"
    );
}

/// 忽略测试:测量 enforce_sparsity 延迟
#[test]
#[ignore = "perf: run with --ignored"]
fn test_enforce_sparsity_latency_under_1ms() {
    let scores: Vec<f32> = (0..256).map(|i| (255 - i) as f32).collect();

    // 预热
    let mut mask = SesaMask::new();
    for i in 0..200 {
        mask.set_bit(i);
    }
    enforce_sparsity(&mut mask, &scores, 256, 0.4);

    // 测量
    let start = Instant::now();
    let mut mask = SesaMask::new();
    for i in 0..200 {
        mask.set_bit(i);
    }
    enforce_sparsity(&mut mask, &scores, 256, 0.4);
    let elapsed = start.elapsed();

    println!(
        "enforce_sparsity (256 专家, 200→102): {elapsed:?} (active={})",
        mask.active_count
    );

    assert!(
        elapsed < Duration::from_millis(1),
        "enforce_sparsity 延迟 {elapsed:?} 超过 1ms"
    );
}

criterion_group!(benches, bench_mask_ops, bench_enforce_sparsity);
criterion_main!(benches);
