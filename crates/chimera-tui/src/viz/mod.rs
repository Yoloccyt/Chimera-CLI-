//! Chimera TUI 可视化组件库 — 高阶图表 widget 集合
//!
//! 对应架构层:L10 Interface
//! 对应 spec:`.trae/specs/enterprise-tui-monitoring-task-viz/spec.md` §四 + `NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6
//!
//! # 核心职责
//! - 提供 5 种可复用可视化 widget:`line_chart` / `heatmap` / `bar_chart` / `gauge` / `histogram`
//! - 与 `render.rs` 基础组件(`sparkline` / `gauge_thresholded` / `horizontal_bar_chart`)互补,
//!   构成完整可视化栈
//! - 全部基于 `ratatui::widgets::Paragraph` / `Block` 渲染,纯文本字符,
//!   不引入额外依赖,符合 §4.1 严禁引入新依赖约束
//!
//! # 设计决策(WHY)
//! - **统一 `VizChartKind` 枚举**:面板需要按用户偏好/配置选择不同图表类型
//!   (如 `MetricsDashboardPanel` 5×2 网格),`VizChartKind` 提供类型安全的标识。
//! - **函数式 + 返回 `impl Widget`**:与 `render.rs` 既有 `sparkline`/`gauge`
//!   风格一致(返回 `Sparkline` / `Gauge` widget),不引入 Panel 抽象,降低
//!   `MetricsDashboardPanel` 集成成本。
//! - **字符渐进 + 颜色编码**:与 `render::heat_bar` 风格一致,使用 `░/▒/▓`
//!   三档强度字符 + 蓝/灰/红三档颜色,在非彩色终端也能区分强度。
//! - **不引入 ratatui-chart**:避免新依赖(`§4.1 严禁引入新依赖`),
//!   自研纯字符渲染已能满足 5×2 网格密度需求。
//!
//! # 模块可见性(§6.4 D3 决策)
//! 全部 `pub`,供 `MetricsDashboardPanel` / `panels::task_manager` / 测试模块复用。
//! `chimera-tui` 是 L10 leaf crate,虽无外部下游,但 `viz` 模块属于 TUI
//! 子系统 API,严格封装通过模块路径 `chimera_tui::viz` 而非 `pub use` 暴露出。

#![forbid(unsafe_code)]

use ratatui::widgets::Widget;

pub mod bar_chart;
pub mod gauge;
pub mod heatmap;
pub mod histogram;
pub mod line_chart;

// 公开 API 重导出 — 与 `render.rs` 模块导出风格一致
pub use bar_chart::bar_chart;
pub use gauge::gauge;
pub use heatmap::heatmap;
pub use histogram::histogram;
pub use line_chart::line_chart;

/// 可视化图表类型 — 5 种高阶图表枚举
///
/// WHY 独立枚举:`MetricsDashboardPanel` 等面板需按用户偏好/配置选择
/// 不同图表类型,提供类型安全的标识替代字符串字面量,避免
/// `"line_chart"` 等 typo 与运行时未匹配错误。
///
/// 与 `render.rs` 基础组件(`sparkline` / `gauge` / `horizontal_bar_chart`)
/// 互补但不重叠:基础组件面向"单点 widget",`VizChartKind` 面向
/// "面板级多 cell 复合图表"(`line_chart` / `heatmap` / `histogram` 等)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VizChartKind {
    /// 折线图(时间序列)— 指标趋势 / 延迟曲线 / 吞吐量
    LineChart,
    /// 热力图(二维矩阵)— CLV 8 分块 / 路由器命中率矩阵
    Heatmap,
    /// 条形图(类别对比)— 三路由器命中率 / 议员投票对比
    BarChart,
    /// 仪表盘(单值 + 阈值)— 健康评分 / 预算使用率 / 稀疏度
    Gauge,
    /// 直方图(分布)— 延迟分布 / 内存占用分布
    Histogram,
}

/// `viz` 模块统一返回类型约束 — 任何实现 ratatui `Widget` 的类型均可返回
///
/// WHY 不绑定具体类型:5 个组件可能返回 `Paragraph` 或自定义 widget
/// struct,统一 trait bound 让 `MetricsDashboardPanel` 可用泛型
/// 存储任一图表。`+ 'static` 约束避免生命周期标注扩散到面板状态。
pub trait VizWidget: Widget + 'static {}

/// 为所有 ratatui 内置 widget 实现 `VizWidget`
///
/// 自动 blanket impl:任何 ratatui widget 均可作为 viz 组件返回
/// (避免对 5 个组件分别实现 trait)。
impl<T> VizWidget for T where T: Widget + 'static {}
