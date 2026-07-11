//! Chimera TUI 集成测试 — 验证布局渲染、输入模式切换与键盘事件处理
//!
//! 对应 Week 8 Task 5(测试体系补齐)
//! 架构层:L10 Interface
//!
//! # 测试目标
//! - 验证多面板布局渲染(ratatui TestBackend 内存渲染,无需真实终端)
//! - 验证输入模式切换(Tab/Shift+Tab/数字键 1-5)
//! - 验证键盘事件处理(crossterm 0.28 KeyEvent::new 双参数 API)
//!
//! # 设计约束
//! - WHY TestBackend 而非 CrosstermBackend:TestBackend 为内存后端,
//!   不依赖真实终端,CI 环境可运行(无 TTY)
//! - WHY KeyEvent::new(code, modifiers):crossterm 0.28 API 变更,
//!   旧的单参数构造已废弃,kind 默认为 Press,state 默认为 NONE
//! - WHY 测试 Release 事件过滤:Windows 平台 crossterm 会触发 Release,
//!   必须过滤避免重复响应(§4.4 平台兼容性)

#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use chimera_tui::config::Theme;
use chimera_tui::{PanelKind, TuiApp, TuiConfig};

// ============================================================
// 辅助函数
// ============================================================

/// 构造测试用 TUI 应用(默认配置)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 在 TestBackend 上渲染应用,返回渲染后的字符串内容
fn render_to_string(app: &TuiApp, width: u16, height: u16) -> String {
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

// ============================================================
// A. 布局渲染测试
// ============================================================

#[test]
fn test_tui_layout_rendering() {
    // WHY TestBackend:验证 render() 产出非空内容,包含核心 UI 元素
    let app = make_app();
    let content = render_to_string(&app, 80, 24);

    // 验证:渲染内容包含面板标签(顶部 Tabs)
    assert!(
        content.contains("Quest") || content.contains("Panels"),
        "rendered layout should contain panel tabs, got: {}",
        &content[..content.len().min(200)]
    );

    // 验证:渲染内容包含状态栏(底部)
    assert!(
        content.contains("Panel:") || content.contains("Running:"),
        "rendered layout should contain status bar"
    );

    // 验证:渲染内容包含当前面板标题(Quest Tasks)
    assert!(
        content.contains("Quest Tasks"),
        "rendered layout should contain Quest panel title"
    );
}

#[test]
fn test_tui_layout_rendering_all_panels() {
    // WHY 全面板渲染:验证 5 个面板都能正确渲染各自内容
    let panels = [
        PanelKind::Quest,
        PanelKind::Parliament,
        PanelKind::Budget,
        PanelKind::Log,
        PanelKind::Help,
    ];

    for panel in panels {
        let mut app = make_app();
        app.state_mut().switch_to(panel);
        let content = render_to_string(&app, 80, 24);

        // 每个面板渲染后应包含面板标题
        assert!(
            content.contains(panel.title().trim()),
            "panel {:?} should render its title '{}'",
            panel,
            panel.title().trim()
        );
    }
}

#[test]
fn test_tui_layout_rendering_theme_switch() {
    // WHY 主题切换:验证 Light/Dark 主题下都能正常渲染
    let dark_app = make_app();
    let dark_content = render_to_string(&dark_app, 80, 24);
    assert!(
        !dark_content.is_empty(),
        "Dark theme should render non-empty content"
    );

    let light_app = TuiApp::new(TuiConfig {
        theme: Theme::Light,
        ..Default::default()
    })
    .unwrap();
    let light_content = render_to_string(&light_app, 80, 24);
    assert!(
        !light_content.is_empty(),
        "Light theme should render non-empty content"
    );

    // 两次渲染内容应包含相同的核心元素(主题只影响颜色,不影响布局)
    assert!(
        dark_content.contains("Quest") && light_content.contains("Quest"),
        "both themes should render Quest panel"
    );
}

// ============================================================
// B. 输入模式切换测试
// ============================================================

#[test]
fn test_tui_input_mode_switching() {
    // WHY 输入模式:验证 Tab/Shift+Tab/数字键切换面板
    let mut app = make_app();
    assert_eq!(app.state().current_panel, PanelKind::Quest);

    // Tab:Quest → Parliament
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.state().current_panel, PanelKind::Parliament);

    // Tab:Parliament → Budget
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.state().current_panel, PanelKind::Budget);

    // Shift+Tab(BackTab):Budget → Parliament
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.state().current_panel, PanelKind::Parliament);

    // 数字键 5:跳转到 Help
    app.handle_key_event(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
    assert_eq!(app.state().current_panel, PanelKind::Help);

    // 数字键 1:跳转回 Quest
    app.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(app.state().current_panel, PanelKind::Quest);
}

#[test]
fn test_tui_input_mode_circular_navigation() {
    // WHY 循环导航:验证 Quest → ... → Help → Quest 的完整循环
    let mut app = make_app();

    // 连续 Tab 5 次应回到原点(Quest → Parliament → Budget → Log → Help → Quest)
    for expected in [
        PanelKind::Parliament,
        PanelKind::Budget,
        PanelKind::Log,
        PanelKind::Help,
        PanelKind::Quest,
    ] {
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.state().current_panel,
            expected,
            "circular Tab navigation failed at {:?}",
            expected
        );
    }

    // Shift+Tab 反向循环 5 次也应回到原点
    for expected in [
        PanelKind::Help,
        PanelKind::Log,
        PanelKind::Budget,
        PanelKind::Parliament,
        PanelKind::Quest,
    ] {
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(
            app.state().current_panel,
            expected,
            "circular Shift+Tab navigation failed at {:?}",
            expected
        );
    }
}

#[test]
fn test_tui_input_mode_direct_jump() {
    // WHY 直接跳转:验证数字键 1-5 直接跳转到对应面板
    let mut app = make_app();

    let cases = [
        ('1', PanelKind::Quest),
        ('2', PanelKind::Parliament),
        ('3', PanelKind::Budget),
        ('4', PanelKind::Log),
        ('5', PanelKind::Help),
    ];

    for (ch, expected_panel) in cases {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        assert_eq!(
            app.state().current_panel,
            expected_panel,
            "key '{}' should jump to {:?}",
            ch,
            expected_panel
        );
    }
}

// ============================================================
// C. 键盘事件处理测试
// ============================================================

#[test]
fn test_tui_keyboard_event_handling() {
    // WHY 键盘事件:crossterm 0.28 KeyEvent::new(code, modifiers) 双参数
    let mut app = make_app();

    // q 键:退出应用
    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.state().running, "'q' should quit the app");

    // Esc 键:也退出
    let mut app2 = make_app();
    app2.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app2.state().running, "Esc should quit the app");
}

#[test]
fn test_tui_keyboard_release_event_ignored() {
    // WHY Release 过滤:Windows crossterm 会触发 Release 事件,
    // handle_key_event 必须过滤避免重复响应(平台兼容性)
    let mut app = make_app();

    // Release 事件应被忽略 — 用 new_with_kind 显式构造 Release
    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert!(
        app.state().running,
        "Release event should be ignored (Windows compatibility)"
    );

    // 同样,Tab 的 Release 也应被忽略
    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Tab,
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert_eq!(
        app.state().current_panel,
        PanelKind::Quest,
        "Tab Release should not switch panel"
    );
}

#[test]
fn test_tui_keyboard_input_buffer() {
    // WHY 输入缓冲:验证 ASCII 可打印字符进入 input_buffer,Backspace 删除
    let mut app = make_app();

    // 输入 "hello"
    for c in ['h', 'e', 'l', 'l', 'o'] {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "hello");

    // Backspace 删除最后一个字符
    app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.state().input_buffer, "hell");

    // 空格也应被接受
    app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.state().input_buffer, "hell ");
}

#[test]
fn test_tui_keyboard_non_printable_ignored() {
    // WHY 非可打印字符:验证 Enter/Arrow 等非可打印键不进入 input_buffer
    let mut app = make_app();

    // Enter 不应进入 input_buffer
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.state().input_buffer.is_empty(),
        "Enter should not enter input_buffer"
    );

    // 方向键不应进入 input_buffer
    app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(
        app.state().input_buffer.is_empty(),
        "Arrow keys should not enter input_buffer"
    );
}

#[test]
fn test_tui_render_after_keyboard_input() {
    // WHY 渲染+输入联合:验证键盘输入后渲染仍能正常工作
    let mut app = make_app();

    // 输入一些字符
    for c in "test".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    // 切换面板
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    // 渲染应正常工作,不 panic
    // WHY 宽度 120:状态栏字符串较长(面板/Quest/事件/帧/输入缓冲),
    // 80 列会导致 "Input: test" 被截断;120 列确保完整显示。
    let content = render_to_string(&app, 120, 24);
    assert!(
        content.contains("Parliament"),
        "render after input should show Parliament panel"
    );
    assert!(
        content.contains("test"),
        "render after input should show input buffer content, got: {content:?}"
    );
}
