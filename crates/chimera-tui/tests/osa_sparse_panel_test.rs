//! OSA Sparse 面板集成测试 — Task P7 TDD-RED→GREEN
//!
//! 验证: 初始化 / 空状态 / j/k 滚动 / 颜色编码边界 / 未映射按键
//!
//! # 设计决策(WHY)
//! - 直接测试 Panel trait 方法(不通过 TuiApp),聚焦面板逻辑正确性
//! - 使用 TuiState::new() 构造测试状态(Panel trait 接收 &TuiState)
//! - 参考 timeline_panel_test.rs 的测试模式,保持一致性

#![forbid(unsafe_code)]

use chimera_tui::panels::{OsaSparsePanel, Panel};
use chimera_tui::types::{PanelId, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ============================================================
// A. 初始化测试
// ============================================================

#[test]
fn test_osa_sparse_panel_initialization() {
    let panel = OsaSparsePanel::new();
    assert_eq!(panel.id(), PanelId::OsaSparse);
    // title 返回 Line<'static>,用 to_string() 比较
    assert_eq!(panel.title().to_string(), " OSA Sparse ");
}

#[test]
fn test_osa_sparse_panel_default() {
    let panel = OsaSparsePanel::default();
    assert_eq!(panel.id(), PanelId::OsaSparse);
    assert_eq!(panel.selected(), 0);
    assert_eq!(panel.scroll_offset(), 0);
}

// ============================================================
// B. 空状态测试
// ============================================================

#[test]
fn test_osa_sparse_panel_empty_state() {
    // 空状态(无 context 文件、无稀疏度)时 handle_key 不应 panic
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    assert!(state.osa_context_mask.is_empty());
    assert!(state.osa_sparsity.is_none());

    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "Down on empty list should return None");
    assert_eq!(
        panel.selected(),
        0,
        "selected should stay at 0 on empty list"
    );
}

#[test]
fn test_osa_sparse_panel_empty_state_enter_returns_none() {
    // 空列表时 Enter 应返回 None
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "Enter on empty list should return None");
}

// ============================================================
// C. j/k 滚动测试
// ============================================================

#[test]
fn test_osa_sparse_panel_navigation_j_k() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec![
        "file1.rs".into(),
        "file2.rs".into(),
        "file3.rs".into(),
        "file4.rs".into(),
        "file5.rs".into(),
    ];

    // j 向下
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "j should not return a command");
    assert_eq!(panel.selected(), 1, "j should move selection down by 1");

    // 继续向下两次
    panel.handle_key(key, &mut state);
    panel.handle_key(key, &mut state);
    assert_eq!(panel.selected(), 3, "after 3x j, selected should be 3");

    // k 向上
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "k should not return a command");
    assert_eq!(panel.selected(), 2, "k should move selection up by 1");
}

#[test]
fn test_osa_sparse_panel_navigation_arrow_keys() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

    // Down 向下
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 1, "Down arrow should move selection down");

    // Up 向上
    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 0, "Up arrow should move selection up");
}

// ============================================================
// D. 滚动边界测试
// ============================================================

#[test]
fn test_osa_sparse_panel_scroll_clamp_at_bottom() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

    // 连续按 j 超过列表长度,应钳位在最后一项
    for _ in 0..10 {
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut state,
        );
    }
    assert_eq!(
        panel.selected(),
        2,
        "selected should clamp at last index (2)"
    );
}

#[test]
fn test_osa_sparse_panel_scroll_clamp_at_top() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

    // 在顶部连续按 k,应保持在 0
    for _ in 0..5 {
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut state,
        );
    }
    assert_eq!(panel.selected(), 0, "selected should clamp at 0 at top");
}

// ============================================================
// E. scroll_to_top / scroll_to_bottom 测试
// ============================================================

#[test]
fn test_osa_sparse_panel_scroll_to_top() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

    // 先移动到中间
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(panel.selected(), 2);

    // 跳到顶部
    panel.scroll_to_top(&mut state);
    assert_eq!(panel.selected(), 0, "scroll_to_top should reset to 0");
}

#[test]
fn test_osa_sparse_panel_scroll_to_bottom() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

    panel.scroll_to_bottom(&mut state);
    assert_eq!(
        panel.selected(),
        2,
        "scroll_to_bottom should move to last index"
    );
}

#[test]
fn test_osa_sparse_panel_scroll_to_bottom_empty() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();

    panel.scroll_to_bottom(&mut state);
    assert_eq!(
        panel.selected(),
        0,
        "scroll_to_bottom on empty list should stay at 0"
    );
}

// ============================================================
// F. 未映射按键测试
// ============================================================

#[test]
fn test_osa_sparse_panel_help_key_returns_none() {
    // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert_eq!(result, None);
}

#[test]
fn test_osa_sparse_panel_unmapped_key_returns_none() {
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_context_mask = vec!["a".into()];

    let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "unmapped keys should return None");
}

// ============================================================
// G. 数据更新测试
// ============================================================

#[test]
fn test_osa_sparse_panel_state_with_sparsity_data() {
    // 验证面板在有稀疏度数据时正常工作(不 panic)
    let mut panel = OsaSparsePanel::new();
    let mut state = TuiState::new();
    state.osa_sparsity = Some(0.45_f32);
    state.osa_context_mask = vec!["main.rs".into(), "lib.rs".into()];
    state.osa_sparsity_history = vec![100, 200, 300, 400, 500];

    // handle_key 不应 panic
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    panel.handle_key(key, &mut state);
    assert_eq!(panel.selected(), 1);
}
