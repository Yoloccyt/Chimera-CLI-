//! 热力图 — 二维矩阵着色
//!
//! 对应 spec:`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6.2 `HeatmapData` 简化版
//!
//! # 设计决策(WHY)
//! - 字符渐进:`render::heat_bar` 风格延续,使用 `░/▒/▓/█` 4 档强度字符
//!   + 蓝/绿/黄/红 4 档颜色,值域 [0.0, 1.0] 自动归一化
//! - 简化 API:`&[f32]` + `rows` + `cols`,spec 中的 `HeatmapPalette` 与
//!   `HeatmapData` 结构体暂不引入,留给 MetricsDashboard 集成时扩展
//! - 维度不匹配优雅降级:`values.len() != rows*cols` 时按实际长度渲染,
//!   缺失位置留空白(便于调试数据提供方 bug)
//! - Cell 尺寸固定 2x1:每 cell 占 2 字符宽(避免水平拉伸),适合 8x8 / 16x16 网格

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

/// 构造热力图 widget — 二维矩阵着色
///
/// # 参数
/// - `values`:值序列(行优先,长度应为 `rows * cols`)
/// - `rows`:行数
/// - `cols`:列数
/// - `title`:图表标题
///
/// # 返回
/// 可直接 `Widget::render` 的 `HeatmapWidget`
///
/// # 边界处理
/// - `rows == 0` 或 `cols == 0`:返回空 widget(不 panic)
/// - `values.len() < rows*cols`:按实际长度渲染,缺失位置留空
/// - `values.len() > rows*cols`:截取前 `rows*cols` 个值
/// - 值范围 [-1.0, 1.0] / [0.0, 100.0]:自动归一化到 [0.0, 1.0]
pub fn heatmap(values: &[f32], rows: usize, cols: usize, title: &str) -> HeatmapWidget {
    // 提取有效值(防止越界访问)+ 归一化
    let max_cells = rows.saturating_mul(cols);
    let normalized: Vec<f32> = if max_cells == 0 {
        Vec::new()
    } else {
        let slice = if values.len() > max_cells {
            &values[..max_cells]
        } else {
            values
        };
        normalize_values(slice)
    };

    HeatmapWidget {
        rows,
        cols,
        values: normalized,
        title: title.to_string(),
    }
}

/// 将值归一化到 [0.0, 1.0]
///
/// WHY 独立函数:支持任意值域([0,1] / [-1,1] / [0,100]),统一归一化
/// 避免调用方在传入前手动归一化,降低 API 使用成本。
fn normalize_values(values: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
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
        // 所有值相同:全部映射到 0.5(中值)
        return vec![0.5; values.len()];
    }
    values.iter().map(|v| (v - lo) / (hi - lo)).collect()
}

/// 热力图 widget — 由 `heatmap()` 构造,实现 `ratatui::widgets::Widget`
///
/// WHY 自定义 struct 而非返回 `Paragraph`:热力图需要直接写入
/// `Buffer::cell_mut` 设置字符(每个 cell 占 2 字符宽,Paragraph
/// 难以精确控制宽度),自定义 widget 更直观。
#[derive(Debug, Clone)]
pub struct HeatmapWidget {
    rows: usize,
    cols: usize,
    /// 归一化后的值(长度 ≤ rows*cols)
    values: Vec<f32>,
    title: String,
}

impl HeatmapWidget {
    /// 将归一化值映射为 (字符, 颜色) 二元组
    ///
    /// 4 档编码:0-25% 蓝/浅, 25-50% 青/中, 50-75% 黄/深, 75-100% 红/深
    /// WHY 4 档:与 `render::heat_bar` 的 3 档互补,热力图需更细粒度
    /// 区分(8x8 网格共 64 cell,3 档每档约 21 cell 区分度不足)
    fn cell_style(normalized: f32) -> (char, Color) {
        if normalized < 0.25 {
            ('░', Color::Blue)
        } else if normalized < 0.5 {
            ('▒', Color::Cyan)
        } else if normalized < 0.75 {
            ('▓', Color::Yellow)
        } else {
            ('█', Color::Red)
        }
    }
}

impl Widget for HeatmapWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 3 {
            // 区域太小,无法显示边框+1 cell
            return;
        }

        // 1) 绘制 Block 边框 + 标题
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 2 || inner.height < 1 {
            return;
        }

        // 2) 计算可用 cell 网格:每 cell 2 字符宽 × 1 行高
        //    留 1 列 padding 防止右侧贴边
        let cell_width = 2u16;
        let available_cols = (inner.width / cell_width) as usize;
        let available_rows = inner.height as usize;
        let render_cols = self.cols.min(available_cols);
        let render_rows = self.rows.min(available_rows);

        // 3) 逐 cell 写入字符
        for r in 0..render_rows {
            for c in 0..render_cols {
                let idx = r * self.cols + c;
                if idx >= self.values.len() {
                    break; // 越界值(数据不足)留空
                }
                let (ch, color) = Self::cell_style(self.values[idx]);
                let x = inner.x + (c as u16) * cell_width;
                let y = inner.y + r as u16;
                if x + 1 < area.right() && y < area.bottom() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(ch).set_style(Style::default().fg(color));
                    }
                    if let Some(cell) = buf.cell_mut(Position::new(x + 1, y)) {
                        cell.set_char(ch).set_style(Style::default().fg(color));
                    }
                }
            }
        }
    }
}

/// 提供 `Paragraph` 包装(供需要文本 widget 的场景使用)
///
/// WHY 双形态:自定义 `HeatmapWidget` 适合直接绘制 cell,但部分
/// 场景(如 `CommandPalette` 预览)需要 `Paragraph` 风格,提供
/// 包装方法保留灵活性。
#[allow(dead_code)]
pub fn heatmap_as_paragraph(
    values: &[f32],
    rows: usize,
    cols: usize,
    title: &str,
) -> Paragraph<'static> {
    let widget = heatmap(values, rows, cols, title);
    // 构造一个包含标题与统计信息的简化 Paragraph(完整渲染走 `Widget::render`)
    let max_cells = rows.saturating_mul(cols);
    let stats = if values.is_empty() {
        "(no data)".to_string()
    } else {
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
        format!(
            "min={:.2} max={:.2} cells={}/{}",
            lo,
            hi,
            values.len().min(max_cells),
            max_cells
        )
    };
    let _ = widget; // 暂未在 paragraph 中使用,保留扩展位
    Paragraph::new(Line::from(vec![
        Span::styled(title.to_string(), Style::default().fg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(stats, Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL))
    .wrap(Wrap { trim: false })
}
