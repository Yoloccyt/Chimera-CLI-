//! TUI Health 面板 — 显示事件速率、慢消费者、平均延迟与健康评分
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 数据驱动:从 `TuiState.health_metrics` 与 `event_rate_history` 读取。
//! - 健康评分使用 Gauge 直观展示 0-100 区间。
//! - 事件速率使用 Sparkline 展示近期趋势。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::{self, FOOTER_TEXT};
use crate::types::{PanelId, TuiCommand, TuiState};

/// Health 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct HealthPanel;

impl HealthPanel {
    /// 创建新的 Health 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建左侧信息文本
    fn info_text(state: &TuiState) -> Text<'static> {
        let hm = &state.health_metrics;
        let health_color = if hm.health_score >= 80 {
            Color::Green
        } else if hm.health_score >= 50 {
            Color::Yellow
        } else {
            Color::Red
        };

        let lines = vec![
            Line::from(vec![
                Span::styled(
                    "Events/sec: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1}", hm.events_per_second)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Slow Consumers: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(hm.slow_consumer_count.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    "Avg Latency: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1} ms", hm.average_latency_ms)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Health Score: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    hm.health_score.to_string(),
                    Style::default().fg(health_color),
                ),
            ]),
            Line::from(""),
            Line::from(FOOTER_TEXT),
        ];
        Text::from(lines)
    }
}

impl Panel for HealthPanel {
    fn id(&self) -> PanelId {
        PanelId::Health
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Health ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 20 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // 左:文字指标
        let info = Paragraph::new(Self::info_text(state));
        info.render(chunks[0], buf);

        // 右:健康评分 Gauge + 事件速率 Sparkline
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(5)])
            .split(chunks[1]);

        let score_color = if state.health_metrics.health_score >= 80 {
            Color::Green
        } else if state.health_metrics.health_score >= 50 {
            Color::Yellow
        } else {
            Color::Red
        };
        let gauge = render::gauge(
            state.health_metrics.health_score as f64,
            100.0,
            &format!("{} / 100", state.health_metrics.health_score),
            score_color,
        );
        gauge.render(right_chunks[0], buf);

        let sparkline =
            render::sparkline(&state.event_rate_history, "Event Rate History", Color::Cyan);
        sparkline.render(right_chunks[1], buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::HealthMetrics;
    use crate::types::TuiState;

    #[test]
    fn test_health_panel_id() {
        let panel = HealthPanel::new();
        assert_eq!(panel.id(), PanelId::Health);
    }

    #[test]
    fn test_health_panel_renders_metrics() {
        let mut state = TuiState::new();
        state.health_metrics = HealthMetrics {
            events_per_second: 42.0,
            slow_consumer_count: 1,
            average_latency_ms: 15.5,
            health_score: 90,
        };
        state.event_rate_history = vec![30, 35, 40, 38, 42, 45, 42];

        let content = HealthPanel::info_text(&state).to_string();
        assert!(content.contains("42.0"));
        assert!(content.contains("1"));
        assert!(content.contains("15.5 ms"));
        assert!(content.contains("90"));
    }

    #[test]
    fn test_health_panel_score_colors() {
        let mut state = TuiState::new();
        state.health_metrics.health_score = 30;
        let content = HealthPanel::info_text(&state).to_string();
        assert!(content.contains("30"));
    }
}
