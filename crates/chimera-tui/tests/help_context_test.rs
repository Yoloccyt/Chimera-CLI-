//! Help 面板上下文感知激活测试(U-2 修复验收,2026-09-06 复评)
//!
//! # 背景(WHY)
//! `HelpPanel::render` 此前恒传空快捷键切片(`Self::content(id, &[])`),
//! `with_context` 宣称的"面板专属快捷键章节"永不显示(评估报告 U-2 死功能)。
//! 修复后:`TuiApp::render` 在焦点变化时把焦点面板的 `shortcuts()` 快照注入
//! `TuiState.help_context`,Help 渲染据此追加上下文章节。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;

// 无修饰符按键
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
use ratatui::Terminal;

/// 渲染当前 UI 到字符串(黑盒;触发 render 侧的 help_context 注入)
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 切到 Help 面板并渲染(需先渲染一次其它面板以注入其上下文)
fn help_content_after_focus(app: &mut TuiApp, focused: PanelId) -> String {
    app.switch_panel_to(focused);
    let _ = render_to_string(app, 120, 60); // 注入 focused 的快捷键快照
    app.switch_panel_to(PanelId::Help);
    render_to_string(app, 120, 60)
}

#[test]
fn help_shows_focused_panel_shortcut_section() {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();

    // i18n(B4-I/F-65):Help 章节标题面板名与 tab 栏同源走 name_key() 键表,
    // 期望串与渲染同源经 t! 求值、locale 无关;持 guard 串行化但不写全局
    // locale——不新增 En 窗口。WHY compact:TestBackend 逐格渲染 CJK 时
    // 字符间含空格,期望串须对压缩后内容匹配。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let expected = chimera_tui::t!(PanelId::TaskManager.name_key());
    let content = help_content_after_focus(&mut app, PanelId::TaskManager);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        compact.contains(expected),
        "Help 应渲染焦点面板专属章节标题(含面板名 {expected})"
    );
}

#[test]
fn help_context_updates_on_focus_change() {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();

    // i18n(B4-I/F-65):期望串与渲染同源经 t! 求值,locale 无关;不写全局
    // locale(WHY 见 help_shows_focused_panel_shortcut_section 注)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let expected = chimera_tui::t!(PanelId::TaskManager.name_key());

    // 焦点 Quest:Help 无 TaskManager 章节
    let content = help_content_after_focus(&mut app, PanelId::Quest);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        !compact.contains(expected),
        "焦点为 Quest 时 Help 不应出现 TaskManager 章节"
    );

    // 焦点切到 TaskManager:Help 章节跟随更新(死功能 → 活)
    let content = help_content_after_focus(&mut app, PanelId::TaskManager);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        compact.contains(expected),
        "焦点切到 TaskManager 后 Help 应更新章节"
    );
}

#[test]
fn help_context_field_tracks_focus() {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();

    app.switch_panel_to(PanelId::Router);
    let _ = render_to_string(&mut app, 120, 40);
    let (id, shortcuts) = app
        .state()
        .help_context
        .clone()
        .expect("渲染后 help_context 应被注入");
    assert_eq!(id, PanelId::Router);
    assert!(!shortcuts.is_empty(), "Router 面板应声明非空快捷键列表");
}

// ============================================================
// U-N3(2026-09-06 复评):Help 内容滚动 —— 上下文章节在小高度可见
// ============================================================

#[test]
fn help_scrolls_to_reveal_context_section() {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    // i18n(B4-I/F-65):期望串与渲染同源经 t! 求值,locale 无关;不写全局
    // locale(WHY 见 help_shows_focused_panel_shortcut_section 注)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let expected = chimera_tui::t!(PanelId::TaskManager.name_key());
    // 注入 TaskManager 上下文 → 切 Help
    app.switch_panel_to(PanelId::TaskManager);
    let _ = render_to_string(&mut app, 120, 40);
    app.switch_panel_to(PanelId::Help);
    let clipped = render_to_string(&mut app, 120, 40);
    let clipped_compact: String = clipped.chars().filter(|c| *c != ' ').collect();
    assert!(
        !clipped_compact.contains(expected),
        "120x40 下上下文章节应被裁掉(滚动前,复现 U-N3 现场)"
    );

    // ↓ 滚动 8 行:上下文章节进入可视区
    for _ in 0..8 {
        app.handle_key_event(key(KeyCode::Down));
    }
    let scrolled = render_to_string(&mut app, 120, 40);
    let scrolled_compact: String = scrolled.chars().filter(|c| *c != ' ').collect();
    assert!(
        scrolled_compact.contains(expected),
        "滚动后应可见焦点面板专属快捷键章节"
    );
}

#[test]
fn help_scroll_to_bottom_shows_footer() {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.switch_panel_to(PanelId::Help);
    let _ = render_to_string(&mut app, 120, 40);

    // 单按 G:Normal 路由 ScrollBottom 目标 → HelpPanel::scroll_to_bottom
    // (g+G 走 GPrefix 的 ExitMode 取消语义,不是跳底)
    app.handle_key_event(key(KeyCode::Char('G')));
    let content = render_to_string(&mut app, 120, 40);
    assert!(content.contains("NEXUS-OMEGA"), "跳底后应可见内容末尾标识");
}
