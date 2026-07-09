//! EventBus 性能基准测试
//!
//! 对应架构层:L1 Core(跨层通信唯一通道,§2.2 依赖铁律)
//!
//! # 基准项
//! - `single_publish_latency`:单次 `publish_blocking` 延迟(同步,无 runtime 开销)
//! - `concurrent_subscribe_throughput`:多订阅者并发接收吞吐量
//!
//! # 设计说明
//! - bench 1 用 `publish_blocking`:同步发布,无 tokio runtime 调度噪声,
//!   反映 EventBus 自身的 publish 路径开销(metadata 拷贝、broadcast::Sender::send
//!   及 Critical mpsc 旁路判定)。
//! - bench 2 用多订阅者并发 recv:每 iter 发布 N 条事件,N 个订阅者各 recv 一条,
//!   `Throughput::Elements(N)` 让 criterion 报告 events/sec,直观反映广播扇出能力。
//!
//! # min-of-N 5 采样(Engineering Convention)
//! criterion 默认 sample_size=100 + 5 warmup,统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。本 bench 沿用默认配置不显式调小 sample_size,保证统计稳健。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::runtime::Runtime;

/// 并发订阅者数量(扇出场景,模拟 L4-L10 多层订阅同事件)
const SUBSCRIBER_COUNT: usize = 8;

/// 每 iter 发布事件数量(与订阅者数对齐,保证每个订阅者恰好收到一条)
const EVENTS_PER_ITER: usize = SUBSCRIBER_COUNT;

/// 构造用于测试的 NexusEvent(QuestCreated 是最常见的高频事件)
///
/// WHY 固定 payload:避免随机文本长度变化干扰测量;QuestCreated 是 L9→L1 高频事件
fn make_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("bench-source"),
        quest_id: "q-bench-001".into(),
        title: "EventBus bench event".into(),
        task_count: 3,
    }
}

/// bench 1:单次 publish 延迟
///
/// WHY 同步 publish_blocking:publish 是 async 但内部为同步 send,保留 async 仅为
/// API 稳定性(见 bus.rs:144 注释);publish_blocking 直接测同步路径开销,
/// 排除 tokio runtime block_on 的调度噪声。每 iter 调用一次,反映单次发布延迟。
fn single_publish_latency(c: &mut Criterion) {
    let bus = EventBus::new();
    // 创建一个订阅者,避免"无订阅者"路径(否则 publish 会 warn Critical 事件丢失)
    let _rx = bus.subscribe();

    let mut group = c.benchmark_group("single_publish_latency");
    group.bench_function("publish_blocking", |b| {
        b.iter(|| {
            let event = make_event();
            // WHY expect:bench 中 publish 失败表明 EventBus 内部状态错误,应 panic 暴露
            bus.publish_blocking(black_box(event))
                .expect("publish 失败");
        });
    });
    group.finish();
}

/// bench 2:多订阅者并发接收吞吐量
///
/// WHY 扇出场景:EventBus 是跨层通信唯一通道,生产中一个事件常被 4-8 个模块订阅
/// (如 BudgetAdjusted 被 Parliament/Quest/efficiency-monitor 订阅)。
/// 此 bench 验证 N 个订阅者并发 recv 的吞吐,反映广播扇出能力。
///
/// 实现策略:
/// - 创建 N 个订阅者(在 spawn 之前同步调用,§4.4 反模式 3)
/// - 发布 N 条事件到 broadcast buffer(所有订阅者共享同一份事件拷贝)
/// - 每个 EventReceiver move 到独立 spawn 任务中 recv 一条
/// - Throughput::Elements(N) 让 criterion 报告 events/sec
///
/// 注:`drain(..)` 取得 EventReceiver 所有权(`'static`),才能 spawn 到 tokio task。
fn concurrent_subscribe_throughput(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("concurrent_subscribe_throughput");
    group.throughput(Throughput::Elements(EVENTS_PER_ITER as u64));

    for &sub_count in &[1usize, 4, 8] {
        group.bench_with_input(
            BenchmarkId::from_parameter(sub_count),
            &sub_count,
            |b, &n| {
                // 每次 iter 创建新 bus,避免历史事件污染
                b.iter(|| {
                    rt.block_on(async {
                        let bus = EventBus::new();
                        // WHY 在 spawn 之前同步 subscribe:§4.4 反模式 3,
                        // 否则事件静默丢失(虽然此处 publish 也在同 task 内,但保持
                        // 与生产代码相同的"先订阅后发布"模式)
                        let rx_list: Vec<_> = (0..n).map(|_| bus.subscribe()).collect();

                        // 发布 n 条事件(每个订阅者恰好收到一条)
                        for _ in 0..n {
                            let event = make_event();
                            bus.publish(event).await.expect("publish 失败");
                        }

                        // 并发 recv:每个订阅者接收一条事件
                        // WHY drain 取得所有权:EventReceiver 必须 'static 才能 spawn,
                        // &mut EventReceiver 借用 rx_list 不满足 'static 约束
                        let mut handles = Vec::with_capacity(n);
                        for mut rx in rx_list.into_iter() {
                            handles.push(tokio::spawn(async move {
                                rx.recv().await.expect("recv 失败")
                            }));
                        }
                        for h in handles {
                            let _ = h.await.expect("recv task panic");
                        }
                    });
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    single_publish_latency,
    concurrent_subscribe_throughput
);
criterion_main!(benches);
