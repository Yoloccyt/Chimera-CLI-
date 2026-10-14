//! Log 面板集成测试 — 验证事件过滤、标题指示器与详情弹窗

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

mod common;
use common::mock::MockDataSource;

use chimera_tui::config::TuiConfig;
use chimera_tui::data::DataSnapshot;
use chimera_tui::types::PanelId;
use chimera_tui::TuiApp;
use chimera_tui::{LogPanel, Panel, PopupKind, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::Terminal;
use std::collections::VecDeque;

fn make_state_with_events(events: Vec<NexusEvent>) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = std::sync::Arc::new(VecDeque::from(events));
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
    // IT-01(批次-B):Search 模式已删除;过滤器为纯状态驱动,直接设值验证
    state.filter_keyword = Some("alpha".into());

    let filtered = LogPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].type_name(), "CacheHit");
}

/// 渲染并返回按行拼接的缓冲文本(行间 '\n',保留行级结构供行级断言)
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(width as usize)
        .map(|row| {
            row.iter()
                .map(|c| c.symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// F-11 回归门:50 条事件连按 Down 到底后,选中行(带 > 前缀)与 showing 提示行
/// 在 render 路径(TestBackend 视口裁剪后)仍可见 —— 修复前虚拟窗口 ±5 缓冲在无
/// .scroll 补偿的直渲染下把可视带整体下移,稳态选中文本行恒落在可视带之外
/// (80x24 复算:inner_h=12、content_height=7,稳态选中文本行 13 > 12)。
#[test]
fn log_panel_down_to_bottom_keeps_selected_and_showing_visible() {
    // WHY pin En:showing 提示 zh 值为 CJK,TestBackend 宽字符续格夹空致
    // contains(zh 文案) 不可靠(i18n_chrome_test.rs:6 先例);En 值 "showing"
    // 为 ASCII。guard 防并行测试污染全局 locale。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    // source 各异使事件行文本可区分;Log 最新在前 → idx 49 = src-0(最早)
    let events: Vec<NexusEvent> = (0..50u32)
        .map(|i| NexusEvent::CacheHit {
            metadata: EventMetadata::new(format!("src-{i}")),
            cache_key: format!("key-{i}"),
        })
        .collect();
    let snapshot = DataSnapshot {
        latest_events: std::sync::Arc::new(VecDeque::from(events)),
        ..Default::default()
    };
    let ds = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(ds),
    )
    .unwrap();
    app.switch_panel_to(PanelId::Log);
    // WHY 先 update():TuiApp::render 不拉快照,不同步则 latest_events 为空
    app.update();

    // 连按 Down 到底(50 次 ≥ 49 步,面板内 Up/Down 导航钳制在末条)
    for _ in 0..50 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let content = render_to_string(&mut app, 80, 24);

    assert!(
        content.contains("CacheHit"),
        "事件列表应渲染, got: {content}"
    );
    // 选中行可见:全帧唯一含 "[src-0] CacheHit" 的行应带 > 前缀
    // (修复前该行稳态恒在可视带外,find 返回 None 必红)
    let selected_line = content
        .lines()
        .find(|l| l.contains("[src-0] CacheHit"))
        .expect("选中事件 src-0 行应渲染在视口内");
    assert!(
        selected_line.contains("> "),
        "底部选中行应带 > 前缀, got: {selected_line}"
    );
    // showing 提示行在 render 路径可见(修复前滚动中部/贴底被推挤出视口)
    assert!(
        content.contains(chimera_tui::t!("panel.log.showing")),
        "showing 提示行应渲染在视口内, got: {content}"
    );
    // 页脚可见性(批次三终审 #2):F-17 迁键后页脚走 panel.footer 键,
    // 断言与键表同源(pin En 下 t! 返回 En 值,渲染面同 locale);
    // 与 event_stream 贴底用例的页脚断言口径对齐
    assert!(
        content.contains(chimera_tui::t!("panel.footer")),
        "页脚应渲染在视口内, got: {content}"
    );
}
