//! 条形图 — 类别对比(水平)
//!
//! 对应 spec:`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6.2 `BarChartData` 简化版
//!
//! # 设计决策(WHY)
//! - 与 `render::horizontal_bar_chart` 风格一致(单行柱状条),不重复造轮子
//! - 简化 API:接受 `&[(label, value)]`,内部归一化到 max
//!   避免调用方预先归一化(对比 horizontal_bar_chart 需传入 labels + values)
//! - 字符:`█` 实心块表示填充部分,`░` 浅色块表示空部分,
//!   比 `horizontal_bar_chart` 的 `█` + 纯空白提供更好的"进度感"
//! - 标签右对齐:统一右对齐标签使柱状条起点对齐,视觉更整齐

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

/// 构造水平条形图 widget — 类别对比
///
/// # 参数
/// - `data`:类别数据,`(label, value)` 元组列表
/// - `title`:图表标题
/// - `color`:柱状条颜色
///
/// # 返回
/// 可直接 `Widget::render` 的 `BarChartWidget`
///
/// # 边界处理
/// - `data` 为空:返回带标题的空白 widget(不 panic)
/// - 所有 value 为 0:全部柱状条留 1 个 `░`(避免空条,与 `render::heat_bar` 一致)
/// - `value` 为负数:截断为 0(条形图不支持负值;负值语义留给未来 axis 扩展)
pub fn bar_chart(data: &[(&str, f64)], title: &str, color: Color) -> BarChartWidget {
    BarChartWidget {
        data: data.iter().map(|(l, v)| (l.to_string(), *v)).collect(),
        title: title.to_string(),
        color,
    }
}

/// 条形图 widget — 由 `bar_chart()` 构造,实现 `ratatui::widgets::Widget`
///
/// WHY 自定义 widget:需精确控制每行布局(标签 + 柱状条 + 数值),
/// `Paragraph` 难以水平对齐柱状条起点(标签宽度不一时)。
#[derive(Debug, Clone)]
pub struct BarChartWidget {
    /// (label, value) 元组列表
    data: Vec<(String, f64)>,
    title: String,
    color: Color,
}

impl Widget for BarChartWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 5 || area.height < 3 {
            return; // 区域太小
        }

        // 1) 绘制 Block 边框 + 标题
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 5 || inner.height < 1 {
            return;
        }

        if self.data.is_empty() {
            // 1 行灰色提示
            if let Some(cell) = buf.cell_mut(Position::new(inner.x, inner.y)) {
                cell.set_char('(')
                    .set_style(Style::default().fg(Color::DarkGray));
            }
            return;
        }

        // 2) 计算 max_value(忽略负数,全部按 0 处理)
        let max_value = self
            .data
            .iter()
            .map(|(_, v)| v.max(0.0))
            .fold(0.0_f64, f64::max);

        // 3) 计算统一标签宽度(右对齐填充)
        let label_width = self
            .data
            .iter()
            .map(|(l, _)| l.chars().count())
            .max()
            .unwrap_or(0)
            .min((inner.width / 2) as usize);

        // 4) 每行: label | bar | value
        //    - label:右对齐 label_width 字符
        //    - bar:占满剩余宽度,按 value/max_value 比例填充
        //    - value:右对齐 6 字符(包含单位)
        let value_col_width = 6usize;
        let bar_max_width = (inner.width as usize)
            .saturating_sub(label_width)
            .saturating_sub(value_col_width)
            .saturating_sub(2); // 留 2 字符 padding

        for (i, (label, value)) in self.data.iter().enumerate() {
            if i >= inner.height as usize {
                break;
            }
            let y = inner.y + i as u16;

            // 3.1) 渲染标签(右对齐)
            let label_chars: Vec<char> = label.chars().collect();
            let label_start_x = inner.x + (label_width - label_chars.len().min(label_width)) as u16;
            for (j, ch) in label_chars.iter().enumerate() {
                let x = label_start_x + j as u16;
                if x < inner.x + label_width as u16 && x < area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(*ch)
                            .set_style(Style::default().fg(self.color));
                    }
                }
            }

            // 3.2) 渲染柱状条
            let bar_x_start = inner.x + label_width as u16 + 1;
            let bar_y = y;
            let ratio = if max_value > 0.0 {
                (value.max(0.0) / max_value).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let filled_width = ((ratio * bar_max_width as f64).round() as usize)
                .max(if !self.data.is_empty() { 1 } else { 0 })
                .min(bar_max_width);

            for j in 0..filled_width {
                let x = bar_x_start + j as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, bar_y)) {
                        cell.set_char('█').set_style(
                            Style::default().fg(self.color).add_modifier(Modifier::BOLD),
                        );
                    }
                }
            }
            // 空余部分用浅色
            for j in filled_width..bar_max_width {
                let x = bar_x_start + j as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, bar_y)) {
                        cell.set_char('░')
                            .set_style(Style::default().fg(Color::DarkGray));
                    }
                }
            }

            // 3.3) 渲染数值(右对齐)
            let value_str = format!("{:>width$.1}", value, width = value_col_width);
            let value_x_start = inner.x + inner.width - value_col_width as u16;
            for (j, ch) in value_str.chars().enumerate() {
                let x = value_x_start + j as u16;
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(ch)
                            .set_style(Style::default().fg(Color::White));
                    }
                }
            }
        }
    }
}
