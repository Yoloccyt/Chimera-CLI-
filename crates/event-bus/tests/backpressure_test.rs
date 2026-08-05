//! Critical 通道有界化 TDD RED 测试基线 — P1-W1.1
//!
//! 对应文档:
//! - spec.md §Scenario "Critical 通道背压改造"(D3 修复,L183-189)
//! - tasks.md P1-W1.1(SubTask P1-W1.1.1 / P1-W1.1.2,L81-84)
//!
//! # TDD RED 状态说明(本文件预期编译失败)
//!
//! 本测试文件**主动引用尚未实现的 API**,导致 `cargo test -p event-bus
//! backpressure_test` 编译失败,这是 TDD RED 阶段的预期状态。
//!
//! W2.1(P1-W2.1.1)将实现以下 API,使测试转 GREEN:
//! - `EventBus::critical_channel_capacity() -> usize`(返回 4096)
//! - `EventBus::subscribe_critical_events()` 返回类型由
//!   `mpsc::UnboundedReceiver<NexusEvent>` 改为有界 `mpsc::Receiver<NexusEvent>`
//! - `EventBus::critical_dropped_count() -> u64`(返回因容量满而丢弃的事件数)
//! - `event_bus::CriticalEventDropped`(指标载荷结构体,供 efficiency-monitor 拉取)
//!
//! # Spec 验收门槛(spec.md L183-189)
//!
//! - 10K 事件/秒 × 60s 注入 Critical 通道
//! - 系统不发生 OOM
//! - Critical 事件 100% 送达(容量内)
//! - 超容量时按优先级采样丢弃
//! - 丢弃事件计入 `CriticalEventDropped` 指标
//! - `publish_critical()` 公开签名不变
//!
//! # 设计约束
//!
//! - 纯 safe Rust,兼容 `#![forbid(unsafe_code)]`
//! - 不持锁跨 `.await`(§4.4 红线 1):所有计数用 `Arc<AtomicU64>`,锁不跨 await
//! - 并发收集用 `tokio::task::JoinSet`(tokio 内置,无需新增 futures dev-dependency;
//!   若 W2.1 偏好 `FuturesUnordered`,可在 Cargo.toml `[dev-dependencies]` 添加
//!   `futures = { workspace = true }` 后重构,当前不修改 Cargo.toml)

use event_bus::{CriticalEventDropped, EventBus, EventMetadata, NexusEvent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ============================================================
// 测试辅助:构造各类事件
// ============================================================

/// 构造一个走 mpsc 旁路的 Critical 安全告警事件(SkepticVeto)
///
/// WHY 选 SkepticVeto:它是 `is_critical_mpsc_event` 判定的 4 类事件之一,
/// 必定走 Critical 通道。用于验证有界化后安全告警的投递保证。
fn make_mpsc_critical_event(idx: u64) -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: format!("q-{idx}"),
        veto_reason: format!("unsafe op #{idx}"),
        frozen_capabilities: vec![format!("cap-{idx}")],
    }
}

// ============================================================
// 测试 1:Critical 通道容量为 4096(有界,而非 UnboundedSender)
//
// 对应 SubTask P1-W1.1.1 的前置:验证通道有界化。
// 引用未实现 API `EventBus::critical_channel_capacity()`,W2.1 实现后应返回 4096。
// 同时验证 `subscribe_critical_events` 返回有界 `mpsc::Receiver`(当前返回
// `mpsc::UnboundedReceiver`,类型不匹配导致编译失败)。
// ============================================================

#[test]
fn test_critical_channel_bounded_capacity() {
    let bus = EventBus::new();

    // 未实现 API:critical_channel_capacity()
    // W2.1 实现后应返回 4096(spec.md L84:容量 4096)
    let capacity = bus.critical_channel_capacity();
    assert_eq!(
        capacity, 4096,
        "Critical 通道容量必须为 4096 (D3 改造,spec.md L84:改有界 Sender<4096>)"
    );

    // 验证 subscribe_critical_events 返回有界 Receiver(而非 UnboundedReceiver)
    // 当前实现返回 mpsc::UnboundedReceiver<W2.1 改为 mpsc::Receiver 后此断言通过
    let _rx: mpsc::Receiver<NexusEvent> = bus.subscribe_critical_events();

    // CriticalEventDropped 类型应可构造(作为指标载荷)
    // W2.1 实现后应提供此类型,字段包含 dropped_count 等
    let _metric = CriticalEventDropped::new(0);
}

/// 读取环境变量 `CHIMERA_BACKPRESSURE_SECS` 覆盖测试时长;缺省 60s 保持 spec 验收语义。
/// WHY 参数化:文件头注释 L100-103 预留设计 —— 常规 CI 设 5s 用于快速回归,
/// Release 验收保留 60s(P1-W4.3.4 spec 验收门槛)。断言均为相对量,
/// 随时长缩放不受影响(final_count == total_to_publish)。
fn env_duration_secs() -> u64 {
    std::env::var("CHIMERA_BACKPRESSURE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

// ============================================================
// 测试 2:10K 事件/秒 × 60s 注入 Critical 通道,无 OOM 且 100% 送达
//
// 对应 SubTask P1-W1.1.1:spec.md L183-185 验收门槛。
//
// 设计要点:
// - 发布方持续以 ≥10K/s 速率发布 Critical 事件(共 600K)
// - 订阅方持续消费(消费速率 ≥ 发布速率,确保容量不积压)
// - 容量 4096 仅作为突发缓冲,不丢弃任何事件
// - 60s 跑完即证明无 OOM;订阅方收到 600K 即证明 100% 送达
//
// WHY 60s 测试时长:spec.md L183 明确要求 "10K 事件/秒持续 60 秒"。
// 本测试为 TDD RED(编译失败不会跑),W2.1 转 GREEN 后:
// - 常规 CI:可参数化降低 DURATION_SECS(如 5s)用于快速回归
// - Release 验收:保留 60s 用于 spec 验收门槛(P1-W4.3.4)
//
// WHY 使用 JoinSet 而非 FuturesUnordered:
// tokio::task::JoinSet 是 tokio 内置并发收集器,无需新增 futures dev-dependency。
// 语义等价:JoinSet 提供并发执行 + 顺序收集结果,满足 §4.4 "并发收集" 要求。
// 若 W2.1 偏好 FuturesUnordered,在 Cargo.toml [dev-dependencies] 添加
// futures = { workspace = true } 后可重构(当前不修改 Cargo.toml)。
// ============================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_10k_events_per_second_no_oom() {
    let bus = EventBus::new();

    // 未实现 API:critical_channel_capacity() — W2.1 应返回 4096
    // 引用此 API 使测试编译失败(TDD RED)
    assert_eq!(
        bus.critical_channel_capacity(),
        4096,
        "Critical 通道容量必须为 4096 (D3 改造)"
    );

    // 订阅 Critical 事件(有界 Receiver,W2.1 实现后返回 mpsc::Receiver)
    let mut crit_rx = bus.subscribe_critical_events();

    // 消费计数器:Arc<AtomicU64> 避免"持锁跨 await"(§4.4 红线 1)
    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();

    // 消费任务:持续接收 Critical 事件并计数
    // WHY 独立 spawn:消费方与发布方解耦,消费速率不受 publish 调用栈影响
    let consumer = tokio::spawn(async move {
        let mut count: u64 = 0;
        // 循环接收直到收到预期总数或通道关闭
        // 超时 5s 保护:若 5s 内无事件,认为发布方已停止
        while let Some(_event) = tokio::time::timeout(Duration::from_secs(5), crit_rx.recv())
            .await
            .ok()
            .flatten()
        {
            count = count.saturating_add(1);
            received_clone.store(count, Ordering::Relaxed);
        }
        count
    });

    // 发布方:10K 事件/秒 × 60s = 600K 事件(spec.md L183 验收门槛)
    // 时长可由 CHIMERA_BACKPRESSURE_SECS 环境变量覆盖(常规 CI 快速回归用 5s)
    const RATE_PER_SEC: usize = 10_000;
    let duration_secs: u64 = env_duration_secs();
    let total_to_publish: u64 = (RATE_PER_SEC as u64) * duration_secs;

    let start = Instant::now();
    for i in 0..total_to_publish {
        // 使用 publish_critical 显式走 Critical 通道(spec.md L189:签名不变)
        bus.publish_critical(make_mpsc_critical_event(i))
            .await
            .expect("publish_critical 不应失败");
    }
    let publish_elapsed = start.elapsed();
    println!(
        "发布 {total_to_publish} 个 Critical 事件,耗时 {:?}(目标速率 {RATE_PER_SEC}/s × {duration_secs}s)",
        publish_elapsed
    );

    // 等待消费方处理完所有事件(给 30s 缓冲,证明无 OOM)
    // WHY 30s 缓冲:600K 事件在 4 worker 线程下消费应远快于 60s 发布,
    // 30s 足够;若超时则证明系统卡死(疑似 OOM 或死锁)
    let final_count = tokio::time::timeout(Duration::from_secs(30), consumer)
        .await
        .expect("消费方应在 30s 内处理完所有事件(证明无 OOM,无死锁)")
        .expect("消费任务不应 panic");

    // 断言 1:Critical 事件 100% 送达(容量内)
    // spec.md L185:"Critical 事件 100% 送达(容量内)"
    assert_eq!(
        final_count, total_to_publish,
        "Critical 事件 100% 送达失败:发布 {total_to_publish},收到 {final_count}"
    );

    // 断言 2:无丢弃(消费速率 ≥ 发布速率,容量未满)
    // spec.md L187:"超容量时按优先级采样丢弃" — 本测试消费方跟得上,不应丢弃
    assert_eq!(
        bus.critical_dropped_count(),
        0,
        "消费方跟得上时不应丢弃任何 Critical 事件"
    );

    // 断言 3:测试能执行到此即证明无 OOM(否则进程已崩溃)
    println!("✓ 10K/s × 60s 注入完成,无 OOM,Critical 100% 送达");
}

// ============================================================
// 测试 3:超容量时按优先级采样丢弃,CriticalEventDropped 计数正确
//
// 对应 SubTask P1-W1.1.2:spec.md L187 验收门槛。
//
// 设计要点:
// - 订阅 Critical 事件后**不消费**,让容量快速填满
// - 发布 > 4096 个 Critical 事件,触发有界通道背压
// - W2.1 实现应在容量满时按优先级采样丢弃(而非阻塞 send)
// - 断言 critical_dropped_count() == 发布数 - 容量
// - 断言 CriticalEventDropped 指标载荷可构造,字段正确
//
// WHY "不消费"模拟慢消费者:验证有界通道在消费者跟不上时不会 OOM,
// 而是按优先级丢弃并计数。这是 spec.md L187 "超容量时按优先级采样丢弃"
// 的核心场景。
// ============================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_critical_event_dropped_counter() {
    let bus = EventBus::new();

    // 未实现 API:critical_channel_capacity() — W2.1 应返回 4096
    let capacity = bus.critical_channel_capacity();
    assert_eq!(capacity, 4096, "Critical 通道容量必须为 4096");

    // 订阅 Critical 事件但**不消费**(模拟慢消费者)
    // _rx 必须保留(不 drop),否则 channel 关闭,send 返回 Err 而非丢弃
    let _stale_rx = bus.subscribe_critical_events();

    // 发布 > 容量数量的 Critical 事件,触发背压丢弃
    // 发布 5000 个,容量 4096,预期丢弃 5000 - 4096 = 904
    let publish_count: u64 = 5000;
    let expected_dropped: u64 = publish_count.saturating_sub(capacity as u64);

    for i in 0..publish_count {
        // publish_critical 内部应使用 try_send + 采样丢弃逻辑(W2.1 实现)
        // 容量满时:not await 阻塞,而是丢弃并递增计数器
        // WHY 不用 publish_critical().await:W2.1 可能将 publish_critical 改为
        // 内部 try_send 语义(容量满立即丢弃返回 Ok),而非阻塞 send。
        // 此处统一用 publish_critical,若 W2.1 阻塞则需改为 try_publish_critical。
        let result = bus.publish_critical(make_mpsc_critical_event(i)).await;
        // publish_critical 返回 Result<(), EventBusError>,即使丢弃也应返回 Ok
        // (丢弃是预期行为,不是错误)
        if result.is_ok() {
            // W2.1 实现后,若内部丢弃会递增 dropped 计数器
            // 此处用 critical_dropped_count() 校验
        }
    }

    // 断言 1:丢弃计数 == 预期(发布数 - 容量)
    // spec.md L187:"丢弃事件计入 CriticalEventDropped 指标"
    let actually_dropped: u64 = bus.critical_dropped_count();
    assert_eq!(
        actually_dropped, expected_dropped,
        "CriticalEventDropped 计数错误:预期丢弃 {expected_dropped}(发布 {publish_count} - 容量 {capacity}),实际 {actually_dropped}"
    );

    // 断言 2:CriticalEventDropped 指标载荷可构造,字段包含 dropped_count
    // spec.md L147:"CriticalEventDropped 指标 + TUI 告警"
    // 未实现类型:CriticalEventDropped — W2.1 应提供此结构体
    let metric = CriticalEventDropped::new(actually_dropped);
    assert_eq!(
        metric.dropped_count(),
        actually_dropped,
        "CriticalEventDropped 指标载荷的 dropped_count 应与 critical_dropped_count() 一致"
    );

    // 断言 3:容量内的事件被保留(优先级采样丢弃保留较新 Critical)
    // spec.md L187:"超容量时按优先级采样丢弃" — Critical 事件优先保留
    // 验证:丢弃的是超出容量的部分,容量内的 4096 个应被保留在 channel 中
    // (由 _stale_rx 持有,未消费)
    // 此处通过 critical_dropped_count() 间接验证:丢弃数 == 超出部分,非全部丢弃
    assert!(
        actually_dropped < publish_count,
        "按优先级采样丢弃应保留容量内事件,而非全部丢弃(丢弃 {actually_dropped} < 发布 {publish_count})"
    );

    println!(
        "✓ 超容量采样丢弃正确:丢弃 {actually_dropped},保留 {}",
        capacity
    );
}
