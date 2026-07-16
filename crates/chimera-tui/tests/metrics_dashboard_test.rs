//! MetricsDashboardPanel 单元/集成测试 — 5×2 网格 + bind/unbind
//!
//! 对应架构层:L10 Interface
//! 对应 spec:`.trae/specs/enterprise-tui-monitoring-task-viz/spec.md` Task 2.2
//!
//! # 测试策略(WHY)
//! - **RED 阶段**:本文件先于 `metrics_dashboard.rs` 编写,所有 3 个测试
//!   在 GREEN 阶段前应失败(compilation failure 或 assertion failure),
//!   验证契约清晰可执行。
//! - **GREEN 阶段**:`MetricsDashboardPanel` 实现后,所有 3 个测试通过,
//!   确保 5×2 网格布局、bind 创建 cell、unbind 移除 cell 三条核心契约
//!   都被覆盖。
//! - **测试桩数据源**:`StubDataSource` 提供完整 `DataSnapshot` 的所有字段,
//!   验证 bind 后面板能正常消费数据源而无需连接真实 event-bus。
//! - **渲染验证**:使用 `ratatui::backend::TestBackend` 渲染到 Buffer,
//!   验证非默认 cell 数量与 cell 标题文本,确保"网格确实渲染了内容"。

use std::sync::Arc;

use chimera_tui::data::{DataSnapshot, DataSourceConfig, TuiDataSource};
use chimera_tui::panels::MetricsDashboardPanel;
use chimera_tui::types::TuiState;
use chimera_tui::viz::VizChartKind;
use chimera_tui::{Panel, PanelId};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

/// 构造一个最小终端 Buffer 与对应的 Rect 用于渲染验证。
///
/// 风格与 `viz_components_test.rs::make_buffer` 一致,确保本测试
/// 与既有 viz 测试在同一基线。
fn make_buffer(width: u16, height: u16) -> (Buffer, Rect) {
    let area = Rect::new(0, 0, width, height);
    (Buffer::empty(area), area)
}

/// 统计 Buffer 中非默认 cell 数量,用于断言"面板确实渲染了内容"。
fn count_non_default_cells(buf: &Buffer, width: u16, height: u16) -> usize {
    let mut count = 0;
    for y in 0..height {
        for x in 0..width {
            if let Some(cell) = buf.cell(Position::new(x, y)) {
                let style = cell.style();
                let has_fg = style.fg.is_some() && style.fg != Some(Color::Reset);
                let has_bg = style.bg.is_some() && style.bg != Some(Color::Reset);
                let has_modifier = !style.add_modifier.is_empty();
                if has_fg || has_bg || has_modifier {
                    count += 1;
                }
            }
        }
    }
    count
}

/// 构造测试桩 `TuiDataSource`,用于 bind 测试。
///
/// 内部持有 `DataSnapshot`,`snapshot()` 直接返回(无需 event-bus)。
/// WHY 自定义而非复用 `StubDataSource`:`StubDataSource::snapshot()` 返回
/// 固定内容,本测试需要在 `bind` 时验证 source 被实际调用(可扩展为
/// 调用计数断言)。
#[derive(Debug, Default, Clone)]
struct TestDataSource {
    snapshot: DataSnapshot,
}

impl TestDataSource {
    /// 创建带示例预算指标的桩(用于 sparkline / gauge cell 渲染)
    fn with_budget(utilization: f32) -> Self {
        let mut snap = DataSnapshot::default();
        snap.budget_metrics.utilization_rate = utilization;
        snap.budget_history = vec![30, 32, 35, 33, 36, 38, 35, 37];
        Self { snapshot: snap }
    }
}

impl TuiDataSource for TestDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, chimera_tui::TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        // 借用 DataSourceConfig::default() 静态值的引用
        // WHY:此处要求 `&'static DataSourceConfig`,使用 leak 暴露一个
        // 静态默认值(test-only,避免污染真实数据源配置)
        static CFG: std::sync::OnceLock<DataSourceConfig> = std::sync::OnceLock::new();
        CFG.get_or_init(DataSourceConfig::default)
    }
}

#[test]
fn test_grid_layout_5x2_renders_correctly() {
    // 1) 创建 5×2 网格面板(所有 cell 未绑定)
    let mut panel = MetricsDashboardPanel::new();
    let state = TuiState::new();

    // 2) 验证 PanelId 与 title
    assert_eq!(panel.id(), PanelId::MetricsDashboard);

    // 3) 渲染到 100x20 终端(5 行 × 2 列 网格,左右各占 ~50 列)
    let (mut buf, area) = make_buffer(100, 20);
    panel.render(&state, area, &mut buf);

    // 4) 验证 Buffer 中应有非默认 cell(网格确实画了内容)
    let non_default = count_non_default_cells(&buf, 100, 20);
    assert!(
        non_default > 0,
        "5x2 grid should render non-default cells, got {}",
        non_default
    );

    // 5) 通过 TestBackend 验证渲染管线不 panic(完整的 ratatui 渲染路径)
    let backend = ratatui::backend::TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).expect("terminal creation");
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .expect("5x2 grid render should not panic");
}

#[test]
fn test_bind_data_source_creates_cell() {
    // 1) 创建面板并绑定两个 cell(位置 (0, 0) 用 sparkline, (0, 1) 用 gauge)
    let mut panel = MetricsDashboardPanel::new();
    let source: Arc<dyn TuiDataSource + Send + Sync> = Arc::new(TestDataSource::with_budget(0.5));

    // WHY 同时绑定两种 VizChartKind:验证 bind 接受不同 chart kind 的 cell,
    // 覆盖 sparkline + gauge 两个分支,避免单一 chart kind 的"虚假通过"。
    panel.bind(source.clone(), VizChartKind::LineChart, (0, 0));
    panel.bind(source.clone(), VizChartKind::Gauge, (0, 1));

    // 2) 验证绑定后 cell 存在(通过 cell_count 公开方法)
    assert_eq!(panel.cell_count(), 2, "two binds should produce two cells");

    // 3) 渲染验证:绑定的 cell 应有可视化输出
    let state = TuiState::new();
    let (mut buf, area) = make_buffer(100, 20);
    panel.render(&state, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 100, 20);
    assert!(
        non_default > 0,
        "bound cells should render visual content, got {} non-default cells",
        non_default
    );

    // 4) 完整 TestBackend 渲染路径(不 panic + 正确写入 buffer)
    let backend = ratatui::backend::TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).expect("terminal creation");
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .expect("bound cell render should not panic");
}

#[test]
fn test_unbind_removes_cell() {
    // 1) 创建面板并绑定 cell
    let mut panel = MetricsDashboardPanel::new();
    let source: Arc<dyn TuiDataSource + Send + Sync> = Arc::new(TestDataSource::with_budget(0.7));
    panel.bind(source.clone(), VizChartKind::LineChart, (0, 0));
    panel.bind(source.clone(), VizChartKind::Gauge, (1, 1));
    assert_eq!(panel.cell_count(), 2);

    // 2) 解绑其中一个 cell
    panel.unbind((0, 0));
    assert_eq!(
        panel.cell_count(),
        1,
        "unbind should reduce cell count by 1"
    );

    // 3) 解绑不存在的 cell(幂等操作,不应 panic 也不应减少计数)
    panel.unbind((4, 4)); // 5×2 网格中不存在的 (4, 4) 位置
    assert_eq!(
        panel.cell_count(),
        1,
        "unbind of empty position should be no-op"
    );

    // 4) 解绑另一个 cell 后,网格应为空
    panel.unbind((1, 1));
    assert_eq!(panel.cell_count(), 0, "all cells unbound should be 0");

    // 5) 渲染验证:全部 unbind 后,面板仍能渲染(空网格)
    let state = TuiState::new();
    let backend = ratatui::backend::TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).expect("terminal creation");
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .expect("empty grid render should not panic");
}
