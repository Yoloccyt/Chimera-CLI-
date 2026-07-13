//! TUI Log 面板 — 显示系统事件流
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变。
//! - 关键事件(Critical severity)使用红色高亮。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::FOOTER_TEXT;
use crate::types::{PanelId, TuiCommand, TuiState};
use event_bus::NexusEvent;

/// Log 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct LogPanel;

impl LogPanel {
    /// 创建新的 Log 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Log 面板文本内容
    fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("System Log"), Line::from("─────────────")];

        if state.latest_events.is_empty() {
            lines.push(Line::from("[INFO]  System initialized"));
            lines.push(Line::from("[DEBUG] Event bus subscribed"));
            lines.push(Line::from("[WARN]  No critical events"));
            lines.push(Line::from("[ERROR] (none)"));
        } else {
            for event in state.latest_events.iter().rev().take(10) {
                let metadata = event.metadata();
                let ts = metadata.timestamp.format("%H:%M:%S").to_string();
                let source = &metadata.source;
                let event_type = event.type_name();

                let is_critical = matches!(
                    event,
                    NexusEvent::SkepticVeto { .. }
                        | NexusEvent::RedTeamAudit { .. }
                        | NexusEvent::AsaIntervention { .. }
                        | NexusEvent::BudgetExceeded { .. }
                );
                let style = if is_critical {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default()
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("[{}] ", ts), style),
                    Span::styled(format!("[{}] ", source), style),
                    Span::styled(event_type.to_string(), style),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }
}

impl Panel for LogPanel {
    fn id(&self) -> PanelId {
        PanelId::Log
    }

    fn title(&self) -> Line<'static> {
        Line::from(" System Log ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
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
    use event_bus::{EventMetadata, NexusEvent};
    use std::collections::VecDeque;

    #[test]
    fn test_log_panel_id() {
        let panel = LogPanel::new();
        assert_eq!(panel.id(), PanelId::Log);
    }

    #[test]
    fn test_log_panel_empty_state() {
        let state = TuiState::new();
        let content = LogPanel::content(&state).to_string();
        assert!(content.contains("System Log"));
        assert!(content.contains("System initialized"));
    }

    #[test]
    fn test_log_panel_renders_events() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("decb-governor"),
                budget_type: "token".into(),
                current: 9500,
                limit: 10000,
            },
        ]);
        let content = LogPanel::content(&state).to_string();
        assert!(content.contains("CacheHit"));
        assert!(content.contains("BudgetExceeded"));
    }
}
