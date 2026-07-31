//! Task 3.5: L5 Knowledge 协同 — DagVizPanel 谱系 DAG 快照集成测试
//!
//! 验证 DagVizPanel 调用 `gsoe_evolution::spec_dag_snapshot()` 显示
//! 谱系 DAG 节点/边计数,实现 L10 Panel ↔ L5 Knowledge 真实数据闭环。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, PanelId, TuiApp, TuiConfig, TuiDataSource, TuiError,
};
use gsoe_evolution::spec_dag_snapshot;
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
fn test_dag_viz_panel_displays_spec_dag() {
    // 验证面板渲染包含 spec_dag_snapshot() 返回的节点/边计数
    let snapshot = DataSnapshot::default();
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(DagVizTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::DagViz);

    let content = render_to_string(&mut app, 80, 30);
    // 面板应包含 "Spec DAG:" 行(谱系 DAG 快照)
    assert!(
        content.contains("Spec DAG:"),
        "面板应显示谱系 DAG 快照,实际内容全文:\n{}",
        content
    );
    assert!(
        content.contains("nodes"),
        "面板应显示节点计数,实际内容前 500 字符: {}",
        &content[..content.len().min(500)]
    );
    assert!(
        content.contains("edges"),
        "面板应显示边计数,实际内容前 500 字符: {}",
        &content[..content.len().min(500)]
    );

    // 验证 gsoe_evolution::spec_dag_snapshot() 默认返回空快照
    let snapshot = spec_dag_snapshot();
    assert!(snapshot.nodes.is_empty(), "默认节点列表应为空");
    assert!(snapshot.edges.is_empty(), "默认边列表应为空");
}
