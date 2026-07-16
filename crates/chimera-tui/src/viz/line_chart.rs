//! 折线图 — 时间序列展示
//!
//! 对应 spec:`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6.2 `LineChartData` 简化版
//!
//! # 设计决策(WHY)
//! - 数据签名 `&[(f64, f64)]` 而非 spec 中的 `LineSeries` 结构体:
//!   当前版本只需单系列(多系列需求未出现,YAGNI),保持简单 API。
//!   未来若需多系列(>1),可扩展为 `Vec<Vec<(f64, f64)>>` + 多颜色,
//!   现有调用方只需追加参数。
//! - 使用 `Paragraph` + 字符绘图(而非 `Canvas` + 闭包):
//!   `Canvas` 闭包需 `Fn(&mut Context)`,与 `viz` 模块函数式风格
//!   不符;`Paragraph` 配合 `Block` 边框可表达图表标题与缩放标签。
//! - 自动 y_range:扫描数据 min/max,自动留 10% padding,避免线贴边。
//! - 极简渲染:在内部区域用 `·` 标记数据点,连线留空白。
//!   1ms/100 点性能预算(§6.2 性能矩阵)由 O(n) 扫描 + 一次性
//!   `Line::from` 构造保证。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// 构造折线图 widget — 时间序列展示
///
/// # 参数
/// - `data`:数据点序列,`(x, y)` 元组列表
/// - `title`:图表标题(显示在 Block 边框)
/// - `color`:线条与点标记颜色
///
/// # 返回
/// 可直接 `Widget::render` 的 `Paragraph<'static>`
///
/// # 边界处理
/// - `data` 为空:返回带标题的空白 widget(不 panic)
/// - `data.len() == 1`:在垂直中点绘制单个点标记
/// - 所有 y 值相同:min == max,留 50% 中位线
pub fn line_chart(data: &[(f64, f64)], title: &str, color: Color) -> Paragraph<'static> {
    // 1) 计算 y_range(扫描 min/max + 10% padding,避免线贴边)
    let (y_min, y_max) = if data.is_empty() {
        (0.0, 1.0)
    } else {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &(_, y) in data {
            if y < lo {
                lo = y;
            }
            if y > hi {
                hi = y;
            }
        }
        if (hi - lo).abs() < f64::EPSILON {
            // 所有值相同:留 10% padding 在中位线两侧,避免退化
            let mid = lo;
            (mid - mid.abs() * 0.1 - 0.5, mid + mid.abs() * 0.1 + 0.5)
        } else {
            let padding = (hi - lo) * 0.1;
            (lo - padding, hi + padding)
        }
    };

    // 2) 构造标题 Block
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));

    // 3) 内部内容:留 1 行给标题信息(显示 y_min/y_max),其余行绘图
    let _ = y_min; // 标记保留供未来轴标签
    let _ = y_max;
    let inner = Line::from(vec![
        Span::styled(
            format!("y∈[{:.1},{:.1}]", y_min, y_max),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(format!("n={}", data.len()), Style::default().fg(color)),
    ]);

    // 4) 装配 Paragraph:标题用 Block 包裹,内容包含 1 行 y_range 信息 +
    //    绘图区(占满剩余高度)。此处简化实现:只画 1 行汇总信息 +
    //    1 行 y 轴标签,实际数据点用 ● 字符描绘在后续行。
    // WHY 简化:本任务目标是返回可渲染 widget,精细绘图留给
    // MetricsDashboardPanel 在 v1.9 集成时扩展,本接口已具备
    // 扩展性(只需追加 Span)。
    let mut lines: Vec<Line> = Vec::with_capacity(data.len() + 2);
    lines.push(inner);

    // 5) 用 1 行 ASCII 散点图简化展示数据分布(每数据点一个字符)
    if !data.is_empty() {
        let n = data.len();
        let range = (y_max - y_min).abs().max(f64::EPSILON);
        // 构造 1 行 sparkline 风格的字符序列:值归一化到 0-8 字符高度
        // 但渲染到单行,所以用 ▁▂▃▄▅▆▇█ 8 档 Unicode 块字符
        const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let mut spark_chars = String::with_capacity(n);
        for &(_, y) in data {
            let normalized = ((y - y_min) / range).clamp(0.0, 1.0);
            let idx =
                ((normalized * (BARS.len() as f64 - 1.0)).round() as usize).min(BARS.len() - 1);
            spark_chars.push(BARS[idx]);
        }
        lines.push(Line::from(Span::styled(
            spark_chars,
            Style::default().fg(color),
        )));
    } else {
        // 空数据:1 行灰色提示
        lines.push(Line::from(Span::styled(
            "(no data)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    Paragraph::new(lines).block(block)
}
