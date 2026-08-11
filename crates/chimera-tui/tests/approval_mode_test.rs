//! 审批模式端到端集成测试 — Concord W4(T4.1 闭环验证)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - 默认态 Normal;Chat 视图 Shift+Tab 三态循环;Dashboard 保留焦点环;
//! - Plan 态拦截 orchestrated 命令(诚实提示),instant 命令不受限;
//! - statusline 徽标渲染(En 文案);approval_mode 持久化往返。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{ApprovalMode, TuiApp, TuiConfig, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化涉及全局 locale 的测试(与 chat_mode_test 同范式)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Chat 默认视图测试应用(禁用持久化,排除用户状态文件干扰)
fn make_app() -> TuiApp {
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

fn shift_tab(app: &mut TuiApp) {
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
}

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

/// 渲染一帧并返回字符串快照(80x24 内存终端)
fn render_to_string(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).expect("memory terminal init");
    term.draw(|f| app.render(f)).expect("render frame");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

// ============================================================
// A. 默认态与三态循环
// ============================================================

#[test]
fn default_approval_mode_is_normal() {
    let app = make_app();
    assert_eq!(app.state().approval_mode, ApprovalMode::Normal);
}

#[test]
fn shift_tab_cycles_approval_mode_in_chat_view() {
    let mut app = make_app();
    shift_tab(&mut app);
    assert_eq!(
        app.state().approval_mode,
        ApprovalMode::Plan,
        "第一次应到 Plan"
    );
    shift_tab(&mut app);
    assert_eq!(
        app.state().approval_mode,
        ApprovalMode::Auto,
        "第二次应到 Auto"
    );
    shift_tab(&mut app);
    assert_eq!(
        app.state().approval_mode,
        ApprovalMode::Normal,
        "第三次应回到 Normal"
    );
}

#[test]
fn shift_tab_keeps_focus_cycle_in_dashboard() {
    let mut app = make_dashboard_app();
    let panel_before = app.current_panel();
    shift_tab(&mut app);
    assert_eq!(
        app.state().approval_mode,
        ApprovalMode::Normal,
        "Dashboard 下 Shift+Tab 不应改变审批模式"
    );
    assert_ne!(
        app.current_panel(),
        panel_before,
        "Dashboard 下 Shift+Tab 应保留焦点环语义(反向切面板)"
    );
}

// ============================================================
// B. Plan 模式拦截语义
// ============================================================

#[test]
fn plan_mode_blocks_orchestrated_command() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    // 切到 Plan 态
    shift_tab(&mut app);
    assert_eq!(app.state().approval_mode, ApprovalMode::Plan);
    // orchestrated 命令(quest pause 带参 → LegacyFallback)应被拦截
    slash_submit(&mut app, "quest pause q-1");
    let status = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        status.contains("plan only"),
        "Plan 态应诚实提示拦截,实际状态:{status}"
    );
}

#[test]
fn plan_mode_allows_instant_command() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    shift_tab(&mut app);
    assert_eq!(app.state().approval_mode, ApprovalMode::Plan);
    // instant 命令(/theme)不受 Plan 限制
    slash_submit(&mut app, "theme");
    let status = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    assert!(
        !status.contains("plan only"),
        "instant 命令不应被 Plan 拦截,实际状态:{status}"
    );
}

// ============================================================
// C. 徽标渲染与持久化
// ============================================================

#[test]
fn statusline_badge_shows_current_mode() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    shift_tab(&mut app); // → Plan
    let out = render_to_string(&mut app);
    assert!(out.contains("Plan"), "statusline 应渲染 Plan 徽标(En 文案)");
}

#[test]
fn approval_mode_persistence_roundtrip() {
    let dir = std::env::temp_dir().join(format!("chimera_w4_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("approval_state.yaml");

    let mut state = TuiState::new();
    state.approval_mode = ApprovalMode::Auto;
    state.save_to_file(&path).expect("save state");

    let loaded = TuiState::load_from_file(&path);
    assert_eq!(
        loaded.approval_mode,
        ApprovalMode::Auto,
        "持久化往返应保留审批模式"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn legacy_state_file_defaults_normal() {
    // 旧状态文件无 approval_mode 字段:白名单恢复 + serde 默认得 Normal
    let dir = std::env::temp_dir().join(format!("chimera_w4b_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("legacy_approval.yaml");
    std::fs::write(
        &path,
        "running: true\ninput_mode: Normal\ninput_buffer: ''\nframe_count: 0\n",
    )
    .expect("write legacy state");
    let loaded = TuiState::load_from_file(&path);
    assert_eq!(
        loaded.approval_mode,
        ApprovalMode::Normal,
        "旧状态文件缺 approval_mode 时应得 Normal 默认"
    );
    std::fs::remove_file(&path).ok();
}
