//! M4 控制事件消费测试 — quest-engine 订阅并处理 TUI 控制请求

#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::{handle_control_event, QuestEngine};

fn make_intent(text: &str) -> UserIntent {
    UserIntent {
        intent_id: "i-1".into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

#[tokio::test]
async fn upstream_handler_consumes_pause_request_and_publishes_paused() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus.clone());

    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();
    // 消费 QuestCreated 事件
    let _ = rx.recv().await.unwrap();

    // 模拟 TUI 发布暂停请求
    let request = NexusEvent::QuestPauseRequested {
        metadata: EventMetadata::new("chimera-tui"),
        quest_id: quest.quest_id.clone(),
        requested_by: "operator".into(),
    };
    handle_control_event(&engine, request).await.unwrap();

    // 验证上游状态变更
    assert!(engine.is_paused(&quest.quest_id));

    // 验证状态变更事件被发布,可被 TUI 接收
    let state_event = rx.recv().await.unwrap();
    match state_event {
        NexusEvent::QuestPaused { quest_id, .. } => {
            assert_eq!(quest_id, quest.quest_id);
        }
        other => panic!("expected QuestPaused, got {other:?}"),
    }
}

#[tokio::test]
async fn upstream_handler_consumes_resume_request_and_publishes_resumed() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus.clone());

    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // QuestCreated

    engine
        .pause_quest(&quest.quest_id, "operator")
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // QuestPaused

    let request = NexusEvent::QuestResumeRequested {
        metadata: EventMetadata::new("chimera-tui"),
        quest_id: quest.quest_id.clone(),
        requested_by: "operator".into(),
    };
    handle_control_event(&engine, request).await.unwrap();

    assert!(!engine.is_paused(&quest.quest_id));

    let state_event = rx.recv().await.unwrap();
    assert!(matches!(state_event, NexusEvent::QuestResumed { .. }));
}

#[tokio::test]
async fn upstream_handler_ignores_unrelated_events() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let event = NexusEvent::CacheHit {
        metadata: EventMetadata::new("test"),
        cache_key: "k-1".into(),
    };
    handle_control_event(&engine, event).await.unwrap();
}
