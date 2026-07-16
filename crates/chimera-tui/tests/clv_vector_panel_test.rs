//! ClvVector 面板集成测试 — Task P7 TDD-RED→GREEN
//!
//! 验证: 初始化 / 默认值 / 空状态 / J/K 滚动 / 未映射按键
//!
//! # 设计决策(WHY)
//! - 直接测试 Panel trait 方法(不通过 TuiApp),聚焦面板逻辑正确性
//! - 使用 TuiState::new() 构造测试状态(空状态,clv_summary = None)
//! - 参考 timeline_panel_test.rs 的测试模式,保持一致性
//! - WHY 空状态测试:ClvSummary 由 event-bus crate 定义,集成测试中保持
//!   空状态(None)验证面板的空状态处理逻辑,避免跨 crate 构造复杂数据

#![forbid(unsafe_code)]

use chimera_tui::panels::{ClvVectorPanel, Panel};
use chimera_tui::types::{PanelId, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ============================================================
// A. 初始化测试
// ============================================================

#[test]
fn test_clv_vector_panel_initialization() {
    let panel = ClvVectorPanel::new();
    assert_eq!(panel.id(), PanelId::ClvVector);
    // title 返回 Line<'static>,用 to_string() 比较
    assert_eq!(panel.title().to_string(), " CLV Vector ");
}

#[test]
fn test_clv_vector_panel_default() {
    let panel = ClvVectorPanel::default();
    assert_eq!(panel.id(), PanelId::ClvVector);
    assert_eq!(panel.selected(), 0);
    assert_eq!(panel.scroll_offset(), 0);
}

// ============================================================
// B. 空状态测试
// ============================================================

#[test]
fn test_clv_vector_panel_empty_state() {
    // 空状态(clv_summary = None)时 handle_key 不应 panic
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();
    assert!(state.clv_summary.is_none());

    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "Down on empty list should return None");
    assert_eq!(
        panel.selected(),
        0,
        "selected should stay at 0 on empty list"
    );
}

// ============================================================
// C. J/K 滚动测试(空状态 — 验证不 panic)
// ============================================================

#[test]
fn test_clv_vector_panel_navigation_j_k() {
    // WHY 空状态测试:clv_summary = None 时 count = 0,
    // j/k 应安全返回 None 且 selected 保持 0,不 panic
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();
    assert!(state.clv_summary.is_none());

    // j 向下(空列表应保持 0)
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "j should not return a command");
    assert_eq!(panel.selected(), 0, "j on empty list should stay at 0");

    // k 向上(空列表应保持 0)
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "k should not return a command");
    assert_eq!(panel.selected(), 0, "k on empty list should stay at 0");
}

// ============================================================
// D. 未映射按键测试
// ============================================================

#[test]
fn test_clv_vector_panel_help_key_returns_none() {
    // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert_eq!(result, None);
}
