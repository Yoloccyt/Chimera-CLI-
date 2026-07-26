//! P1-W4.1 tracing 贯穿观测测试 — 验证 event-bus 膜背压路径的 span 与结构化丢弃日志
//!
//! 架构层:L1 Core(event-bus)
//!
//! # 测试范围
//! 1. `test_critical_dropped_tracing` — send_critical_mpsc 容量满时发出结构化 warn(dropped_count 字段)
//! 2. `test_critical_no_drop_no_warn` — 正常投递时不发出丢弃 warn 日志
//! 3. `test_critical_span_fields` — send_critical_mpsc span 携带 event_type / severity / event_id 字段
//!
//! # 测试方法
//! 使用 `tracing_test::traced_test` 宏自动捕获 tracing 事件。
//! WHY 属性顺序 `#[tracing_test::traced_test]` 在 `#[tokio::test]` 之前:
//! proc macro 属性从上到下应用,traced_test 需先包装函数注入 mock subscriber,
//! 再由 tokio::test 包装为同步入口。若顺序反转,tokio::test 先将 async fn 转为
//! sync fn,traced_test 无法正确注入 `logs_contain` 函数。
//! API: `logs_contain("substring")` 返回 bool,检查捕获日志是否包含子串。
//! SkepticVeto 是 §6.2 红线 4 类 Critical 安全告警事件之一,会强制走 mpsc 旁路通道。

#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 构造 SkepticVeto 事件(§6.2 红线:Critical 安全告警事件之一)。
///
/// WHY SkepticVeto:它通过 `is_critical_mpsc_event` 判定强制走 mpsc 旁路通道,
/// 是验证 `send_critical_mpsc` tracing 行为的最直接事件类型。
fn make_skeptic_veto(quest_id: &str) -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("test"),
        quest_id: quest_id.to_string(),
        veto_reason: "test veto".into(),
        frozen_capabilities: vec![],
    }
}

// ============================================================
// SubTask P1-W4.1.1 路径 1:膜背压路径 send_critical_mpsc
// ============================================================

/// 验证 `EventBus::send_critical_mpsc` 在容量满时发出结构化 warn 日志,
/// 携带 `dropped_count` 字段与 "Critical 事件被丢弃" 消息。
///
/// # 流程
/// 1. 创建 EventBus
/// 2. 订阅 Critical 通道但不消费(模拟慢消费者填满 4096 容量)
/// 3. 通过 `publish_critical` 发布超过容量的 SkepticVeto 事件,触发丢弃
/// 4. 验证 logs 包含 `dropped_count` 字段与 "Critical 事件被丢弃" 消息
/// 5. 验证 `critical_dropped_count()` 返回值 > 0
///
/// # P1-W4.1 验证目标
/// - `#[tracing::instrument(skip(self, event), fields(event_type, severity, event_id))]` 正确捕获 span
/// - 容量满时 retain 闭包外单次发出 `tracing::warn!(dropped_count, ...)`
/// - `dropped_count` 是本次发送导致的丢弃增量,而非全局累计值
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_dropped_tracing() {
    let bus = EventBus::new();
    // 订阅但不消费,模拟慢消费者填满 4096 容量
    let _stale_rx = bus.subscribe_critical_events();

    let capacity = bus.critical_channel_capacity();
    // 发布超过容量的 Critical 事件,触发丢弃
    for i in 0..(capacity + 10) as u64 {
        let _ = bus
            .publish_critical(make_skeptic_veto(&format!("q-{i}")))
            .await;
    }

    // P1-W4.1:验证丢弃日志含 dropped_count 字段
    assert!(
        logs_contain("dropped_count"),
        "tracing 日志应包含 dropped_count 字段"
    );
    assert!(
        logs_contain("Critical 事件被丢弃"),
        "tracing 日志应包含 'Critical 事件被丢弃' 消息"
    );

    // 验证 dropped_count > 0(累计丢弃)
    assert!(
        bus.critical_dropped_count() > 0,
        "Critical 通道应有丢弃事件发生(发布 {} 个事件到容量 {} 的通道)",
        capacity + 10,
        capacity
    );
}

/// 验证 `EventBus::send_critical_mpsc` 在正常投递时不发出 warn 日志。
///
/// # 流程
/// 1. 创建 EventBus
/// 2. 订阅 Critical 通道并消费(避免容量满)
/// 3. 发布单个 SkepticVeto 事件
/// 4. 验证 logs 不包含 "Critical 事件被丢弃" 消息
/// 5. 验证 `critical_dropped_count()` 返回值为 0
///
/// # P1-W4.1 验证目标
/// - 丢弃日志只在 `dropped_count > 0` 时发出(避免 false positive)
/// - 正常投递路径不产生噪声日志,保持 efficiency-monitor 订阅清爽
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_no_drop_no_warn() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe_critical_events();

    // 发布单个事件,订阅者会消费,不应触发丢弃
    bus.publish_critical(make_skeptic_veto("q-normal"))
        .await
        .expect("发布应成功");

    // 排空 receiver 确保不阻塞
    let received = rx.recv().await;
    assert!(received.is_some(), "应收到发布的 SkepticVeto 事件");

    // 不应有丢弃日志
    assert!(
        !logs_contain("Critical 事件被丢弃"),
        "正常投递不应触发丢弃日志"
    );
    assert_eq!(bus.critical_dropped_count(), 0, "正常投递不应有丢弃计数");
}

/// 验证 `send_critical_mpsc` span 携带 `event_type` / `severity` / `event_id` 结构化字段。
///
/// # 流程
/// 1. 创建 EventBus 并订阅 Critical 通道
/// 2. 发布 SkepticVeto 事件(type_name = "SkepticVeto",severity = Critical)
/// 3. 验证 logs 包含 "SkepticVeto" 与 "Critical" 字段值
///
/// # P1-W4.1 验证目标
/// - `event_type = %event.type_name()` 记录事件类型名(用于过滤聚合)
/// - `severity = ?event.severity()` 记录事件严重级别(用于优先级排序)
/// - `event_id = %event.metadata().event_id` 记录事件唯一标识(UUIDv7,跨进程因果追踪)
/// - 这三个字段是 efficiency-monitor 关联丢弃事件与原始事件类型的依据
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_span_fields() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe_critical_events();

    bus.publish_critical(make_skeptic_veto("q-fields"))
        .await
        .expect("发布应成功");

    // 排空 receiver
    let _ = rx.recv().await;

    // 验证 event_type 字段(SkepticVeto 是 §6.2 红线 4 类 Critical 事件之一)
    assert!(
        logs_contain("SkepticVeto"),
        "tracing 日志应包含 event_type=SkepticVeto"
    );
    // 验证 severity 字段(SkepticVeto 的 severity() 返回 Critical)
    assert!(
        logs_contain("Critical"),
        "tracing 日志应包含 severity=Critical"
    );
}
