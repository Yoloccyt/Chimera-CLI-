//! M4 双向控制端到端测试 — TUI 命令输入 → EventBus → 上游消费

#![forbid(unsafe_code)]

use chimera_tui::{PopupKind, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventBus, NexusEvent, VoteValue};

/// 构造一个处于命令模式且已输入文本的 TuiApp
fn app_with_command_input(input: &str) -> TuiApp {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in input.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app
}

/// 将 EventBus 注入 TUI 并订阅返回的接收者
fn app_with_bus(app: TuiApp) -> (TuiApp, EventBus, event_bus::EventReceiver) {
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let app = TuiApp::with_event_bus(app, bus.clone());
    (app, bus, rx)
}

#[test]
fn bidirectional_refresh_publishes_event() {
    let app = app_with_command_input("refresh");
    let (mut app, _bus, mut rx) = app_with_bus(app);

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("should receive event").unwrap();
    assert!(
        matches!(event, NexusEvent::RefreshStateRequested { .. }),
        "expected RefreshStateRequested, got {event:?}"
    );
    assert!(
        app.state()
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("RefreshStateRequested request published"),
        "status should confirm publish"
    );
}

#[test]
fn bidirectional_pause_shows_confirm_then_publishes_on_yes() {
    let app = app_with_command_input("pause q-42");
    let (mut app, _bus, mut rx) = app_with_bus(app);

    // 提交命令应弹出确认框
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.state().popup_stack.is_empty());
    match app.state().popup_stack.current() {
        Some(PopupKind::Confirm { prompt, .. }) => {
            assert!(prompt.contains("Pause quest"));
            assert!(prompt.contains("q-42"));
        }
        other => panic!("expected Confirm popup, got {other:?}"),
    }

    // 默认选中 Yes,Enter 确认后发布事件
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.state().popup_stack.is_empty());

    let event = rx.try_recv().expect("should receive event").unwrap();
    match event {
        NexusEvent::QuestPauseRequested {
            quest_id,
            requested_by,
            ..
        } => {
            assert_eq!(quest_id, "q-42");
            assert_eq!(requested_by, "operator");
        }
        other => panic!("expected QuestPauseRequested, got {other:?}"),
    }
}

#[test]
fn bidirectional_pause_cancel_does_not_publish() {
    let app = app_with_command_input("pause q-42");
    let (mut app, _bus, mut rx) = app_with_bus(app);

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // 切换到 No
    app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    // 确认取消
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.state().popup_stack.is_empty());
    assert_eq!(
        rx.try_recv().unwrap(),
        None,
        "cancel should not publish any event"
    );
}

#[test]
fn bidirectional_vote_publishes_event() {
    let app = app_with_command_input("vote abstain p-7");
    let (mut app, _bus, mut rx) = app_with_bus(app);

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("should receive event").unwrap();
    match event {
        NexusEvent::VoteCastRequested {
            proposal_id,
            voter,
            vote,
            ..
        } => {
            assert_eq!(proposal_id, "p-7");
            assert_eq!(voter, "operator");
            assert_eq!(vote, VoteValue::Abstain);
        }
        other => panic!("expected VoteCastRequested, got {other:?}"),
    }
}

#[test]
fn bidirectional_event_bus_missing_reports_error() {
    // 不注入 EventBus,直接提交 refresh
    let mut app = app_with_command_input("refresh");
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(
        app.state()
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("event bus not available"),
        "status should report missing event bus"
    );
    assert_eq!(
        app.state().status_message.as_ref().unwrap().1,
        chimera_tui::Severity::Error
    );
}

#[test]
fn bidirectional_resume_publishes_event() {
    let app = app_with_command_input("resume q-9");
    let (mut app, _bus, mut rx) = app_with_bus(app);

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("should receive event").unwrap();
    match event {
        NexusEvent::QuestResumeRequested { quest_id, .. } => {
            assert_eq!(quest_id, "q-9");
        }
        other => panic!("expected QuestResumeRequested, got {other:?}"),
    }
}
