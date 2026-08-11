//! M3a InputRouter 完整接线 + Insert 模式 — 集成测试(ADR-029,v3.1 §4.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **黑盒事件流**:经 `TuiApp::handle_key_event` 公共 API 驱动,验证对外可观测行为
//!   (`input_mode`/`palette_is_open`/`current_panel`/`running`),不触碰私有字段。
//! - **决策 B 核心**:`:` 打开命令栏(`InputMode::Command`)、Ctrl+P 打开命令面板 overlay,
//!   二者为独立入口不可混同——这是 M3a 保留 `:` 带参命令能力的关键契约。
//! - **零回归锚点**:数字/F 键/Tab/g 前缀/主题/布局经 InputRouter 决策 + 既有 app 方法执行,
//!   效果与旧 `handle_global_key` 逐键一致;`gq` 不误退出是 GPrefix 退出态重映射的关键验证。

#![forbid(unsafe_code)]

use chimera_tui::{InputMode, PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造默认 TuiApp(无 event-bus,内存桩数据源)
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

/// 无修饰符按键
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Ctrl+<char> 组合键
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// 逐字符送入(用于 Insert / 命令栏输入)
fn type_chars(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
}

/// 在内存后端渲染一次,验证不 panic 且产出非空(用于渲染路径回归)
fn render_once(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

// ============================================================
// 决策 B:`:` 命令栏 与 Ctrl+P palette 为两个独立入口
// ============================================================

#[test]
fn colon_opens_command_bar_not_palette() {
    // Concord W2:`:` 进入斜杠命令模式(废弃窗口期别名,不再是 vi 式命令栏)
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char(':')));
    assert_eq!(
        app.state().input_mode,
        InputMode::Slash,
        "`:` 应进入斜杠命令模式(InputMode::Slash)"
    );
    assert!(!app.palette_is_open(), "`:` 不应打开 palette overlay");
}

#[test]
fn ctrl_p_opens_palette_not_command_bar() {
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "Ctrl+P 应打开 palette overlay");
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "Ctrl+P 不应进入命令栏,input_mode 仍为 Normal"
    );
}

#[test]
fn colon_command_bar_still_parses_parameterized_command() {
    // 决策 B 的价值:保留 `:` 带参命令(此处验证 `:budget` 面板切换仍可用)
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char(':')));
    type_chars(&mut app, "budget");
    assert_eq!(app.state().input_buffer, "budget");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.current_panel(),
        PanelId::Budget,
        "`:budget` 应切到 Budget"
    );
    assert_eq!(app.state().input_mode, InputMode::Normal);
}

#[test]
fn slash_enters_search_mode() {
    // Concord W2:`/` 翻转为斜杠命令第一入口;原搜索语义由 `/search` 命令承接
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('/')));
    assert_eq!(
        app.state().input_mode,
        InputMode::Slash,
        "`/` 应进入斜杠命令模式"
    );
}

// ============================================================
// Insert 模式(M3a):进入 / 缓冲 / 退格 / 退出 / 提交占位
// ============================================================

#[test]
fn i_enters_insert_then_esc_exits() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i')));
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "`i` 应进入 Insert"
    );
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "Insert 下 Esc 应退回 Normal"
    );
    assert!(
        app.state().input_buffer.is_empty(),
        "退出 Insert 应清空缓冲"
    );
}

#[test]
fn insert_buffers_chars_and_backspace() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i')));
    type_chars(&mut app, "hi");
    assert_eq!(app.state().input_buffer, "hi", "Insert 应逐字符进缓冲");
    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(app.state().input_buffer, "h", "退格应删末尾字符");
    // Insert 模式下渲染底部输入行不应 panic
    assert!(!render_once(&mut app).trim().is_empty());
}

#[test]
fn insert_submit_switches_to_chat_and_clears() {
    // M3b:Submit 非空输入 → 自动切到 Chat 面板 + 清空缓冲 + 保持 Insert(REPL)。
    // 默认无 event_bus(make_app),不发布事件也不 panic;面板切换与缓冲清空照常。
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i')));
    type_chars(&mut app, "hello");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.state().input_buffer.is_empty(), "Submit 应清空输入缓冲");
    assert_eq!(
        app.current_panel(),
        PanelId::Chat,
        "Submit 应自动切到 Chat 面板"
    );
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "Submit 后仍停留 Insert(chat REPL)"
    );
    assert!(app.state().running, "Submit 不应退出应用");
}

#[test]
fn insert_ctrl_l_stays_in_insert() {
    // Insert 下 Ctrl+L 允许(中英切换 GlobalAction),但不应打断输入流(仍在 Insert)
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i')));
    app.handle_key_event(ctrl('l'));
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "Ctrl+L 不应退出 Insert"
    );
}

// ============================================================
// 零回归锚点:router 决策 + 既有 app 方法执行,效果不变
// ============================================================

#[test]
fn number_key_jumps_panel_via_router() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('3')));
    assert_eq!(app.current_panel(), PanelId::Budget);
}

#[test]
fn tab_cycles_focus_via_router() {
    let mut app = make_app();
    let before = app.current_panel();
    app.handle_key_event(key(KeyCode::Tab));
    assert_ne!(app.current_panel(), before, "Tab 应切换焦点面板");
}

#[test]
fn g_prefix_jumps_extended_panel() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('1')));
    assert_eq!(
        app.current_panel(),
        PanelId::EventStream,
        "g+1 应跳转扩展面板 EventStream"
    );
}

#[test]
fn gg_scrolls_top_no_panic() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('g')));
    assert!(app.state().running, "gg 应正常执行(滚动到顶)不退出");
    assert!(!render_once(&mut app).trim().is_empty());
}

#[test]
fn gq_does_not_quit() {
    // 关键回归:`g` 后的非预期次键 `q` 应按面板级委托处理,不触发 Normal 的 `q`→Quit,
    // 与旧 handle_global_key `_ => return false` 行为一致(GPrefix 退出态重映射为 FocusPanel)。
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.state().running, "`gq` 不应退出应用(q 交面板级处理)");
}

#[test]
fn layout_key_cycles_layout_mode() {
    let mut app = make_app();
    let before = app.state().layout_mode;
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_ne!(app.state().layout_mode, before, "`l` 应循环切换布局模式");
}

#[test]
fn theme_key_no_panic_and_renders() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('t')));
    assert!(app.state().running);
    assert!(
        !render_once(&mut app).trim().is_empty(),
        "主题切换后渲染应正常"
    );
}

#[test]
fn backslash_toggles_view_mode() {
    // Concord W3 T3.4:`\` 复用为 Chat⇄Dashboard 视图模式互切
    // (原 companion 键语义迁移;view.toggle_companion 改经命令面板/
    // 程序化入口访问)。make_app 显式置 Dashboard,按 `\` 应切到 Chat。
    let mut app = make_app();
    assert_eq!(app.state().view_mode, chimera_tui::ViewMode::Dashboard);
    app.handle_key_event(key(KeyCode::Char('\\')));
    assert_eq!(
        app.state().view_mode,
        chimera_tui::ViewMode::Chat,
        "`\\` 应切到会话模式"
    );
    app.handle_key_event(key(KeyCode::Char('\\')));
    assert_eq!(
        app.state().view_mode,
        chimera_tui::ViewMode::Dashboard,
        "再按 `\\` 应切回仪表盘"
    );
}

#[test]
fn quit_via_router_q_and_esc() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app.state().running, "`q` 应退出应用");

    let mut app2 = make_app();
    app2.handle_key_event(key(KeyCode::Esc));
    assert!(!app2.state().running, "Esc 应退出应用");
}
