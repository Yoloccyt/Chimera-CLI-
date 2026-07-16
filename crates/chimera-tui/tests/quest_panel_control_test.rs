//! QuestPanel 控制命令测试 — 取消与优先级调整(M4 双向控制扩展)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试范围
//! - `d` 键:取消选中 Quest(弹出确认弹窗,二次确认后发布 QuestCancelRequested)
//! - `+` 键:优先级 +1(直接发布 QuestPriorityChanged,上限 255)
//! - `-` 键:优先级 -1(直接发布 QuestPriorityChanged,下限 0)
//! - 确认弹窗 Enter:确认取消(默认选中 Yes)
//! - 边界保护:priority=0 时 `-` 不发布;priority=255 时 `+` 不发布
//!
//! # 测试策略(WHY)
//! - 通过 EventBus + subscribe + try_recv 验证 TuiApp 真正发布了控制事件,
//!   而非仅验证 TuiCommand 返回值(端到端验证事件发布闭环)
//! - 使用同步 `try_recv` 而非 async `recv`,避免引入 tokio test 依赖
//!   (chimera-tui dev-dependencies 不含 tokio test features)
//! - 遵循 §4.4 反模式 #3:`bus.subscribe()` 必须在 `TuiApp::with_event_bus`
//!   之前同步调用,确保不会错过后续发布的控制事件
//! - `d` 键从原 detail 功能迁移到 cancel,detail 改用 `i` 键(info)

use chimera_tui::{PopupKind, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::NexusEvent;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

/// 构造单个 Quest(指定 ID 与优先级)
fn make_quest(id: &str, priority: u8) -> Quest {
    Quest {
        quest_id: id.into(),
        title: format!("Quest {id}"),
        tasks: vec![Task {
            task_id: format!("{id}-t1"),
            description: "test task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority,
    }
}

/// 构造带单个 Quest + EventBus 的 TuiApp,返回订阅者用于验证事件发布
///
/// WHY 返回 EventReceiver:测试通过 `try_recv` 验证事件真正发布到 EventBus
/// WHY subscribe 在 with_event_bus 之前:§4.4 反模式 #3,subscribe 必须在
///   任何 publish 之前同步调用,否则事件会静默丢失
fn make_app_with_quest(quest_id: &str, priority: u8) -> (TuiApp, event_bus::EventReceiver) {
    let bus = event_bus::EventBus::new();
    // 先订阅,再注入 bus 到 app,确保后续 publish 的事件都能被接收
    let rx = bus.subscribe();

    let app = TuiApp::new(TuiConfig::default()).expect("TuiApp with default config should succeed");
    let mut app = TuiApp::with_event_bus(app, bus);
    app.state_mut().quest_list = vec![make_quest(quest_id, priority)];

    (app, rx)
}

/// 构造无 Quest 的 TuiApp + EventBus(测试边界:无选中项时的按键行为)
fn make_app_empty() -> (TuiApp, event_bus::EventReceiver) {
    let bus = event_bus::EventBus::new();
    let rx = bus.subscribe();
    let app = TuiApp::new(TuiConfig::default()).expect("TuiApp with default config should succeed");
    let app = TuiApp::with_event_bus(app, bus);
    (app, rx)
}

// ============================================================
// d 键 — 取消 Quest(弹出确认弹窗)
// ============================================================

#[test]
fn test_press_d_shows_confirm_popup() {
    let (mut app, _rx) = make_app_with_quest("q1", 128);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    // d 键应弹出 Confirm 弹窗(破坏性操作需二次确认,与 pause/resume 一致)
    assert!(
        !app.state().popup_stack.is_empty(),
        "d should show confirm popup for cancel action"
    );
    match app.state().popup_stack.current() {
        Some(PopupKind::Confirm {
            prompt, on_confirm, ..
        }) => {
            assert!(
                prompt.contains("Cancel"),
                "prompt should mention Cancel, got: {prompt}"
            );
            assert!(
                on_confirm.starts_with("cancel:"),
                "on_confirm should use 'cancel:' prefix for apply_confirm_command dispatch, got: {on_confirm}"
            );
        }
        other => panic!(
            "expected Confirm popup, got {:?}",
            other.map(|p| format!("{p:?}"))
        ),
    }
}

#[test]
fn test_press_d_no_quest_does_nothing() {
    // 无 Quest 时按 d 不应弹窗,避免操作员误触后卡在无意义弹窗
    let (mut app, _rx) = make_app_empty();

    app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert!(
        app.state().popup_stack.is_empty(),
        "d with no quest should not show popup"
    );
}

// ============================================================
// 确认弹窗 — Enter 确认取消,发布 QuestCancelRequested
// ============================================================

#[test]
fn test_confirm_cancel_publishes_event() {
    let (mut app, mut rx) = make_app_with_quest("q1", 128);

    // 步骤 1:d 弹出确认弹窗(control_confirm 默认选中 Yes)
    app.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(!app.state().popup_stack.is_empty());

    // 步骤 2:Enter 确认(默认 Yes,直接发布取消请求)
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // 验证发布了 QuestCancelRequested 事件
    let event = rx
        .try_recv()
        .expect("try_recv should succeed (channel open)");
    match event {
        Some(NexusEvent::QuestCancelRequested { quest_id, .. }) => {
            assert_eq!(quest_id, "q1");
        }
        other => panic!(
            "expected QuestCancelRequested, got {:?}",
            other.map(|e| format!("{e:?}"))
        ),
    }
}

// ============================================================
// + 键 — 优先级 +1(直接发布,非破坏性操作无需确认)
// ============================================================

#[test]
fn test_press_plus_increments_priority() {
    let (mut app, mut rx) = make_app_with_quest("q1", 128);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));

    let event = rx
        .try_recv()
        .expect("try_recv should succeed (channel open)");
    match event {
        Some(NexusEvent::QuestPriorityChanged {
            quest_id,
            new_priority,
            ..
        }) => {
            assert_eq!(quest_id, "q1");
            assert_eq!(new_priority, 129, "128 + 1 = 129");
        }
        other => panic!(
            "expected QuestPriorityChanged(129), got {:?}",
            other.map(|e| format!("{e:?}"))
        ),
    }
}

#[test]
fn test_plus_at_255_does_nothing() {
    // 边界保护:priority=255 时 + 不发布事件(上限)
    let (mut app, mut rx) = make_app_with_quest("q1", 255);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE));

    let event = rx
        .try_recv()
        .expect("try_recv should succeed (channel open)");
    assert!(
        event.is_none(),
        "+ at 255 (max) should not publish event (boundary protection)"
    );
}

// ============================================================
// - 键 — 优先级 -1(直接发布,非破坏性操作无需确认)
// ============================================================

#[test]
fn test_press_minus_decrements_priority() {
    let (mut app, mut rx) = make_app_with_quest("q1", 128);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));

    let event = rx
        .try_recv()
        .expect("try_recv should succeed (channel open)");
    match event {
        Some(NexusEvent::QuestPriorityChanged {
            quest_id,
            new_priority,
            ..
        }) => {
            assert_eq!(quest_id, "q1");
            assert_eq!(new_priority, 127, "128 - 1 = 127");
        }
        other => panic!(
            "expected QuestPriorityChanged(127), got {:?}",
            other.map(|e| format!("{e:?}"))
        ),
    }
}

#[test]
fn test_minus_at_zero_does_nothing() {
    // 边界保护:priority=0 时 - 不发布事件(下限)
    let (mut app, mut rx) = make_app_with_quest("q1", 0);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE));

    let event = rx
        .try_recv()
        .expect("try_recv should succeed (channel open)");
    assert!(
        event.is_none(),
        "- at 0 (min) should not publish event (boundary protection)"
    );
}
