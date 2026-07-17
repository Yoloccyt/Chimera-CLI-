//! TUI 指标仪表盘面板 — 5×2 可绑定网格
//!
//! 对应架构层:L10 Interface
//! 对应 spec:`.trae/specs/enterprise-tui-monitoring-task-viz/spec.md` Task 2.2
//!
//! # 核心职责
//! - 5×2 网格布局:左列 5 个 sparkline 实时指标,右列 5 个 gauge 当前值
//! - 任意 cell 通过 `bind(source, kind, position)` 绑定任意 `TuiDataSource`
//! - `unbind` 后 cell 槽位保留为"(empty)"占位符,网格布局不抖动
//! - 复用 `viz/` 组件库(`VizChartKind` 5 种全支持)
//!
//! # 设计决策(WHY)
//! - **`Vec<Option<Cell>>` 而非 `HashMap<(row,col), Cell>`**:5×2 固定网格
//!   用连续 Vec 存储更紧凑,索引 `row * 2 + col` 时间复杂度 O(1);
//!   `Option<Cell>` 自然支持 unbind 后保留槽位,网格布局保持 5×2 不变。
//! - **每帧调用 `source.snapshot()`(拉模式)**:与 `OsaSparsePanel` 等
//!   其他面板保持一致,拉模式保证数据新鲜度;`TuiDataSource::snapshot()`
//!   返回 `Result`,错误时回退到默认空快照(避免 TUI 渲染循环因瞬时
//!   错误而黑屏退出)。
//! - **按 `VizChartKind` match 分发 viz/ 组件**:`LineChart→line_chart`、
//!   `Heatmap→heatmap`、`BarChart→bar_chart`、`Gauge→viz::gauge`、
//!   `Histogram→histogram`。match 各分支独立编译,无需 dyn dispatch
//!   (符合 `viz/mod.rs` 的 `VizWidget` 设计)。
//! - **不镜像到 `TuiState`**:与 `QuestPanel` 等不同,本面板的数据
//!   直接来自 `TuiDataSource`(无对应业务事件),保持面板职责单一
//!   (纯视图层),避免 `TuiState` 字段膨胀。
//! - **不与全局 `FocusManager` 耦合**:5×2 网格的 cell 选择(高亮/导航)
//!   由面板内部 `selected` 状态管理,网格 cell 是局部子状态,
//!   不与 16 面板级焦点冲突(TuiApp FocusManager 注册 16 面板,本面板为新增第 16 个)。

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::data::TuiDataSource;
use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};
use crate::viz::{bar_chart, gauge as viz_gauge, heatmap, histogram, line_chart, VizChartKind};

/// 5×2 网格行数(spec Task 2.2 明确:5 个 sparkline / 5 个 gauge)
const GRID_ROWS: usize = 5;
/// 5×2 网格列数(spec Task 2.2 明确:sparkline 列 + gauge 列)
const GRID_COLS: usize = 2;
/// 5×2 网格总槽位数
const GRID_SIZE: usize = GRID_ROWS * GRID_COLS;

/// 网格 cell — 绑定一个数据源与一个图表类型
///
/// WHY 私有字段:`source` 是 `Arc<dyn TuiDataSource + Send + Sync>`,
/// 跨 crate 使用,公开字段会暴露内部数据结构;
/// 通过 `bind`/`unbind` 间接访问即可。
///
/// WHY `Send + Sync` 边界:Panel trait 要求 `Self: Send`,`Arc<dyn Trait>`
/// 本身默认 `!Send`,需要 `Trait + Send + Sync` 才能跨任务传递;
/// `TuiDataSource` 的所有实现(`StubDataSource` / `DataPipeline` 等)
/// 实际都满足 `Send + Sync`,显式标注只是让编译期可见。
///
/// WHY 不派生 Debug/Clone:`dyn TuiDataSource` 不实现 `Debug`,
/// 派生会编译失败;且 `Cell` 包含 `Arc<dyn ...>`,派生 Clone 会
/// 强制 `dyn Clone`,同样失败。`Cell` 内部仅 `MetricsDashboardPanel`
/// 持有,无需外部 Debug/Clone 能力。
struct Cell {
    source: Arc<dyn TuiDataSource + Send + Sync>,
    kind: VizChartKind,
}

impl PartialEq for Cell {
    /// WHY 自定义 PartialEq:`Arc<dyn TuiDataSource>` 不实现 PartialEq,
    /// 但测试断言需要区分"已绑定 vs 未绑定",用 kind 一致性作代理:
    /// 同 kind 且数据源指针相同时视为等价(实际数据差异不影响网格结构)。
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && Arc::ptr_eq(&self.source, &other.source)
    }
}

/// 指标仪表盘面板 — 5×2 可绑定网格
///
/// 每个 cell 可独立绑定 `Arc<dyn TuiDataSource>` + `VizChartKind`,
/// 渲染时按位置拉取快照并构造对应 viz widget。
///
/// WHY 不派生 Debug/Clone:含 `Vec<Option<Cell>>`(`Cell` 含 `Arc<dyn ...>`),
/// 派生会强制 `dyn Debug`/`dyn Clone` 编译失败。
pub struct MetricsDashboardPanel {
    /// 5×2 网格 cell 数组(`Option` 保留槽位以支持 unbind)
    cells: Vec<Option<Cell>>,
    /// 当前选中 cell 索引(键盘导航,默认 0)
    selected: usize,
}

impl Default for MetricsDashboardPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsDashboardPanel {
    /// 创建新的 MetricsDashboard 面板(5×2 网格,所有 cell 初始为 None)
    pub fn new() -> Self {
        // 预分配 10 个槽位,避免后续 bind 触发 realloc
        Self {
            cells: (0..GRID_SIZE).map(|_| None).collect(),
            selected: 0,
        }
    }

    /// 绑定一个数据源到指定网格 cell
    ///
    /// # 参数
    /// - `source`:数据源(通过 `Arc<dyn TuiDataSource>` 共享所有权,
    ///   避免与全局 TuiApp 持有的 `DataPipeline` 重复克隆)
    /// - `kind`:图表类型(`VizChartKind` 5 种)
    /// - `position`:(row, col) 二元组,`row ∈ [0, 5)`,`col ∈ [0, 2)`
    ///
    /// # 行为
    /// - 越界 `position`:静默 no-op(避免 panic 污染 TUI 事件循环)
    /// - 已绑定 cell:覆盖原绑定(同位置重复 bind 不会泄漏旧 source)
    /// - 同一 source 可绑定到多个 cell(Arc 共享,无额外开销)
    pub fn bind(
        &mut self,
        source: Arc<dyn TuiDataSource + Send + Sync>,
        kind: VizChartKind,
        position: (usize, usize),
    ) {
        if let Some(idx) = self.position_to_index(position) {
            self.cells[idx] = Some(Cell { source, kind });
        }
    }

    /// 解绑指定网格 cell(若该 cell 未绑定,no-op 幂等)
    ///
    /// WHY 保留槽位:unbind 后槽位仍占位,网格布局保持 5×2 不变,
    /// 避免"某 cell 移除后其他 cell 上移"导致布局抖动。
    pub fn unbind(&mut self, position: (usize, usize)) {
        if let Some(idx) = self.position_to_index(position) {
            self.cells[idx] = None;
        }
    }

    /// 返回当前已绑定 cell 数(测试与状态观察用)
    pub fn cell_count(&self) -> usize {
        self.cells.iter().filter(|c| c.is_some()).count()
    }

    /// 返回当前选中 cell 索引(测试用)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 将 (row, col) 位置转换为 Vec 索引
    ///
    /// 返回 `None` 表示越界(`row ≥ GRID_ROWS` 或 `col ≥ GRID_COLS`)。
    /// WHY 独立函数:bind/unbind/导航 共享索引计算,集中边界检查。
    fn position_to_index(&self, position: (usize, usize)) -> Option<usize> {
        let (row, col) = position;
        if row < GRID_ROWS && col < GRID_COLS {
            Some(row * GRID_COLS + col)
        } else {
            None
        }
    }

    /// 从 data source 拉取快照(错误时返回默认空快照)
    ///
    /// WHY 错误吞咽:`TuiDataSource::snapshot()` 返回 `Result`,但 TUI
    /// 渲染循环不应因数据源瞬时错误而退出(会黑屏),改为静默
    /// 回退到默认快照,下一帧继续尝试。
    fn snapshot_or_default(cell: &Cell) -> crate::data::DataSnapshot {
        cell.source.snapshot().unwrap_or_default()
    }

    /// 渲染单个 cell 到给定 area(按 `VizChartKind` 分发 viz 组件)
    ///
    /// WHY 5 个分支独立编写:5 个 viz 组件的构造参数签名不同
    /// (line_chart 要 `&[(f64,f64)]`、bar_chart 要 `&[(&str,f64)]`、
    /// histogram 要 `&[f32]` 等),无法用统一 trait 抽象;
    /// 牺牲 5 个 match 分支的代码量换各 viz 组件的类型安全。
    fn render_cell(cell: &Cell, area: Rect, buf: &mut Buffer) {
        let snap = Self::snapshot_or_default(cell);
        match cell.kind {
            VizChartKind::LineChart => {
                // 折线图:取 budget_history 序列化为 (idx, value) 元组
                let data: Vec<(f64, f64)> = snap
                    .budget_history
                    .iter()
                    .enumerate()
                    .map(|(i, &v)| (i as f64, v as f64))
                    .collect();
                let widget = line_chart(&data, "Metric", Color::Cyan);
                Widget::render(widget, area, buf);
            }
            VizChartKind::Heatmap => {
                // 热力图:取 budget_history 重组为 1xN 矩阵(spec 简化用法)
                let values: Vec<f32> = snap.budget_history.iter().map(|&v| v as f32).collect();
                let cols = values.len().max(1);
                let widget = heatmap(&values, 1, cols, "Heatmap");
                Widget::render(widget, area, buf);
            }
            VizChartKind::BarChart => {
                // 条形图:取 budget_history 作为类目(截断前 8 个避免长 label)
                let entries: Vec<(String, f64)> = snap
                    .budget_history
                    .iter()
                    .take(8)
                    .enumerate()
                    .map(|(i, &v)| (format!("b{i}"), v as f64))
                    .collect();
                let label_refs: Vec<(&str, f64)> =
                    entries.iter().map(|(l, v)| (l.as_str(), *v)).collect();
                let widget = bar_chart(&label_refs, "Bar", Color::Yellow);
                Widget::render(widget, area, buf);
            }
            VizChartKind::Gauge => {
                // 仪表盘:取 budget utilization_rate [0, 1] × 100
                let value = (snap.budget_metrics.utilization_rate as f64) * 100.0;
                let widget = viz_gauge(value, 100.0, 80.0, "Util");
                Widget::render(widget, area, buf);
            }
            VizChartKind::Histogram => {
                // 直方图:取 health_metrics.average_latency_ms
                let data = vec![snap.health_metrics.average_latency_ms as f32];
                let widget = histogram(&data, 4, "Latency");
                Widget::render(widget, area, buf);
            }
        }
    }

    /// 渲染空槽位占位符
    ///
    /// WHY 独立函数:unbind 后槽位仍占 1 cell,显示 "(empty)" 提示,
    /// 与 viz widget 风格一致(Border + 文字)。
    fn render_empty(area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" (empty) ")
            .border_style(Style::default().fg(Color::DarkGray));
        let paragraph = Paragraph::new(Line::from("(unbound)"))
            .style(Style::default().fg(Color::DarkGray))
            .block(block);
        Widget::render(paragraph, area, buf);
    }
}

impl Panel for MetricsDashboardPanel {
    fn id(&self) -> PanelId {
        PanelId::MetricsDashboard
    }

    fn title(&self) -> Line<'static> {
        // WHY BOLD 修饰:5×2 网格是密集视图,标题需醒目
        Line::from(" Metrics Dashboard ").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        // 1) 水平切分为 2 列(左 sparkline 列,右 gauge 列)
        //    WHY Percentage(50/50):与 spec "5×2 网格 左列 + 右列" 一致
        //    等宽划分,无侧重点(spec 未指定权重,等宽最自然)
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // 2) 每列垂直切分为 5 行
        //    WHY Percentage(20):5 行等高,避免首行/末行 cell 因高度
        //    不一致导致 viz 组件渲染异常(ratatui 高度 < 阈值时回退空白)
        let row_constraints: Vec<Constraint> = (0..GRID_ROWS)
            .map(|_| Constraint::Percentage((100 / GRID_ROWS) as u16))
            .collect();

        let col_rows: Vec<Vec<Rect>> = cols
            .iter()
            .map(|col| {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(row_constraints.clone())
                    .split(*col)
                    .to_vec()
            })
            .collect();

        // 3) 遍历 5×2 网格,按索引选择 cell 或渲染空槽位
        //
        // WHY enumerate 风格:cell 在 `self.cells` 中按 `row * GRID_COLS + col`
        // 线性存储,`enumerate` 提供 idx 同时迭代 cell,消除
        // `needless_range_loop` 警告;row/col 从 idx 反算(用于 `col_rows`
        // 二维布局索引,顺序与 self.cells 一致)。
        for (idx, cell_opt) in self.cells.iter().enumerate() {
            let row = idx / GRID_COLS;
            let col = idx % GRID_COLS;
            let cell_area = col_rows[col][row];
            match cell_opt {
                Some(cell) => Self::render_cell(cell, cell_area, buf),
                None => Self::render_empty(cell_area, buf),
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 5×2 网格的 10 个 cell 导航:vim 风格 hjkl
        // 简化:Up/Down 跳行,Left/Right 跳列,边界 clamp 不环绕
        // WHY 不环绕:测试规范未要求环绕行为,clamp 更安全
        // (避免用户按错方向跳到对侧 cell 误以为是 bug)
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected >= GRID_COLS {
                    self.selected -= GRID_COLS;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + GRID_COLS < GRID_SIZE {
                    self.selected += GRID_COLS;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if !self.selected.is_multiple_of(GRID_COLS) {
                    self.selected -= 1;
                }
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.selected % GRID_COLS < GRID_COLS - 1 && self.selected + 1 < GRID_SIZE =>
            {
                self.selected += 1;
            }
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // M1 不启用鼠标处理(与既有 16 面板一致)
        None
    }
}
