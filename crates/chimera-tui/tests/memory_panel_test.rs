//! Memory 面板集成测试 — M2
//!
//! 验证 MemoryPanel 在自定义数据下正确渲染命中率、上下文窗口、压缩率与层级。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, MemoryMetrics, PanelId, TuiApp, TuiConfig, TuiDataSource,
    TuiError,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 测试数据源 — 返回预设 Memory 指标
#[derive(Debug)]
struct MemoryTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl MemoryTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for MemoryTestSource {
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
fn test_memory_panel_renders_with_sample_data() {
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics {
            hit_rate_percent: 92.5,
            evictions: 3,
            context_window_size: 8192,
            compressed_ratio: 0.65,
            cache_hits: 500,
            cache_misses: 42,
            tier: "L2".into(),
        },
        budget_history: vec![70, 75, 80, 85, 90, 92],
        memory_history: vec![80, 82, 85, 88, 90, 92],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Memory"),
        "Memory panel title should be rendered"
    );
    assert!(
        content.contains("92.5%"),
        "hit rate should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("8192 bytes"),
        "context window size should be rendered"
    );
    assert!(
        content.contains("65.0%") || content.contains("65%"),
        "compressed ratio should be rendered"
    );
    assert!(content.contains("L2"), "tier should be rendered");
}

#[test]
fn test_memory_panel_empty_data_renders_defaults() {
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Memory"),
        "Memory panel should render even with default data"
    );
    assert!(content.contains("L0"));
}
