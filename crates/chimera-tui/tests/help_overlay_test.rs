//! Help overlay 集成测试 — 验证全局 `?` 键触发的帮助浮层行为
//!
//! 覆盖场景:
//! - 任意面板按 `?` 弹出 Help overlay
//! - Esc 关闭 Help overlay
//! - Help overlay 内容包含关键快捷键(q/Tab/:/j/k/Enter)
//! - 弹出 Help overlay 不切换当前面板

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
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
    // M2 增量3:`?` 帮助浮层追加与命令面板同源的 Registry 命令章节。
    // B5-V(F-64)对账加严:章节源头是 ActionRegistry + codegen::help_lines
    // (open_help_action 用构造期缓存 registry 全量生成,app/event_loop.rs),
    // 旧断言仅 contains("Ctrl+P")(自认薄弱),现对账:
    // ①代表性动作标题抽样(经 t! 期望串,≥5);
    // ②命令章节与 help_lines(&ActionRegistry::with_builtin_domains(), None)
    //   逐行一致(popup 构造把 action_lines 全量追加在 entries 尾部,
    //   行数/顺序/内容一次锁死)。
    // B4 治理口径:locale-guard + pin,消除 t! 读值的并行翻转窗口。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    let mut app = make_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

    let entries: Vec<(String, String)> = match app.state().popup_stack.current().unwrap() {
        chimera_tui::popup::PopupKind::HelpOverlay { entries, .. } => entries.clone(),
        _ => panic!("expected HelpOverlay popup"),
    };
    let content = entries
        .iter()
        .map(|(k, d)| format!("{k} {d}"))
        .collect::<String>();

    // "Ctrl+P" 仅出现于命令章节标题,基础全局键不含它 → 章节已追加的稳定证明。
    assert!(
        content.contains("Ctrl+P"),
        "Help overlay should include the Registry command section (Ctrl+P header)"
    );

    // ①代表性动作标题抽样(≥5):期望串与浮层同经 t! 解析(同 locale 同值),
    //   动作若从注册表移除或标题键漏译,此断言即红。
    let sampled_title_keys: &[&str] = &[
        "action.system.toggle_locale",
        "action.export.run",
        "action.monitor.pause_sampling",
        "action.overwindow.run",
        "action.viz.switch_dimension",
        "action.view.apply_saved",
    ];
    assert!(
        sampled_title_keys.len() >= 5,
        "抽样动作数不得少于 5(对账口径)"
    );
    for &title_key in sampled_title_keys {
        let expected = chimera_tui::t!(title_key);
        assert!(
            entries.iter().any(|(_, title)| title == expected),
            "帮助浮层应包含动作 {title_key} 的标题(期望 {expected:?}),实际:{entries:?}"
        );
    }

    // ②命令章节逐行对账:entries 尾部 n 行 == help_lines 全量(行数+顺序+内容)。
    let registry = chimera_tui::ActionRegistry::with_builtin_domains();
    let expected_lines: Vec<(String, String)> =
        chimera_tui::actions::codegen::help_lines(&registry, None)
            .into_iter()
            .map(|line| (line.key, line.title))
            .collect();
    let n = expected_lines.len();
    assert!(n >= 5, "内建注册表动作数应足以支撑章节对账, got {n}");
    assert_eq!(
        &entries[entries.len() - n..],
        &expected_lines[..],
        "帮助浮层命令章节应与 codegen::help_lines 全量一致"
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
