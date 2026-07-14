//! EventStream 面板 auto_scroll 行为测试 — P3.4
//!
//! 验证 Claude Code 风格的流式追加交互:
//! - 默认自动跟随最新事件
//! - 用户手动滚动(Up/k)时暂停跟随
//! - 暂停期间新事件到达显示 "[新事件 N 条] 按 G 跟随" 顶部提示
//! - 按 G 恢复到底部并重新启用 auto_scroll
//! - 弹窗打开时冻结 auto_scroll,避免详情 overlay 后方列表跳动

#![forbid(unsafe_code)]

use chimera_tui::{EventStreamPanel, Panel, PopupKind, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::collections::VecDeque;

/// 构造带预设事件的 TuiState
fn make_state(events: Vec<NexusEvent>) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from(events);
    state
}

/// 构造一个 CacheHit 事件
fn cache_hit(id: usize) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("k{id}"),
    }
}

/// 标准渲染区域
fn standard_area() -> Rect {
    Rect::new(0, 0, 120, 40)
}

// ============================================================
// 测试 1:auto_scroll=true 时 selected 指向最后一条事件
// ============================================================

#[test]
fn auto_scroll_true_selects_last_event() {
    let state = make_state(vec![cache_hit(0), cache_hit(1), cache_hit(2)]);
    assert!(state.auto_scroll, "TuiState::new() 默认应启用 auto_scroll");

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);

    assert_eq!(
        panel.selected(),
        2,
        "auto_scroll=true 时 selected 应指向过滤列表末尾"
    );
}

// ============================================================
// 测试 2:按 Up/k 后 auto_scroll=false
// ============================================================

#[test]
fn pressing_up_disables_auto_scroll() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1), cache_hit(2)]);
    let mut panel = EventStreamPanel::new();

    // 先渲染使 selected 跟随到末尾
    let area = standard_area();
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);
    assert_eq!(panel.selected(), 2);
    assert!(state.auto_scroll);

    // 按 Up 应暂停 auto_scroll
    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert!(!state.auto_scroll, "按 Up 后应关闭 auto_scroll");
    assert_eq!(panel.selected(), 1);

    // 按 k 同理
    state.auto_scroll = true;
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        &mut state,
    );
    assert!(!state.auto_scroll, "按 k 后应关闭 auto_scroll");
    assert_eq!(panel.selected(), 0);
}

// ============================================================
// 测试 3:暂停期间新事件到达显示 "[新事件 N 条]" 顶部提示
// ============================================================

#[test]
fn paused_auto_scroll_shows_new_events_banner() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1), cache_hit(2)]);
    state.auto_scroll = false;

    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("新事件 2 条"),
        "selected=0 且共 3 条事件时应提示 2 条新事件, got: {content}"
    );
    assert!(
        content.contains("按 G 跟随"),
        "提示应包含恢复快捷键 G, got: {content}"
    );
}

// ============================================================
// 测试 4:按 G 后恢复 auto_scroll=true 且 selected 指向末尾
// ============================================================

#[test]
fn pressing_g_restores_auto_scroll_to_bottom() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1), cache_hit(2)]);
    state.auto_scroll = false;

    let mut panel = EventStreamPanel::new();
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        &mut state,
    );

    assert!(state.auto_scroll, "按 G 后应恢复 auto_scroll=true");
    assert_eq!(panel.selected(), 2, "按 G 后 selected 应指向过滤列表末尾");
}

// ============================================================
// 测试 5:按 g 跳到顶部但不改变 auto_scroll 状态
// ============================================================

#[test]
fn pressing_small_g_scrolls_to_top_without_changing_auto_scroll() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1), cache_hit(2)]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);
    assert_eq!(panel.selected(), 2);

    panel.handle_key(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &mut state,
    );

    assert_eq!(panel.selected(), 0, "按 g 后应跳到顶部");
    assert!(state.auto_scroll, "按 g 不应改变 auto_scroll 状态");
}

// ============================================================
// 测试 6:按 j/Down 在底部时保持 auto_scroll,否则暂停
// ============================================================

#[test]
fn pressing_down_at_bottom_keeps_auto_scroll() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1)]);
    let mut panel = EventStreamPanel::new();

    let area = standard_area();
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);
    assert_eq!(panel.selected(), 1);

    // 在底部按 Down:保持 auto_scroll=true
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert!(state.auto_scroll, "在底部按 Down 应保持 auto_scroll");
    assert_eq!(panel.selected(), 1);

    // 不在底部按 Down:暂停 auto_scroll
    // WHY 使用 'g' 键跳回顶部:selected 字段为私有,无法直接赋值,
    // 'g' 是 EventStreamPanel 公开的跳顶快捷键,可达到同样效果。
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        &mut state,
    );
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );
    assert!(!state.auto_scroll, "不在底部按 j 应暂停 auto_scroll");
    assert_eq!(panel.selected(), 1);
}

// ============================================================
// 测试 7:auto_scroll 与过滤共存,selected 指向过滤后列表末尾
// ============================================================

#[test]
fn auto_scroll_works_with_filters() {
    let mut state = make_state(vec![
        cache_hit(0),
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q1".into(),
            veto_reason: "unsafe".into(),
            frozen_capabilities: vec![],
        },
        cache_hit(2),
    ]);
    state.filter_topic = Some("security".into());
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);

    assert_eq!(
        panel.selected(),
        0,
        "过滤后仅 1 条安全事件,selected 应指向其索引 0"
    );
}

// ============================================================
// 测试 8:弹窗打开时 auto_scroll 行为冻结
// ============================================================

#[test]
fn popup_freezes_auto_scroll() {
    let mut state = make_state(vec![cache_hit(0), cache_hit(1)]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);

    // 正常渲染:selected 跟随到末尾
    panel.render(&state, area, &mut buf);
    assert_eq!(panel.selected(), 1);

    // 打开弹窗
    state.popup_stack.push(PopupKind::Notification {
        message: "detail".into(),
        severity: chimera_tui::Severity::Info,
    });

    // 新事件到达
    state.latest_events.push_back(cache_hit(2));

    // 弹窗打开期间渲染:selected 不应自动跟随
    let mut buf2 = Buffer::empty(area);
    panel.render(&state, area, &mut buf2);
    assert_eq!(
        panel.selected(),
        1,
        "弹窗打开时 auto_scroll 应冻结,selected 保持原值"
    );
    assert!(state.auto_scroll, "auto_scroll 状态本身不应被弹窗改变");

    // 关闭弹窗后再次渲染:恢复跟随
    state.popup_stack.pop();
    let mut buf3 = Buffer::empty(area);
    panel.render(&state, area, &mut buf3);
    assert_eq!(panel.selected(), 2, "弹窗关闭后应恢复自动跟随");
}
