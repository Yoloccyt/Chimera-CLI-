//! P4.2 Parliament 面板虚拟滚动集成测试
//!
//! 验证 Parliament 面板在 1000+ 条事件下的虚拟滚动行为

use chimera_tui::config::TuiConfig;
use chimera_tui::data::DataSnapshot;
use chimera_tui::data::DataSourceConfig;
use chimera_tui::data::TuiDataSource;
use chimera_tui::error::TuiError;
use chimera_tui::types::PanelId;
use chimera_tui::TuiApp;
use event_bus::{EventMetadata, NexusEvent};
use ratatui::Terminal;
use std::collections::VecDeque;

/// Mock 数据源
struct MockDataSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl TuiDataSource for MockDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }
    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 构造含 1000 条 VoteCast 事件的快照
fn snapshot_with_1000_votes() -> DataSnapshot {
    let mut events = VecDeque::new();
    for i in 0..1000u32 {
        let event = NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: format!("prop-{i}"),
            voter: format!("voter-{i}"),
            vote: i % 2 == 0,
        };
        events.push_back(event);
    }
    DataSnapshot {
        latest_events: events,
        ..Default::default()
    }
}

/// 渲染并返回字符串
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 构造带 1000 事件的 app
fn app_with_1000_votes() -> TuiApp {
    let snapshot = snapshot_with_1000_votes();
    let ds = MockDataSource {
        snapshot,
        config: DataSourceConfig::default(),
    };
    let mut app = TuiApp::with_data_source(TuiConfig::default(), Box::new(ds)).unwrap();
    app.switch_panel_to(PanelId::Parliament);
    app
}

/// 1000 条事件渲染不 panic
#[test]
fn parliament_1000_events_renders_without_panic() {
    let mut app = app_with_1000_votes();
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Parliament"));
}

/// 1000 条事件下 j 键滚动
#[test]
fn parliament_1000_events_j_scroll() {
    let mut app = app_with_1000_votes();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for _ in 0..5 {
        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    }
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Parliament"));
}

/// 1000 条事件下 G 跳底不 panic
#[test]
fn parliament_1000_events_shift_g_jump_bottom() {
    let mut app = app_with_1000_votes();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.handle_key_event(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Parliament"));
}

/// 1000 条事件下 g 跳顶不 panic
#[test]
fn parliament_1000_events_g_jump_top() {
    let mut app = app_with_1000_votes();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for _ in 0..10 {
        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Parliament"));
}
