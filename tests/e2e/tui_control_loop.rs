//! E2E 测试 — TUI 双向控制闭环验证
//!
//! 验证 TUI→EventBus→quest-engine→EventBus→TUI 完整链路:
//! 1. TUI 发布控制请求 → quest-engine 消费并执行
//! 2. quest-engine 发布反馈 → TUI DataPipeline(QuestSync)消费并更新状态
//!
//! # 架构层
//! L10 TUI → L1 EventBus → L9 QuestEngine → L1 EventBus → L10 TUI(QuestSync)
//!
//! # 测试策略
//! 由于 `TuiApp::run` 阻塞终端,E2E 测试不启动真实 TUI,而是:
//! 1. 构造 EventBus + QuestEngine(Arc 共享)
//! 2. 启动 quest-engine 的 `spawn_control_subscriber`(后台消费控制请求)
//! 3. 通过 EventBus 发布控制请求事件(模拟 TUI 发布)
//! 4. 等待 quest-engine 处理并发布反馈事件(通过另一个 subscriber 接收)
//! 5. 验证 QuestSync 正确消费反馈事件(构造 QuestSync,调用 apply_event)
//!
//! # 架构红线对齐
//! - `#![forbid(unsafe_code)]`:测试代码也遵守,不引入 unsafe
//! - §4.4 反模式 #3:`bus.subscribe()` 必须在 `tokio::spawn()` 之前同步调用,
//!   `spawn_control_subscriber` 内部已遵循此规则;测试中的 feedback_rx 也在
//!   publish 之前 subscribe,避免事件静默丢失

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use chimera_tui::data::QuestSync;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::{spawn_control_subscriber, QuestEngine};

/// 构造测试用 UserIntent — 单句子对应单个 Task,最小化测试输入
fn make_intent(intent_id: &str, text: &str) -> UserIntent {
    UserIntent {
        intent_id: intent_id.into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 30,
    }
}

/// 等待特定事件出现,超时返回 false
///
/// WHY 非阻塞轮询:control_subscriber 是后台 spawn 的任务,需要给它处理时间;
/// 用 50ms 超时轮询避免永久阻塞,2 秒总超时防止测试挂起。
/// 匹配到目标事件立即返回 true,非目标事件继续轮询(broadcast 下所有
/// subscriber 都会收到所有事件,需要用 predicate 过滤)。
async fn wait_for_event(
    rx: &mut event_bus::EventReceiver,
    predicate: impl Fn(&NexusEvent) -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)).await {
            Ok(event) => {
                if predicate(&event) {
                    return true;
                }
            }
            // 超时或 channel 关闭:继续轮询直到总 deadline
            Err(_) => continue,
        }
    }
    false
}

// ============================================================
// 测试 1:Quest 取消闭环
//
// 验证完整链路:
//   TUI 发布 QuestCancelRequested
//   → quest-engine(control_subscriber)消费 → cancel_quest 执行
//   → quest-engine 发布 QuestCancelled
//   → TUI(QuestSync)消费 → Quest 从列表移除
// ============================================================

#[tokio::test]
async fn test_e2e_quest_cancel_loop() {
    // 1. 构造 EventBus + QuestEngine(Arc 共享,供 control_subscriber 持有)
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));

    // 2. 创建 Quest(create_quest 会发布 QuestCreated 事件,此时 control_subscriber
    //    尚未启动,该事件不会干扰后续控制闭环)
    let quest = engine
        .create_quest(make_intent("i-cancel-1", "分析需求。"))
        .await
        .expect("Quest 创建失败");

    // 3. 启动 control_subscriber(后台消费 QuestCancelRequested 等控制请求)
    //    spawn_control_subscriber 内部先 subscribe 再 spawn,遵循 §4.4 反模式 #3
    let _handle = spawn_control_subscriber(Arc::clone(&engine), bus.clone());

    // 4. 订阅反馈事件(在 publish 之前 subscribe,避免事件静默丢失)
    let mut feedback_rx = bus.subscribe();

    // 5. 发布 QuestCancelRequested(模拟 TUI 发布控制请求)
    bus.publish(NexusEvent::QuestCancelRequested {
        metadata: EventMetadata::new("chimera-tui"),
        quest_id: quest.quest_id.clone(),
        requested_by: "e2e-test".into(),
    })
    .await
    .expect("发布 QuestCancelRequested 失败");

    // 6. 等待 QuestCancelled 反馈事件(quest-engine 处理后发布)
    let received = wait_for_event(&mut feedback_rx, |e| {
        matches!(e, NexusEvent::QuestCancelled { .. })
    })
    .await;
    assert!(
        received,
        "应在 2 秒内收到 QuestCancelled 反馈事件,quest-engine 应处理取消请求并发布反馈"
    );

    // 7. 验证 quest-engine 已从注册表移除 Quest(控制请求已生效)
    assert!(
        engine.get_quest(&quest.quest_id).is_none(),
        "quest-engine 应已从注册表移除被取消的 Quest"
    );

    // 8. 验证 TUI 侧 QuestSync 正确消费 QuestCancelled 反馈事件
    //    WHY 先用 QuestListUpdated 初始化 QuestSync 列表:模拟 TUI 冷启动
    //    或周期同步场景,QuestSync 需先持有 Quest 才能验证移除语义
    let mut quest_sync = QuestSync::new();
    quest_sync.apply_event(&NexusEvent::QuestListUpdated {
        metadata: EventMetadata::new("quest-engine"),
        quests: vec![quest.clone()],
        source: "quest-engine".into(),
    });
    assert_eq!(
        quest_sync.quests().len(),
        1,
        "QuestSync 初始化后应持有 1 个 Quest"
    );

    // 消费 QuestCancelled 事件(QuestSync 应移除对应 Quest)
    let updated = quest_sync.apply_event(&NexusEvent::QuestCancelled {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: quest.quest_id.clone(),
        requested_by: "e2e-test".into(),
    });
    assert!(
        updated.is_some(),
        "QuestSync 消费 QuestCancelled 应返回更新后的列表"
    );
    assert!(
        quest_sync.quests().is_empty(),
        "QuestSync 消费 QuestCancelled 后 Quest 列表应为空(双向闭环完成)"
    );
}

// ============================================================
// 测试 2:Quest 优先级调整闭环
//
// 验证完整链路:
//   TUI 发布 QuestPriorityChanged
//   → quest-engine(control_subscriber)消费 → adjust_priority 执行
//   → quest-engine 发布 QuestPriorityAdjusted
//   → TUI(QuestSync)消费 → priority 字段更新
// ============================================================

#[tokio::test]
async fn test_e2e_quest_priority_loop() {
    // 1. 构造 EventBus + QuestEngine
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));

    // 2. 创建 Quest(默认优先级 128)
    let quest = engine
        .create_quest(make_intent("i-priority-1", "分析需求。"))
        .await
        .expect("Quest 创建失败");
    assert_eq!(quest.priority, 128, "新 Quest 默认优先级应为 128");

    // 3. 启动 control_subscriber
    let _handle = spawn_control_subscriber(Arc::clone(&engine), bus.clone());

    // 4. 订阅反馈事件
    let mut feedback_rx = bus.subscribe();

    // 5. 发布 QuestPriorityChanged(模拟 TUI 发布优先级调整请求)
    let new_priority: u8 = 200;
    bus.publish(NexusEvent::QuestPriorityChanged {
        metadata: EventMetadata::new("chimera-tui"),
        quest_id: quest.quest_id.clone(),
        new_priority,
        requested_by: "e2e-test".into(),
    })
    .await
    .expect("发布 QuestPriorityChanged 失败");

    // 6. 等待 QuestPriorityAdjusted 反馈事件
    //    WHY predicate 匹配 new_priority:确保收到的反馈携带正确的优先级值
    let received = wait_for_event(&mut feedback_rx, |e| {
        matches!(
            e,
            NexusEvent::QuestPriorityAdjusted {
                new_priority: 200,
                ..
            }
        )
    })
    .await;
    assert!(
        received,
        "应在 2 秒内收到 QuestPriorityAdjusted(new_priority=200)反馈事件"
    );

    // 7. 验证 quest-engine 已更新 priority(控制请求已生效)
    let stored = engine
        .get_quest(&quest.quest_id)
        .expect("adjust_priority 不应移除 Quest");
    assert_eq!(
        stored.priority, new_priority,
        "quest-engine 中 Quest 优先级应更新为 {new_priority}"
    );

    // 8. 验证 TUI 侧 QuestSync 正确消费 QuestPriorityAdjusted 反馈事件
    let mut quest_sync = QuestSync::new();
    quest_sync.apply_event(&NexusEvent::QuestListUpdated {
        metadata: EventMetadata::new("quest-engine"),
        quests: vec![quest.clone()],
        source: "quest-engine".into(),
    });
    assert_eq!(
        quest_sync.quests()[0].priority,
        128,
        "QuestSync 初始优先级应为 128(从 QuestListUpdated 加载)"
    );

    // 消费 QuestPriorityAdjusted 事件(QuestSync 应更新 priority 字段)
    let updated = quest_sync.apply_event(&NexusEvent::QuestPriorityAdjusted {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: quest.quest_id.clone(),
        new_priority,
        requested_by: "e2e-test".into(),
    });
    assert!(
        updated.is_some(),
        "QuestSync 消费 QuestPriorityAdjusted 应返回更新后的列表"
    );
    let quests = quest_sync.quests();
    assert_eq!(quests.len(), 1, "Quest 列表应仍持有 1 个 Quest(不移除)");
    assert_eq!(
        quests[0].priority, new_priority,
        "QuestSync 消费后优先级应更新为 {new_priority}(双向闭环完成)"
    );
}
