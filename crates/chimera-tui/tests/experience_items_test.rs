//! 体验四项端到端集成测试 — Concord W4 T4.5
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - /focus:Dashboard 切 SinglePane;Chat 态诚实提示已聚焦;
//! - Esc Esc rewind:Chat 单 Esc 不退出、双击诚实反馈;Dashboard Esc 退出保留;
//! - @ 引用补全:Insert 态 Tab 补全首个候选;无候选不改缓冲;
//! - ! shell:HonestTodo 占位(不伪造直通)。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_core::Quest;

/// 串行化涉及全局 locale 的测试(与既有集成测试同范式)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn make_chat_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

fn make_dashboard_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

fn press(app: &mut TuiApp, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn slash_submit(app: &mut TuiApp, cmd: &str) {
    press(app, KeyCode::Char('/'));
    type_str(app, cmd);
    press(app, KeyCode::Enter);
}

fn status_text(app: &TuiApp) -> String {
    app.state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default()
}

// ============================================================
// A. /focus 专注视图
// ============================================================

#[test]
fn focus_command_switches_dashboard_to_single_pane() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_dashboard_app();
    slash_submit(&mut app, "focus");
    assert_eq!(
        app.state().layout_mode,
        chimera_tui::LayoutMode::SinglePane,
        "/focus 应切到 SinglePane 专注布局"
    );
    assert!(
        status_text(&app).contains("focus layout"),
        "状态栏应提示布局切换"
    );
}

#[test]
fn focus_command_in_chat_view_honest_hint() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_chat_app();
    slash_submit(&mut app, "focus");
    assert!(
        status_text(&app).contains("Already in focused chat view"),
        "Chat 态应诚实提示已聚焦: {}",
        status_text(&app)
    );
}

// ============================================================
// B. Esc Esc rewind
// ============================================================

#[test]
fn chat_single_esc_does_not_quit() {
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Esc);
    assert!(app.state().running, "Chat 视图单 Esc 不应退出");
}

#[test]
fn chat_double_esc_rewind_honest_feedback() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert!(app.state().running, "双击 rewind 不应退出");
    assert!(
        status_text(&app).contains("Nothing to rewind"),
        "无可弹层时双击应诚实反馈: {}",
        status_text(&app)
    );
}

#[test]
fn dashboard_esc_still_quits() {
    // 回归守护:Dashboard 保留 Esc 退出肌肉记忆
    let mut app = make_dashboard_app();
    press(&mut app, KeyCode::Esc);
    assert!(!app.state().running, "Dashboard Esc 应退出");
}

// ============================================================
// B2. q 键双语义(Concord W5 T5.1,方案 §7.4 对齐)
// ============================================================

#[test]
fn chat_q_blurs_not_quits() {
    // Chat 态 q = 失焦(不退出);退出统一走 /exit
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('q'));
    assert!(app.state().running, "Chat 态 q 不应退出");
}

#[test]
fn chat_q_in_open_palette_is_search_not_quit() {
    // Chat 态 palette 打开时,按键由面板接管(检索字符,既有行为跨视图一致):
    // q 不退出、不误关面板;面板自身经 Esc 关闭(失焦链兼容)
    let mut app = make_chat_app();
    app.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert!(app.palette_is_open(), "Ctrl+P 应打开命令面板");
    press(&mut app, KeyCode::Char('q'));
    assert!(app.state().running, "Chat 态 palette 内 q 不应退出");
    assert!(
        app.palette_is_open(),
        "palette 打开时 q 是检索字符,面板不应被误关"
    );
}

#[test]
fn dashboard_q_still_quits() {
    // 回归守护:Dashboard 保留 q 退出肌肉记忆
    let mut app = make_dashboard_app();
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.state().running, "Dashboard q 应退出");
}

// ============================================================
// C. @ 引用补全
// ============================================================

#[test]
fn mention_tab_completes_first_candidate() {
    let mut app = make_chat_app();
    app.state_mut().quest_list.push(Quest {
        quest_id: "q-test".into(),
        ..Default::default()
    });
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "hi @q-");
    press(&mut app, KeyCode::Tab);
    assert!(
        app.state().input_buffer.contains("@q-test"),
        "Tab 应补全首个候选: {}",
        app.state().input_buffer
    );
}

#[test]
fn mention_tab_without_candidates_keeps_buffer() {
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "hi @zzz-no-such");
    press(&mut app, KeyCode::Tab);
    assert_eq!(
        app.state().input_buffer,
        "hi @zzz-no-such",
        "无候选时 Tab 不应改动缓冲(诚实降级)"
    );
}

// ============================================================
// D. composer 历史 ↑↓ 回溯(Concord W6 T6.2)
// ============================================================

#[test]
fn composer_history_up_down_recall() {
    // 输入两条提交 → ↑ 回溯最新 → ↑ 到顶 → ↓ 回新 → ↓ 回底恢复草稿
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "hello");
    press(&mut app, KeyCode::Enter);
    type_str(&mut app, "world");
    press(&mut app, KeyCode::Enter);
    assert!(app.state().input_buffer.is_empty(), "提交后缓冲应清空");

    press(&mut app, KeyCode::Up);
    assert_eq!(app.state().input_buffer, "world", "↑ 应回溯最新一条");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.state().input_buffer, "hello", "再 ↑ 应回溯更早一条");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.state().input_buffer, "hello", "到顶应保持不越界");
    press(&mut app, KeyCode::Down);
    assert_eq!(app.state().input_buffer, "world", "↓ 应前进回较新一条");
    press(&mut app, KeyCode::Down);
    assert_eq!(
        app.state().input_buffer,
        "",
        "回底应恢复进入回溯前的草稿(提交后缓冲为空)"
    );
}

#[test]
fn composer_history_dedups_consecutive_on_resubmit() {
    // 回溯后再提交相同内容:连续重复去重,历史不堆叠
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "same");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.state().input_buffer, "same");
    press(&mut app, KeyCode::Enter); // 再提交相同内容
    let history: Vec<&str> = app
        .state()
        .input_history
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(history, vec!["same"], "连续重复提交应去重");
}

#[test]
fn composer_history_up_without_history_noop() {
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "draft");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.state().input_buffer, "draft", "无历史时 ↑ 不应改动缓冲");
}

// ============================================================
// E. ! shell HonestTodo
// ============================================================

#[test]
fn shell_passthrough_honest_todo() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_chat_app();
    press(&mut app, KeyCode::Char('i'));
    type_str(&mut app, "!ls -la");
    press(&mut app, KeyCode::Enter);
    assert!(app.state().running, "shell 占位不应退出");
    assert!(
        status_text(&app).contains("not wired"),
        "! 直通应诚实提示未接线: {}",
        status_text(&app)
    );
}
