//! TUI PVL 过程评分面板 — 九维度过程评分可视化（Task 3.7:L10 → L7 向下依赖）
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:PvlScore
//! 对应创新点:PVL(Producer-Verifier Loop,九维度过程评分,快手 KAT,ADR-049)
//!
//! # 核心职责
//! - 调用 `pvl_layer::pvl_score()` 获取九维度过程评分静态快照
//! - 九维度纵向布局:每维度显示名称 + 数值 + 颜色编码进度条
//! - 底部显示总分（Total Score）
//!
//! # 设计决策(WHY)
//! - **静态快照模式**:`pvl_score()` 返回 `ProcessScore` 值类型,无需异步上下文,
//!   面板渲染不阻塞 TUI 事件循环。TODO: v3.x 接入 RuntimeAuditor 实时采集。
//! - **颜色编码**:≥0.8 绿色(优秀)/ 0.5-0.8 黄色(一般)/ <0.5 红色(需关注),
//!   与 OsaSparsePanel 的颜色策略一致。
//! - **九维度标签映射**:real_execution→真实执行 / coverage→覆盖率 / verification→验证通过
//!   / confidence→置信度 / efficiency→效率 / retry_discipline→重试纪律
//!   / output_substance→产出实质性 / orphan_free→零孤儿 / sandbox_clean→沙箱清洁

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};
use pvl_layer::pvl_score;

/// 九维度标签（按 ProcessScore 字段顺序）
const DIMENSION_LABELS: [(&str, &str); 9] = [
    ("real_execution", "真实执行"),
    ("coverage", "覆盖率"),
    ("verification", "验证通过"),
    ("confidence", "置信度"),
    ("efficiency", "效率"),
    ("retry_discipline", "重试纪律"),
    ("output_substance", "产出实质性"),
    ("orphan_free", "零孤儿"),
    ("sandbox_clean", "沙箱清洁"),
];

/// PVL 过程评分面板
///
/// 展示 PVL 九维度过程评分（快手 KAT,ADR-049）。
/// 数据来源：`pvl_layer::pvl_score()`。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PvlScorePanel {
    /// 当前选中维度索引（0-8,键盘导航）
    selected: usize,
}

impl PvlScorePanel {
    /// 创建新的 PVL 过程评分面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中索引（测试用）
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 根据评分值返回对应的颜色编码
    ///
    /// WHY 独立函数:颜色映射与渲染解耦,便于未来调整阈值。
    fn score_color(value: f32) -> Color {
        if value >= 0.8 {
            Color::Green
        } else if value >= 0.5 {
            Color::Yellow
        } else {
            Color::Red
        }
    }

    /// 获取当前评分快照（每帧 render 时调用,保证数据新鲜度）
    fn current_score() -> pvl_layer::ProcessScore {
        pvl_score()
    }
}

impl Panel for PvlScorePanel {
    fn id(&self) -> PanelId {
        PanelId::PvlScore
    }

    fn title(&self) -> Line<'static> {
        Line::from(PanelId::PvlScore.title()).style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        let score = Self::current_score();
        let inner = Block::default()
            .borders(Borders::ALL)
            .title(Self::title(self))
            .border_style(Style::default().fg(Color::Magenta));

        // 最小终端高度检查:标题(1) + 9 维度(各 2 行) + 总分(2) + 边框(2) = 23
        if area.height < 15 {
            let text = Text::from("Terminal too small for PVL Score panel (min 15 rows)");
            let p = Paragraph::new(text).block(inner);
            Widget::render(p, area, buf);
            return;
        }

        let inner_area = inner.inner(area);
        Widget::render(inner, area, buf);

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(20);

        // 九维度评分（每维度:名称 + 进度条 + 数值）
        let dimension_values: [f32; 9] = [
            score.real_execution,
            score.coverage,
            score.verification,
            score.confidence,
            score.efficiency,
            score.retry_discipline,
            score.output_substance,
            score.orphan_free,
            score.sandbox_clean,
        ];

        for (idx, (&(en_label, zh_label), &value)) in DIMENSION_LABELS
            .iter()
            .zip(dimension_values.iter())
            .enumerate()
        {
            let color = Self::score_color(value);
            let is_selected = idx == self.selected;
            let marker = if is_selected { "▶ " } else { "  " };
            let gauge_width = if inner_area.width > 30 {
                (inner_area.width as usize).saturating_sub(30)
            } else {
                10
            };
            let filled = (value * gauge_width as f32) as usize;
            let empty = gauge_width.saturating_sub(filled);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

            let mut style = Style::default().fg(color);
            if is_selected {
                style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }

            lines.push(Line::from(vec![Span::styled(
                format!("{marker}{zh_label} ({en_label})",),
                style,
            )]));
            lines.push(Line::from(vec![
                Span::styled(format!("  [{bar}]"), style),
                Span::styled(format!(" {:.1}%", value * 100.0), style),
            ]));
        }

        // 总分
        lines.push(Line::from(""));
        let total_color = Self::score_color(score.total);
        lines.push(Line::from(vec![
            Span::styled(
                "  TOTAL SCORE: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:.1}%", score.total * 100.0),
                Style::default()
                    .fg(total_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        let text = Text::from(lines);
        let p = Paragraph::new(text);
        Widget::render(p, inner_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.selected < 8 => {
                self.selected += 1;
            }
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("↑/↓", "选择维度"), ("j/k", "导航")]
    }
}
