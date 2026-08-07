//! Budget 面板集成测试(评估报告 P0-3:此前仅 1 个既有测试,密度远低于同级面板)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - 正常态 / 超限态(is_exceeded 切换)渲染;
//! - 利用率边界(0% / 100% / 超 100% 钳位)进度条与数值;
//! - alert 行显隐;
//! - 快捷键诚实性:R 刷新声明即可达,其余按键无命令。

#![forbid(unsafe_code)]

use chimera_tui::data::BudgetMetrics;
use chimera_tui::panels::{BudgetPanel, Panel};
use chimera_tui::{TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 构造指定利用率的 BudgetMetrics(其余字段默认)
fn budget(utilization: f32) -> BudgetMetrics {
    BudgetMetrics {
        utilization_rate: utilization,
        ..Default::default()
    }
}

/// 构造含指定 Budget 的 TuiState
fn state_with(b: BudgetMetrics) -> TuiState {
    let mut state = TuiState::new();
    state.budget = b;
    state
}

// ============================================================
// A. 正常态 / 超限态渲染
// ============================================================

#[test]
fn normal_state_shows_metrics_and_ok_status() {
    let state = state_with(BudgetMetrics {
        total_consumption: 350.0,
        remaining_budget: 650.0,
        utilization_rate: 0.35,
        current_tier: "High".into(),
        coefficient: 1.0,
        is_exceeded: false,
        alert: None,
    });
    let content = BudgetPanel::content(&state).to_string();
    assert!(content.contains("High"), "应显示预算档位");
    assert!(content.contains("35.0%"), "应显示利用率百分比");
    assert!(content.contains("350.0"), "应显示消耗量");
    assert!(content.contains("650.0"), "应显示剩余预算");
    assert!(content.contains("OK"), "未超限应显示 OK 状态");
    assert!(!content.contains("EXCEEDED"), "未超限不应显示 EXCEEDED");
}

#[test]
fn exceeded_state_shows_exceeded_and_alert() {
    let state = state_with(BudgetMetrics {
        total_consumption: 1200.0,
        remaining_budget: -200.0,
        utilization_rate: 1.2,
        current_tier: "Critical".into(),
        coefficient: 1.5,
        is_exceeded: true,
        alert: Some("budget limit breached".into()),
    });
    let content = BudgetPanel::content(&state).to_string();
    assert!(content.contains("EXCEEDED"), "超限应显示 EXCEEDED 状态");
    assert!(content.contains("120.0%"), "应显示原始利用率百分比(120%)");
    assert!(
        content.contains("budget limit breached"),
        "超限时应显示告警内容"
    );
    // 进度条应钳位到满(不因 120% 溢出)
    let bar_full = "=".repeat(30);
    assert!(
        content.contains(&bar_full),
        "超 100% 利用率进度条应钳位为满格"
    );
}

// ============================================================
// B. 利用率边界(0% / 100% / 超限钳位)
// ============================================================

#[test]
fn zero_utilization_renders_empty_bar() {
    let state = state_with(budget(0.0));
    let content = BudgetPanel::content(&state).to_string();
    assert!(content.contains("0.0%"), "零利用率应显示 0.0%");
    let bar_empty = "-".repeat(30);
    assert!(content.contains(&bar_empty), "零利用率进度条应全空");
}

#[test]
fn full_utilization_renders_full_bar() {
    let state = state_with(budget(1.0));
    let content = BudgetPanel::content(&state).to_string();
    assert!(content.contains("100.0%"), "满利用率应显示 100.0%");
    let bar_full = "=".repeat(30);
    assert!(content.contains(&bar_full), "满利用率进度条应全满");
}

#[test]
fn negative_utilization_clamps_to_zero() {
    // 极值防护:负数利用率不应产生下溢/panic,进度条按 0 处理
    let state = state_with(budget(-0.5));
    let content = BudgetPanel::content(&state).to_string();
    assert!(content.contains("-50.0%"), "负利用率数值如实显示(数据可信)");
    let bar_empty = "-".repeat(30);
    assert!(content.contains(&bar_empty), "负利用率进度条应钳位为空");
}

// ============================================================
// C. alert 行显隐
// ============================================================

#[test]
fn alert_line_appears_only_when_present() {
    let with_alert = state_with(BudgetMetrics {
        alert: Some("approaching limit".into()),
        ..budget(0.8)
    });
    let without_alert = state_with(budget(0.8));

    assert!(
        BudgetPanel::content(&with_alert)
            .to_string()
            .contains("approaching limit"),
        "存在 alert 时应显示告警行"
    );
    assert!(
        !BudgetPanel::content(&without_alert)
            .to_string()
            .contains("approaching limit"),
        "无 alert 时不应显示告警行"
    );
}

// ============================================================
// D. 快捷键诚实性(handle_key)
// ============================================================

#[test]
fn r_key_returns_request_refresh() {
    // shortcuts 声明 "R 刷新" 即可达
    let mut panel = BudgetPanel::new();
    let mut state = TuiState::new();
    let cmd = panel.handle_key(key(KeyCode::Char('R')), &mut state);
    assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
}

#[test]
fn other_keys_return_none() {
    // 未声明按键不应产生命令(Enter/空格/数字等)
    let mut panel = BudgetPanel::new();
    let mut state = TuiState::new();
    for code in [KeyCode::Enter, KeyCode::Char(' '), KeyCode::Char('1')] {
        assert!(
            panel.handle_key(key(code), &mut state).is_none(),
            "{code:?} 不应产生命令"
        );
    }
}

#[test]
fn shortcuts_declare_refresh_only() {
    let panel = BudgetPanel::new();
    let keys: Vec<&str> = panel.shortcuts().iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, vec!["R"], "shortcuts 应仅声明真实可达的 R 刷新");
}

// ============================================================
// E. 面板身份与空态稳定性
// ============================================================

#[test]
fn panel_id_is_budget() {
    assert_eq!(BudgetPanel::new().id(), chimera_tui::PanelId::Budget);
}

#[test]
fn default_state_renders_without_panic() {
    // 全默认快照(零值)渲染不 panic 且产出非空
    let state = TuiState::new();
    let content = BudgetPanel::content(&state).to_string();
    assert!(!content.trim().is_empty());
    assert!(content.contains("0.0%"));
}
