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
    TuiApp::new(TuiConfig::default()).unwrap()
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
