//! `/compact` 策展闭环集成测试(FC-2 接线验收,ADR-081)
//!
//! # 覆盖链路
//! 1. 斜杠输入 → `DispatchPlan::Orchestrated` → 统一派发桥 →
//!    `TuiActionRequested{action_id:"compact"}` + 2s 超时兜底挂起;
//! 2. 参数透传(payload `{"args": ...}`,合法性由执行层权威校验);
//! 3. Plan 审批态拦截(压缩会话历史属变更型操作);
//! 4. `TuiChatHistoryReplaced` 经真实 DataPipeline 回写会话历史
//!    (ChatSync 唯一所有权设计的事件控制信道)。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use chimera_tui::{
    ApprovalMode, DataPipeline, DataSourceConfig, EventSubscriber, TuiApp, TuiConfig,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventBus, EventReceiver, NexusEvent, TuiChatMessagePayload};

/// 无修饰符按键
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 构造注入 EventBus 的 TuiApp 并返回订阅接收端
///
/// WHY subscribe 先于输入:bus.subscribe() 必须在事件产生前同步调用,
/// 否则 broadcast 不缓存历史消息会静默丢失(§4.4 #3 红线)。
fn app_with_bus() -> (TuiApp, EventReceiver) {
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let app = TuiApp::with_event_bus(
        TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .unwrap(),
        bus,
    );
    (app, rx)
}

/// 经斜杠模式输入命令并回车(黑盒按键驱动)
fn submit_slash(app: &mut TuiApp, input: &str) {
    app.handle_key_event(key(KeyCode::Char('/')));
    for c in input.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
    app.handle_key_event(key(KeyCode::Enter));
}

// ============================================================
// 1. /compact 无参 → 编排域派发
// ============================================================

#[test]
fn compact_slash_dispatches_orchestrated_action() {
    let (mut app, mut rx) = app_with_bus();

    submit_slash(&mut app, "compact");

    // 清空 submit 回环可能携带的其它事件,检索 TuiActionRequested
    let mut found = None;
    while let Ok(Some(ev)) = rx.try_recv() {
        if let NexusEvent::TuiActionRequested {
            action_id, payload, ..
        } = ev
        {
            found = Some((action_id, payload));
        }
    }
    let (action_id, payload) = found.expect("/compact 应发布 TuiActionRequested");
    assert_eq!(action_id, "compact");
    assert!(
        payload.contains("args"),
        "payload 应携带原始参数串,got: {payload}"
    );
    // 2s 超时兜底已挂起(编排域派发的代理断言,与 key_drift 范式一致)
    assert_eq!(
        app.state().pending_actions.len(),
        1,
        "编排域派发应挂起超时兜底"
    );
}

// ============================================================
// 2. /compact <policy> 参数透传
// ============================================================

#[test]
fn compact_with_policy_carries_args_in_payload() {
    let (mut app, mut rx) = app_with_bus();

    submit_slash(&mut app, "compact aggressive");

    let mut found = None;
    while let Ok(Some(ev)) = rx.try_recv() {
        if let NexusEvent::TuiActionRequested {
            action_id, payload, ..
        } = ev
        {
            found = Some((action_id, payload));
        }
    }
    let (_, payload) = found.expect("/compact aggressive 应发布 TuiActionRequested");
    assert_eq!(
        payload, r#"{"args":"aggressive"}"#,
        "参数应原样进 payload,由执行层权威校验"
    );
}

// ============================================================
// 3. Plan 审批态拦截(变更型操作受治理)
// ============================================================

#[test]
fn plan_approval_mode_blocks_compact() {
    let (mut app, mut rx) = app_with_bus();
    app.state_mut().approval_mode = ApprovalMode::Plan;

    submit_slash(&mut app, "compact");

    while let Ok(Some(ev)) = rx.try_recv() {
        assert!(
            !matches!(ev, NexusEvent::TuiActionRequested { action_id, .. } if action_id == "compact"),
            "Plan 审批态不得放行 /compact"
        );
    }
    let (msg, severity) = app
        .state()
        .status_message
        .clone()
        .expect("Plan 态拦截应有状态栏提示");
    assert_eq!(
        severity,
        chimera_tui::Severity::Warning,
        "拦截提示应为 Warning 级"
    );
    assert!(!msg.is_empty());
}

// ============================================================
// 4. TuiChatHistoryReplaced 经真实管道回写会话历史
// ============================================================

#[tokio::test]
async fn chat_history_replaced_event_updates_snapshot() {
    let bus = EventBus::new();
    let pipeline = std::sync::Arc::new(DataPipeline::new(
        EventSubscriber::new(bus.clone()),
        DataSourceConfig {
            tick_interval_ms: 25,
            ..Default::default()
        },
    ));

    // 先构造 3 条历史(2 轮对话)
    for i in 0..2 {
        bus.publish(NexusEvent::TuiChatSubmitted {
            metadata: event_bus::EventMetadata::new("test"),
            session_id: "s".into(),
            query: format!("question {i}"),
            slash_command: None,
        })
        .await
        .unwrap();
        bus.publish(NexusEvent::TuiChatResponseChunk {
            metadata: event_bus::EventMetadata::new("test"),
            session_id: "s".into(),
            delta: format!("answer {i}\n"),
            cursor_hint: 0,
        })
        .await
        .unwrap();
        bus.publish(NexusEvent::TuiChatCompleted {
            metadata: event_bus::EventMetadata::new("test"),
            session_id: "s".into(),
            tool_use: None,
        })
        .await
        .unwrap();
    }
    // 等待历史就绪(4 条)
    let deadline = Instant::now() + Duration::from_secs(3);
    while pipeline.snapshot().chat_messages.len() < 4 {
        assert!(Instant::now() < deadline, "等待历史就绪超时");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 模拟编排器策展完成:整史替换为 1 条摘要
    bus.publish(NexusEvent::TuiChatHistoryReplaced {
        metadata: event_bus::EventMetadata::new("orchestrator"),
        session_id: "s".into(),
        messages: vec![
            TuiChatMessagePayload {
                role: "user".into(),
                content: "question 0".into(),
            },
            TuiChatMessagePayload {
                role: "assistant".into(),
                content: "[上下文策展摘要] answer 1".into(),
            },
        ],
    })
    .await
    .unwrap();

    // 轮询快照:历史被替换为 2 条(唯一所有权 ChatSync 接受控制指令)
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snap = pipeline.snapshot();
        if snap.chat_messages.len() == 2
            && snap.chat_messages[1]
                .content
                .starts_with("[上下文策展摘要]")
        {
            break;
        }
        assert!(Instant::now() < deadline, "等待历史替换超时");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    pipeline.shutdown().await;
}
