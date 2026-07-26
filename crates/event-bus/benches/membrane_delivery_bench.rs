//! 膜投递性能基准测试 — P2-W7.1.2
//!
//! 对应架构层:L1 Core(膜深化,spec.md L423)
//! 验证目标:跨膜事件投递 p95<10ms(spec.md L262)
//!
//! # 基准项
//! - `membrane_pass_to_core`:PassToCore 事件穿膜发布延迟(膜决策 + publish)
//! - `membrane_local_consume`:LocalConsume 事件膜边界消化延迟(膜决策 + 跳过)
//! - `membrane_e2e_delivery`:端到端投递(publish_membrane → recv)
//!
//! # 设计说明
//! - bench 1/2 用 `publish_membrane_blocking`:同步路径,排除 runtime 噪声,
//!   反映膜决策 + channel send 的纯开销
//! - bench 3 用 tokio runtime:block_on 测端到端(含 recv),验证 p95<10ms
//! - 对比组:`publish_blocking` 无膜基准,量化膜决策开销增量
//!
//! # p95<10ms 验收标准(spec.md L262)
//! criterion 报告的 p95 分位 < 10ms。膜决策是 O(1) match(categorize) +
//! O(1) 条件判定(decide),不应显著增加 publish 延迟。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use event_bus::membrane::{InnerLoad, MembraneFilter};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::runtime::Runtime;

/// 构造 PassToCore 事件(Critical 事件,任何负载下都穿膜)
fn make_critical_event() -> NexusEvent {
    NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("bench-source"),
        quest_id: "q-bench-001".into(),
        checkpoint_id: "ckpt-bench-001".into(),
        memory_snapshot_hash: "sha256:deadbeef".into(),
    }
}

/// 构造 LocalConsume 事件(CacheLocal,High 档被本地消化)
fn make_cache_event() -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("bench-source"),
        cache_key: "k-bench-001".into(),
    }
}

/// 构造 Normal 事件(NormalLow,用于无膜基准对比)
fn make_normal_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("bench-source"),
        quest_id: "q-bench-001".into(),
        title: "bench event".into(),
        task_count: 3,
    }
}

/// bench 1:PassToCore 事件穿膜发布延迟
///
/// 测量:MembraneFilter::decide() + publish_blocking() 的总延迟。
/// PassToCore 事件走完整发布路径(broadcast send + Critical mpsc 旁路),
/// 是膜开销 + 发布开销的上界。
fn membrane_pass_to_core(c: &mut Criterion) {
    let bus = EventBus::new();
    let _rx = bus.subscribe(); // 创建订阅者避免"无订阅者"路径
    let membrane = MembraneFilter::new(); // Low 负载,默认厚度 0

    let mut group = c.benchmark_group("membrane_delivery");
    group.bench_function("pass_to_core_blocking", |b| {
        b.iter(|| {
            let event = make_critical_event();
            bus.publish_membrane_blocking(black_box(event), &membrane)
                .expect("publish_membrane 失败");
        });
    });
    group.finish();
}

/// bench 2:LocalConsume 事件膜边界消化延迟
///
/// 测量:MembraneFilter::decide() + 跳过发布的总延迟。
/// LocalConsume 事件不进入任何 channel,是膜过滤的"快速路径"——
/// 仅做决策(should be near-zero overhead)。
fn membrane_local_consume(c: &mut Criterion) {
    let bus = EventBus::new();
    let _rx = bus.subscribe();
    // High 负载:CacheLocal 被本地消化
    let membrane = MembraneFilter::with_load(InnerLoad::High);

    let mut group = c.benchmark_group("membrane_delivery");
    group.bench_function("local_consume_blocking", |b| {
        b.iter(|| {
            let event = make_cache_event();
            bus.publish_membrane_blocking(black_box(event), &membrane)
                .expect("publish_membrane 失败");
        });
    });
    group.finish();
}

/// bench 3:无膜基准对比(量化膜决策开销增量)
///
/// 测量:publish_blocking() 无膜延迟,与 bench 1 对比可得出膜决策的纯开销。
fn membrane_baseline_no_membrane(c: &mut Criterion) {
    let bus = EventBus::new();
    let _rx = bus.subscribe();

    let mut group = c.benchmark_group("membrane_delivery");
    group.bench_function("baseline_no_membrane", |b| {
        b.iter(|| {
            let event = make_normal_event();
            bus.publish_blocking(black_box(event))
                .expect("publish 失败");
        });
    });
    group.finish();
}

/// bench 4:端到端跨膜投递(publish_membrane → recv)
///
/// 验收标准:spec.md L262 "跨膜事件投递 p95<10ms"
/// 测量从 publish_membrane 调用到订阅者 recv 收到事件的完整延迟。
/// 含:膜决策 + broadcast send + tokio 调度 + recv。
///
/// 使用 Throughput::Elements(1) 让 criterion 报告 events/sec。
fn membrane_e2e_delivery(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("membrane_e2e_delivery");
    group.throughput(Throughput::Elements(1));
    group.sample_size(100); // 默认 100,保持统计稳健

    group.bench_function("publish_to_recv", |b| {
        b.iter(|| {
            rt.block_on(async {
                let bus = EventBus::new();
                let membrane = MembraneFilter::new(); // Low 负载
                let mut rx = bus.subscribe();

                // PassToCore 事件:发布并接收
                let event = make_critical_event();
                bus.publish_membrane(event, &membrane)
                    .await
                    .expect("publish_membrane 失败");
                let _received = rx.recv().await.expect("recv 失败");
            });
        });
    });

    group.finish();
}

/// bench 5:批量膜决策吞吐(decide_batch)
///
/// 测量对 1000 个事件批量决策的吞吐,验证 InnerLoad::Critical 档
/// 的快速过滤能力(Critical 档仅放行 Critical 事件,其他全部跳过)。
fn membrane_batch_decision(c: &mut Criterion) {
    let membrane = MembraneFilter::with_load(InnerLoad::Critical);

    // 构造 1000 个混合事件(100 Critical + 900 Normal)
    let events: Vec<NexusEvent> = (0..1000)
        .map(|i| {
            if i % 10 == 0 {
                make_critical_event()
            } else {
                make_normal_event()
            }
        })
        .collect();

    let mut group = c.benchmark_group("membrane_batch_decision");
    group.throughput(Throughput::Elements(1000));
    group.bench_function("decide_batch_1000", |b| {
        b.iter(|| {
            let decisions = membrane.decide_batch(black_box(&events));
            // 验证 Critical 档仅 100 个 PassToCore
            let pass_count = decisions.iter().filter(|d| d.passes_to_core()).count();
            assert_eq!(pass_count, 100, "Critical 档应仅放行 Critical 事件");
        });
    });
    group.finish();
}

criterion_group!(
    membrane_benches,
    membrane_pass_to_core,
    membrane_local_consume,
    membrane_baseline_no_membrane,
    membrane_e2e_delivery,
    membrane_batch_decision,
);
criterion_main!(membrane_benches);
