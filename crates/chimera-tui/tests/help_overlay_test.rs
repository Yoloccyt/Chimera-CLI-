//! Help overlay 集成测试 — 验证全局 `?` 键触发的帮助浮层行为
//!
//! 覆盖场景:
//! - 任意面板按 `?` 弹出 Help overlay
//! - Esc 关闭 Help overlay
//! - Help overlay 内容包含关键快捷键(q/Tab/:/j/k/Enter)
//! - 弹出 Help overlay 不切换当前面板

#![forbid(unsafe_code)]

use chimera_tui::{PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn make_app() -> TuiApp {
    {
        let mut __app = TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .unwrap();
        __app.state_mut().view_mode = chimera_tui::ViewMode::Dashboard;
        __app
    }
}

#[test]
fn question_mark_opens_help_overlay_from_any_panel() {
    let panels = [
        PanelId::Quest,
        PanelId::Parliament,
        PanelId::Budget,
        PanelId::Memory,
        PanelId::Security,
        PanelId::Health,
        PanelId::Log,
        PanelId::Help,
        PanelId::Decay,
        PanelId::EventStream,
        PanelId::Router,
        PanelId::McpNodes,
        PanelId::Chtc,
    ];

    for panel in panels {
        let mut app = make_app();
        app.switch_panel_to(panel);
        assert_eq!(app.current_panel(), panel);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

        assert!(
            !app.state().popup_stack.is_empty(),
            "'?' should open a popup from panel {panel:?}"
        );
        assert!(
            matches!(
                app.state().popup_stack.current().unwrap(),
                chimera_tui::popup::PopupKind::HelpOverlay { .. }
            ),
            "'?' should open Help overlay from panel {panel:?}"
        );
        assert_eq!(
            app.current_panel(),
            panel,
            "Help overlay should not switch away from panel {panel:?}"
        );
    }
}

#[test]
fn esc_closes_help_overlay() {
    let mut app = make_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(!app.state().popup_stack.is_empty());

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        app.state().popup_stack.is_empty(),
        "Esc should close Help overlay"
    );
    assert!(
        app.state().running,
        "Esc on Help overlay should not quit app"
    );
}

#[test]
fn help_overlay_contains_expected_shortcuts() {
    let mut app = make_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

    let popup = app.state().popup_stack.current().unwrap();
    let entries: Vec<(String, String)> = match popup {
        chimera_tui::popup::PopupKind::HelpOverlay { entries, .. } => entries.clone(),
        _ => panic!("expected HelpOverlay popup"),
    };

    let content = entries
        .iter()
        .map(|(k, d)| format!("{k} {d}"))
        .collect::<String>()
        .to_lowercase();

    assert!(
        content.contains('q'),
        "Help overlay should mention quit key 'q'"
    );
    assert!(
        content.contains("tab"),
        "Help overlay should mention panel switching key 'Tab'"
    );
    assert!(
        content.contains(':'),
        "Help overlay should mention command key ':'"
    );
    assert!(
        content.contains('j'),
        "Help overlay should mention scroll key 'j'"
    );
    assert!(
        content.contains('k'),
        "Help overlay should mention scroll key 'k'"
    );
    assert!(
        content.contains("enter"),
        "Help overlay should mention detail key 'Enter'"
    );
}

#[test]
fn help_overlay_includes_registry_commands() {
    // M2 增量3:`?` 帮助浮层应追加与命令面板同源的 Registry 命令章节。
    let mut app = make_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

    let popup = app.state().popup_stack.current().unwrap();
    let entries: Vec<(String, String)> = match popup {
        chimera_tui::popup::PopupKind::HelpOverlay { entries, .. } => entries.clone(),
        _ => panic!("expected HelpOverlay popup"),
    };
    let content = entries
        .iter()
        .map(|(k, d)| format!("{k} {d}"))
        .collect::<String>();

    // "Ctrl+P" 仅出现于新增的命令章节标题,基础全局键不含它 → 稳定证明命令章节已追加。
    assert!(
        content.contains("Ctrl+P"),
        "Help overlay should include the Registry command section (Ctrl+P header)"
    );
    // 回归保护:命令章节为追加而非替换,基础导航键仍在。
    assert!(
        content.to_lowercase().contains("tab"),
        "Registry command section must be additive, base nav keys must remain"
    );
}

#[test]
fn help_overlay_can_be_scrolled() {
    let mut app = make_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        match app.state().popup_stack.current().unwrap() {
            chimera_tui::popup::PopupKind::HelpOverlay { scroll, .. } => *scroll,
            _ => panic!("expected HelpOverlay"),
        },
        1,
        "Down should scroll Help overlay"
    );

    app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        match app.state().popup_stack.current().unwrap() {
            chimera_tui::popup::PopupKind::HelpOverlay { scroll, .. } => *scroll,
            _ => panic!("expected HelpOverlay"),
        },
        0,
        "Up should scroll Help overlay back to top"
    );
}
