//! P5 跨面板联动 — Quest→EventStream 跳转集成测试
//!
//! 验证在 Quest 面板按 Enter 时:
//! 1. 切换到 EventStream 面板
//! 2. `filter_keyword` 被设置为 quest_id
//! 3. EventStream 面板渲染时应用了筛选(仅显示包含 quest_id 的事件)
//!
//! 这是 P5 "Quest→Event 跳转" 的端到端验证。

#![forbid(unsafe_code)]

use chimera_tui::{TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

/// 构造简单 Quest
fn sample_quest(id: &str, title: &str) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![Task {
            task_id: format!("{id}-t1"),
            description: "test task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 渲染 TuiApp 并返回缓冲区文本内容
fn render_content(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 测试替身数据源
#[derive(Debug)]
struct MockDataSource {
    snapshot: chimera_tui::DataSnapshot,
    config: chimera_tui::DataSourceConfig,
}

impl MockDataSource {
    fn new(snapshot: chimera_tui::DataSnapshot) -> Self {
        Self {
            snapshot,
            config: chimera_tui::DataSourceConfig::default(),
        }
    }
}

impl chimera_tui::TuiDataSource for MockDataSource {
    fn snapshot(
        &self,
    ) -> Result<std::sync::Arc<chimera_tui::DataSnapshot>, chimera_tui::TuiError> {
        Ok(std::sync::Arc::new(self.snapshot.clone()))
    }

    fn config(&self) -> &chimera_tui::DataSourceConfig {
        &self.config
    }
}

// ============================================================
// Quest→EventStream 跳转测试
// ============================================================

#[test]
fn quest_enter_switches_to_event_stream_panel() {
    // 准备 Quest 列表(含关联事件)
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q-alpha", "Alpha Quest"),
            sample_quest("q-beta", "Beta Quest"),
        ],
        latest_events: std::sync::Arc::new(VecDeque::from([
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-alpha".into(),
                title: "Alpha Quest".into(),
                task_count: 1,
            },
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-beta".into(),
                title: "Beta Quest".into(),
                task_count: 1,
            },
        ])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Quest);

    // 按 Enter,应跳转到 EventStream 面板
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.current_panel(),
        chimera_tui::PanelId::EventStream,
        "Enter should switch to EventStream panel"
    );
}

#[test]
fn quest_enter_sets_filter_keyword_to_quest_id() {
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![sample_quest("q-alpha", "Alpha Quest")],
        latest_events: std::sync::Arc::new(VecDeque::from([NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-alpha".into(),
            title: "Alpha Quest".into(),
            task_count: 1,
        }])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // 按 Enter 跳转
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // 验证 filter_keyword 被设置为 quest_id
    assert_eq!(
        app.state().filter_keyword,
        Some("q-alpha".to_string()),
        "filter_keyword should be set to quest_id after Enter"
    );
}

#[test]
fn quest_enter_event_stream_applies_filter() {
    // 准备:2 个 Quest,各自有关联事件;选中第 1 个 Quest(q-alpha)
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q-alpha", "Alpha Quest"),
            sample_quest("q-beta", "Beta Quest"),
        ],
        latest_events: std::sync::Arc::new(VecDeque::from([
            // q-alpha 的关联事件
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-alpha".into(),
                title: "Alpha Quest".into(),
                task_count: 1,
            },
            // q-beta 的关联事件
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-beta".into(),
                title: "Beta Quest".into(),
                task_count: 1,
            },
            // 无关事件
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "unrelated".into(),
            },
        ])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // 选中第 1 个 Quest(q-alpha),按 Enter 跳转
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::EventStream);

    // 渲染 EventStream,验证只显示 q-alpha 相关事件
    let content = render_content(&mut app, 120, 30);
    assert!(
        content.contains("q-alpha"),
        "EventStream should show q-alpha related event after jump, content: {content}"
    );
    assert!(
        !content.contains("q-beta"),
        "EventStream should NOT show q-beta event (filtered out), content: {content}"
    );
    assert!(
        !content.contains("unrelated"),
        "EventStream should NOT show unrelated CacheHit event, content: {content}"
    );
}

#[test]
fn quest_enter_on_second_quest_filters_correctly() {
    // 选中第 2 个 Quest(q-beta),验证筛选的是 q-beta 而非 q-alpha
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q-alpha", "Alpha Quest"),
            sample_quest("q-beta", "Beta Quest"),
        ],
        latest_events: std::sync::Arc::new(VecDeque::from([
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-alpha".into(),
                title: "Alpha Quest".into(),
                task_count: 1,
            },
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q-beta".into(),
                title: "Beta Quest".into(),
                task_count: 1,
            },
        ])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // 按 Down 选中第 2 个 Quest(q-beta)
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    // 按 Enter 跳转
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::EventStream);
    assert_eq!(
        app.state().filter_keyword,
        Some("q-beta".to_string()),
        "filter should be q-beta for the second quest"
    );

    // 渲染验证:只显示 q-beta,不显示 q-alpha
    let content = render_content(&mut app, 120, 30);
    assert!(
        content.contains("q-beta"),
        "EventStream should show q-beta event, content: {content}"
    );
    assert!(
        !content.contains("q-alpha"),
        "EventStream should NOT show q-alpha event, content: {content}"
    );
}

#[test]
fn quest_enter_with_no_quest_does_not_jump() {
    // 无 Quest 时 Enter 不应跳转
    let snapshot = chimera_tui::DataSnapshot::default();

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Quest);

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    // 应仍停留在 Quest 面板,filter_keyword 未设置
    assert_eq!(
        app.current_panel(),
        chimera_tui::PanelId::Quest,
        "Enter with no quest should not switch panel"
    );
    assert!(
        app.state().filter_keyword.is_none(),
        "filter_keyword should not be set when no quest is selected"
    );
}

#[test]
fn quest_jump_sets_status_message() {
    // 验证跳转后状态栏显示确认消息
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![sample_quest("q-test", "Test Quest")],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let (msg, severity) = app
        .state()
        .status_message
        .as_ref()
        .expect("status message should be set after jump");
    assert!(
        msg.contains("q-test"),
        "status message should contain quest_id, got: {msg}"
    );
    assert!(
        msg.contains("EventStream"),
        "status message should mention EventStream, got: {msg}"
    );
    assert_eq!(*severity, chimera_tui::Severity::Info);
}
