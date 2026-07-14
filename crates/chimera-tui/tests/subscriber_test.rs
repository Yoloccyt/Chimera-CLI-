//! EventSubscriber 测试 — 验证事件订阅、缓冲、优雅关闭与 lag 处理
//!
//! 对应 Task P1.1 验收:
//! - 先订阅后发布不丢失早期事件
//! - 发布的事件可通过 try_recv 非阻塞消费
//! - shutdown 后 subscriber 终止,不再接收事件
//! - lag 场景优雅处理(warn 日志,不 panic)

#![forbid(unsafe_code)]

use std::time::Duration;

use chimera_tui::EventSubscriber;
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 构造一个可识别的测试事件
fn make_test_event(quest_id: impl Into<String>) -> NexusEvent {
    let quest_id = quest_id.into();
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("subscriber-test"),
        quest_id: quest_id.clone(),
        title: format!("Test Quest {quest_id}"),
        task_count: 3,
    }
}

/// 给后台任务一小段时间完成转发
///
/// WHY 50ms:本地单测中 tokio 调度通常 < 10ms,取 50ms 留足余量,
/// 同时避免测试过长。
async fn yield_to_subscriber() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn subscriber_does_not_lose_early_events() {
    // 先创建订阅者,再发布事件 — 验证 subscribe 在 spawn 之前完成
    let bus = EventBus::new();
    let mut sub = EventSubscriber::new(bus.clone());

    let event = make_test_event("early");
    bus.publish(event.clone()).await.unwrap();

    yield_to_subscriber().await;

    assert_eq!(sub.try_recv(), Some(event), "先订阅后发布的事件不应丢失");
}

#[tokio::test]
async fn published_events_are_received_via_try_recv() {
    let bus = EventBus::new();
    let mut sub = EventSubscriber::new(bus.clone());

    let event1 = make_test_event("q-1");
    let event2 = make_test_event("q-2");
    bus.publish(event1.clone()).await.unwrap();
    bus.publish(event2.clone()).await.unwrap();

    yield_to_subscriber().await;

    assert_eq!(sub.try_recv(), Some(event1), "应收到第一条事件");
    assert_eq!(sub.try_recv(), Some(event2), "应收到第二条事件");
    assert_eq!(sub.try_recv(), None, "缓冲区已空时应返回 None");
}

#[tokio::test]
async fn shutdown_stops_event_forwarding() {
    let bus = EventBus::new();
    let mut sub = EventSubscriber::new(bus.clone());

    let event = make_test_event("before-shutdown");
    bus.publish(event.clone()).await.unwrap();
    yield_to_subscriber().await;
    assert_eq!(sub.try_recv(), Some(event), "关闭前应能收到事件");

    // shutdown 消费 self,之后该订阅者即被类型系统回收,无法再接收事件。
    sub.shutdown().await;
}

#[tokio::test]
async fn lagged_events_are_handled_gracefully() {
    let bus = EventBus::new();
    let mut sub = EventSubscriber::new(bus.clone());

    // 用同步发布快速灌入远超 broadcast 容量的事件,制造 Lagged 场景。
    // publish_blocking 不 await,可在当前任务不放弃 CPU 的情况下尽可能快地
    // 填满 broadcast 缓冲区,使后台任务来不及消费。
    let overflow_count = 2048;
    for i in 0..overflow_count {
        bus.publish_blocking(make_test_event(format!("lag-{i}")))
            .unwrap();
    }

    yield_to_subscriber().await;

    // 订阅者应仍然存活,且能从缓冲区中拿到事件(具体数量因 lag 而异,
    // 但不应 panic,也不应无限阻塞)。
    let mut received = 0;
    while sub.try_recv().is_some() {
        received += 1;
    }

    assert!(
        received > 0,
        "lag 后订阅者应继续接收新事件, received={received}"
    );
    assert!(
        received <= 1024,
        "本地缓冲区容量 1024,不应超过该上限, received={received}"
    );
}

#[tokio::test]
async fn buffer_drops_oldest_events_on_overflow() {
    let bus = EventBus::new();
    let mut sub = EventSubscriber::new(bus.clone());

    // 发布 1500 条事件,让缓冲区先填满再覆盖旧事件。
    // 为避免 broadcast 本身丢事件,发布速度放慢,给后台任务时间消费。
    let total = 1500;
    for i in 0..total {
        bus.publish(make_test_event(format!("overflow-{i}")))
            .await
            .unwrap();
    }

    yield_to_subscriber().await;

    // 统计收到的事件数量,应被截断到 1024。
    let mut received = 0;
    while sub.try_recv().is_some() {
        received += 1;
    }

    assert_eq!(received, 1024, "缓冲区溢出时应只保留最新的 1024 条事件");
}
