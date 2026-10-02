//! BlackboardPanel 集成测试 — F-59 专项测试补齐(R5)
//!
//! 覆盖: 面板注册可达 / next/prev 往返 / i18n 标题键 / 空态诚实展示 /
//! BlackboardEntry 事件注入派生渲染(app 级,经 latest_events)/ 同 key 最新覆盖 /
//! trait 对象分派。范式: experience_card_viz_panel_test.rs。
//!
//! # 范围注记(F-60 关联)
//! BlackboardEntry 当前生产侧零发布(见 event_coverage_drift_test.rs 的 F-60
//! 诚实登记;mlc-engine/blackboard_projector 为消费投影方),本文件以测试注入
//! 事件驱动派生渲染——锁定的是消费侧契约,不伪造生产数据链。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

mod common;

use common::mock::MockDataSource;
use std::collections::VecDeque;
use std::sync::Arc;

use chimera_tui::panels::{BlackboardPanel, Panel};
use chimera_tui::types::{PanelId, TuiState, ViewMode};
use chimera_tui::{TuiApp, TuiConfig};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::Terminal;

fn board(key: &str, epistemic: &str, summary: &str) -> NexusEvent {
    NexusEvent::BlackboardEntry {
        metadata: EventMetadata::new("t"),
        key: key.into(),
        summary: summary.into(),
        ttl_secs: 60,
        epistemic: epistemic.into(),
        provenance: None,
    }
}

/// app 级渲染(Dashboard):注入快照事件后收集整帧文本。
///
/// WHY 不 pin locale:断言对象为行内数据字段(epistemic/key/ttl,与 locale 无关),
/// 不触碰全局 locale 即无双写竞态;唯一 i18n 文案为面板 subtitle/chrome,不在断言内。
fn render_app_with_events(events: Vec<NexusEvent>, w: u16, h: u16) -> String {
    let snapshot = chimera_tui::data::DataSnapshot {
        latest_events: Arc::new(VecDeque::from(events)),
        ..Default::default()
    };
    let ds = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(ds),
    )
    .expect("with_data_source should succeed");
    app.switch_panel_to(PanelId::Blackboard);
    app.update();

    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|f| app.render(f)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

// ----------------------------------------------------------
// 面板注册闭环(D-5 红线)
// ----------------------------------------------------------

#[test]
fn panel_registered_in_focus_order() {
    assert!(
        PanelId::REGISTERED_FOCUS_ORDER.contains(&PanelId::Blackboard),
        "Blackboard 应注册进焦点环"
    );
}

#[test]
fn panel_next_prev_roundtrip() {
    let panel = PanelId::Blackboard;
    assert_eq!(panel.next().prev(), panel);
    assert_eq!(panel.prev().next(), panel);
}

#[test]
fn panel_type_name_and_title() {
    assert_eq!(PanelId::Blackboard.as_str(), "Blackboard");
    assert_eq!(PanelId::Blackboard.title(), " Blackboard ");
}

#[test]
fn i18n_title_key_exists() {
    assert!(
        !chimera_tui::i18n::tr("panel.border.blackboard").is_empty(),
        "panel.border.blackboard 键应存在"
    );
}

// ----------------------------------------------------------
// 渲染闭环
// ----------------------------------------------------------

#[test]
fn empty_state_is_honest_without_events() {
    let content = BlackboardPanel::content(&TuiState::new()).to_string();
    assert!(
        content.contains("无黑板条目") || content.contains("No board entries"),
        "空事件流应诚实展示无黑板条目, got: {content}"
    );
}

#[test]
fn app_level_entries_derived_from_events() {
    // 注入 BlackboardEntry 事件(经快照 latest_events)→ app 集成层派生渲染:
    // 行格式 "[{epistemic}] {key} (ttl={n}s)",按 epistemic 分组(Fact < Observation)。
    let content = render_app_with_events(
        vec![
            board("alpha", "Fact", "s1"),
            board("beta", "Observation", "s2"),
        ],
        100,
        30,
    );
    assert!(
        content.contains("[Fact] alpha (ttl=60s)"),
        "Fact 条目应派生渲染, got: {content}"
    );
    assert!(
        content.contains("[Observation] beta (ttl=60s)"),
        "Observation 条目应派生渲染, got: {content}"
    );
    assert!(
        content.find("[Fact] alpha").unwrap() < content.find("[Observation] beta").unwrap(),
        "应按 epistemic 升序分组渲染(Fact 在前)"
    );
}

#[test]
fn same_key_latest_wins_at_panel_level() {
    // 同 key 去重、最新事件覆盖:同一 key 两条事件只渲染后写入的 epistemic
    let mut state = TuiState::new();
    state.latest_events = Arc::new(VecDeque::from([
        board("k", "Fact", "v1"),
        board("k", "Episode", "v2"),
    ]));
    let content = BlackboardPanel::content(&state).to_string();
    assert!(
        content.contains("[Episode] k"),
        "同 key 最新事件应覆盖渲染, got: {content}"
    );
    assert!(
        !content.contains("[Fact]"),
        "被覆盖的旧条目不得残留, got: {content}"
    );
}

#[test]
fn panel_trait_object_dispatch() {
    let panel: Box<dyn Panel> = Box::new(BlackboardPanel::new());
    assert_eq!(panel.id(), PanelId::Blackboard);
    assert!(!panel.shortcuts().is_empty());
}
