//! TUI 应用核心测试 — 应用初始化/面板切换/键盘事件/渲染/主题/布局/弹窗/数据接入/鼠标事件测试
//!
//! Task 1.15.5:从 mod.rs 抽离内联测试到独立文件,使 mod.rs < 800 行。
//!
//! 对应架构层:L10 Interface

use std::collections::VecDeque;

use crossterm::event::{self, KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Color;
use ratatui::Terminal;

use super::event_loop::{ratio_preset_next, tick_preset_next};
use super::*;
use crate::config::Theme;
use crate::data::{BudgetMetrics, DataSnapshot, DataSourceConfig, TuiDataSource};
use crate::popup::PopupKind;
use crate::types::{InputMode, LayoutMode};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;

use crate::popup::Severity;

fn make_app() -> Result<TuiApp, TuiError> {
    TuiApp::new(TuiConfig::default())
}

/// 构造一个简单 Quest，用于数据驱动面板测试
fn sample_quest(id: &str, title: &str) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![Task {
            task_id: format!("{id}-t1"),
            description: "test task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 测试替身数据源 — 返回预设快照
#[derive(Debug)]
struct MockDataSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl MockDataSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for MockDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

// ============================================================
// 应用初始化测试
// ============================================================

#[test]
fn test_app_new() -> Result<(), Box<dyn std::error::Error>> {
    let app = make_app()?;
    assert_eq!(app.current_panel(), PanelId::Quest);
    assert!(app.state().running);
    assert_eq!(app.config().theme, Theme::Dark);
    Ok(())
}

#[test]
fn test_app_invalid_config_rejected() {
    let config = TuiConfig {
        main_panel_ratio: 0.0,
        ..Default::default()
    };
    assert!(TuiApp::new(config).is_err());
}

// ============================================================
// 面板切换测试
// ============================================================

#[test]
fn test_switch_panel_next() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert_eq!(app.current_panel(), PanelId::Quest);
    app.switch_panel_next();
    assert_eq!(app.current_panel(), PanelId::Parliament);
    app.switch_panel_next();
    assert_eq!(app.current_panel(), PanelId::Budget);
    app.switch_panel_next();
    assert_eq!(app.current_panel(), PanelId::Memory);
    Ok(())
}

#[test]
fn test_switch_panel_prev() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_prev();
    // Task 3.7/3.9:FocusManager 现注册 22 面板(PvlScore/TaskManager 追加到末尾);
    // Quest 的上一个 = 列表末尾的 TaskManager 面板。
    assert_eq!(app.current_panel(), PanelId::TaskManager);
    Ok(())
}

#[test]
fn test_switch_panel_to() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Budget);
    assert_eq!(app.current_panel(), PanelId::Budget);
    Ok(())
}

#[test]
fn test_quit() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert!(app.state().running);
    app.quit();
    assert!(!app.state().running);
    Ok(())
}

// ============================================================
// 键盘事件处理测试
// ============================================================

#[test]
fn test_handle_key_q_quits() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), event::KeyModifiers::NONE));
    assert!(!app.state().running);
    Ok(())
}

#[test]
fn test_handle_key_esc_quits() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
    assert!(!app.state().running);
    Ok(())
}

#[test]
fn test_handle_key_tab_switches_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Parliament);
    Ok(())
}

#[test]
fn test_handle_key_number_jumps_to_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('3'), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Budget);
    Ok(())
}

#[test]
fn test_handle_key_new_panels() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('4'), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Memory);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('5'), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Security);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('6'), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Health);
    Ok(())
}

#[test]
fn test_handle_key_9_jumps_to_decay() -> Result<(), Box<dyn std::error::Error>> {
    // P2 TUI v1.7-omega:数字键 9 跳转到 Decay 面板(P0 Note 第 1 节)
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('9'), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Decay);
    Ok(())
}

#[test]
fn test_handle_key_f_keys_jump_to_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::F(2), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Parliament);
    Ok(())
}

#[test]
fn test_handle_key_f_keys_new_panels() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;

    app.handle_key_event(KeyEvent::new(KeyCode::F(6), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Memory);

    app.handle_key_event(KeyEvent::new(KeyCode::F(7), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Security);

    app.handle_key_event(KeyEvent::new(KeyCode::F(8), event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Health);
    Ok(())
}

#[test]
fn test_handle_key_release_ignored() -> Result<(), Box<dyn std::error::Error>> {
    // WHY Windows 兼容:Release 事件应被忽略
    // 用 new_with_kind 显式指定 Release,验证 handle_key_event 的 kind 过滤
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        event::KeyModifiers::NONE,
        event::KeyEventKind::Release,
    ));
    assert!(app.state().running, "Release event should be ignored");
    Ok(())
}

#[test]
fn test_handle_key_command_mode() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Command);

    // 输入命令
    for c in "budget".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "budget");

    // 提交
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Budget);
    assert_eq!(app.state().input_mode, InputMode::Normal);
    Ok(())
}

#[test]
fn test_handle_key_search_mode_sets_filter() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Search);

    for c in "Error".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "Error");

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Normal);
    assert_eq!(app.state().filter_keyword, Some("error".into()));
    Ok(())
}

#[test]
fn test_handle_key_esc_cancels_command_mode() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
    for c in "quit".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Normal);
    assert!(app.state().input_buffer.is_empty());
    assert!(app.state().running);
    Ok(())
}

#[test]
fn test_handle_key_question_mark_shows_help_overlay() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), event::KeyModifiers::NONE));
    assert!(!app.state.popup_stack.is_empty());
    assert!(
        app.state
            .popup_stack
            .current()
            .ok_or("expected current popup")?
            .is_help_overlay(),
        "'?' should open Help overlay instead of switching to Help panel"
    );
    // P3.2:不切换当前面板,焦点仍保持在 Quest
    assert_eq!(app.current_panel(), PanelId::Quest);
    Ok(())
}

#[test]
fn test_handle_key_ctrl_up_increases_ratio() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // Task 1.15.4:字段访问改为方法调用(委托到 pane_manager)
    let before = app.main_panel_ratio();
    app.handle_key_event(KeyEvent::new(KeyCode::Up, event::KeyModifiers::CONTROL));
    assert!(app.main_panel_ratio() > before);
    Ok(())
}

#[test]
fn test_handle_key_ctrl_down_decreases_ratio() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    let before = app.main_panel_ratio();
    app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::CONTROL));
    assert!(app.main_panel_ratio() < before);
    Ok(())
}

#[test]
fn test_main_panel_ratio_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    for _ in 0..100 {
        app.adjust_main_panel_ratio(true);
    }
    assert!((app.main_panel_ratio() - RATIO_MAX).abs() < f32::EPSILON);

    for _ in 0..100 {
        app.adjust_main_panel_ratio(false);
    }
    assert!((app.main_panel_ratio() - RATIO_MIN).abs() < f32::EPSILON);
    Ok(())
}

// ============================================================
// 弹窗测试
// ============================================================

#[test]
fn test_popup_esc_closes() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.popup_stack.push(PopupKind::Notification {
        message: "test".into(),
        severity: crate::popup::Severity::Info,
    });
    assert!(!app.state.popup_stack.is_empty());

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
    assert!(app.state.popup_stack.is_empty());
    Ok(())
}

#[test]
fn test_detail_popup_scroll() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.popup_stack.push(PopupKind::Detail {
        title: "Detail".into(),
        content: "line1\nline2\nline3".into(),
        scroll: 0,
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::NONE));
    assert_eq!(
        app.state
            .popup_stack
            .current()
            .ok_or("expected current popup")?
            .detail_scroll(),
        Some(1)
    );
    Ok(())
}

#[test]
fn test_confirm_popup_yes_quits() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.popup_stack.push(PopupKind::Confirm {
        prompt: "Quit?".into(),
        on_confirm: "quit".into(),
        confirmed: true,
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert!(app.state.popup_stack.is_empty());
    assert!(!app.state.running);
    Ok(())
}

#[test]
fn test_confirm_popup_no_dismisses() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.popup_stack.push(PopupKind::Confirm {
        prompt: "Quit?".into(),
        on_confirm: "quit".into(),
        confirmed: false,
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert!(app.state.popup_stack.is_empty());
    assert!(app.state.running);
    Ok(())
}

// ============================================================
// 渲染测试(使用 TestBackend,无需真实终端)
// ============================================================

#[test]
fn test_render_produces_output() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("Panel:") || content.contains("Quest"),
        "rendered output should contain panel info"
    );
    Ok(())
}

#[test]
fn test_render_switches_panel_content() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_next(); // Quest → Parliament

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("Parliament"),
        "rendered output should contain Parliament panel"
    );
    Ok(())
}

#[test]
fn test_render_memory_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Memory);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("Memory") || content.contains("Cache Hit Rate"),
        "rendered output should contain Memory panel"
    );
    Ok(())
}

#[test]
fn test_render_security_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Security);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("Security") || content.contains("VETO"),
        "rendered output should contain Security panel"
    );
    Ok(())
}

#[test]
fn test_render_health_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Health);

    let backend = TestBackend::new(80, 24);
    let _locale_guard = crate::i18n::locale_test_guard();
    // i18n:面板文案随 locale 切换;固定英文捕获后复位,断言 ASCII 文案。
    crate::i18n::set_locale(crate::i18n::Locale::En);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    crate::i18n::set_locale(crate::i18n::Locale::Zh);
    assert!(
        content.contains("Health") || content.contains("Events/sec"),
        "rendered output should contain Health panel"
    );
    Ok(())
}

#[test]
fn test_render_help_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Help);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("Help") || content.contains("Quit"),
        "rendered output should contain Help panel content"
    );
    Ok(())
}

// ============================================================
// 主题颜色测试
// ============================================================

#[test]
fn test_theme_fg_dark() -> Result<(), Box<dyn std::error::Error>> {
    let app = make_app()?;
    assert_eq!(app.theme_fg(), Color::White);
    Ok(())
}

#[test]
fn test_theme_fg_light() -> Result<(), Box<dyn std::error::Error>> {
    let app = TuiApp::new(TuiConfig {
        theme: Theme::Light,
        ..Default::default()
    })?;
    assert_eq!(app.theme_fg(), Color::Black);
    assert_eq!(app.theme_accent(), Color::Blue);
    Ok(())
}

#[test]
fn test_theme_accent_dark() -> Result<(), Box<dyn std::error::Error>> {
    let app = make_app()?;
    assert_eq!(app.theme_accent(), Color::Cyan);
    Ok(())
}

// ============================================================
// P6.1/P6.2 handle_global_key 主题/布局切换测试
// ============================================================

/// P6.1.1 TDD-RED:按 `t` 键,主题从 Dark → Light
#[test]
fn test_handle_key_t_switches_theme_dark_to_light() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert_eq!(app.config().theme, Theme::Dark);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    assert_eq!(app.config().theme, Theme::Light);
    Ok(())
}

/// P6.1.1 TDD-RED:按 `t` 键 3 次,主题循环回到 Dark
#[test]
fn test_handle_key_t_cycles_through_all_themes() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // Dark → Light
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    assert_eq!(app.config().theme, Theme::Light);
    // Light → HighContrast
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    assert_eq!(app.config().theme, Theme::HighContrast);
    // HighContrast → Dark
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    assert_eq!(app.config().theme, Theme::Dark);
    Ok(())
}

/// P6.1.1 TDD-RED:按 `t` 键后,所有面板被标记 dirty(立即重绘)
#[test]
fn test_handle_key_t_marks_all_panels_dirty() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 初始无 dirty 面板
    assert!(app.state().dirty_panels.is_empty());
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    // 所有已注册面板都应被标记 dirty
    assert!(!app.state().dirty_panels.is_empty());
    // 验证至少 Quest 与 Parliament 被标记(代表性断言)
    assert!(app.state().dirty_panels.contains(&PanelId::Quest));
    assert!(app.state().dirty_panels.contains(&PanelId::Parliament));
    Ok(())
}

/// P6.1:按 `t` 键后,status_message 显示新主题名
#[test]
fn test_handle_key_t_sets_status_message() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
    let (msg, severity) = app
        .state()
        .status_message
        .clone()
        .ok_or("status_message should be set")?;
    // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
    // 此处只断言 locale 无关的主题值,避免并行测试切换 locale 造成拖动。
    assert!(
        msg.contains("light"),
        "status_message should contain 'light', got: {msg}"
    );
    assert_eq!(severity, Severity::Info);
    Ok(())
}

/// P6.2:按 `l` 键,布局从 DualPane → TriplePane
#[test]
fn test_handle_key_l_switches_layout_dual_to_triple() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
    Ok(())
}

/// M3d:按 `l` 键 4 次,布局循环回到 DualPane(纳入 VimSplit)
#[test]
fn test_handle_key_l_cycles_through_all_layouts() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // DualPane → TriplePane
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
    // TriplePane → VimSplit
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::VimSplit);
    // VimSplit → SinglePane
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::SinglePane);
    // SinglePane → DualPane
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
    Ok(())
}

/// P6.2:按 `l` 键后,status_message 显示新布局名
#[test]
fn test_handle_key_l_sets_status_message() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    let (msg, severity) = app
        .state()
        .status_message
        .clone()
        .ok_or("status_message should be set")?;
    // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
    // 此处只断言 locale 无关的布局值。
    assert!(
        msg.contains("triple"),
        "status_message should contain 'triple', got: {msg}"
    );
    assert_eq!(severity, Severity::Info);
    Ok(())
}

/// P6.2:SinglePane 布局下 render 不崩溃(专注模式跳过 tabs/status_bar)
#[test]
fn test_render_single_pane_layout_no_panic() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 切换到 SinglePane(按 `l` 三次:Dual → Triple → VimSplit → Single,M3d 4 循环)
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::SinglePane);

    // 渲染不应 panic(SinglePane 跳过 tabs 和 status_bar)
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;
    Ok(())
}

/// P6.2:TriplePane 布局下 render 不崩溃
#[test]
fn test_render_triple_pane_layout_no_panic() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 切换到 TriplePane
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
    assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;
    Ok(())
}

/// P0 交互链 Phase 2:panel.drill_down 派发进入 Focus 全屏(SinglePane)
#[test]
fn dispatch_drill_down_enters_focus_layout() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
    app.dispatch_action(
        "panel.drill_down",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(
        app.state().layout_mode,
        LayoutMode::SinglePane,
        "panel.drill_down 应进入 Focus 全屏(SinglePane)"
    );
    Ok(())
}

/// 入口三:bare `a` 唤出焦点面板的非空上下文动作菜单(端到端:键→路由→打开)
#[test]
fn key_a_opens_panel_action_menu() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), event::KeyModifiers::NONE));
    let is_menu = matches!(
        app.state().popup_stack.current(),
        Some(PopupKind::ActionMenu { entries, .. }) if !entries.is_empty()
    );
    assert!(is_menu, "bare `a` 应唤出非空面板动作菜单");
    Ok(())
}

/// 入口三:菜单 Enter 派发选中动作(用本地 arm drill_down 断言,不依赖 cli 异步)
#[test]
fn action_menu_enter_dispatches_selected_local_action() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 手工压入含本地 arm 动作(drill_down)的菜单;选中项即 drill_down
    app.state.popup_stack.push(PopupKind::action_menu(
        "Test",
        vec![("panel.drill_down".to_string(), "下钻".to_string())],
    ));
    assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert_eq!(
        app.state().layout_mode,
        LayoutMode::SinglePane,
        "菜单 Enter 应派发选中动作(drill_down → SinglePane)"
    );
    assert!(app.state().popup_stack.is_empty(), "派发后菜单应关闭");
    Ok(())
}

/// M3 monitor.pause_sampling:派发切换冻结标志(幂等切换)
#[test]
fn dispatch_monitor_pause_toggles_freeze_flag() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert!(!app.state().monitor_paused);
    app.dispatch_action(
        "monitor.pause_sampling",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert!(app.state().monitor_paused, "首次派发应暂停");
    app.dispatch_action(
        "monitor.pause_sampling",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert!(!app.state().monitor_paused, "再次派发应恢复");
    Ok(())
}

/// M3 monitor.time_window:派发循环时间窗(默认 Long → Short)
#[test]
fn dispatch_monitor_time_window_cycles() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    assert_eq!(
        app.state().monitor_window,
        crate::types::MonitorWindow::Long
    );
    app.dispatch_action(
        "monitor.time_window",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(
        app.state().monitor_window,
        crate::types::MonitorWindow::Short,
        "Long.next() 应为 Short"
    );
    Ok(())
}

/// M3 viz.switch_dimension:ClvVector 焦点切换热图值域自适应
#[test]
fn dispatch_viz_switch_dimension_clv_toggles_autoscale() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::ClvVector);
    assert!(!app.state().clv_heatmap_autoscale);
    app.dispatch_action(
        "viz.switch_dimension",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert!(
        app.state().clv_heatmap_autoscale,
        "ClvVector 焦点应切换热图值域自适应"
    );
    Ok(())
}

/// M3 viz.switch_dimension:OsaSparse 焦点无可切维度→诚实反馈且不误改 CLV 值域
#[test]
fn dispatch_viz_switch_dimension_osa_honest_no_toggle() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::OsaSparse);
    app.dispatch_action(
        "viz.switch_dimension",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert!(
        !app.state().clv_heatmap_autoscale,
        "OsaSparse 焦点不应改 CLV 值域"
    );
    let (msg, _) = app.state().status_message.clone().ok_or("应给诚实反馈")?;
    assert!(
        msg.contains("暂无可切换维度"),
        "OsaSparse 应给诚实反馈,got: {msg}"
    );
    Ok(())
}

/// M3 monitor.pause_sampling:暂停时 update() 冻结 sys_metrics(不被快照覆盖)
#[test]
fn update_freezes_sys_metrics_when_monitor_paused() -> Result<(), Box<dyn std::error::Error>> {
    // Mock 数据源固定返回 global_usage=10 的 sys_metrics
    let mut snap = DataSnapshot::default();
    snap.sys_metrics.cpu.global_usage = 10.0;
    let mut app =
        TuiApp::with_data_source(TuiConfig::default(), Box::new(MockDataSource::new(snap)))?;

    // 未暂停:update 刷新为 mock 值
    app.update();
    assert_eq!(app.state().sys_metrics.cpu.global_usage, 10.0);

    // 暂停后手工置可辨识冻结值,update 不应覆盖
    app.state.monitor_paused = true;
    app.state.sys_metrics.cpu.global_usage = 42.0;
    app.update();
    assert_eq!(
        app.state().sys_metrics.cpu.global_usage,
        42.0,
        "暂停时 sys_metrics 应冻结,不被 update 覆盖"
    );

    // 恢复后:update 重新刷新为 mock 值
    app.state.monitor_paused = false;
    app.update();
    assert_eq!(
        app.state().sys_metrics.cpu.global_usage,
        10.0,
        "恢复后 sys_metrics 应被 update 刷新"
    );
    Ok(())
}

/// M4 view.apply_saved:apply_view_fields 仅拷贝视图偏好,不碰运行时字段
#[test]
fn apply_view_fields_copies_view_prefs_only() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    let mut saved = crate::types::TuiState::new();
    saved.layout_mode = LayoutMode::TriplePane;
    saved.filter_keyword = Some("q1".to_string());
    saved.monitor_window = crate::types::MonitorWindow::Short;
    saved.clv_heatmap_autoscale = true;
    saved.running = false; // 运行时字段,不应被拷贝
    app.apply_view_fields(&saved);
    assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
    assert_eq!(app.state().filter_keyword.as_deref(), Some("q1"));
    assert_eq!(
        app.state().monitor_window,
        crate::types::MonitorWindow::Short
    );
    assert!(app.state().clv_heatmap_autoscale);
    assert!(
        app.state().running,
        "running 是运行时字段,不应被视图应用覆盖"
    );
    Ok(())
}

/// M4 view.apply_saved:无持久化文件时给出诚实反馈(不静默/不伪造)
#[test]
fn dispatch_view_apply_saved_no_file_gives_honest_status() -> Result<(), Box<dyn std::error::Error>>
{
    let mut app = make_app()?;
    // 用确定不存在的路径,保证测试确定性(不依赖真实文件)
    app.config.state_file_path = std::path::PathBuf::from("nonexistent_dir_xyz/no_such_view.yaml");
    app.dispatch_action(
        "view.apply_saved",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
    assert!(msg.contains("无已保存"), "无文件应给诚实反馈,got: {msg}");
    Ok(())
}

/// M4 config.edit:派发打开非空配置菜单
#[test]
fn dispatch_config_edit_opens_config_menu() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.dispatch_action(
        "config.edit",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    let is_menu = matches!(
        app.state().popup_stack.current(),
        Some(PopupKind::ConfigMenu { entries, .. }) if !entries.is_empty()
    );
    assert!(is_menu, "config.edit 应打开非空配置菜单");
    Ok(())
}

/// M4 config.edit:菜单 Enter 就地循环选中项(默认 selected=0=主题)且菜单常驻
#[test]
fn config_menu_enter_cycles_selected_theme() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.open_config_menu();
    let before = app.config.theme.as_str();
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
    assert_ne!(
        app.config.theme.as_str(),
        before,
        "Enter 选中主题项应循环主题"
    );
    assert!(
        matches!(
            app.state().popup_stack.current(),
            Some(PopupKind::ConfigMenu { .. })
        ),
        "配置菜单 Enter 后应常驻(不关闭)"
    );
    Ok(())
}

/// M4 config.edit:预设循环闭合 + 非预设值归最近档
#[test]
fn config_presets_cycle_closed_and_snap_nearest() {
    // ratio 0.7→0.8→0.5 闭合;0.72 归最近 0.7 → 0.8
    assert_eq!(ratio_preset_next(0.7), 0.8);
    assert_eq!(ratio_preset_next(0.8), 0.5);
    assert_eq!(ratio_preset_next(0.72), 0.8);
    // tick 250→500→1000→100 闭合;300 归最近 250 → 500
    assert_eq!(tick_preset_next(250), 500);
    assert_eq!(tick_preset_next(1000), 100);
    assert_eq!(tick_preset_next(300), 500);
}

/// Phase 3 quest.jump:单 Quest → 切事件流并按其 id 过滤(复用 JumpToEventStream)
#[test]
fn dispatch_quest_jump_single_quest_filters_eventstream() -> Result<(), Box<dyn std::error::Error>>
{
    let mut app = make_app()?;
    app.state.quest_list = vec![sample_quest("q1", "First")];
    app.dispatch_action(
        "quest.jump",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(app.current_panel(), PanelId::EventStream, "应切到事件流");
    assert_eq!(
        app.state().filter_keyword.as_deref(),
        Some("q1"),
        "单 Quest 应按其 id 过滤"
    );
    Ok(())
}

/// Phase 3 quest.jump:无 Quest → 切事件流 + 诚实反馈(不臆测目标)
#[test]
fn dispatch_quest_jump_empty_switches_eventstream_honest() -> Result<(), Box<dyn std::error::Error>>
{
    let mut app = make_app()?;
    app.state.quest_list = vec![];
    app.dispatch_action(
        "quest.jump",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(app.current_panel(), PanelId::EventStream);
    let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
    assert!(msg.contains("无 Quest"), "空列表应诚实提示,got: {msg}");
    Ok(())
}

/// Phase 3 quest.jump:多 Quest 无选中 → 切事件流 + 提示精确跳转(不臆测目标)
#[test]
fn dispatch_quest_jump_multi_switches_eventstream_hint() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
    // §1.3b:焦点切非 Quest 面板(无选中上下文)以测多 Quest 回退路径
    app.switch_panel_to(PanelId::Budget);
    app.dispatch_action(
        "quest.jump",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(app.current_panel(), PanelId::EventStream);
    let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
    assert!(
        msg.contains("多 Quest"),
        "多 Quest 无选中上下文应提示精确跳转,got: {msg}"
    );
    Ok(())
}

/// §1.3b:焦点 Quest 面板有选中项时 quest.jump 精确跳转(不走多 Quest 回退)
#[test]
fn dispatch_quest_jump_precise_uses_focused_selection() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 默认焦点 = Quest 面板,selected=0 → 选中 q1
    app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
    app.dispatch_action(
        "quest.jump",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(app.current_panel(), PanelId::EventStream);
    assert_eq!(
        app.state().filter_keyword.as_deref(),
        Some("q1"),
        "多 Quest 下焦点 Quest 选中项应精确过滤 q1"
    );
    Ok(())
}

/// §1.3b:enrich_payload_with_focused_quest 三态(注入 / 不覆盖 / 透传)
#[test]
fn enrich_payload_with_focused_quest_three_states() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
    // 焦点 Quest(默认)+ 空 payload → 注入选中 quest_id
    let enriched = app.enrich_payload_with_focused_quest("{}".to_string());
    assert!(
        enriched.contains("q1"),
        "应注入焦点选中 quest_id,got: {enriched}"
    );
    // payload 已含 quest_id → 尊重不覆盖
    let explicit = app.enrich_payload_with_focused_quest(r#"{"quest_id":"qX"}"#.to_string());
    assert!(
        explicit.contains("qX") && !explicit.contains("q1"),
        "已含 quest_id 不应被覆盖,got: {explicit}"
    );
    // 焦点非 Quest 面板(无选中上下文)→ 透传
    app.switch_panel_to(PanelId::Budget);
    let passthrough = app.enrich_payload_with_focused_quest("{}".to_string());
    assert_eq!(passthrough, "{}", "焦点无选中上下文应透传原 payload");
    Ok(())
}

// ============================================================
// 数据接入测试
// ============================================================

#[test]
fn test_with_data_source_accepts_custom_source() -> Result<(), Box<dyn std::error::Error>> {
    let app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(DataSnapshot::default())),
    )?;
    assert!(app.state().quest_list.is_empty());
    assert_eq!(app.state().budget.current_tier, "High");
    Ok(())
}

#[test]
fn test_update_pulls_snapshot_into_state() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot {
        quest_list: vec![sample_quest("q1", "Data Driven Quest")],
        budget_metrics: BudgetMetrics {
            current_tier: "Critical".into(),
            utilization_rate: 0.95,
            ..Default::default()
        },
        latest_events: VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k1".into(),
        }]),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();

    assert_eq!(app.state().quest_list.len(), 1);
    assert_eq!(app.state().quest_list[0].title, "Data Driven Quest");
    assert_eq!(app.state().budget.current_tier, "Critical");
    assert_eq!(app.state().latest_events.len(), 1);
    Ok(())
}

#[test]
fn test_update_sets_status_message_on_error() -> Result<(), Box<dyn std::error::Error>> {
    /// 总是返回错误的数据源
    #[derive(Debug)]
    struct FailingDataSource;

    impl TuiDataSource for FailingDataSource {
        fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
            Err(TuiError::DataSource("forced failure".into()))
        }

        fn config(&self) -> &DataSourceConfig {
            static CONFIG: std::sync::OnceLock<DataSourceConfig> = std::sync::OnceLock::new();
            CONFIG.get_or_init(DataSourceConfig::default)
        }
    }

    let mut app = TuiApp::with_data_source(TuiConfig::default(), Box::new(FailingDataSource))?;
    app.update();

    assert!(
        app.state().status_message.is_some(),
        "data source failure should set status message"
    );
    let (msg, severity) = app
        .state()
        .status_message
        .as_ref()
        .ok_or("expected status message")?;
    assert!(msg.contains("forced failure"));
    assert_eq!(*severity, Severity::Warning);
    Ok(())
}

#[test]
fn test_quest_panel_renders_real_quest_data() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
        ],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(content.contains("First Quest"));
    assert!(content.contains("Second Quest"));
    Ok(())
}

#[test]
fn test_budget_panel_content_uses_state() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot {
        budget_metrics: BudgetMetrics {
            total_consumption: 800.0,
            remaining_budget: 200.0,
            utilization_rate: 0.8,
            current_tier: "Medium".into(),
            coefficient: 0.8,
            is_exceeded: false,
            alert: None,
        },
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    app.switch_panel_to(PanelId::Budget);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(content.contains("Medium"));
    assert!(content.contains("800.0"));
    assert!(content.contains("OK"));
    Ok(())
}

#[test]
fn test_log_panel_content_uses_state() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot {
        latest_events: VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        }]),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    app.switch_panel_to(PanelId::Log);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(content.contains("System Log"));
    assert!(content.contains("CacheHit"));
    Ok(())
}

// ============================================================
// 鼠标事件测试
// ============================================================

#[test]
fn test_mouse_scroll_in_main_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.switch_panel_to(PanelId::Log);
    let state = app.state_mut();
    state.latest_events = VecDeque::from([
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]);

    // 先渲染以设置 last_area
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    // 在主面板区域(80x24 默认布局)滚动
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 10,
        modifiers: event::KeyModifiers::NONE,
    });

    // 滚动 Down 在 Log 面板中选择下一条事件
    // 由于 selected 初始为 0,ScrollDown 应使其变为 1
    // 但面板状态无法直接从 app 访问,这里只验证不 panic
    Ok(())
}

#[test]
fn test_mouse_tab_click_switches_panel() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 先渲染以设置 last_area
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    // M3b:标签栏宽度 80,17 个面板(含 Chat),每标签约 4 列。
    // WHY column=5:落在第 2 个标签(index 1 = Parliament)内——tab_width 为 4 或 5 时
    // 5/tab_width 均 = 1,避开边界且不受面板数微调影响。
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 5,
        row: 1,
        modifiers: event::KeyModifiers::NONE,
    });
    assert_eq!(app.current_panel(), PanelId::Parliament);
    Ok(())
}

#[test]
fn test_mouse_command_bar_click_focuses() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 先渲染以设置 last_area
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| app.render(f))?;

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: 20,
        modifiers: event::KeyModifiers::NONE,
    });
    assert_eq!(app.state().input_mode, InputMode::Command);
    Ok(())
}

// ============================================================
// Task 1.16: event_loop poll 与 tick_mode 联动测试
// ============================================================

/// Task 1.16.2:Normal 模式下 poll_duration 返回 100ms
#[test]
fn test_poll_duration_normal_mode() -> Result<(), Box<dyn std::error::Error>> {
    let app = make_app()?;
    // 默认 tick_mode = Normal
    assert_eq!(app.state().tick_mode, crate::types::TickMode::Normal);
    // Normal 模式 poll 间隔应为 100ms(高响应)
    assert_eq!(app.poll_duration(), std::time::Duration::from_millis(100));
    Ok(())
}

/// Task 1.16.2:Eco 模式下 poll_duration 返回 1000ms
#[test]
fn test_poll_duration_eco_mode() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    // 切换到 Eco 模式(低 CPU 占用)
    app.state.tick_mode = crate::types::TickMode::Eco;
    // Eco 模式 poll 间隔应为 1000ms(降低空轮询开销)
    assert_eq!(app.poll_duration(), std::time::Duration::from_millis(1000));
    Ok(())
}

// ============================================================
// Task 1.17.3: dirty_map! 宏声明式映射测试
// ============================================================

/// Task 1.17.3:宏正确生成映射 — 单字段变化标记正确的面板
///
/// 验证 `dirty_map!` 宏展开后,`quest_list` 字段变化能正确标记 Quest + Health
/// 两个面板(单字段 → 多面板映射)。先 update() 同步 state 与快照,清 dirty,
/// 再修改 state 字段触发 mark_dirty_panels_from_snapshot 的宏路径。
#[test]
fn test_dirty_map_macro_generates_correct_mappings() -> Result<(), Box<dyn std::error::Error>> {
    // 用默认快照创建 app,update() 后 state 与快照一致
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    // 清除首次 update 产生的 dirty 标记(default state != default snapshot 会标 dirty)
    app.state_mut().clear_dirty();

    // 修改 state.quest_list,使其与快照不一致(触发宏的 quest_list arm)
    app.state_mut()
        .quest_list
        .push(sample_quest("dirty-q1", "Dirty Quest"));
    // 再次 update:mark_dirty_panels_from_snapshot 经宏检测到 quest_list 变化
    app.update();

    // 宏应标记 Quest(直接绑定)和 Health(Active Quests 指标派生自 quest_list.len())
    assert!(
        app.state().is_dirty(PanelId::Quest),
        "quest_list 变化应标记 Quest 面板 dirty"
    );
    assert!(
        app.state().is_dirty(PanelId::Health),
        "quest_list 变化应标记 Health 面板 dirty"
    );
    Ok(())
}

/// Task 1.17.3:多字段 OR 逻辑 — 任一字段变化均触发面板(新增字段自动生效)
///
/// 验证 `dirty_map!` 宏的 multi-field OR 语义:`budget` + `budget_history` 任一变化
/// 都标记 Budget 面板。这验证了"新增字段自动生效"——映射表 arm 中的每个字段
/// 都独立参与 `||` 比较,无需额外接线。
#[test]
fn test_dirty_map_macro_multi_field_or_logic() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    app.state_mut().clear_dirty();

    // 修改 state.budget(对应快照的 budget_metrics)—— OR 逻辑的第一个字段
    app.state_mut().budget = BudgetMetrics {
        total_consumption: 999.0,
        remaining_budget: 1.0,
        utilization_rate: 0.99,
        current_tier: "Critical".into(),
        coefficient: 0.99,
        is_exceeded: true,
        alert: None,
    };
    app.update();
    assert!(
        app.state().is_dirty(PanelId::Budget),
        "budget 变化应标记 Budget 面板 dirty"
    );

    // 清除 dirty,修改另一个字段 budget_history—— OR 逻辑的第二个字段
    app.state_mut().clear_dirty();
    app.state_mut().budget_history = vec![50u64, 60, 70];
    app.update();
    assert!(
        app.state().is_dirty(PanelId::Budget),
        "budget_history 变化也应标记 Budget 面板 dirty(OR 逻辑)"
    );
    Ok(())
}

/// Task 1.17.3:多面板标记稳定性 — 单字段变化标记多面板,不误伤无关面板
///
/// 验证 `dirty_map!` 宏的 multi-panel 语义:`latest_events` 变化同时标记
/// Parliament + Log + EventStream 三个面板,且不误标记无关面板(如 Quest)。
/// 这验证了"旧字段删除不报错"——映射表各 arm 独立,面板间互不干扰,
/// 删除/修改某个 arm 不会影响其他 arm 的标记行为。
#[test]
fn test_dirty_map_macro_multi_panel_marking() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    app.state_mut().clear_dirty();

    // 修改 state.latest_events — 单字段映射到 3 个面板
    app.state_mut().latest_events = VecDeque::from([NexusEvent::CacheHit {
        metadata: EventMetadata::new("test-dirty-map"),
        cache_key: "dirty-macro-key".into(),
    }]);
    app.update();

    // 宏应同时标记 Parliament + Log + EventStream 三面板(共享事件流)
    assert!(
        app.state().is_dirty(PanelId::Parliament),
        "latest_events 变化应标记 Parliament 面板 dirty"
    );
    assert!(
        app.state().is_dirty(PanelId::Log),
        "latest_events 变化应标记 Log 面板 dirty"
    );
    assert!(
        app.state().is_dirty(PanelId::EventStream),
        "latest_events 变化应标记 EventStream 面板 dirty"
    );

    // 不应误标记无关面板(quest_list 未变,Quest 不应 dirty)—— 验证 arm 间隔离
    assert!(
        !app.state().is_dirty(PanelId::Quest),
        "latest_events 变化不应标记 Quest 面板 dirty(无映射)"
    );
    Ok(())
}
