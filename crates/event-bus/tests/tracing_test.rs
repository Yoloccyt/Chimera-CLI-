//! P1-W4.1 tracing 贯穿观测测试 — 验证 event-bus 膜背压路径的 span 与结构化丢弃日志
//!
//! 架构层:L1 Core(event-bus)
//!
//! # 测试范围
//! 1. `test_critical_dropped_tracing` — send_critical_mpsc 容量满时发出结构化 warn(dropped_count 字段)
//! 2. `test_critical_no_drop_no_warn` — 正常投递时不发出丢弃 warn 日志
//! 3. `test_critical_span_fields` — send_critical_mpsc span 携带 event_type / severity / event_id 字段
//! 4. `test_critical_mpsc_bypass_without_subscriber_warns` — C3:旁路无订阅者时发出 warn(不再静默)
//! 5. `test_has_critical_subscribers_lifecycle` — C3:订阅者存在性查询 API 生命周期
//!
//! # 测试方法
//! 使用 `tracing_test::traced_test` 宏自动捕获 tracing 事件。
//! WHY 属性顺序 `#[tracing_test::traced_test]` 在 `#[tokio::test]` 之前:
//! proc macro 属性从上到下应用,traced_test 需先包装函数注入 mock subscriber,
//! 再由 tokio::test 包装为同步入口。若顺序反转,tokio::test 先将 async fn 转为
//! sync fn,traced_test 无法正确注入 `logs_contain` 函数。
//! API: `logs_contain("substring")` 返回 bool,检查捕获日志是否包含子串。
//! SkepticVeto 是 §6.2 红线 Critical 安全告警事件之一(is_critical_mpsc_event 清单),会强制走 mpsc 旁路通道。

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

    // 验证 event_type 字段(SkepticVeto 是 §6.2 红线 Critical 事件之一,is_critical_mpsc_event 清单)
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

// ============================================================
// C3(2026-09-04): Critical mpsc 旁路无订阅者可观测性
// ============================================================
// 背景(F-A5-6):旁路通道按需初始化,无订阅者时 send_critical_mpsc 直接
// 静默返回 —— 此时 Critical 事件仅有 broadcast 单通道保障,若订阅者 Lagged
// 则事件永久丢失且无任何告警。C5 评审将此列为可观测性风险点,修复为:
// 1. publish 路径遇"旁路清单事件 + 零旁路订阅者"时发出 warn;
// 2. 提供 has_critical_subscribers() 供组合根/运维查询订阅状态。

/// 验证 broadcast 有订阅者但 Critical mpsc 旁路无订阅者时,发布
/// `is_critical_mpsc_event` 判定的事件会发出 warn 告警(不再静默)。
///
/// # 流程
/// 1. 创建 EventBus,仅调用 `subscribe()`(broadcast 有订阅者,
///    避免误触发既有"Critical 无 broadcast 订阅者"告警路径)
/// 2. 通过 `publish` 发布 SkepticVeto(命中 is_critical_mpsc_event)
/// 3. 验证 logs 包含 "Critical mpsc 旁路无订阅者"
///
/// # WHY 不用 publish_critical
/// publish_critical 语义是"调用方明确知道事件为 Critical",为隔离
/// `is_critical_mpsc_event` 判定路径(publish 分支)的告警行为,本测试走 publish。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_mpsc_bypass_without_subscriber_warns() {
    let bus = EventBus::new();
    // broadcast 有订阅者(排除 severity==Critical 且 subscriber_count==0 的既有告警分支)
    let mut _broadcast_rx = bus.subscribe();

    bus.publish(make_skeptic_veto("q-no-bypass-sub"))
        .await
        .expect("发布应成功");

    assert!(
        logs_contain("Critical mpsc 旁路无订阅者"),
        "旁路无订阅者时发布 Critical 事件应发出 warn 告警,当前日志未包含该消息"
    );
}

// TDD-CYCLE-2: API 缺失 RED 已观察(E0599 no method named `has_critical_subscribers`),现恢复
/// 验证 `has_critical_subscribers()` 的生命周期语义:
/// 初始 false → subscribe_critical_events 后 true。
///
/// # WHY 需要此 API
/// 组合根/运维需要在不发布事件的前提下探询旁路订阅状态
/// (发布时告警是事件驱动的事后观测,此 API 是主动查询通道)。
/// 惰性清理说明:Receiver drop 后 Sender 由下次 send_critical_mpsc 的
/// retain 清理,本查询反映"已建立且未清理"的订阅,与活跃订阅可能存在短暂偏差。
#[tokio::test]
async fn test_has_critical_subscribers_lifecycle() {
    let bus = EventBus::new();
    assert!(
        !bus.has_critical_subscribers(),
        "初始状态下不应存在 Critical 旁路订阅者"
    );

    let _rx = bus.subscribe_critical_events();
    assert!(
        bus.has_critical_subscribers(),
        "subscribe_critical_events 后应存在 Critical 旁路订阅者"
    );
}
