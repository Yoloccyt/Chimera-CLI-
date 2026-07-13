//! TUI Parliament 面板 — 显示议会相关事件(Skeptic 否决、红队审计、ASA 干预、投票、共识)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变,同时修复 `unreachable!()` 的安全隐患。
//! - 对未识别事件使用安全回退(skip)而非 panic,符合 §4 编码红线。

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

/// Parliament 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ParliamentPanel;

impl ParliamentPanel {
    /// 创建新的 Parliament 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Parliament 面板文本内容
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Parliament"), Line::from("─────────────")];

        let parliament_events: Vec<&NexusEvent> = state
            .latest_events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    NexusEvent::VoteCast { .. }
                        | NexusEvent::ConsensusReached { .. }
                        | NexusEvent::SkepticVeto { .. }
                        | NexusEvent::RedTeamAudit { .. }
                        | NexusEvent::AsaIntervention { .. }
                )
            })
            .collect();

        if parliament_events.is_empty() {
            lines.push(Line::from("No recent parliament events"));
        } else {
            for event in parliament_events.iter().rev().take(10) {
                let (label, summary, style) = match event {
                    NexusEvent::SkepticVeto {
                        quest_id,
                        veto_reason,
                        ..
                    } => (
                        "SkepticVeto",
                        format!("{} | {}", quest_id, veto_reason),
                        Style::default().fg(Color::Red),
                    ),
                    NexusEvent::AsaIntervention {
                        operation_id,
                        action,
                        block_reason,
                        ..
                    } => {
                        let detail = block_reason
                            .as_deref()
                            .filter(|r| !r.is_empty())
                            .unwrap_or(action);
                        (
                            "AsaIntervention",
                            format!("{} | {}", operation_id, detail),
                            Style::default().fg(Color::Yellow),
                        )
                    }
                    NexusEvent::RedTeamAudit {
                        vulnerability_type,
                        detection_rate,
                        remediation_suggestion,
                        ..
                    } => (
                        "RedTeamAudit",
                        format!(
                            "{} | risk={:.0}% | {}",
                            vulnerability_type,
                            detection_rate * 100.0,
                            remediation_suggestion
                        ),
                        Style::default().fg(Color::LightYellow),
                    ),
                    NexusEvent::ConsensusReached {
                        quest_id,
                        decision_hash,
                        ..
                    } => (
                        "ParliamentConsensusReached",
                        format!("{} | {}", quest_id, decision_hash),
                        Style::default().fg(Color::Green),
                    ),
                    NexusEvent::VoteCast {
                        proposal_id,
                        voter,
                        vote,
                        ..
                    } => (
                        "ParliamentVoteCast",
                        format!(
                            "{} | {}: {}",
                            proposal_id,
                            voter,
                            if *vote { "FOR" } else { "AGAINST" }
                        ),
                        Style::default(),
                    ),
                    // 安全回退:过滤条件之外的 Parliament 事件直接跳过,
                    // 避免 `unreachable!()` 在新增事件变体时 panic。
                    _ => continue,
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("[{}] ", label), style),
                    Span::raw(summary),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }
}

impl Panel for ParliamentPanel {
    fn id(&self) -> PanelId {
        PanelId::Parliament
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Parliament ")
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
    fn test_parliament_panel_id() {
        let panel = ParliamentPanel::new();
        assert_eq!(panel.id(), PanelId::Parliament);
    }

    #[test]
    fn test_parliament_panel_empty_state() {
        let state = TuiState::new();
        let content = ParliamentPanel::content(&state).to_string();
        assert!(content.contains("Parliament"));
        assert!(content.contains("No recent parliament events"));
    }

    #[test]
    fn test_parliament_panel_no_panic_on_unknown_event() {
        // 即使过滤条件意外包含未处理变体,也不应 panic。
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("test"),
                cache_key: "k1".into(),
            },
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p1".into(),
                voter: "alice".into(),
                vote: true,
            },
        ]);
        let content = ParliamentPanel::content(&state).to_string();
        assert!(content.contains("ParliamentVoteCast"));
        assert!(!content.contains("CacheHit"));
    }
}
