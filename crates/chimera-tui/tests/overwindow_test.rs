//! OverWindow 面板专项集成测试 — Concord W4 T4.3(P8 盲区清零)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - 面板注册与焦点切换;空态提示;触发事件渲染(最新在前/字段齐备);
//! - MAX_TRIGGERS_SHOWN 截断与 "more" 指示;键处理 None(展示型面板);
//! - scaled_timeout! 异步护栏示范用例(异步等待状态更新)。
//!
//! WHY 本文件补齐 P8:OverWindow(ADR-073 落地的最新机制)此前是唯一无专项
//! 测试文件的注册面板;断言统一用 En 文案(宽字符重组歧义规避,W3 口径沿袭)。

#![forbid(unsafe_code)]

#[macro_use]
mod common;

use std::sync::Mutex;

use chimera_tui::{PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化涉及全局 locale 的测试(与既有集成测试同范式)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Dashboard 视图测试应用(面板断言语义;禁用持久化排除环境依赖)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

fn trigger(
    corpus_tokens: u64,
    effective_window: u64,
    candidate_count: u32,
    loaded_count: u32,
) -> NexusEvent {
    NexusEvent::OverWindowFallbackTriggered {
        metadata: EventMetadata::new("test"),
        corpus_tokens,
        effective_window,
        candidate_count,
        loaded_count,
    }
}

/// 渲染一帧并返回字符串快照(指定视口尺寸的内存终端)
fn render_to_string_wh(app: &mut TuiApp, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
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

/// 默认 80x24 视口渲染
fn render_to_string(app: &mut TuiApp) -> String {
    render_to_string_wh(app, 80, 24)
}

// ============================================================
// A. 注册与焦点
// ============================================================

#[test]
fn overwindow_panel_registered_and_focusable() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::OverWindow);
    assert_eq!(
        app.current_panel(),
        PanelId::OverWindow,
        "OverWindow 应已注册且可聚焦"
    );
}

// ============================================================
// B. 空态与数据驱动渲染
// ============================================================

#[test]
fn empty_state_shows_hint_en() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    app.switch_panel_to(PanelId::OverWindow);
    let out = render_to_string(&mut app);
    assert!(
        out.contains("No overwindow fallback triggered yet"),
        "空态应展示等待提示(En 文案)"
    );
}

#[test]
fn trigger_events_render_latest_first_with_fields() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    app.state_mut()
        .latest_events
        .push_back(trigger(100_000, 131_072, 0, 0));
    app.state_mut()
        .latest_events
        .push_back(trigger(600_000, 131_072, 42, 128));
    app.switch_panel_to(PanelId::OverWindow);
    let out = render_to_string(&mut app);
    assert!(out.contains("corpus=600000 tok"), "最新触发应展示语料规模");
    assert!(out.contains("candidates=42"), "候选数应展示");
    assert!(out.contains("loaded=128"), "装窗数应展示");
    assert!(
        out.find("#1").unwrap() < out.find("#2").unwrap(),
        "最新触发应排在 #1"
    );
}

#[test]
fn truncates_beyond_max_triggers_with_more_indicator() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    // 灌入 10 条触发事件,超过 MAX_TRIGGERS_SHOWN=8
    for i in 0..10u64 {
        app.state_mut()
            .latest_events
            .push_back(trigger(1000 + i, 131_072, 1, 1));
    }
    app.switch_panel_to(PanelId::OverWindow);
    // WHY 高视口:24 行下 more 截断行可能被面板区域裁剪,40 行保证可见
    let out = render_to_string_wh(&mut app, 80, 40);
    assert!(
        out.contains("more triggers"),
        "超出上限应展示 more 截断指示"
    );
    assert!(out.contains("#8"), "前 8 条应完整展示");
    assert!(!out.contains("#9"), "第 9 条起应被截断");
}

// ============================================================
// C. 键处理与异步护栏示范
// ============================================================

#[test]
fn panel_key_delegation_is_noop_no_panic() {
    // 展示型面板:聚焦后按面板级键不 panic、不退出、焦点不变
    let mut app = make_app();
    app.switch_panel_to(PanelId::OverWindow);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.state().running, "面板级键不应退出应用");
    assert_eq!(app.current_panel(), PanelId::OverWindow, "焦点不应漂移");
}

#[tokio::test]
async fn scaled_timeout_guards_async_state_wait() {
    // scaled_timeout! 护栏示范:异步轮询等待状态更新,超时可诊断而非挂死
    let app = make_app();
    let deadline = tokio::time::Instant::now() + scaled_timeout!(1.0);
    let wait_result = tokio::time::timeout_at(deadline, async {
        loop {
            if app.state().frame_count < u64::MAX {
                return app.state().approval_mode;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(wait_result.is_ok(), "护栏内应完成等待(未超时)");
    assert_eq!(
        wait_result.unwrap(),
        chimera_tui::ApprovalMode::Normal,
        "默认审批模式应为 Normal"
    );
}

#[tokio::test]
async fn scaled_timeout_detects_stuck_wait() {
    // 护栏反向验证:不可能满足的条件应在缩放超时内返回 Err(而非挂死)
    let stuck = tokio::time::timeout(scaled_timeout!(0.05), async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(stuck.is_err(), "超时护栏应拦截挂死等待");
}
