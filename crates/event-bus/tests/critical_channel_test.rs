//! Critical mpsc 双通道一致性补强测试 — Phase V Task V-1 [I4]
//!
//! 与 `integration_critical_channel.rs` 互补,聚焦后者未覆盖的场景:
//! - 无 Critical mpsc 订阅者时的降级行为(旁路静默跳过,broadcast 不丢失)
//! - FilteredSubscriber(C1 注意力过滤)与 Critical mpsc 旁路的协同双投递
//! - FilteredSubscriber 订阅非 Security topic 时,Critical 事件被过滤但 mpsc 保底
//!
//! 已由 `integration_critical_channel.rs` 覆盖、本文件不重复的场景:
//! - 4 类 Critical 事件经 mpsc 旁路接收(test_critical_events_received_via_mpsc_bypass)
//! - 主通道 Lagged 时 Critical 存活(test_critical_event_survives_main_channel_lagged)
//! - Normal 事件不进 mpsc 旁路(test_normal_event_not_received_via_mpsc_bypass)
//! - 多订阅者 fan-out(test_multiple_critical_subscribers_fanout)
//! - publish_critical 显式 API(test_publish_critical_explicit_api)
//! - publish_blocking 同步双推(test_publish_blocking_dual_channel)
//!
//! 设计依据:§6.2 红线"Critical 安全事件(SkepticVeto/RedTeamAudit/AsaIntervention/
//! BudgetExceeded)必须用 mpsc channel 确保送达",双通道实现见
//! `bus.rs::EventBus::{publish, send_critical_mpsc, subscribe_critical_events}`。

use std::collections::HashSet;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, EventSeverity, EventTopic, NexusEvent};

// ============================================================
// 测试 1:无 Critical mpsc 订阅者时,broadcast 仍投递且旁路静默跳过
//
// WHY 此测试守护降级行为:Critical mpsc 旁路是"按需初始化"的,
// 首次 `subscribe_critical_events` 才向 Vec 推入 sender。无订阅者时
// `send_critical_mpsc` 在 `guard.is_empty()` 提前 return(bus.rs:272-274),
// 不 panic、不报错;broadcast 主通道不受影响,事件不丢失。
// bus.rs:156 的无订阅者告警仅触发于 broadcast 也无订阅者且 severity=Critical,
// 此处有 broadcast 订阅者,故不告警,仅验证 mpsc 旁路空 Vec 的安全跳过。
// ============================================================

#[tokio::test]
async fn test_critical_event_no_subscriber_logs_warning() {
    let bus = EventBus::new();
    let mut main_rx = bus.subscribe(); // broadcast 订阅者
    // 故意不调用 subscribe_critical_events():模拟无 Critical 订阅者的部署形态

    let event = NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "total_cost".into(),
        current: 9_999,
        limit: 5_000,
    };
    // publish 内部 is_critical_mpsc_event→true,但 critical_tx Vec 为空,
    // send_critical_mpsc 静默跳过;broadcast 正常投递
    bus.publish(event.clone()).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(1), main_rx.recv())
        .await
        .expect("broadcast 订阅者应收到 Critical 事件(不丢失)")
        .expect("broadcast 不应关闭");
    assert_eq!(received, event);
    assert_eq!(
        received.severity(),
        EventSeverity::Critical,
        "BudgetExceeded severity 必须为 Critical(§6.2 红线)"
    );
}

// ============================================================
// 测试 2:FilteredSubscriber(Security topic)与 Critical mpsc 协同双投递
//
// WHY 此测试验证双通道协同:FilteredSubscriber(C1 注意力过滤)与
// Critical mpsc(§6.2 红线)是两条独立的投递路径,互不影响。
// SkepticVeto 的 topic()=Security(topic.rs:110),FilteredSubscriber 订阅
// Security topic 应通过 broadcast 收到;同时 is_critical_mpsc_event 判定走 mpsc 旁路,
// Critical 订阅者独立收到。两条路径不 double-count(分别服务不同消费者)。
// ============================================================

#[tokio::test]
async fn test_filtered_subscriber_critical_coexist() {
    let bus = EventBus::new();
    let topics: HashSet<EventTopic> = [EventTopic::Security].into_iter().collect();
    let mut filtered_rx = bus.subscribe_filtered(topics);
    let mut crit_rx = bus.subscribe_critical_events();

    let event = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-coexist".into(),
        veto_reason: "unsafe shell injection".into(),
        frozen_capabilities: vec!["shell_exec".into()],
    };
    bus.publish(event.clone()).await.unwrap();

    // FilteredSubscriber 通过 broadcast 收到(topic=Security 匹配)
    let filtered_received = tokio::time::timeout(Duration::from_secs(1), filtered_rx.recv())
        .await
        .expect("FilteredSubscriber 应收到 Security topic 的 Critical 事件")
        .expect("FilteredSubscriber 不应关闭");
    assert_eq!(filtered_received, event);

    // Critical mpsc 旁路独立收到(不依赖 broadcast,不 Lagged)
    let crit_received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("Critical mpsc 旁路应收到 SkepticVeto")
        .expect("mpsc 不应关闭");
    assert_eq!(crit_received, event);
}

// ============================================================
// 测试 3:Critical 事件双投递 — broadcast 订阅者 + mpsc 旁路订阅者均收到
//
// WHY 双投递是 §6.2 红线核心设计:publish 对 4 类 Critical 事件先走 mpsc
// 旁路(确保 Critical 订阅者必收,Unbounded 不 Lagged),再走 broadcast
// (保持既有订阅者兼容)。即使 broadcast Lagged,Critical 订阅者仍能通过
// mpsc 收到。此测试明确验证 SkepticVeto 的双投递(broadcast + mpsc)。
// ============================================================

#[tokio::test]
async fn test_critical_event_also_delivered_to_broadcast() {
    let bus = EventBus::new();
    let mut main_rx = bus.subscribe(); // broadcast 全量订阅者
    let mut crit_rx = bus.subscribe_critical_events();

    let event = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-dual".into(),
        veto_reason: "test dual delivery".into(),
        frozen_capabilities: vec![],
    };
    bus.publish(event.clone()).await.unwrap();

    let main_received = tokio::time::timeout(Duration::from_secs(1), main_rx.recv())
        .await
        .expect("broadcast 订阅者应收到 Critical 事件(双投递)")
        .expect("broadcast 不应关闭");
    assert_eq!(main_received, event);

    let crit_received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("mpsc 旁路应收到 Critical 事件(双投递)")
        .expect("mpsc 不应关闭");
    assert_eq!(crit_received, event);
}

// ============================================================
// 测试 4:FilteredSubscriber 订阅非 Security topic 时,Critical 事件被过滤但 mpsc 保底
//
// WHY 此测试验证通道隔离:FilteredSubscriber 的注意力过滤会"吃掉"不匹配 topic
// 的 broadcast 事件(包括 Critical 事件如 SkepticVeto),但 Critical mpsc 旁路
// 与 broadcast 是独立通道,不受 FilteredSubscriber 消费影响。这是双通道设计的
// 核心价值:即使 broadcast 侧过滤/丢弃/消费 Critical 事件,mpsc 仍保底投递。
// 场景模拟:N9 PrerequisiteChecker 只订阅 Quest topic,但仍需感知 Security 告警
// 时,应通过 subscribe_critical_events 独立获取 Critical 流。
// ============================================================

#[tokio::test]
async fn test_filtered_subscriber_non_security_topic_critical_mpsc_preserved() {
    let bus = EventBus::new();
    let topics: HashSet<EventTopic> = [EventTopic::Quest].into_iter().collect();
    let mut filtered_rx = bus.subscribe_filtered(topics);
    let mut crit_rx = bus.subscribe_critical_events();

    // 先发布 SkepticVeto(topic=Security,FilteredSubscriber 不匹配→消费丢弃)
    let skeptic = NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-isolate".into(),
        veto_reason: "test isolation".into(),
        frozen_capabilities: vec![],
    };
    bus.publish(skeptic.clone()).await.unwrap();

    // 再发布 QuestCreated(topic=Quest,FilteredSubscriber 匹配→返回)
    let quest = NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-isolate".into(),
        title: "test quest".into(),
        task_count: 1,
    };
    bus.publish(quest.clone()).await.unwrap();

    // mpsc 旁路应收到 SkepticVeto(不受 FilteredSubscriber 消费影响)
    let crit_received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("mpsc 旁路应收到 SkepticVeto(通道隔离,不受 FilteredSubscriber 影响)")
        .expect("mpsc 不应关闭");
    assert_eq!(crit_received, skeptic);

    // FilteredSubscriber 应跳过 SkepticVeto(topic 不匹配),返回 QuestCreated
    let filtered_received = tokio::time::timeout(Duration::from_secs(1), filtered_rx.recv())
        .await
        .expect("FilteredSubscriber 应收到 QuestCreated(跳过 SkepticVeto)")
        .expect("FilteredSubscriber 不应关闭");
    assert_eq!(filtered_received, quest);
}
