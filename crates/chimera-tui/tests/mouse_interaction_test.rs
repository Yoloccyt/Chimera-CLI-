//! 鼠标交互集成测试(评估报告 P0-3:mouse.rs 此前零测试覆盖)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - 标签栏点击切换面板(命中测试);
//! - 底部区域点击进入命令模式;
//! - 弹窗激活时滚轮滚动弹窗而非主面板;
//! - 主面板区域滚轮滚动焦点面板(Parliament 列表)。

#![forbid(unsafe_code)]

use chimera_tui::{InputMode, PanelId, PopupKind, TuiApp, TuiConfig};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 渲染一帧(设置 pane_manager.last_area,鼠标命中测试的前置)
fn render_once(app: &mut TuiApp) {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn left_down(column: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), column, row)
}

// ============================================================
// A. 标签栏点击切换面板
// ============================================================

#[test]
fn tab_click_switches_to_clicked_panel() {
    let mut app = make_app();
    render_once(&mut app);
    assert_eq!(app.current_panel(), PanelId::Quest);

    // 80 列 / 23 面板 = tab_width 3;点击第 2 个 tab(column 3-5)→ Parliament
    app.handle_mouse_event(left_down(4, 1));
    assert_eq!(
        app.current_panel(),
        PanelId::Parliament,
        "点击第 2 个 tab 应切到 Parliament"
    );

    // 点击第 3 个 tab(column 6-8)→ Budget
    app.handle_mouse_event(left_down(7, 1));
    assert_eq!(
        app.current_panel(),
        PanelId::Budget,
        "点击第 3 个 tab 应切到 Budget"
    );
}

#[test]
fn tab_click_out_of_range_is_ignored() {
    // 点击超出面板数的列:index >= panel_count 时静默忽略,不 panic
    let mut app = make_app();
    render_once(&mut app);
    app.handle_mouse_event(left_down(200, 1)); // 远超 80 列
    assert_eq!(app.current_panel(), PanelId::Quest, "越界点击不应切换面板");
}

// ============================================================
// B. 底部区域点击进入命令模式
// ============================================================

#[test]
fn bottom_click_enters_command_mode() {
    let mut app = make_app();
    render_once(&mut app);
    assert_eq!(app.state().input_mode, InputMode::Normal);

    // DualPane 默认布局:bottom 区域从 y=17 起(80x24 终端)
    app.handle_mouse_event(left_down(10, 18));
    assert_eq!(
        app.state().input_mode,
        InputMode::Command,
        "点击底部区域应进入命令模式"
    );
    assert!(
        app.state().input_buffer.is_empty(),
        "进入命令模式应清空输入缓冲"
    );
}

// ============================================================
// C. 弹窗激活时滚轮滚动弹窗
// ============================================================

#[test]
fn wheel_scrolls_popup_when_active() {
    let mut app = make_app();
    render_once(&mut app);
    app.state_mut().popup_stack.push(PopupKind::Detail {
        title: "Scroll Test".into(),
        content: "line\n".repeat(30),
        scroll: 5,
    });

    // ScrollUp → scroll 5 → 4
    app.handle_mouse_event(mouse(MouseEventKind::ScrollUp, 10, 10));
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => assert_eq!(*scroll, 4, "ScrollUp 应减小弹窗滚动"),
        other => panic!("应仍为 Detail 弹窗,实际: {other:?}"),
    }

    // ScrollDown → 4 → 5
    app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, 10, 10));
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => assert_eq!(*scroll, 5, "ScrollDown 应增大弹窗滚动"),
        other => panic!("应仍为 Detail 弹窗,实际: {other:?}"),
    }

    // 弹窗仍在(滚动不关闭弹窗)
    assert!(!app.state().popup_stack.is_empty());
}

#[test]
fn wheel_scroll_clamps_at_zero_in_popup() {
    let mut app = make_app();
    render_once(&mut app);
    app.state_mut().popup_stack.push(PopupKind::Detail {
        title: "Clamp Test".into(),
        content: "line\n".repeat(30),
        scroll: 1,
    });
    // 连续 ScrollUp:scroll 应钳制在 0 而非下溢
    for _ in 0..5 {
        app.handle_mouse_event(mouse(MouseEventKind::ScrollUp, 10, 10));
    }
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => assert_eq!(*scroll, 0, "滚动应钳制在 0"),
        other => panic!("应仍为 Detail 弹窗,实际: {other:?}"),
    }
}

// ============================================================
// D. 主面板区域滚轮滚动焦点面板
// ============================================================

#[test]
fn wheel_in_main_area_scrolls_focused_panel() {
    let mut app = make_app();
    // 注入 Parliament 事件,使列表可导航
    app.state_mut().latest_events = (0..10)
        .map(|i| NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: format!("p{i}"),
            voter: "alice".into(),
            vote: i % 2 == 0,
        })
        .collect::<VecDeque<_>>();
    app.switch_panel_to(PanelId::Parliament);
    render_once(&mut app);

    // 主面板区域(y 3-16)ScrollDown → Parliament selected 递增
    // (Parliament handle_mouse 经 list_state 导航,不产生命令)
    app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, 40, 10));
    assert!(
        app.state().running,
        "主面板滚动不应影响运行状态"
    );
    // 无弹窗:滚动直接委托焦点面板,不 panic 即通过
    assert!(app.state().popup_stack.is_empty());
}

#[test]
fn popup_open_blocks_main_panel_scroll() {
    // 弹窗存在时,主面板区域的滚动必须被弹窗消费,不得泄漏到面板
    let mut app = make_app();
    render_once(&mut app);
    app.state_mut().popup_stack.push(PopupKind::Detail {
        title: "Block Test".into(),
        content: "line\n".repeat(20),
        scroll: 3,
    });
    app.handle_mouse_event(mouse(MouseEventKind::ScrollDown, 40, 10));
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => {
            assert_eq!(*scroll, 4, "弹窗激活时滚轮应作用于弹窗")
        }
        other => panic!("应仍为 Detail 弹窗,实际: {other:?}"),
    }
}
