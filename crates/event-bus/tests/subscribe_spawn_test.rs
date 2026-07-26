//! P1-W4.2 broadcast 红线结构化测试 — 验证 subscribe-then-spawn 原子化 API
//!
//! 对应文档:
//! - tasks.md P1-W4.2(SubTask P1-W4.2.1 / P1-W4.2.2)
//! - §6.2 红线:broadcast 先 subscribe 再 spawn
//! - §4.4 反模式 3:bus.subscribe() 必须在 tokio::spawn() 之前同步调用
//!
//! # 测试矩阵
//! 1. 正常流程:subscribe → spawn 编译通过且运行正确
//! 2. 编译期拒绝:未 subscribe 直接 spawn(doctest compile_fail,见 subscriber.rs)
//! 3. 通过新 API 订阅后能收到事件
//! 4. Critical 事件原子化 API
//! 5. 向后兼容:旧 API 仍可用
//! 6. into_receiver 提取 Receiver 自行处理
//!
//! # 编译失败测试说明
//! 测试 2"未 subscribe 直接 spawn 编译失败"通过 `subscriber.rs` 中
//! `SubscriberBuilder` 和 `CriticalSubscriberBuilder` 文档注释的
//! `compile_fail` doctest 实现。`cargo test --doc` 会验证这些 doctest
//! 确实编译失败(若未来误加 spawn 方法到 Unsubscribed,doctest 会转 GREEN 失败)。
//! 此处不再重复,改为运行时验证 TypeState 转换正确性。

use event_bus::{EventBus, EventMetadata, NexusEvent};
use std::time::Duration;

// ============================================================
// 测试辅助:构造各类事件
// ============================================================

/// 构造一个普通事件(QuestCreated)
fn make_test_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test-source"),
        quest_id: "q-001".into(),
        title: "测试任务".into(),
        task_count: 1,
    }
}

/// 构造一个 Critical 安全告警事件(SkepticVeto,走 mpsc 旁路)
fn make_critical_event() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("test-source"),
        quest_id: "q-001".into(),
        veto_reason: "unsafe op".into(),
        frozen_capabilities: vec!["cap-1".into()],
    }
}

// ============================================================
// 测试 1:正常流程 subscribe → spawn
// ============================================================

#[tokio::test]
async fn test_subscribe_then_spawn_works() {
    let bus = EventBus::new();

    // 1. 创建构建器(Unsubscribed 状态)
    let builder = bus.subscriber();
    // 2. 订阅(同步,返回 Subscribed 状态)
    let builder = builder.subscribe();
    // 3. spawn(仅 Subscribed 状态可调用)
    let handle = builder.spawn(|mut rx| async move { rx.recv().await });

    // 发布事件
    bus.publish(make_test_event()).await.unwrap();

    // 等待 spawn 的任务完成
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("spawn 任务应在 1s 内完成")
        .expect("spawn 任务不应 panic");
    assert!(result.is_ok(), "应成功收到事件");
}

// ============================================================
// 测试 2:TypeState 转换正确性(运行时验证编译期约束)
//
// 此测试验证 subscribe() 返回的 builder 确实处于 Subscribed 状态
// (即 spawn 方法可调用)。编译期拒绝由 doctest compile_fail 保证。
// ============================================================

#[tokio::test]
async fn test_typestate_transition_correct() {
    let bus = EventBus::new();

    // subscriber() 返回 Unsubscribed 状态
    let unsubscribed = bus.subscriber();
    // subscribe() 消费 Unsubscribed,返回 Subscribed
    let subscribed = unsubscribed.subscribe();
    // spawn() 仅在 Subscribed 状态可调用 — 若 TypeState 失效此处编译失败
    let handle = subscribed.spawn(|mut rx| async move {
        let event = rx.recv().await;
        event
    });

    bus.publish(make_test_event()).await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("任务应在 1s 内完成")
        .expect("任务不应 panic");
    assert!(event.is_ok(), "应通过 TypeState API 收到事件");
}

// ============================================================
// 测试 3:通过新 API 订阅后能收到多个事件
// ============================================================

#[tokio::test]
async fn test_subscriber_receives_events() {
    let bus = EventBus::new();

    // 使用新 API 订阅
    let handle = bus.subscriber().subscribe().spawn(|mut rx| async move {
        let mut events = Vec::new();
        for _ in 0..3 {
            if let Ok(event) = rx.recv().await {
                events.push(event);
            }
        }
        events
    });

    // 发布 3 个事件
    for i in 0..3u32 {
        bus.publish(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("test-source"),
            quest_id: format!("q-{i}"),
            completed: i,
            total: 3,
        })
        .await
        .unwrap();
    }

    // 等待并验证
    let events = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("任务应在 2s 内完成")
        .expect("任务不应 panic");
    assert_eq!(events.len(), 3, "应收到 3 个事件");

    // 验证事件顺序与 quest_id
    for (i, event) in events.iter().enumerate() {
        match event {
            NexusEvent::QuestProgressUpdated { quest_id, .. } => {
                assert_eq!(*quest_id, format!("q-{i}"), "事件顺序应与发布顺序一致");
            }
            _ => panic!("期望 QuestProgressUpdated 事件"),
        }
    }
}

// ============================================================
// 测试 4:Critical 事件原子化 API
// ============================================================

#[tokio::test]
async fn test_critical_subscriber_api() {
    let bus = EventBus::new();

    // 使用新 API 订阅 Critical 事件
    let handle = bus
        .critical_subscriber()
        .subscribe()
        .spawn(|mut rx| async move { rx.recv().await });

    // 发布 Critical 事件(SkepticVeto 走 mpsc 旁路)
    bus.publish_critical(make_critical_event()).await.unwrap();

    // 等待并验证
    let result = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("任务应在 1s 内完成")
        .expect("任务不应 panic");
    assert!(result.is_some(), "应通过 mpsc 旁路收到 Critical 事件");

    // 验证事件类型确实是 SkepticVeto
    match result {
        Some(NexusEvent::SkepticVeto { quest_id, .. }) => {
            assert_eq!(quest_id, "q-001");
        }
        Some(other) => panic!("期望 SkepticVeto 事件,实际收到 {:?}", other.type_name()),
        None => panic!("不应为 None"),
    }
}

// ============================================================
// 测试 5:向后兼容 — 旧 API 仍可用
// ============================================================

#[tokio::test]
async fn test_backward_compat_subscribe_still_works() {
    let bus = EventBus::new();

    // 旧 API:直接 subscribe(返回 EventReceiver)
    let mut rx = bus.subscribe();
    let event = make_test_event();
    bus.publish(event.clone()).await.unwrap();
    let received = rx.recv().await.unwrap();
    assert_eq!(received, event, "旧 subscribe API 应仍能正常工作");

    // 旧 API:subscribe_critical_events(返回 mpsc::Receiver)
    let mut crit_rx = bus.subscribe_critical_events();
    let crit_event = make_critical_event();
    bus.publish_critical(crit_event.clone()).await.unwrap();
    let crit_received = crit_rx.recv().await;
    assert!(
        crit_received.is_some(),
        "旧 subscribe_critical_events API 应仍能正常工作"
    );
    assert_eq!(
        crit_received.unwrap(),
        crit_event,
        "旧 Critical API 收到的事件应与发布的一致"
    );
}

// ============================================================
// 测试 6:into_receiver 提取 Receiver 自行处理
// ============================================================

#[tokio::test]
async fn test_into_receiver_extraction() {
    let bus = EventBus::new();

    // 使用新 API 订阅,但提取 Receiver 自己处理(不通过 spawn)
    let mut rx = bus.subscriber().subscribe().into_receiver();

    let event = make_test_event();
    bus.publish(event.clone()).await.unwrap();
    let received = rx.recv().await.unwrap();
    assert_eq!(
        received, event,
        "into_receiver 提取的 Receiver 应能收到事件"
    );
}

// ============================================================
// 测试 7:Critical into_receiver 提取
// ============================================================

#[tokio::test]
async fn test_critical_into_receiver_extraction() {
    let bus = EventBus::new();

    // 使用新 API 订阅 Critical 事件,提取 mpsc::Receiver 自己处理
    let mut rx = bus.critical_subscriber().subscribe().into_receiver();

    let event = make_critical_event();
    bus.publish_critical(event.clone()).await.unwrap();
    let received = rx.recv().await;
    assert!(
        received.is_some(),
        "应通过 into_receiver 收到 Critical 事件"
    );
    assert_eq!(received.unwrap(), event, "收到的事件应与发布的一致");
}

// ============================================================
// 测试 8:订阅在 spawn 之前完成 — 事件不丢失
//
// WHY: 这是 P1-W4.2 的核心验证 — 通过新 API,subscribe 同步完成后才 spawn,
// 确保订阅者不会错过订阅时刻之后发布的任何事件。
// 对比旧模式(先 spawn 再 subscribe):可能丢失 spawn 和 subscribe 之间发布的事件。
// ============================================================

#[tokio::test]
async fn test_subscribe_before_spawn_no_event_loss() {
    let bus = EventBus::new();

    // 新 API 保证:subscribe() 同步完成,然后 spawn()
    let handle = bus
        .subscriber()
        .subscribe() // 同步订阅,此刻起的事件都会被接收
        .spawn(|mut rx| async move {
            let mut count = 0u32;
            // 接收 5 个事件后返回
            while let Ok(_event) = rx.recv().await {
                count += 1;
                if count >= 5 {
                    break;
                }
            }
            count
        });

    // 立即发布 5 个事件(spawn 已完成订阅,不会丢失)
    for i in 0..5u32 {
        bus.publish(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("test-source"),
            quest_id: format!("q-{i}"),
            completed: i,
            total: 5,
        })
        .await
        .unwrap();
    }

    let count = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("任务应在 2s 内完成")
        .expect("任务不应 panic");
    assert_eq!(count, 5, "订阅在 spawn 之前完成,5 个事件应全部收到");
}
