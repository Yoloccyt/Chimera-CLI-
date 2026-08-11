//! M2 交互接线集成测试 — Ctrl+L 中英切换 + 统一命令面板 overlay(ADR-029,v3.1 M2)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **黑盒事件流**:经 `TuiApp::handle_key_event` 注入按键,验证对外可观测行为
//!   (palette 开关、locale 切换、`running` 状态、渲染文案),不触碰私有字段。
//! - **locale 串行化**:界面语言为全局静态,涉及 locale 的测试用同一 `Mutex`
//!   互斥,避免并行线程相互切换语言导致断言抖动。
//! - **渲染安全**:`TestBackend` 内存渲染,验证 overlay 不 panic 且标题随 locale 呈现。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{current_locale, set_locale, Locale, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化所有会读写全局 locale 的测试,规避并行切换语言造成的断言抖动。
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

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

/// 在内存后端渲染并收集全部 cell 字符(与 integration.rs 快照方式一致)
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
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
// M2.2 统一命令面板开关与键盘路由
// ============================================================

#[test]
fn ctrl_p_opens_palette_and_esc_closes() {
    let mut app = make_app();
    assert!(!app.palette_is_open(), "初始应关闭命令面板");
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "Ctrl+P 应打开命令面板");
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.palette_is_open(), "Esc 应关闭命令面板");
}

#[test]
fn palette_open_routes_q_to_filter_not_global_quit() {
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open());
    // 面板打开时按 'q':应作为检索字符进入面板,而非触发全局退出。
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.state().running, "面板打开时 'q' 不应退出应用");
    assert!(app.palette_is_open(), "面板打开时 'q' 应保持面板开启(过滤)");
}

#[test]
fn palette_enter_dispatches_and_closes() {
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open());
    // Enter 执行选中动作并关闭面板(无 event-bus 时派发降级为状态消息,不 panic)。
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.palette_is_open(), "Enter 执行后应关闭面板");
    assert!(app.state().running, "派发动作不应退出应用");
}

#[test]
fn palette_can_reopen_after_close() {
    // 复用既有模型:关闭再打开应正常工作(不重建注册表也不 panic)。
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "关闭后应能再次打开命令面板");
}

// ============================================================
// M2.1 Ctrl+L 运行时中英切换
// ============================================================

#[test]
fn ctrl_l_toggles_locale_between_zh_and_en() {
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::Zh);
    let mut app = make_app();
    app.handle_key_event(ctrl('l'));
    assert_eq!(current_locale(), Locale::En, "Ctrl+L 应从中文切到英文");
    app.handle_key_event(ctrl('l'));
    assert_eq!(current_locale(), Locale::Zh, "再次 Ctrl+L 应切回中文");
    set_locale(Locale::Zh); // 复位,避免污染其他测试
}

// ============================================================
// 渲染安全 + locale 感知
// ============================================================

#[test]
fn palette_renders_localized_title_without_panic() {
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::En);
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    // 英文标题为纯 ASCII 单宽字符,在标题行连续出现,便于稳定断言。
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Command Palette"),
        "命令面板英文标题应出现在渲染输出中"
    );
    set_locale(Locale::Zh); // 复位
}

#[test]
fn render_without_palette_omits_title() {
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::En);
    let mut app = make_app();
    // 未打开面板:渲染不应包含命令面板标题。
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        !content.contains("Command Palette"),
        "未打开命令面板时不应渲染面板标题"
    );
    set_locale(Locale::Zh); // 复位
}

// ============================================================
// M2 增量2:三入口统一派发桥接(命令面板 Enter → 真实效果)
// ============================================================

/// 逐字符把查询串输入命令面板(面板须已打开)
///
/// WHY 走 `handle_key_event`:与真实交互一致(字符经面板路由进入检索缓冲),
/// fuzzy_search 按 action_id 子串匹配,选择唯一候选后 Enter 即可精准派发。
fn type_query(app: &mut TuiApp, q: &str) {
    for c in q.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
}

#[test]
fn palette_dispatch_toggle_locale_actually_switches() {
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::Zh);
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    type_query(&mut app, "locale"); // 唯一命中 id `system.toggle_locale`
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        current_locale(),
        Locale::En,
        "命令面板执行 system.toggle_locale 应真实切换语言(而非仅发事件)"
    );
    assert!(!app.palette_is_open(), "执行后面板应关闭");
    set_locale(Locale::Zh); // 复位
}

#[test]
fn palette_dispatch_switch_layout_changes_mode() {
    let mut app = make_app();
    // 不依赖具体默认值:只断言布局模式发生了变化(as_str 与 locale 无关)
    let before = app.state().layout_mode.as_str().to_string();
    app.handle_key_event(ctrl('p'));
    type_query(&mut app, "switch_layout"); // 唯一命中 id `view.switch_layout`
    app.handle_key_event(key(KeyCode::Enter));
    assert_ne!(
        app.state().layout_mode.as_str(),
        before.as_str(),
        "命令面板执行 view.switch_layout 应循环切换布局模式"
    );
    assert!(!app.palette_is_open(), "执行后面板应关闭");
}

#[test]
fn palette_dispatch_open_help_pushes_overlay() {
    let mut app = make_app();
    assert!(app.state().popup_stack.is_empty(), "初始无弹窗");
    app.handle_key_event(ctrl('p'));
    type_query(&mut app, "open_help"); // 唯一命中 id `system.open_help`
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.palette_is_open(), "执行后面板应关闭");
    assert!(
        !app.state().popup_stack.is_empty(),
        "system.open_help 应压入帮助 overlay(本地即时效果)"
    );
}

#[test]
fn palette_dispatch_unknown_action_falls_back_without_panic() {
    let mut app = make_app();
    app.handle_key_event(ctrl('p'));
    type_query(&mut app, "chat"); // 命中 agent.chat:无本地通路,回退发事件
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.palette_is_open(), "执行后面板应关闭");
    assert!(app.state().running, "回退派发不应退出应用");
    assert!(
        app.state().popup_stack.is_empty(),
        "回退动作不产生本地 overlay"
    );
}
