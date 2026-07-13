//! Security 面板集成测试 — M2
//!
//! 验证 SecurityPanel 正确渲染安全事件列表、冻结能力、键盘导航与详情弹窗。

#![forbid(unsafe_code)]

use chimera_tui::{
    AsaInterventionSummary, BudgetMetrics, DataSnapshot, DataSourceConfig, HealthMetrics,
    MemoryMetrics, PanelId, RedTeamAuditSummary, SecurityState, SkepticVetoSummary, TuiApp,
    TuiConfig, TuiDataSource, TuiError,
};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

#[derive(Debug)]
struct SecurityTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl SecurityTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for SecurityTestSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
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

fn sample_security_snapshot() -> DataSnapshot {
    DataSnapshot {
        quest_list: Vec::new(),
        latest_events: VecDeque::new(),
        budget_metrics: BudgetMetrics::default(),
        memory_metrics: MemoryMetrics::default(),
        security_state: SecurityState {
            active_vetoes: vec![SkepticVetoSummary {
                quest_id: "q-1".into(),
                veto_reason: "unsafe shell injection".into(),
                frozen_capabilities: vec!["shell_exec".into()],
                timestamp: Utc::now(),
            }],
            recent_audits: vec![RedTeamAuditSummary {
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 3,
                total_probes: 10,
                detection_rate: 0.3,
                remediation_suggestion: "add input sanitization".into(),
                timestamp: Utc::now(),
            }],
            recent_interventions: vec![AsaInterventionSummary {
                operation_id: "op-1".into(),
                action: "Block".into(),
                safety_score: 0.15,
                block_reason: Some("malicious payload".into()),
                timestamp: Utc::now(),
            }],
            frozen_capabilities: vec!["shell_exec".into(), "file_delete".into()],
        },
        health_metrics: HealthMetrics::default(),
        budget_history: Vec::new(),
        memory_history: Vec::new(),
        event_rate_history: Vec::new(),
    }
}

#[test]
fn test_security_panel_renders_events() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Security"),
        "Security panel title should be rendered"
    );
    assert!(content.contains("VETO"), "Skeptic veto should be rendered");
    assert!(
        content.contains("unsafe shell injection"),
        "veto reason should be rendered"
    );
    assert!(
        content.contains("prompt_injection"),
        "red team audit should be rendered"
    );
    assert!(
        content.contains("shell_exec"),
        "frozen capability should be rendered"
    );
}

#[test]
fn test_security_panel_navigation_opens_detail_popup() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // 按下 Enter 打开选中事件的详情弹窗
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.state().popup_stack.is_empty(),
        "Enter should open detail popup"
    );

    // 弹窗应包含 Veto 详情
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Skeptic Veto Detail") || content.contains("q-1"),
        "popup should render detail content"
    );

    // Esc 关闭弹窗
    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state().popup_stack.is_empty());
}

#[test]
fn test_security_panel_down_navigation() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // 按 Down 三次不应 panic
    for _ in 0..3 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Security"));
}

#[test]
fn test_security_panel_empty_state() {
    let snapshot = DataSnapshot {
        security_state: SecurityState::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(SecurityTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("No security events"),
        "empty state should be shown"
    );
    assert!(
        content.contains("None"),
        "frozen capabilities empty hint should be shown"
    );
}

#[test]
fn test_security_panel_color_by_severity() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // 渲染不应 panic;颜色通过 ratatui style 施加,此处仅验证内容存在
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("[VETO]"));
    assert!(content.contains("[AUDIT]"));
    assert!(content.contains("[ASA]"));
}
