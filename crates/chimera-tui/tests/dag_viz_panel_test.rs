//! DagVizPanel 集成测试
//!
//! PS-2 批次3 起:DagVizPanel 不再直调 `gsoe_evolution::spec_dag_snapshot()`
//! (该全局在生产装配面恒空 → 恒显示 "0 nodes, 0 edges",属假数据)。
//! 现改为**诚实标注未接线**;本测试守护"不再渲染失效计数"这一新契约。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, PanelId, TuiApp, TuiConfig, TuiDataSource, TuiError,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 测试数据源 — 返回空快照(Quest 列表为空,展示"等待"状态)
#[derive(Debug)]
struct DagVizTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl DagVizTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for DagVizTestSource {
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

#[test]
fn test_dag_viz_panel_shows_unwired_marker_instead_of_dead_counts() {
    // PS-2 批次3:GSOE 谱系图在生产装配面从未实例化(SPEC_DAG 恒空),
    // 故不再渲染 "Spec DAG: N nodes, M edges" —— 那是**假数据**
    // (语义误导为"无规范"而非"未接线")。改为诚实标注。
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DagVizTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::DagViz);

    let content = render_to_string(&mut app, 80, 30);
    assert!(
        !content.contains("nodes") && !content.contains("edges"),
        "不得再渲染失效的 DAG 计数行,实际内容全文:\n{}",
        content
    );
    assert!(
        content.contains("DAG"),
        "面板仍应保留 DAG 区块标识,实际内容全文:\n{}",
        content
    );
}
