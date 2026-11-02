//! Security 面板集成测试 — M2
//!
//! 验证 SecurityPanel 正确渲染安全事件列表、冻结能力、键盘导航与详情弹窗。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{
    AsaInterventionSummary, DataSnapshot, DataSourceConfig, PanelId, RedTeamAuditSummary,
    SecurityState, SkepticVetoSummary, TuiApp, TuiConfig, TuiDataSource, TuiError,
};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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
    fn snapshot(&self) -> Result<std::sync::Arc<DataSnapshot>, TuiError> {
        Ok(std::sync::Arc::new(self.snapshot.clone()))
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
        ..Default::default()
    }
}

#[test]
fn test_security_panel_renders_events() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // i18n(B4-I):面板名随 locale 切换,期望串与渲染同源经 t! 求值、locale 无关;
    // 持 guard 串行化但不写全局 locale——不新增 En 窗口。VETO/事件原因等面板
    // 正文为 locale 无关的既有渲染,断言保持原文(raw content)。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let panel_label = chimera_tui::t!(PanelId::Security.name_key());
    let content = render_to_string(&mut app, 80, 24);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        compact.contains(panel_label),
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
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(SecurityTestSource::new(sample_security_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // 按 Down 三次不应 panic
    for _ in 0..3 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    // i18n(B4-I):期望串与渲染同源经 t! 求值,locale 无关;不写全局 locale
    // (WHY 见 test_security_panel_renders_events 注)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let panel_label = chimera_tui::t!(PanelId::Security.name_key());
    let content = render_to_string(&mut app, 80, 24);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(compact.contains(panel_label));
}

#[test]
fn test_security_panel_empty_state() {
    let snapshot = DataSnapshot {
        security_state: SecurityState::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(SecurityTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Security);

    // i18n:断言默认中文文案,持 locale guard 与同二进制内 pin En 的测试互斥
    // (i18n::set_locale 文档 2026-08-17 互斥口径)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let content = render_to_string(&mut app, 80, 24);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        compact.contains("暂无安全事件"),
        "empty state should be shown"
    );
    assert!(
        compact.contains("(无)"),
        "frozen capabilities empty hint should be shown"
    );
}

#[test]
fn test_security_panel_color_by_severity() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
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
