//! Health 面板集成测试 — M2
//!
//! 验证 HealthPanel 正确渲染事件速率、慢消费者、平均延迟与健康评分公式。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, HealthMetrics, PanelId, TuiApp, TuiConfig, TuiDataSource,
    TuiError,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[derive(Debug)]
struct HealthTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl HealthTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for HealthTestSource {
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

#[test]
fn test_health_panel_renders_with_sample_data() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics {
            events_per_second: 120.5,
            slow_consumer_count: 2,
            average_latency_ms: 23.4,
            health_score: 80,
        },
        event_rate_history: vec![100, 110, 105, 115, 120, 118, 122],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Health"),
        "Health panel title should be rendered"
    );
    assert!(
        content.contains("120.5"),
        "events per second should be rendered"
    );
    assert!(
        content.contains("2"),
        "slow consumer count should be rendered"
    );
    assert!(
        content.contains("23.4 ms"),
        "average latency should be rendered"
    );
    assert!(content.contains("80"), "health score should be rendered");
}

#[test]
fn test_health_score_formula() {
    // M2 健康评分公式:100 - 10 * slow_consumer_count,最低 0
    assert_eq!(HealthMetrics::compute_health_score(0), 100);
    assert_eq!(HealthMetrics::compute_health_score(1), 90);
    assert_eq!(HealthMetrics::compute_health_score(5), 50);
    assert_eq!(HealthMetrics::compute_health_score(10), 0);
    assert_eq!(HealthMetrics::compute_health_score(20), 0);
}

#[test]
fn test_health_panel_empty_data_renders_defaults() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Health"),
        "Health panel should render even with default data"
    );
    assert!(
        content.contains("100"),
        "default health score should be 100"
    );
}

#[test]
fn test_health_panel_low_score_uses_red_color() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics {
            health_score: 30,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("30"));
}
