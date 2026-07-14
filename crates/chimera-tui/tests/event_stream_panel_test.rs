//! EventStream 面板集成测试 — P2.2 TUI v1.7-omega
//!
//! 验证 EventStreamPanel 正确渲染事件流、支持万级事件虚拟滚动(帧时间 < 16ms)、
//! auto_scroll 自动跟随新事件、用户导航时关闭 auto_scroll。
//!
//! # 设计决策(WHY)
//! - 使用 `Buffer::empty` + `Rect::new` 直接调用 `Panel::render`,
//!   无需 TestBackend/Terminal,测试更轻量且可测量纯渲染时间。
//! - 帧时间断言 < 16ms(60fps):虚拟滚动只渲染可见区域 + 上下 5 行缓冲,
//!   10000 事件的实际渲染行数约 50 行,远低于全量渲染。

#![forbid(unsafe_code)]

use chimera_tui::{EventStreamPanel, Panel, PopupKind, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::Instant;

/// 构造带预设事件的 TuiState
fn make_state_with_events(events: Vec<NexusEvent>) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from(events);
    state
}

/// 构造一个 CacheHit 事件(用于批量生成测试数据)
fn cache_hit_event(id: usize) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("k{id}"),
    }
}

/// 标准 120x40 渲染区域(与 TUI 默认窗口接近)
fn standard_area() -> Rect {
    Rect::new(0, 0, 120, 40)
}

// ============================================================
// 测试 1:EventStream 面板渲染 latest_events 中的事件
// ============================================================

#[test]
fn event_stream_panel_renders_events() {
    let state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        },
    ]);

    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("CacheHit"),
        "content should contain event type name 'CacheHit', got: {content}"
    );
    assert!(
        content.contains("BudgetExceeded"),
        "content should contain event type name 'BudgetExceeded', got: {content}"
    );
}

// ============================================================
// 测试 2:虚拟滚动 — 10000 条事件不 panic 且帧时间 < 16ms
// ============================================================

#[test]
fn event_stream_panel_virtual_scroll_10000_events_no_panic() {
    // 构造 10000 条事件,验证虚拟滚动不 panic 且帧时间 < 16ms
    let events: Vec<NexusEvent> = (0..10000).map(cache_hit_event).collect();
    let state = make_state_with_events(events);

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);

    // 测量纯渲染时间(不含 Buffer::empty 的初始化开销)
    let start = Instant::now();
    panel.render(&state, area, &mut buf);
    let elapsed = start.elapsed();

    // 60fps 对应 16.67ms,断言 < 16ms(虚拟滚动只渲染 ~50 行,应远低于此)
    assert!(
        elapsed.as_millis() < 16,
        "render 10000 events took {:?}, expected < 16ms (virtual scroll should only render visible rows)",
        elapsed
    );
}

// ============================================================
// 测试 3:auto_scroll=true 时,新事件到达后选中项跟随到最新
// ============================================================

#[test]
fn event_stream_panel_auto_scroll_follows_new_events() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();

    // 第一次渲染:auto_scroll=true,selected 应跟随到最后一条事件(索引 1)
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);
    assert_eq!(
        panel.selected(),
        1,
        "auto_scroll=true should follow to last event (index 1)"
    );

    // 新事件到达
    state.latest_events.push_back(NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k3".into(),
    });

    // 第二次渲染:auto_scroll 仍为 true,selected 应跟随到最新事件(索引 2)
    let mut buf2 = Buffer::empty(area);
    panel.render(&state, area, &mut buf2);
    assert_eq!(
        panel.selected(),
        2,
        "auto_scroll=true should follow new event (index 2)"
    );
}

// ============================================================
// 测试 4:用户按 Up/Down 时 auto_scroll 设为 false
// ============================================================

#[test]
fn event_stream_panel_key_navigation_disables_auto_scroll() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();

    // 用户按 Down,应关闭 auto_scroll(用户接管导航)
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert!(
        !state.auto_scroll,
        "Down key should disable auto_scroll (user takes over navigation)"
    );

    // 重新启用 auto_scroll,验证 Up 也会关闭
    state.auto_scroll = true;
    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert!(
        !state.auto_scroll,
        "Up key should disable auto_scroll (user takes over navigation)"
    );
}

// ============================================================
// 测试 5:Enter 打开详情弹窗(复用 Log 面板模式)
// ============================================================

#[test]
fn event_stream_panel_enter_opens_detail_popup() {
    let mut panel = EventStreamPanel::new();
    let mut state = make_state_with_events(vec![NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    }]);

    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );

    match cmd {
        Some(TuiCommand::OpenPopup(PopupKind::EventDetail {
            title,
            payload_decoded,
            ..
        })) => {
            assert!(
                title.contains("CacheHit"),
                "detail popup title should contain event type name"
            );
            assert!(
                payload_decoded.contains("scc-cache"),
                "detail popup content should contain event source"
            );
        }
        _ => panic!("expected EventDetail popup command, got {cmd:?}"),
    }
}

// ============================================================
// 测试 6:filter_topic / filter_level 过滤生效
// ============================================================

#[test]
fn event_stream_panel_filter_by_topic() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q1".into(),
            veto_reason: "unsafe".into(),
            frozen_capabilities: vec![],
        },
    ]);
    state.filter_topic = Some("security".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(
        filtered.len(),
        1,
        "security topic should filter to SkepticVeto only"
    );
    assert_eq!(filtered[0].type_name(), "SkepticVeto");
}

#[test]
fn event_stream_panel_filter_by_level_critical() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        },
    ]);
    state.filter_level = Some("critical".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(
        filtered.len(),
        1,
        "critical level should filter to BudgetExceeded only"
    );
    assert_eq!(filtered[0].type_name(), "BudgetExceeded");
}

// ============================================================
// 测试 7:空状态显示 "No events"
// ============================================================

#[test]
fn event_stream_panel_empty_state_shows_no_events() {
    let state = TuiState::new(); // latest_events 为空
    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("No events") || content.contains("no events"),
        "empty state should show 'No events', got: {}",
        &content[..content.len().min(200)]
    );
}

// ============================================================
// 测试 8:关键字过滤
// ============================================================

#[test]
fn event_stream_panel_filter_by_keyword() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha-key".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "beta-key".into(),
        },
    ]);
    state.filter_keyword = Some("alpha".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1, "keyword filter should narrow to 1 event");
    assert_eq!(filtered[0].type_name(), "CacheHit");
}

// ============================================================
// 测试 9:Shift+G 跳到底部并恢复 auto_scroll
// ============================================================

#[test]
fn event_stream_panel_shift_g_jumps_to_bottom_and_restores_auto_scroll() {
    let events: Vec<NexusEvent> = (0..10).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = false; // 模拟用户之前手动滚动过

    let mut panel = EventStreamPanel::new();
    // 先按 Down 让 selected != 0
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    let selected_before = panel.selected();
    assert!(selected_before > 0, "Down should move selection");

    // 按 Shift+G 跳到底部
    panel.handle_key(
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
        &mut state,
    );

    let filtered = EventStreamPanel::filtered_events(&state);
    let last_idx = filtered.len().saturating_sub(1);
    assert_eq!(
        panel.selected(),
        last_idx,
        "Shift+G should select last event"
    );
    assert!(state.auto_scroll, "Shift+G should restore auto_scroll=true");
}

// ============================================================
// 测试 10:auto_scroll=false 时显示 "[新事件 N 条]" 提示
// ============================================================

#[test]
fn event_stream_panel_shows_new_events_indicator_when_auto_scroll_off() {
    let events: Vec<NexusEvent> = (0..3).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = false;

    let content = EventStreamPanel::content(&state, 0).to_string();
    // WHY 提示文案:参考 Claude Code 工具调用流式输出 UX
    // 当用户暂停 auto_scroll 后,新事件累积应通过提示告知操作员
    assert!(
        content.contains("新事件") || content.contains("auto-scroll"),
        "should show new events indicator when auto_scroll=false, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// 测试 11:鼠标滚轮关闭 auto_scroll
// ============================================================

#[test]
fn event_stream_panel_mouse_scroll_disables_auto_scroll() {
    use crossterm::event::{MouseEvent, MouseEventKind};

    let events: Vec<NexusEvent> = (0..5).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let mouse = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(mouse, &mut state);

    assert!(
        !state.auto_scroll,
        "mouse scroll should disable auto_scroll"
    );
}

// ============================================================
// 测试 12:基础 Panel trait 行为
// ============================================================

#[test]
fn event_stream_panel_id_is_registered() {
    let panel = EventStreamPanel::new();
    assert_eq!(panel.id(), chimera_tui::PanelId::EventStream);
}

#[test]
fn event_stream_panel_question_mark_returns_none() {
    let mut panel = EventStreamPanel::new();
    let mut state = TuiState::new();
    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(
        panel.handle_key(key, &mut state),
        None,
        "'?' should be handled globally by TuiApp as Help overlay"
    );
}
