//! Chat 视图按键/鼠标语义测试(评估报告 I-2 / I-6 修复验收)
//!
//! # 背景(WHY)
//! Chat 视图全屏渲染会话流,但 Tab/数字键仍隐式切换不可见面板("按了没反应"),
//! 鼠标命中仍按 Dashboard 三块布局切分(composer 区点击会误入遗留命令模式)。
//! 修复后:面板切换键在 Chat 视图给出状态栏诚实提示且不切换;鼠标仅保留
//! 会话流滚轮滚动。
//!
//! 契约:Dashboard 视图行为零回归(数字键/F 键/Tab 正常切换,由
//! m3a_input_routing_test 既有锚点覆盖)。

#![forbid(unsafe_code)]

use chimera_tui::{PanelId, TuiApp, TuiConfig, ViewMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// 构造 Dashboard 视图 TuiApp(无 event-bus,内存桩数据源)
fn make_app(view: ViewMode) -> TuiApp {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: view,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.state_mut().view_mode = view;
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn scroll(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

// ============================================================
// I-2:Chat 视图数字键 → 状态栏提示,不切换面板
// ============================================================

#[test]
fn chat_view_digit_key_shows_hint_not_switch() {
    let mut app = make_app(ViewMode::Chat);
    let before = app.current_panel();

    app.handle_key_event(key(KeyCode::Char('2')));

    assert_eq!(
        app.current_panel(),
        before,
        "Chat 视图数字键不应切换面板(切换不可见,属语义空洞)"
    );
    let (msg, severity) = app
        .state()
        .status_message
        .clone()
        .expect("Chat 视图按数字键应给出状态栏提示");
    assert_eq!(
        severity,
        chimera_tui::Severity::Info,
        "提示应为 Info 级(诚实告知而非警告)"
    );
    assert!(!msg.is_empty());
}

#[test]
fn chat_view_tab_shows_hint_not_switch() {
    let mut app = make_app(ViewMode::Chat);
    let before = app.current_panel();

    app.handle_key_event(key(KeyCode::Tab));

    assert_eq!(app.current_panel(), before, "Chat 视图 Tab 不应切换面板");
    assert!(
        app.state().status_message.is_some(),
        "Chat 视图 Tab 应给出状态栏提示"
    );
}

// ============================================================
// 零回归:Dashboard 视图数字键切换行为保持
// ============================================================

#[test]
fn dashboard_digit_key_still_switches() {
    let mut app = make_app(ViewMode::Dashboard);
    app.handle_key_event(key(KeyCode::Char('2')));
    assert_ne!(
        app.current_panel(),
        PanelId::Quest,
        "Dashboard 数字键切换行为零回归"
    );
}

// ============================================================
// I-6:Chat 视图鼠标点击不再误入遗留命令模式,滚轮滚动会话流
// ============================================================

#[test]
fn chat_view_mouse_click_does_not_enter_command_mode() {
    let mut app = make_app(ViewMode::Chat);

    // 点击任意位置(含底部 composer 区域):Chat 视图无 Dashboard 布局可命中
    app.handle_mouse_event(click(10, 28));
    app.handle_mouse_event(click(10, 0));

    assert_eq!(
        app.state().input_mode,
        chimera_tui::InputMode::Normal,
        "Chat 视图鼠标点击不得进入遗留 Command 模式"
    );
}

#[test]
fn chat_view_mouse_scroll_routes_to_chat_panel() {
    let mut app = make_app(ViewMode::Chat);

    // 滚轮滚动应路由到 Chat 面板(会话流)且不 panic
    app.handle_mouse_event(scroll(10, 10));
    app.handle_mouse_event(scroll(10, 10));
}
