//! Timeline 面板集成测试 — Task 7+8 TDD-RED→GREEN
//!
//! 验证: 初始化 / 空状态 / J/K 滚动 / Enter 弹窗 / 快照导航边界
//!
//! # 设计决策(WHY)
//! - 直接测试 Panel trait 方法(不通过 TuiApp),聚焦面板逻辑正确性
//! - 使用 TuiState::new() 构造测试状态(Panel trait 接收 &TuiState)
//! - 参考 quest.rs / event_stream.rs 的测试模式,保持一致性

#![forbid(unsafe_code)]

use chimera_tui::panels::{Panel, TimelinePanel};
use chimera_tui::popup::PopupKind;
use chimera_tui::types::{PanelId, TimelineSnapshot, TuiCommand, TuiState};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 构造测试用 TimelineSnapshot
fn make_snapshot(event_count: u64, event_rate: u64) -> TimelineSnapshot {
    TimelineSnapshot {
        timestamp: Utc::now(),
        event_count,
        event_rate,
        budget_utilization: 0.35,
        health_score: 90,
        decay_coefficient: 0.85,
    }
}

// ============================================================
// A. 初始化测试
// ============================================================

#[test]
fn test_timeline_panel_initialization() {
    let panel = TimelinePanel::new();
    assert_eq!(panel.id(), PanelId::Timeline);
    // title 返回 Line<'static>,用 to_string() 比较
    assert_eq!(panel.title().to_string(), " Timeline ");
}

#[test]
fn test_timeline_panel_default() {
    let panel = TimelinePanel::default();
    assert_eq!(panel.id(), PanelId::Timeline);
    assert_eq!(panel.selected(), 0);
}

// ============================================================
// B. 空状态测试
// ============================================================

#[test]
fn test_timeline_panel_empty_state_no_panic() {
    // 空状态(无快照)时 handle_key 不应 panic
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    assert!(state.timeline_snapshots.is_empty());

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
fn test_timeline_panel_empty_state_enter_returns_none() {
    // 无快照时 Enter 应返回 None(无内容可展示)
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "Enter on empty list should return None");
}

// ============================================================
// C. J/K 滚动测试
// ============================================================

#[test]
fn test_timeline_panel_scroll_down_with_j() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..5 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "j should not return a command");
    assert_eq!(panel.selected(), 1, "j should move selection down by 1");
}

#[test]
fn test_timeline_panel_scroll_down_with_arrow() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..3 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    panel.handle_key(key, &mut state);
    assert_eq!(panel.selected(), 1, "Down arrow should move selection down");
}

#[test]
fn test_timeline_panel_scroll_up_with_k() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..5 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

    // 先向下滚动两次
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(panel.selected(), 2);

    // 向上滚动一次
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "k should not return a command");
    assert_eq!(panel.selected(), 1, "k should move selection up by 1");
}

#[test]
fn test_timeline_panel_scroll_up_with_arrow() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..3 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

    // 先到底部
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 2);

    // 向上
    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 1, "Up arrow should move selection up");
}

// ============================================================
// D. 滚动边界测试
// ============================================================

#[test]
fn test_timeline_panel_scroll_clamp_at_bottom() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..3 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

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
fn test_timeline_panel_scroll_clamp_at_top() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..3 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

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
// E. Enter 弹窗测试
// ============================================================

#[test]
fn test_timeline_panel_enter_shows_detail() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    state.timeline_snapshots.push(make_snapshot(42, 10));

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(
        result.is_some(),
        "Enter with snapshots should return a popup"
    );

    match result {
        Some(TuiCommand::OpenPopup(PopupKind::Detail { title, content, .. })) => {
            // 详情弹窗应包含快照相关信息
            assert!(
                title.contains("Timeline") || title.contains("Snapshot"),
                "popup title should mention Timeline/Snapshot, got: {title}"
            );
            assert!(
                content.contains("42") || content.contains("event_count"),
                "popup content should include event count, got: {content}"
            );
        }
        other => panic!("expected OpenPopup(Detail), got {:?}", other),
    }
}

#[test]
fn test_timeline_panel_enter_shows_selected_snapshot() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    // 添加两个快照,选中第二个后按 Enter,应显示第二个的详情
    state.timeline_snapshots.push(make_snapshot(10, 1));
    state.timeline_snapshots.push(make_snapshot(99, 2));

    // 移动到第二个
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(panel.selected(), 1);

    let result = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );
    match result {
        Some(TuiCommand::OpenPopup(PopupKind::Detail { content, .. })) => {
            assert!(
                content.contains("99"),
                "popup should show selected snapshot's event_count (99), got: {content}"
            );
        }
        other => panic!("expected OpenPopup(Detail), got {:?}", other),
    }
}

// ============================================================
// F. scroll_to_top / scroll_to_bottom 测试
// ============================================================

#[test]
fn test_timeline_panel_scroll_to_top() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..5 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

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
fn test_timeline_panel_scroll_to_bottom() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    for i in 0..5 {
        state.timeline_snapshots.push(make_snapshot(i, i * 10));
    }

    panel.scroll_to_bottom(&mut state);
    assert_eq!(
        panel.selected(),
        4,
        "scroll_to_bottom should move to last index"
    );
}

#[test]
fn test_timeline_panel_scroll_to_bottom_empty() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();

    panel.scroll_to_bottom(&mut state);
    assert_eq!(
        panel.selected(),
        0,
        "scroll_to_bottom on empty list should stay at 0"
    );
}

// ============================================================
// G. 未映射按键测试
// ============================================================

#[test]
fn test_timeline_panel_unmapped_key_returns_none() {
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();
    state.timeline_snapshots.push(make_snapshot(1, 1));

    let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "unmapped keys should return None");
}

#[test]
fn test_timeline_panel_help_key_returns_none() {
    // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
    let mut panel = TimelinePanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert_eq!(result, None);
}
