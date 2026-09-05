//! 斜杠命令端到端集成测试 — Concord W2(T2.1~T2.3 闭环验证)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - `/` 与 `:` 均进入 Slash 模式;`:` 触发一次性弃用提示(R1 缓解);
//! - Instant 层:/theme 主题循环生效、/search 设置与清除过滤、/panel 切面板;
//! - Legacy 回退:未命中命令表的输入经遗留 parse_command 处理(面板切换/
//!   pause 确认弹窗),零功能断裂;
//! - Agent 层:/review 提示词模板预置进 composer(Insert 模式);
//! - 诚实反馈:未知命令报错、未接线命令提示后续波次;
//! - Tab 前缀补全与补全 overlay 渲染。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{InputMode, PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化涉及全局 locale 的测试(与 integration.rs 同范式,消除竞态 flaky)
/// WHY into_inner 容错:并行用例若持锁 panic 会中毒,其余用例不应连带失败
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 构造测试用 TUI 应用(默认配置)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 发送单个按键
fn press(app: &mut TuiApp, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

/// 逐字符输入字符串
fn type_str(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

/// 输入并提交一条斜杠命令(不含前导 `/`)
fn slash_submit(app: &mut TuiApp, cmd: &str) {
    press(app, KeyCode::Char('/'));
    type_str(app, cmd);
    press(app, KeyCode::Enter);
}

// ============================================================
// A. 模式进入与 `:` 弃用提示(R1 缓解)
// ============================================================

#[test]
fn slash_enters_slash_mode_without_deprecation() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    assert_eq!(
        app.state().input_mode,
        InputMode::Slash,
        "'/' 应进 Slash 模式"
    );
    assert!(
        !app.state().colon_deprecation_shown,
        "'/' 进入不应触发弃用提示标记"
    );
}

#[test]
fn colon_enters_slash_mode_with_one_time_deprecation() {
    let _guard = locale_guard();
    let mut app = make_app();

    press(&mut app, KeyCode::Char(':'));
    assert_eq!(
        app.state().input_mode,
        InputMode::Slash,
        "':' 应同进 Slash 模式"
    );
    assert!(
        app.state().colon_deprecation_shown,
        "首次 ':' 应置位弃用标记"
    );
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("v2.27.0") || msg.contains("deprecated") || msg.contains("弃用"),
        "应展示弃用提示,got: {msg}"
    );

    // 一次性语义:清除状态消息后再次 ':' 不应重复提示
    app.state_mut().status_message = None;
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char(':'));
    assert!(
        app.state().status_message.is_none(),
        "弃用提示仅一次,第二次 ':' 不应再提示"
    );
}

// ============================================================
// B. Instant 层即时效果
// ============================================================

#[test]
fn theme_command_cycles_theme() {
    let _guard = locale_guard();
    let mut app = make_app();
    // 默认 Dark → /theme → Light(状态栏回显主题名)
    slash_submit(&mut app, "theme");
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("light"),
        "/theme 应循环到 Light 并回显,got: {msg}"
    );
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "执行后应回 Normal"
    );
}

#[test]
fn search_command_sets_and_clears_filter() {
    let mut app = make_app();
    // /search <kw> 设置全局关键字过滤(承接原 EnterSearch 语义)
    slash_submit(&mut app, "search hello");
    assert_eq!(
        app.state().filter_keyword.as_deref(),
        Some("hello"),
        "/search 应设置过滤关键字"
    );
    // 空参清除过滤
    slash_submit(&mut app, "search");
    assert!(
        app.state().filter_keyword.is_none(),
        "/search 空参应清除过滤"
    );
}

#[test]
fn panel_command_switches_panel_by_name() {
    let mut app = make_app();
    slash_submit(&mut app, "panel Budget");
    assert_eq!(
        app.current_panel(),
        PanelId::Budget,
        "/panel Budget 应切换到 Budget 面板(大小写不敏感)"
    );
}

#[test]
fn filter_level_invalid_args_report_errors() {
    let mut app = make_app();
    slash_submit(&mut app, "filter bogus_topic");
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("invalid topic"),
        "非法 topic 应报错,got: {msg}"
    );

    slash_submit(&mut app, "level bogus");
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("invalid level"),
        "非法 level 应报错,got: {msg}"
    );
}

// ============================================================
// C. Legacy 回退(零功能断裂)
// ============================================================

#[test]
fn unknown_input_falls_back_to_legacy_panel_switch() {
    let mut app = make_app();
    // "budget" 不在斜杠命令表 → Legacy 回退 → 遗留面板切换命令
    slash_submit(&mut app, "budget");
    assert_eq!(
        app.current_panel(),
        PanelId::Budget,
        "遗留面板切换词应经回退继续工作"
    );
}

#[test]
fn quest_pause_slash_bridges_legacy_confirm_popup() {
    let mut app = make_app();
    // /quest pause <id> → Legacy 桥接 "pause <id>" → 确认弹窗(M4 review fix 保留)
    slash_submit(&mut app, "quest pause q-1");
    assert!(
        !app.state().popup_stack.is_empty(),
        "quest pause 应弹出确认框(破坏性操作二次确认)"
    );
}

#[test]
fn unknown_command_reports_honest_error() {
    let mut app = make_app();
    slash_submit(&mut app, "frobnicate");
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("unknown command"),
        "未知命令应诚实报错,got: {msg}"
    );
}

#[test]
fn unwired_command_gives_honest_todo() {
    let _guard = locale_guard();
    let mut app = make_app();
    // /compact 已登记但后端未接线(W3+)→ 诚实提示而非伪造执行
    slash_submit(&mut app, "compact");
    let msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("接线") || msg.contains("wired"),
        "未接线命令应给诚实反馈,got: {msg}"
    );
}

// ============================================================
// D. Agent 层模板入 composer
// ============================================================

#[test]
fn agent_command_prefills_composer_template() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _guard = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let mut app = make_app();
    slash_submit(&mut app, "review 登录模块");
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "Agent 命令应进 Insert 模式(composer)"
    );
    let buf = app.state().input_buffer.clone();
    assert!(
        buf.contains("评审") && buf.contains("登录模块"),
        "composer 应预置模板 + 参数,got: {buf}"
    );
}

// ============================================================
// E. 补全交互(Tab 补全 + overlay 渲染)
// ============================================================

#[test]
fn tab_completion_inserts_selected_command_name() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "the");
    press(&mut app, KeyCode::Tab);
    assert_eq!(
        app.state().input_buffer,
        "theme ",
        "Tab 应补全选中候选命令词 + 尾随空格"
    );
}

#[test]
fn up_down_moves_selection_within_candidates() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "theme");
    let before = app.state().slash_selected;
    press(&mut app, KeyCode::Down);
    // 单一候选(theme 精确过滤后含 theme 相关条目)时循环回自身;断言不越界
    let reg_len = chimera_tui::slash_candidates(
        &chimera_tui::SlashCommandRegistry::with_builtin_commands(),
        "theme",
    )
    .len();
    assert!(
        app.state().slash_selected < reg_len.max(1),
        "选中项不得越界"
    );
    let _ = before;
}

#[test]
fn slash_overlay_renders_candidates_above_input_bar() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _guard = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "the");

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    // 归一化:去除填充/宽字符续格空白后再断言(CJK 宽字符在缓冲中占两格,
    // 直接 contains 会因续格符号失配)
    let flat: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(flat.contains("/theme"), "补全 overlay 应渲染候选命令词");
    assert!(
        flat.contains("切换主题"),
        "补全 overlay 应渲染 i18n 标题(zh locale)"
    );
}

#[test]
fn esc_closes_slash_mode_and_clears_buffer() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "the");
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "Esc 应退出 Slash 模式"
    );
    assert!(app.state().input_buffer.is_empty(), "Esc 应清空输入缓冲");
}
