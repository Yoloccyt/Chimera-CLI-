//! TUI 应用核心测试 — 应用初始化/面板切换/键盘事件/渲染/主题/布局/弹窗/数据接入/鼠标事件测试
//!
//! Task 1.15.5:从 mod.rs 抽离内联测试到独立文件,使 mod.rs < 800 行。
//!
//! 对应架构层:L10 Interface

use std::collections::VecDeque;
use std::sync::Arc;

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
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: crate::types::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })?;
    // Concord W3:遗留测试断言 Dashboard 布局;Chat 为第一默认视图(ADR-076)
    app.state_mut().view_mode = crate::types::ViewMode::Dashboard;
    Ok(app)
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
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        Ok(Arc::new(self.snapshot.clone()))
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
    // Concord T1.4:FocusManager 注册序派生自 PanelId::REGISTERED_FOCUS_ORDER;
    // FC-05:Quest 的上一个 = 环尾 ExperienceCardViz(InjectionStrategy 已下线)。
    assert_eq!(app.current_panel(), PanelId::ExperienceCardViz);
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
    // Concord W2:`:` 进入斜杠命令模式(废弃窗口别名);未命中命令表的
    // "budget" 经 Legacy 回退继续完成面板切换(零功能断裂)。
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Slash);

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
    // Concord W2:`/` 翻转为斜杠命令第一入口;原搜索语义由 `/search` 命令承接。
    let mut app = make_app()?;
    app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), event::KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Slash);

    for c in "search Error".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "search Error");

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
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snap)),
    )?;

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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
        latest_events: std::sync::Arc::new(VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k1".into(),
        }])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();

    assert_eq!(app.state().quest_list.len(), 1);
    assert_eq!(app.state().quest_list[0].title, "Data Driven Quest");
    assert_eq!(app.state().budget.current_tier, "Critical");
    assert_eq!(app.state().latest_events.len(), 1);
    Ok(())
}

/// P-A(评估报告 v2):update() 必须与 DataSnapshot 共享 `latest_events` 的 Arc,
/// 不再对事件流做 ≤256 事件的深拷贝;revision 不变的二次 update 走早退路径,
/// 共享关系保持,且面板只读消费(Deref 迭代)不影响引用计数。
#[test]
fn test_update_shares_latest_events_arc_without_deep_copy() -> Result<(), Box<dyn std::error::Error>>
{
    let snapshot = DataSnapshot {
        revision: 1,
        latest_events: Arc::new(VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "arc-share".into(),
        }])),
        ..Default::default()
    };
    // 先保留一份 Arc 句柄,用于断言 update 后与状态指向同一分配
    let events_arc = Arc::clone(&snapshot.latest_events);

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();

    // 零拷贝:state.latest_events 与快照内 Arc 共享同一堆分配
    assert!(
        Arc::ptr_eq(&events_arc, &app.state().latest_events),
        "update 应 Arc 共享事件流,而非深拷贝"
    );
    assert_eq!(app.state().latest_events.len(), 1);

    // 只读消费(迭代)不改变 Arc 引用计数
    let count_before = Arc::strong_count(&events_arc);
    let total: usize = app.state().latest_events.iter().count();
    assert_eq!(total, 1);
    assert_eq!(Arc::strong_count(&events_arc), count_before);

    // 同 revision 二次 update:早退路径不重绑,共享关系保持
    app.update();
    assert!(Arc::ptr_eq(&events_arc, &app.state().latest_events));
    Ok(())
}

#[test]
fn test_update_sets_status_message_on_error() -> Result<(), Box<dyn std::error::Error>> {
    /// 总是返回错误的数据源
    #[derive(Debug)]
    struct FailingDataSource;

    impl TuiDataSource for FailingDataSource {
        fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
            Err(TuiError::DataSource("forced failure".into()))
        }

        fn config(&self) -> &DataSourceConfig {
            static CONFIG: std::sync::OnceLock<DataSourceConfig> = std::sync::OnceLock::new();
            CONFIG.get_or_init(DataSourceConfig::default)
        }
    }

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(FailingDataSource),
    )?;
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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
    // WHY locale 锁:断言依赖中文文案“系统日志”,并行测试切换语言会偶发失败
    // (既有 flaky,2026-08-07 修复)
    let _locale_guard = crate::i18n::locale_test_guard();
    let snapshot = DataSnapshot {
        latest_events: std::sync::Arc::new(VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        }])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
    // WHY 压缩空白:TestBackend 逐格渲染中文时汉字间含空格("系 统 日 志")
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(compact.contains("系统日志"));
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
    state.latest_events = std::sync::Arc::new(VecDeque::from([
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]));

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

    // Phase 10:标签栏宽度 80,27 个面板,tab_width = 80/27 = 2 列。
    // WHY column=3:3/2 = 1,落在第 2 个标签(index 1 = Parliament)内,
    // 避开边界(2/2=1 与 4/2=2 均为边界列)。
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
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
    // I-B(2026-09-06 复评):底栏点击改入 Slash 模式(与 `:`/`/` 斜杠入口统一)
    assert_eq!(app.state().input_mode, InputMode::Slash);
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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snapshot)),
    )?;
    app.update();
    app.state_mut().clear_dirty();

    // 修改 state.latest_events — 单字段映射到 3 个面板
    app.state_mut().latest_events = std::sync::Arc::new(VecDeque::from([NexusEvent::CacheHit {
        metadata: EventMetadata::new("test-dirty-map"),
        cache_key: "dirty-macro-key".into(),
    }]));
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

// ============================================================
// U-1(2026-09-06 复评):responsive_collapse_threshold 生产接线
// ============================================================

#[test]
fn responsive_threshold_controls_companion_folding() {
    // 默认阈值 100:宽 80 < 100 → 次窗格折叠为单窗格
    let app = TuiApp::new(TuiConfig {
        default_view_mode: crate::types::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    let mut app = app;
    app.state_mut().layout_mode = LayoutMode::VimSplit; // VimSplit 内在多窗格
    let narrow = ratatui::layout::Rect::new(0, 0, 80, 24);
    let wide = ratatui::layout::Rect::new(0, 0, 120, 24);
    assert_eq!(
        app.pane_rects(narrow).len(),
        1,
        "窄视口(80 < 默认阈值 100)应折叠次窗格"
    );
    assert_eq!(
        app.pane_rects(wide).len(),
        2,
        "宽视口(120 ≥ 100)应保留 VimSplit 双窗格"
    );

    // 阈值 0 = 禁用自动折叠:窄视口也保留次窗格(配置语义,pane_manager 文档)
    let app0 = TuiApp::new(TuiConfig {
        default_view_mode: crate::types::ViewMode::Dashboard,
        persist_state: false,
        responsive_collapse_threshold: 0,
        ..Default::default()
    })
    .unwrap();
    let mut app0 = app0;
    app0.state_mut().layout_mode = LayoutMode::VimSplit;
    assert_eq!(
        app0.pane_rects(narrow).len(),
        2,
        "阈值 0 应禁用折叠(窄视口保留次窗格)"
    );

    // 自定义阈值 70:宽 80 ≥ 70 → 不折叠(阈值真正驱动行为,死配置回归锁)
    let app70 = TuiApp::new(TuiConfig {
        default_view_mode: crate::types::ViewMode::Dashboard,
        persist_state: false,
        responsive_collapse_threshold: 70,
        ..Default::default()
    })
    .unwrap();
    let mut app70 = app70;
    app70.state_mut().layout_mode = LayoutMode::VimSplit;
    assert_eq!(
        app70.pane_rects(narrow).len(),
        2,
        "阈值 70 时 80 列不应折叠(配置值生效)"
    );
}

// ============================================================
// FC-05: 经验卡片可视化面板接线(组合根 install 路径)
// ============================================================

/// Mock 经验卡片统计提供者 — 固定统计快照
#[derive(Debug)]
struct MockCardStatsProvider(crate::panels::ExperienceCardVizStats);

impl crate::panels::ExperienceCardStatsProvider for MockCardStatsProvider {
    fn global_stats(&self) -> crate::panels::ExperienceCardVizStats {
        self.0.clone()
    }
}

#[test]
fn install_experience_card_provider_wires_real_stats() -> Result<(), TuiError> {
    let mut app = make_app()?;

    // 默认(未接线):面板诚实展示等待提示
    let idx = app
        .panel_index(PanelId::ExperienceCardViz)
        .expect("ExperienceCardViz 应已注册");
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 10));
    app.panels[idx].render(
        &TuiState::new(),
        ratatui::layout::Rect::new(0, 0, 60, 10),
        &mut buf,
    );
    let text: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(
        text.contains("Awaiting stats provider"),
        "默认面板应诚实展示等待提示"
    );

    // 接线:替换为带提供者实例 → 渲染真实统计
    let provider = Arc::new(MockCardStatsProvider(
        crate::panels::ExperienceCardVizStats {
            total_cards: 42,
            evaluated: 7,
            unique_errors: 2,
            method_distribution: vec![("draft_pipeline".into(), 5)],
            best_score: 0.9,
            average_score: 0.6,
        },
    ));
    app.install_experience_card_provider(provider);
    assert_eq!(
        app.panels[idx].id(),
        PanelId::ExperienceCardViz,
        "替换后面板身份不变"
    );

    let mut buf2 = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 10));
    app.panels[idx].render(
        &TuiState::new(),
        ratatui::layout::Rect::new(0, 0, 60, 10),
        &mut buf2,
    );
    let text2: String = buf2.content.iter().map(|c| c.symbol()).collect();
    assert!(
        text2.contains("Total: 42"),
        "接线后应渲染真实统计(Total: 42)"
    );
    assert!(!text2.contains("Awaiting stats provider"));
    Ok(())
}

// ============================================================
// P1(2026-09-09 复评):TuiActionRequested 回执 request_id 归属
// ============================================================

/// 构造带指定数据快照的 app(revision 保持 0,`update()` 恒拷贝,便于桩驱动)
fn make_app_with_snapshot(snapshot: DataSnapshot) -> Result<TuiApp, TuiError> {
    TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snapshot)),
    )
}

/// 守护不变量:**每个派发的请求独立占位**。
///
/// WHY:旧实现用单个 `Option<Instant>` 槽位,连续/并发派发两个动作时后者覆盖前者,
/// 先到的回执会清掉后者的超时计时,形成“失败却显示成功”的静默失败。
#[test]
fn pending_actions_keyed_by_request_id() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    app.dispatch_action(
        "quest.pause",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    app.dispatch_action(
        "quest.cancel",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(
        app.state().pending_actions.len(),
        2,
        "两次派发应在 pending_actions 各占一项"
    );
    let mut keys: Vec<&String> = app.state().pending_actions.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["tui-1", "tui-2"],
        "两次派发的 request_id 必须互不相同"
    );
    Ok(())
}

/// 守护不变量:**回执只清除自己那一条**。
///
/// WHY:并发派发两个请求后,仅其中一条收到 Completed/Failed 时,另一条仍须保留
/// 超时兜底计时,否则它会永久静默(无回执也无超时提示)。
#[test]
fn feedback_removes_only_matching_request() -> Result<(), Box<dyn std::error::Error>> {
    // WHY 结构体更新语法:clippy::field_reassign_with_default 禁止对 Default
    // 实例逐字段赋值(与 state.rs:341 既有范式一致)
    let snapshot = DataSnapshot {
        action_feedback: Some(("paused".to_string(), false)),
        action_feedback_seq: 1,
        action_feedback_request_id: Some("tui-1".to_string()),
        ..Default::default()
    };
    let mut app = make_app_with_snapshot(snapshot)?;

    app.dispatch_action(
        "quest.pause",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    app.dispatch_action(
        "quest.cancel",
        "{}".to_string(),
        event_bus::ActionSource::Palette,
    );
    assert_eq!(app.state().pending_actions.len(), 2);

    app.update();

    assert!(
        !app.state().pending_actions.contains_key("tui-1"),
        "已回执的 tui-1 应被精准移除"
    );
    assert!(
        app.state().pending_actions.contains_key("tui-2"),
        "未回执的 tui-2 必须保留超时计时(否则静默失败)"
    );
    Ok(())
}

/// 守护不变量:**超时只摘除已过期项**,未过期请求继续等待。
///
/// WHY:同帧多条过期只上屏一条告警,但仍须清空全部过期键,避免下一帧重复告警;
/// 未过期项若被连带清空则同样退化为静默失败。
#[test]
fn timeout_reports_and_clears_only_expired() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    let now = std::time::Instant::now();
    app.state_mut()
        .pending_actions
        .insert("tui-1".to_string(), now - std::time::Duration::from_secs(1));
    app.state_mut().pending_actions.insert(
        "tui-2".to_string(),
        now + std::time::Duration::from_secs(60),
    );

    app.check_action_timeout();

    assert_eq!(app.state().pending_actions.len(), 1, "只有过期项应被摘除");
    assert!(
        app.state().pending_actions.contains_key("tui-2"),
        "未过期的 tui-2 仍应等待回执"
    );
    let warned = matches!(
        &app.state().status_message,
        Some((msg, Severity::Warning)) if msg.contains("orchestrator not connected")
    );
    assert!(warned, "过期应上屏编排器未接线警告");
    Ok(())
}

/// 守护不变量:request_id 单调递增(`tui-1`/`tui-2`/`tui-3`),不重复发号。
///
/// WHY:回执配对依赖 request_id 唯一;重复发号会让先到回执误清后发请求。
#[test]
fn action_request_seq_monotonic() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = make_app()?;
    for _ in 0..3 {
        app.dispatch_action(
            "agent.chat",
            r#"{"query":"hi"}"#.to_string(),
            event_bus::ActionSource::Chat,
        );
    }
    assert_eq!(app.state().action_request_seq, 3);
    assert_eq!(app.state().pending_actions.len(), 3);
    for rid in ["tui-1", "tui-2", "tui-3"] {
        assert!(
            app.state().pending_actions.contains_key(rid),
            "request_id {rid} 应存在且唯一"
        );
    }
    Ok(())
}

/// P1 回执归属属性测试(与上方单测互补:单测覆盖典型次数,属性测试覆盖任意次数)
#[cfg(test)]
mod p1_request_id_proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 属性:任意 n 次连续派发,`pending_actions` 恰有 n 项且 `request_id` 两两互异。
        ///
        /// WHY 属性化:回执配对依赖"发号不重复"这一全量不变量,固定 2-3 次的
        /// 单测无法穷尽;由 n ∈ [1,16] 随机采样,用 HashSet 判定两两互异。
        #[test]
        fn dispatched_requests_get_distinct_ids(n in 1usize..=16) {
            let mut app = make_app().unwrap();
            for _ in 0..n {
                app.dispatch_action(
                    "quest.pause",
                    "{}".to_string(),
                    event_bus::ActionSource::Palette,
                );
            }
            prop_assert_eq!(app.state().pending_actions.len(), n, "每条请求独立占位");
            let keys: std::collections::HashSet<&String> =
                app.state().pending_actions.keys().collect();
            prop_assert_eq!(keys.len(), n, "request_id 必须两两互异");
        }
    }
}

// ============================================================
// PS-3(I-3):弹窗内 Ctrl+L 中英切换(此前被 handle_popup_key 吞掉,
// 是 Insert/Slash/palette 之外唯一失效上下文)
// ============================================================

#[test]
fn ctrl_l_toggles_locale_while_popup_open() {
    // 持 guard 钉 Zh:并行 i18n 测试不再干扰,断言确定性成立
    let _guard = crate::i18n::locale_test_guard();
    crate::i18n::set_locale(crate::i18n::Locale::Zh);

    let mut app = make_app().expect("make_app should succeed");
    app.state_mut().popup_stack.push(PopupKind::Confirm {
        prompt: "confirm?".into(),
        on_confirm: "quit".into(),
        confirmed: false,
    });
    assert!(!app.state().popup_stack.is_empty(), "前置:弹窗已打开");

    app.handle_key_event(KeyEvent::new(
        KeyCode::Char('l'),
        event::KeyModifiers::CONTROL,
    ));

    assert_eq!(
        crate::i18n::current_locale(),
        crate::i18n::Locale::En,
        "弹窗内 Ctrl+L 应切换中英(Zh → En)"
    );
    assert!(!app.state().popup_stack.is_empty(), "Ctrl+L 不应关闭弹窗");
}

// ============================================================
// PS-2(F-1):协议握手回执上屏(HandshakeSync → snapshot → status_message)
// ============================================================

#[test]
fn handshake_ack_promotes_to_status_and_state() {
    use crate::types::{HandshakeLevel, HandshakeState};

    // Degraded 回执:Warning 级上屏,携降级项;状态随之落库
    let snap = DataSnapshot {
        handshake: Some(HandshakeState {
            level: HandshakeLevel::Degraded,
            degraded_items: vec!["agent-tree".into(), "overwindow".into()],
            server_version: "2.28.2-omega".into(),
        }),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snap)),
    )
    .expect("app with mock source");

    app.update();

    let st = app.state();
    let hs = st.handshake.as_ref().expect("握手状态应已落库");
    assert_eq!(hs.level, HandshakeLevel::Degraded);
    assert_eq!(hs.degraded_items.len(), 2);

    let (msg, severity) = st
        .status_message
        .as_ref()
        .expect("Degraded 回执应上屏状态栏");
    assert!(
        msg.contains("agent-tree") && msg.contains("overwindow"),
        "Degraded 提示应携降级项, got: {msg}"
    );
    assert_eq!(*severity, Severity::Warning, "Degraded 应为 Warning 级");

    // 同一快照再次 update:revision 未变 → 跳过,不重复上屏(自然去重)
    app.update();
    // 状态保持(无回退),不再额外断言文案变化
    assert!(app.state().handshake.is_some());
}

#[test]
fn refused_ack_promotes_error_severity() {
    use crate::types::{HandshakeLevel, HandshakeState};

    let snap = DataSnapshot {
        handshake: Some(HandshakeState {
            level: HandshakeLevel::Refused,
            degraded_items: Vec::new(),
            server_version: "0.0.1".into(),
        }),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: crate::types::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snap)),
    )
    .expect("app with mock source");

    app.update();

    let (msg, severity) = app
        .state()
        .status_message
        .as_ref()
        .expect("Refused 回执应上屏状态栏");
    assert_eq!(*severity, Severity::Error, "Refused 应为 Error 级");
    assert!(msg.contains("0.0.1"), "提示应携服务端版本, got: {msg}");
}

// ============================================================
// PS-2(F-6):子代理失败聚合 → 状态栏 Error 告警 + 状态落库
// ============================================================

#[test]
fn agent_failure_promotes_to_error_status_and_state() {
    use crate::types::AgentFailureSummary;

    let snap = DataSnapshot {
        agent_failures: vec![AgentFailureSummary {
            from: "agent-a".into(),
            to: "orchestrator".into(),
            task_id: "task-7".into(),
            error: "boom".into(),
            retry_count: 2,
        }],
        agent_failure_total: 3,
        agent_failure_seq: 3,
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            persist_state: false,
            ..Default::default()
        },
        Box::new(MockDataSource::new(snap)),
    )
    .expect("app with mock source");

    app.update();

    let st = app.state();
    assert_eq!(st.agent_failure_total, 3, "累计数应落库");
    assert_eq!(st.agent_failures.len(), 1, "最近失败应落库");

    let (msg, severity) = st
        .status_message
        .as_ref()
        .expect("Critical 子代理失败应上屏状态栏");
    assert_eq!(*severity, Severity::Error, "Critical 失败应为 Error 级");
    assert!(msg.contains("task-7"), "告警应携 task_id, got: {msg}");
    assert!(msg.contains("agent-a"), "告警应携失败方 Agent, got: {msg}");
    assert_eq!(st.last_agent_failure_seq, 3, "上屏游标应推进");

    // 序号未增(无新失败)→ 不重复上屏,避免重试风暴刷屏
    let before = app.state().status_message.clone();
    app.update();
    assert_eq!(
        app.state().status_message,
        before,
        "无新失败时不得重复上屏(告警会被自身覆盖而无法阅读)"
    );
}

// ============================================================
// PS-2(F-7):动作路由分类不变量 —— 本地执行 vs 编排器发布
// ============================================================
//
// 背景:评估报告 F-7 断言"monitor.pause_sampling / viz.switch_dimension 等
// 本地动作仍经 TuiActionRequested 发往编排器 → 悬挂至 2s 超时"。经代码核查
// **该断言不成立**(全仓 `TuiActionRequested` 仅一处构造点 = dispatch_action
// 的 `_ =>` 兜底,本地动作均有本地 arm)。但原 F-7 揭示的**陷阱是真实的**:
// 本地/编排的区分完全依赖"是否写了 arm",新动作漏写即静默变成编排动作。
//
// 本不变量把该隐性约定变为可执行断言:
//   1. 两份清单的并集 == `ActionRegistry` 动作全集(新增动作必须登记);
//   2. 声明为本地者:实调 dispatch_action 后**不得**登记 pending(即未发布);
//   3. 声明为编排者:实调后**必须**登记 pending(即已发布并等待回执)。

/// 本地执行动作(dispatch_action 有本地 arm,不发布事件)
const LOCAL_ACTION_IDS: &[&str] = &[
    "config.edit",
    "export.run",
    "monitor.pause_sampling",
    "monitor.time_window",
    "panel.drill_down",
    "quest.jump",
    "system.open_help",
    "system.toggle_locale",
    "view.apply_saved",
    "view.cycle_companion",
    "view.focus_pane",
    "view.switch_layout",
    "view.toggle_companion",
    "viz.switch_dimension",
];

/// 编排器动作(落入 `_ =>` 兜底,发布 `TuiActionRequested` 等待回执)
const ORCHESTRATED_ACTION_IDS: &[&str] = &[
    "agent.chat",
    "overwindow.run",
    "quest.cancel",
    "quest.pause",
    "quest.resume",
    "quest.start",
];

#[test]
fn every_registered_action_is_classified() {
    let reg = crate::actions::ActionRegistry::with_builtin_domains();
    let mut declared: Vec<&str> = LOCAL_ACTION_IDS
        .iter()
        .chain(ORCHESTRATED_ACTION_IDS.iter())
        .copied()
        .collect();
    declared.sort_unstable();
    let mut actual: Vec<&str> = reg.all().iter().map(|d| d.id).collect();
    actual.sort_unstable();

    let missing: Vec<&&str> = actual.iter().filter(|id| !declared.contains(id)).collect();
    assert!(
        missing.is_empty(),
        "注册表存在未分类动作(必须登记进 LOCAL/ORCHESTRATED 清单): {missing:?}"
    );
    let stale: Vec<&&str> = declared.iter().filter(|id| !actual.contains(id)).collect();
    assert!(
        stale.is_empty(),
        "清单存在已不在注册表的动作(应删除): {stale:?}"
    );
    assert_eq!(declared, actual, "分类清单必须与注册表全集一一对应");
}

#[test]
fn declared_local_actions_never_publish_to_orchestrator() {
    for id in LOCAL_ACTION_IDS {
        let mut app = make_app().expect("make_app should succeed");
        app.dispatch_action(id, "{}".to_string(), event_bus::ActionSource::Panel);
        assert!(
            app.state().pending_actions.is_empty(),
            "本地动作 {id} 不得发布 TuiActionRequested —— \
             否则事件发往无 handler 的编排器、悬挂至 ACTION_TIMEOUT(2s),\
             用户会看到与实际结果不符的超时提示"
        );
    }
}

#[test]
fn declared_orchestrated_actions_publish_and_await_receipt() {
    for id in ORCHESTRATED_ACTION_IDS {
        let mut app = make_app().expect("make_app should succeed");
        app.dispatch_action(id, "{}".to_string(), event_bus::ActionSource::Panel);
        assert_eq!(
            app.state().pending_actions.len(),
            1,
            "编排动作 {id} 必须发布 TuiActionRequested 并登记待回执(反向漂移:\
             若误加本地 arm,该动作将不再抵达 chimera-cli 编排器)"
        );
    }
}
