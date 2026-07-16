//! viz/ 可视化组件库测试 — line_chart/heatmap/bar_chart/gauge/histogram 行为验证
//!
//! 对应架构层:L10 Interface
//! 对应 spec:`.trae/specs/enterprise-tui-monitoring-task-viz/spec.md` Task 2.1
//!
//! # 测试策略(WHY)
//! - **RED 阶段**:所有 5 个测试在组件实现前应失败,验证契约清晰。
//! - **GREEN 阶段**:实现 `viz/` 模块,所有 5 个测试通过。
//! - **构造 + 渲染双验证**:验证 widget 构造不 panic 且实际渲染到 Buffer 后包含正确数据
//!   痕迹(非默认色 cell),确保组件不仅构造成功还能真正渲染。
//! - **风格统一**:沿用 `render_test.rs` 既有 `make_buffer` + `gauge_rendered_fg` 模式。

use chimera_tui::viz::gauge as viz_gauge;
use chimera_tui::viz::{bar_chart, heatmap, histogram, line_chart, VizChartKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

/// 构造一个最小终端 Buffer 与对应的 Rect 用于渲染验证。
///
/// 风格与 `render_test.rs::make_buffer` 一致,确保 viz 测试与既有测试在同一基线。
fn make_buffer(width: u16, height: u16) -> (Buffer, Rect) {
    let area = Rect::new(0, 0, width, height);
    (Buffer::empty(area), area)
}

/// 统计 Buffer 中非默认 cell 数量,用于断言"组件确实渲染了内容"。
///
/// WHY:ratatui 0.29 部分 widget 渲染后只修改 cell style 而不重置背景,
/// 统计非默认色 cell 可作为"渲染有内容"的代理指标(比精确字符匹配更稳健)。
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

// ============================================================
// 1. line_chart 折线图测试
// ============================================================

#[test]
fn test_line_chart_renders_time_series() {
    // 时间序列 5 个点:(0,10), (1,20), (2,15), (3,25), (4,30)
    let data: Vec<(f64, f64)> = vec![
        (0.0, 10.0),
        (1.0, 20.0),
        (2.0, 15.0),
        (3.0, 25.0),
        (4.0, 30.0),
    ];
    let widget = line_chart(&data, "Latency (ms)", Color::Cyan);

    // 1) 构造不应 panic
    let _ = widget;

    // 2) 实际渲染到 Buffer 并验证有非默认 cell(说明真的画了内容,不是空 widget)
    let (mut buf, area) = make_buffer(40, 10);
    let widget = line_chart(&data, "Latency (ms)", Color::Cyan);
    Widget::render(widget, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 40, 10);
    assert!(
        non_default > 0,
        "line_chart should render non-default cells, got {}",
        non_default
    );
}

#[test]
fn test_line_chart_handles_empty_data() {
    // 空数据:不应 panic,应优雅降级(返回的 widget 仍可渲染)
    let data: Vec<(f64, f64)> = vec![];
    let widget = line_chart(&data, "Empty", Color::Yellow);
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过(空数据下渲染区域可为空)
}

#[test]
fn test_line_chart_handles_single_point() {
    // 单点数据:应能正常渲染
    let data: Vec<(f64, f64)> = vec![(1.0, 42.0)];
    let widget = line_chart(&data, "Single", Color::Magenta);
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    let _ = buf; // 渲染不 panic 即通过
}

// ============================================================
// 2. heatmap 热力图测试
// ============================================================

#[test]
fn test_heatmap_renders_2d_grid() {
    // 3x3 网格,值 0.0 - 1.0 渐变
    let values: Vec<f32> = vec![
        0.1, 0.2, 0.3, // row 0
        0.4, 0.5, 0.6, // row 1
        0.7, 0.8, 0.9, // row 2
    ];
    let widget = heatmap(&values, 3, 3, "Heatmap");

    // 1) 构造不应 panic
    let _ = widget;

    // 2) 实际渲染到 Buffer 并验证有非默认 cell
    let (mut buf, area) = make_buffer(20, 10);
    let widget = heatmap(&values, 3, 3, "Heatmap");
    Widget::render(widget, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 20, 10);
    assert!(
        non_default > 0,
        "heatmap should render non-default cells, got {}",
        non_default
    );
}

#[test]
fn test_heatmap_dimensions_mismatch_does_not_panic() {
    // values.len() != rows*cols:应优雅降级不 panic
    let values: Vec<f32> = vec![0.1, 0.2, 0.3]; // 只有 3 个值,但声明 2x2=4
    let widget = heatmap(&values, 2, 2, "Mismatch");
    let (mut buf, area) = make_buffer(10, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

// ============================================================
// 3. bar_chart 水平条形图测试
// ============================================================

#[test]
fn test_bar_chart_renders_horizontal() {
    // 3 类:CPU 30, MEM 60, NET 90
    let data: Vec<(&str, f64)> = vec![("CPU", 30.0), ("MEM", 60.0), ("NET", 90.0)];
    let widget = bar_chart(&data, "Utilization", Color::Blue);

    // 1) 构造不应 panic
    let _ = widget;

    // 2) 实际渲染到 Buffer 并验证有非默认 cell
    let (mut buf, area) = make_buffer(30, 8);
    let widget = bar_chart(&data, "Utilization", Color::Blue);
    Widget::render(widget, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 30, 8);
    assert!(
        non_default > 0,
        "bar_chart should render non-default cells, got {}",
        non_default
    );
}

#[test]
fn test_bar_chart_handles_empty_data() {
    let data: Vec<(&str, f64)> = vec![];
    let widget = bar_chart(&data, "Empty", Color::Green);
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

#[test]
fn test_bar_chart_max_value_normalization() {
    // 验证 max=100% 时,99 几乎全填充,1 几乎空(不应 panic)
    let data: Vec<(&str, f64)> = vec![("A", 1.0), ("B", 50.0), ("C", 99.0)];
    let widget = bar_chart(&data, "Range", Color::Cyan);
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

// ============================================================
// 4. gauge 弧形 gauge 含阈值测试
// ============================================================

#[test]
fn test_gauge_renders_arc_with_threshold() {
    // value=75, max=100, threshold=70 (黄色警告区)
    let widget = viz_gauge(75.0, 100.0, 70.0, "Health");

    // 1) 构造不应 panic
    let _ = widget;

    // 2) 实际渲染到 Buffer 并验证有非默认 cell
    let (mut buf, area) = make_buffer(30, 5);
    let widget = viz_gauge(75.0, 100.0, 70.0, "Health");
    Widget::render(widget, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 30, 5);
    assert!(
        non_default > 0,
        "gauge should render non-default cells, got {}",
        non_default
    );
}

#[test]
fn test_gauge_zero_max_does_not_panic() {
    // max=0 边界:应优雅降级(ratio=0)
    let widget = viz_gauge(50.0, 0.0, 30.0, "ZeroMax");
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

#[test]
fn test_gauge_value_exceeds_max_clamps() {
    // value > max 边界:应钳位到 100%
    let widget = viz_gauge(150.0, 100.0, 90.0, "Overload");
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

// ============================================================
// 5. histogram 直方图测试
// ============================================================

#[test]
fn test_histogram_renders_buckets() {
    // 100 个均匀分布的值,10 个桶
    let values: Vec<f32> = (0..100).map(|i| (i as f32) / 100.0).collect();
    let widget = histogram(&values, 10, "Distribution");

    // 1) 构造不应 panic
    let _ = widget;

    // 2) 实际渲染到 Buffer 并验证有非默认 cell
    let (mut buf, area) = make_buffer(40, 10);
    let widget = histogram(&values, 10, "Distribution");
    Widget::render(widget, area, &mut buf);
    let non_default = count_non_default_cells(&buf, 40, 10);
    assert!(
        non_default > 0,
        "histogram should render non-default cells, got {}",
        non_default
    );
}

#[test]
fn test_histogram_handles_empty_data() {
    // 空数据:应优雅降级
    let values: Vec<f32> = vec![];
    let widget = histogram(&values, 5, "Empty");
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

#[test]
fn test_histogram_single_value_lands_in_bucket() {
    // 单值:应落入某一桶
    let values: Vec<f32> = vec![0.5];
    let widget = histogram(&values, 4, "Single");
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    let _ = buf; // 渲染不 panic 即通过
}

#[test]
fn test_histogram_constant_values_single_bucket() {
    // 所有值相同:应集中在一个桶
    let values: Vec<f32> = vec![0.5; 50];
    let widget = histogram(&values, 10, "Constant");
    let (mut buf, area) = make_buffer(20, 5);
    Widget::render(widget, area, &mut buf);
    // 不 panic 即通过
}

// ============================================================
// 6. VizChartKind 枚举测试
// ============================================================

#[test]
fn test_viz_chart_kind_variants_exist() {
    // 验证 5 个变体均可构造(编译期)
    let _ = VizChartKind::LineChart;
    let _ = VizChartKind::Heatmap;
    let _ = VizChartKind::BarChart;
    let _ = VizChartKind::Gauge;
    let _ = VizChartKind::Histogram;
}

#[test]
fn test_viz_chart_kind_distinct() {
    // 5 个变体应互不相等
    assert_ne!(VizChartKind::LineChart, VizChartKind::Heatmap);
    assert_ne!(VizChartKind::BarChart, VizChartKind::Gauge);
    assert_ne!(VizChartKind::Gauge, VizChartKind::Histogram);
    assert_ne!(VizChartKind::LineChart, VizChartKind::BarChart);
    assert_ne!(VizChartKind::Heatmap, VizChartKind::Histogram);
}
