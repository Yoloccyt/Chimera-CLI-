//! TUI Budget 面板 — 显示预算级别、消耗与利用率
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移原有渲染逻辑,保持进度条与超限高亮行为不变。
//! - 使用 `Panel` trait 统一接口,便于 `TuiApp` 通过 `Box<dyn Panel>` 管理。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// Budget 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BudgetPanel;

impl BudgetPanel {
    /// 创建新的 Budget 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Budget 面板文本内容
    pub fn content(state: &TuiState) -> Text<'static> {
        let budget = &state.budget;
        let total = budget.total_consumption + budget.remaining_budget;
        let utilization_pct = budget.utilization_rate * 100.0;

        // 基础信息行
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Budget"),
            Line::from("─────────────"),
            Line::from(format!("Current Tier: {}", budget.current_tier)),
            Line::from(format!("Coefficient:  {:.1}", budget.coefficient)),
            Line::from(format!(
                "Consumption:  {:.1} / {:.1}",
                budget.total_consumption, total
            )),
            Line::from(format!("Remaining:    {:.1}", budget.remaining_budget)),
            Line::from(format!("Utilization:  {:.1}%", utilization_pct)),
        ];

        // Status 行:超限时红色加粗,否则默认样式
        let status_text = if budget.is_exceeded { "EXCEEDED" } else { "OK" };
        let status_style = if budget.is_exceeded {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("Status:       {}", status_text),
            status_style,
        )));

        // 利用率进度条:宽度 30,已用 = Cyan,剩余 = Gray
        const BAR_WIDTH: usize = 30;
        let clamped_rate = budget.utilization_rate.clamp(0.0, 1.0);
        let used_chars = ((clamped_rate * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH);
        let remaining_chars = BAR_WIDTH - used_chars;
        let bar_label = format!("{:.1}%", utilization_pct);
        lines.push(Line::from(vec![
            Span::from("["),
            Span::styled("=".repeat(used_chars), Style::default().fg(Color::Cyan)),
            Span::styled(
                "-".repeat(remaining_chars),
                Style::default().fg(Color::Gray),
            ),
            Span::from(format!("] {}", bar_label)),
        ]));

        // Alert 行:存在时显示;超限时同样红色加粗
        if let Some(ref alert) = budget.alert {
            let alert_style = if budget.is_exceeded {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("Alert:        {}", alert),
                alert_style,
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(
            "Budget tier controls cost coefficient and thresholds.",
        ));

        Text::from(lines)
    }
}

impl Panel for BudgetPanel {
    fn id(&self) -> PanelId {
        PanelId::Budget
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Budget ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // M1:Budget 面板不处理专属按键。
        match key.code {
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::BudgetMetrics;

    #[test]
    fn test_budget_panel_id() {
        let panel = BudgetPanel::new();
        assert_eq!(panel.id(), PanelId::Budget);
    }

    #[test]
    fn test_budget_panel_default_state() {
        let state = TuiState::new();
        let content = BudgetPanel::content(&state).to_string();
        assert!(content.contains("Budget"));
        assert!(content.contains("High"));
        assert!(content.contains("OK"));
    }

    #[test]
    fn test_budget_panel_exceeded_state() {
        let mut state = TuiState::new();
        state.budget = BudgetMetrics {
            total_consumption: 9500.0,
            remaining_budget: 500.0,
            utilization_rate: 0.95,
            current_tier: "Critical".into(),
            coefficient: 1.2,
            is_exceeded: true,
            alert: Some("Budget cap exceeded".into()),
        };
        let content = BudgetPanel::content(&state).to_string();
        assert!(content.contains("EXCEEDED"));
        assert!(content.contains("Budget cap exceeded"));
        assert!(content.contains("Critical"));
    }
}
