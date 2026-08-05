//! 内环跨层集成性能基准 — P3-2
//!
//! 对应架构层:跨层(L1 EventBus → L2 Memory → L6 Router → L7 Execution)
//!
//! # 内环 9 个候选 crate
//!
//! | 层级 | Crate | 职责 |
//! |------|-------|------|
//! | L1   | event-bus | 跨层通信唯一通道(§2.2 依赖铁律) |
//! | L2   | hcw-window | 分层上下文窗口管理 |
//! | L2   | mlc-engine | 四级潜在记忆引擎 |
//! | L6   | kvbsr-router | KV 块语义路由器 |
//! | L6   | faae-router | Function-as-Expert 语义路由 |
//! | L6   | gea-activator | 门控专家激活(实际归 L9,但属内环) |
//! | L6   | osa-coordinator | 全维稀疏协调器 |
//! | L7   | gqep-executor | 聚集查询执行协议 |
//! | L7   | pvl-layer | 生产验证闭环 |
//!
//! # 基准项
//!
//! - `cross_layer_pipeline`:模拟 L1→L2→L6→L7 跨层数据流,发布事件到 4 层订阅者,
//!   测量端到端延迟。对应真实场景:QuestCreated → ContextWindowSwitched →
//!   ToolsRouted → ExpertActivated。
//! - `cross_layer_fanout`:并发跨层扇出,多事件同时发布,各层同时处理。
//!   模拟真实场景中多事件流同时进行的场景。
//! - `cross_layer_critical_bypass`:Critical 事件 mpsc 旁路通道延迟。
//!   测量 Critical 事件通过 bypass 通道的延迟。
//!
//! # 设计说明
//!
//! 所有基准基于 EventBus 作为跨层通信唯一通道,通过分层订阅者模式模拟
//! 各层对事件的消费。EventBus 是内环 9 个 crate 共享的唯一通信基础设施,
//! 其性能直接影响整体跨层集成性能。
//!
//! # 运行方式
//!
//! ```bash
//! cargo bench -p event-bus --bench cross_layer_bench
//! ```

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::runtime::Runtime;

/// 层数 — 模拟 L1/L2/L6/L7 四层
const LAYER_COUNT: usize = 4;

/// 每 iter 事件数(与扇出测试对齐)
const EVENTS_PER_ITER: usize = 4;

/// 构造 QuestCreated 事件 — L10→L9 高频事件,触发 Quest 分解
fn make_quest_created() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("bench-cross-layer"),
        quest_id: "q-cross-001".into(),
        title: "Cross-layer benchmark quest".into(),
        task_count: 5,
    }
}

/// 构造 ContextWindowSwitched 事件 — L2 HCW 发出,触发窗口切换
fn make_context_switched() -> NexusEvent {
    NexusEvent::ContextWindowSwitched {
        metadata: EventMetadata::new("hcw-window"),
        from_tier: "L1".into(),
        to_tier: "L2".into(),
        reason: "capacity exceeded".into(),
    }
}

/// 构造 ToolsRouted 事件 — L6 KVBSR+FaaE 发出,通知路由结果
fn make_tools_routed() -> NexusEvent {
    NexusEvent::ToolsRouted {
        metadata: EventMetadata::new("kvbsr-router"),
        routed_count: 8,
        top_tool: "tool-code-gen".into(),
        routed_tools: vec![
            "tool-code-gen".into(),
            "tool-test".into(),
            "tool-deploy".into(),
            "tool-review".into(),
        ],
    }
}

/// 构造 ExpertActivated 事件 — L6/L9 GEA 发出,通知专家激活
fn make_expert_activated() -> NexusEvent {
    NexusEvent::ExpertActivated {
        metadata: EventMetadata::new("gea-activator"),
        activated_experts: vec!["expert-code".into(), "expert-test".into()],
        suppressed_experts: vec!["expert-legacy".into()],
        top_gate_value: 0.92,
    }
}

/// 构造 Critical 事件 — 模拟治理/安全事件,走 mpsc bypass 通道
///
/// WHY BudgetExceeded: 其 severity() = Critical(mpsc 旁路保证送达),
/// 是治理面 Critical 事件的代表(decb-governor 成本治理语义匹配)
fn make_critical_event() -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "bench-cross-layer".into(),
        current: 100,
        limit: 50,
    }
}

/// 为指定层创建订阅者,返回层标识与 receiver
///
/// 模拟各层对事件的订阅:
/// - L1(event-bus):订阅所有事件(基础设施层)
/// - L2(Memory):订阅 QuestCreated + ContextWindowSwitched
/// - L6(Router):订阅 QuestCreated + ToolsRouted
/// - L7(Execution):订阅 QuestCreated + ExpertActivated
fn create_layer_subscribers(bus: &EventBus, layer_count: usize) -> Vec<event_bus::EventReceiver> {
    (0..layer_count).map(|_| bus.subscribe()).collect()
}

/// Bench 1: L1→L2→L6→L7 跨层流水线延迟
///
/// 模拟真实跨层数据流:
/// 1. L10 Interface 发布 QuestCreated → L1 event-bus 广播
/// 2. L2 hcw-window 处理上下文 → 发布 ContextWindowSwitched
/// 3. L6 kvbsr-router 路由工具 → 发布 ToolsRouted
/// 4. L6/L9 gea-activator 激活专家 → 发布 ExpertActivated
///
/// 测量:从第一个事件发布到最后一个事件被所有层接收的端到端延迟。
/// 实际场景中,各层处理是异步并行的,本基准模拟串行流水线以测量
/// EventBus 的传播延迟。
fn cross_layer_pipeline(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("cross_layer_pipeline");
    group.sample_size(30);
    group.measurement_time(std::time::Duration::from_secs(10));

    group.bench_function("pipeline_4_layer", |b| {
        b.iter(|| {
            rt.block_on(async {
                let bus = EventBus::new();
                let _rxs = create_layer_subscribers(&bus, LAYER_COUNT);

                // 模拟 4 层数据流:依次发布 4 个事件
                // WHY 数组而非 vec!: 固定 4 元素,遍历只读(clippy useless_vec)
                let events = [
                    make_quest_created(),
                    make_context_switched(),
                    make_tools_routed(),
                    make_expert_activated(),
                ];

                for event in events {
                    bus.publish(black_box(event)).await.expect("publish 失败");
                }
            });
        });
    });

    group.finish();
}

/// Bench 2: 并发跨层扇出吞吐量
///
/// 模拟真实场景中多事件同时发布,各层同时消费:
/// - 4 个事件同时发布(QuestCreated/ContextWindowSwitched/ToolsRouted/ExpertActivated)
/// - 每事件被 4 层订阅者同时接收
/// - 测量:单位时间内成功交付的事件总数(events/sec)
///
/// 使用 Throughput::Elements 让 criterion 报告 events/sec。
fn cross_layer_fanout(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("cross_layer_fanout");
    group.throughput(Throughput::Elements(EVENTS_PER_ITER as u64));

    for &layer_count in &[2usize, 4, 8] {
        group.bench_function(format!("fanout_{}_layers", layer_count), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let bus = EventBus::new();
                    let _rxs = create_layer_subscribers(&bus, layer_count);

                    // 并发发布 4 个事件
                    // WHY 数组而非 vec!: 固定 4 元素,仅索引访问(clippy useless_vec)
                    let events = [
                        make_quest_created(),
                        make_context_switched(),
                        make_tools_routed(),
                        make_expert_activated(),
                    ];

                    // 使用 tokio::join! 并发发布(must_use:Result 需显式处理)
                    let f1 = bus.publish(black_box(events[0].clone()));
                    let f2 = bus.publish(black_box(events[1].clone()));
                    let f3 = bus.publish(black_box(events[2].clone()));
                    let f4 = bus.publish(black_box(events[3].clone()));
                    let (r1, r2, r3, r4) = tokio::join!(f1, f2, f3, f4);
                    r1.expect("publish f1 失败");
                    r2.expect("publish f2 失败");
                    r3.expect("publish f3 失败");
                    r4.expect("publish f4 失败");
                });
            });
        });
    }

    group.finish();
}

/// Bench 3: Critical 事件 mpsc 旁路通道延迟
///
/// Critical 事件(如 BudgetAdjusted with Critical severity)走 mpsc bypass
/// 通道而非 broadcast 通道。测量 Critical 事件通过 bypass 通道的发布延迟。
///
/// 设计:
/// - 创建一个 Critical 订阅者和一个普通订阅者(模拟真实场景中两者共存)
/// - 发布 Critical 事件,测量 publish 延迟
/// - 对比:Critical 事件 vs 普通事件的发布延迟差异
fn cross_layer_critical_bypass(c: &mut Criterion) {
    let rt = Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("cross_layer_critical_bypass");
    group.sample_size(30);
    group.measurement_time(std::time::Duration::from_secs(10));

    // 基准 1:Critical 事件 bypass 通道延迟
    group.bench_function("critical_bypass", |b| {
        b.iter(|| {
            rt.block_on(async {
                let bus = EventBus::new();
                // 创建 Critical 订阅者(模拟治理/安全模块)
                let _critical_rx = bus.subscribe_critical_events();
                // 同时创建普通订阅者(模拟其他模块)
                let _normal_rx = bus.subscribe();

                let event = make_critical_event();
                bus.publish(black_box(event))
                    .await
                    .expect("Critical 事件发布失败");
            });
        });
    });

    // 基准 2:普通事件默认通道延迟(对照)
    group.bench_function("normal_publish", |b| {
        b.iter(|| {
            rt.block_on(async {
                let bus = EventBus::new();
                let _normal_rx = bus.subscribe();

                let event = make_quest_created();
                bus.publish(black_box(event))
                    .await
                    .expect("普通事件发布失败");
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    cross_layer_pipeline,
    cross_layer_fanout,
    cross_layer_critical_bypass
);
criterion_main!(benches);
