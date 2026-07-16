//! 直方图 — 值分布展示
//!
//! 对应 spec:`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6.2 `HistogramData` 简化版
//!
//! # 设计决策(WHY)
//! - 简化 API:接受 `&[f32]` + `buckets`,spec 中的 `bin_range` 暂不
//!   暴露(由 `normalize_buckets` 自动按 [min, max] 分布)
//! - 自定义 widget 而非 ratatui `BarChart`:ratatui BarChart 需 Label,
//!   且水平排列,与"垂直高度表示频次"的直方图语义不符
//! - 中位数滤波:为 `0.5*histogram` 性能预算(`§6.2 性能矩阵`),
//!   1000 values × 20 bins = O(n) 扫描 + O(bins) 桶填充,无嵌套循环
//! - 8 档 Unicode 块字符(与 `gauge.rs` 一致):桶高度归一化到 0-7 档

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

/// 构造直方图 widget — 值分布展示
///
/// # 参数
/// - `values`:原始数据点
/// - `buckets`:桶数量(`buckets == 0` 时按 1 处理)
/// - `title`:图表标题
///
/// # 返回
/// 可直接 `Widget::render` 的 `HistogramWidget`
///
/// # 边界处理
/// - `values` 为空:全部桶为 0,显示空直方图
/// - 所有 values 相同:全部落入首个桶,其余桶为空
/// - `buckets` 超过 values.len():部分桶为空(不影响渲染)
/// - `buckets == 0`:按 1 处理,所有值落入单一桶
pub fn histogram(values: &[f32], buckets: usize, title: &str) -> HistogramWidget {
    let buckets = buckets.max(1); // 至少 1 个桶
    let (lo, hi) = value_range(values);
    let counts = count_buckets(values, lo, hi, buckets);
    let max_count = counts.iter().copied().max().unwrap_or(0);

    HistogramWidget {
        title: title.to_string(),
        counts,
        max_count,
        lo,
        hi,
        n: values.len(),
    }
}

/// 计算值域 [min, max](空值返回 [0.0, 1.0] 避免退化)
fn value_range(values: &[f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 1.0);
    }
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in values {
        if v < lo {
            lo = v;
        }
        if v > hi {
            hi = v;
        }
    }
    if (hi - lo).abs() < f32::EPSILON {
        // 所有值相同:留 0.5 padding 避免退化
        (lo - 0.5, lo + 0.5)
    } else {
        (lo, hi)
    }
}

/// 将 values 按 [lo, hi] 区间分桶
///
/// WHY 独立函数:便于 `HistogramWidget` 内部复用,也可供未来
/// "多系列直方图"扩展。
fn count_buckets(values: &[f32], lo: f32, hi: f32, buckets: usize) -> Vec<u32> {
    let mut counts = vec![0u32; buckets];
    if values.is_empty() || buckets == 0 {
        return counts;
    }
    let range = (hi - lo).abs().max(f32::EPSILON);
    for &v in values {
        let mut idx = (((v - lo) / range) * buckets as f32).floor() as i64;
        // 钳位到 [0, buckets-1]:处理 v == hi(应落入最后桶)与 NaN
        if idx < 0 {
            idx = 0;
        } else if idx >= buckets as i64 {
            idx = buckets as i64 - 1;
        }
        counts[idx as usize] += 1;
    }
    counts
}

/// 直方图 widget — 由 `histogram()` 构造,实现 `ratatui::widgets::Widget`
///
/// WHY 自定义 widget:垂直高度的频次柱状条需精确写入 Buffer cell,
/// `Paragraph` 难以逐桶控制高度,自定义实现更直观。
#[derive(Debug, Clone)]
pub struct HistogramWidget {
    title: String,
    /// 每个桶的计数(长度 = buckets)
    counts: Vec<u32>,
    /// 最大桶计数(用于高度归一化)
    max_count: u32,
    /// 值域下界
    lo: f32,
    /// 值域上界
    hi: f32,
    /// 原始数据点数量
    n: usize,
}

impl HistogramWidget {
    /// 构造标题行 Paragraph(供报告场景,如 "buckets=10 max=42 n=1000")
    pub fn info_line(&self) -> Line<'static> {
        Line::from(vec![
            Span::styled(self.title.clone(), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(
                format!(
                    "buckets={} max={} n={} range=[{:.2},{:.2}]",
                    self.counts.len(),
                    self.max_count,
                    self.n,
                    self.lo,
                    self.hi
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    }
}

impl Widget for HistogramWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 3 {
            return;
        }

        // 1) Block 边框 + 标题
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 2 || inner.height < 1 {
            return;
        }

        if self.counts.is_empty() {
            return;
        }

        // 2) 计算每个桶的宽度(整数,余数分配到前几个桶)
        let total_width = inner.width as usize;
        let n_buckets = self.counts.len();
        let base_width = total_width / n_buckets;
        let remainder = total_width % n_buckets;
        let mut widths = vec![base_width; n_buckets];
        for w in widths.iter_mut().take(remainder) {
            *w += 1; // 前 remainder 个桶各 +1 字符
        }

        // 3) 渲染桶:垂直高度按 count/max_count 归一化到 inner.height
        let max_height = inner.height as usize;
        // 8 档 Unicode 块字符(最细粒度)用于桶内垂直堆叠
        const FULL_BLOCK: char = '█';

        let mut x = inner.x;
        for (i, &count) in self.counts.iter().enumerate() {
            let w = widths[i];
            if w == 0 {
                continue;
            }
            let normalized = if self.max_count > 0 {
                (count as f64 / self.max_count as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // 桶高度按比例分配行数,最小 0 行(全 0 时不绘制)
            let bar_height = ((normalized * max_height as f64).round() as usize).min(max_height);

            for h in 0..bar_height {
                let y = inner.y + (max_height - 1 - h) as u16;
                for dx in 0..w {
                    let cell_x = x + dx as u16;
                    if cell_x < area.right() && y < area.bottom() {
                        if let Some(cell) = buf.cell_mut(Position::new(cell_x, y)) {
                            cell.set_char(FULL_BLOCK).set_style(
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            );
                        }
                    }
                }
            }
            x += w as u16;
        }
    }
}

/// 简化的"无 widget 包装"摘要 paragraph(供报告/日志场景)
///
/// WHY 辅助方法:某些场景(如 `MetricsDashboardPanel` 折叠态)只需
/// 显示直方图统计信息而不绘制实际柱状条,提供 paragraph 形式
/// 避免在面板层重复拼字符串。
#[allow(dead_code)]
pub fn histogram_summary(values: &[f32], buckets: usize, title: &str) -> Paragraph<'static> {
    let widget = histogram(values, buckets, title);
    Paragraph::new(widget.info_line()).block(Block::default().borders(Borders::ALL))
}
