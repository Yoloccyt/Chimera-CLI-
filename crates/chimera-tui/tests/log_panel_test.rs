//! Log 面板集成测试 — 验证事件过滤、标题指示器与详情弹窗

#![forbid(unsafe_code)]

use chimera_tui::{InputMode, LogPanel, Panel, PopupKind, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use std::collections::VecDeque;

fn make_state_with_events(events: Vec<NexusEvent>) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from(events);
    state
}

#[test]
fn log_panel_renders_events() {
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

    let content = LogPanel::content(&state, 0).to_string();
    assert!(content.contains("CacheHit"));
    assert!(content.contains("BudgetExceeded"));
}

#[test]
fn log_panel_filter_by_keyword() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "beta".into(),
        },
    ]);
    state.filter_keyword = Some("alpha".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "CacheHit");
}

#[test]
fn log_panel_filter_by_topic() {
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

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "SkepticVeto");
}

#[test]
fn log_panel_filter_by_topic_quest() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            title: "Test Quest".into(),
            task_count: 3,
        },
    ]);
    state.filter_topic = Some("quest".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "QuestCreated");
}

#[test]
fn log_panel_filter_by_level_critical() {
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

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "BudgetExceeded");
}

#[test]
fn log_panel_filter_by_level_error() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::OperationTimedOut {
            metadata: EventMetadata::new("gqep-executor"),
            operation_id: "op1".into(),
            timeout_ms: 1000,
        },
    ]);
    state.filter_level = Some("error".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "OperationTimedOut");
}

#[test]
fn log_panel_filter_by_level_warn() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("decb-governor"),
            quest_id: "q1".into(),
            old_tier: "High".into(),
            new_tier: "Medium".into(),
            coefficient: 0.8,
            reason: "consumption".into(),
        },
    ]);
    state.filter_level = Some("warn".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "BudgetAdjusted");
}

#[test]
fn log_panel_combined_filters() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "alpha-q".into(),
            veto_reason: "unsafe".into(),
            frozen_capabilities: vec![],
        },
    ]);
    state.filter_keyword = Some("alpha".into());
    state.filter_topic = Some("security".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "SkepticVeto");
}

#[test]
fn log_panel_navigation() {
    let mut panel = LogPanel::new();
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

    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 1);

    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert_eq!(panel.selected(), 0);
}

#[test]
fn log_panel_enter_opens_detail_popup() {
    let mut panel = LogPanel::new();
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
            assert!(title.contains("CacheHit"));
            assert!(payload_decoded.contains("scc-cache"));
            assert!(payload_decoded.contains("k1"));
        }
        _ => panic!("expected EventDetail popup command, got {:?}", cmd),
    }
}

#[test]
fn log_panel_search_input_via_state() {
    // 模拟用户通过搜索模式设置关键字过滤器
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "beta".into(),
        },
    ]);
    state.input_mode = InputMode::Search;
    state.input_buffer = "alpha".into();

    // 搜索提交由 CommandPalette 处理,这里直接验证状态驱动过滤
    state.filter_keyword = Some("alpha".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "CacheHit");
}
