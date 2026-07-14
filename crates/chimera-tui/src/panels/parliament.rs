//! TUI Parliament 面板 — 显示议会相关事件(Skeptic 否决、红队审计、ASA 干预、投票、共识)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变,同时修复 `unreachable!()` 的安全隐患。
//! - 对未识别事件使用安全回退(skip)而非 panic,符合 §4 编码红线。
//! - M3 增加滚动选择与详情弹窗。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::FOOTER_TEXT;
use crate::types::{PanelId, TuiCommand, TuiState};
use event_bus::NexusEvent;

/// Parliament 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParliamentPanel {
    /// 当前选中事件的索引
    selected: usize,
    /// 事件列表的滚动偏移
    scroll_offset: usize,
}

impl ParliamentPanel {
    /// 创建新的 Parliament 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回 Parliament 相关事件列表(最新在前)
    fn parliament_events(state: &TuiState) -> Vec<&NexusEvent> {
        state
            .latest_events
            .iter()
            .rev()
            .filter(|e| {
                matches!(
                    e,
                    NexusEvent::VoteCast { .. }
                        | NexusEvent::ConsensusReached { .. }
                        | NexusEvent::SkepticVeto { .. }
                        | NexusEvent::RedTeamAudit { .. }
                        | NexusEvent::AsaIntervention { .. }
                        | NexusEvent::VetoOverridden { .. }
                        | NexusEvent::DebateStarted { .. }
                        | NexusEvent::AhirtProbeCompleted { .. }
                )
            })
            .collect()
    }

    /// 构建 Parliament 面板文本内容
    fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Parliament"), Line::from("─────────────")];

        let parliament_events = Self::parliament_events(state);

        if parliament_events.is_empty() {
            lines.push(Line::from("No recent parliament events"));
        } else {
            for (idx, event) in parliament_events.iter().enumerate().take(50) {
                let is_selected = idx == selected;
                let prefix = if is_selected { "> " } else { "  " };
                let (label, summary, style) = match event {
                    NexusEvent::SkepticVeto {
                        quest_id,
                        veto_reason,
                        ..
                    } => (
                        "SkepticVeto",
                        format!("{} | {}", quest_id, veto_reason),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                    NexusEvent::VetoOverridden {
                        quest_id,
                        proposal_id,
                        override_reason,
                        ..
                    } => (
                        "VetoOverridden",
                        format!("{} | {} | {}", quest_id, proposal_id, override_reason),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Red)
                        },
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
                            if is_selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Yellow)
                            },
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
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::LightYellow)
                        },
                    ),
                    NexusEvent::ConsensusReached {
                        quest_id,
                        decision_hash,
                        ..
                    } => (
                        "ParliamentConsensusReached",
                        format!("{} | {}", quest_id, decision_hash),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Green)
                        },
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
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    NexusEvent::DebateStarted {
                        quest_id,
                        proposal_id,
                        ..
                    } => (
                        "DebateStarted",
                        format!("{} | {}", quest_id, proposal_id),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Cyan)
                        },
                    ),
                    NexusEvent::AhirtProbeCompleted {
                        probe_type,
                        total,
                        failed,
                        ..
                    } => (
                        "AhirtProbeCompleted",
                        format!("{} | failed={}/{}", probe_type, failed, total),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Yellow)
                        },
                    ),
                    // 安全回退:过滤条件之外的 Parliament 事件直接跳过,
                    // 避免 `unreachable!()` 在新增事件变体时 panic。
                    _ => continue,
                };

                lines.push(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(format!("[{}] ", label), style),
                    Span::styled(summary, style),
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
        let events = Self::parliament_events(state);
        self.selected = list_state::clamp_selected(self.selected, events.len());

        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(Self::content(state, self.selected))
            .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::parliament_events(state).len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            KeyCode::Enter => {
                let events = Self::parliament_events(state);
                events
                    .get(self.selected)
                    .map(|event| TuiCommand::OpenPopup(PopupKind::event_detail(event)))
            }
            KeyCode::Char('g') => {
                self.scroll_to_top(state);
                None
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom(state);
                None
            }
            // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let count = Self::parliament_events(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::parliament_events(state).len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
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
        let content = ParliamentPanel::content(&state, 0).to_string();
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
        let content = ParliamentPanel::content(&state, 0).to_string();
        assert!(content.contains("ParliamentVoteCast"));
        assert!(!content.contains("CacheHit"));
    }

    #[test]
    fn test_parliament_panel_navigation() {
        let mut panel = ParliamentPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p1".into(),
                voter: "alice".into(),
                vote: true,
            },
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p2".into(),
                voter: "bob".into(),
                vote: false,
            },
        ]);

        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 1);

        panel.handle_key(
            KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 0);
    }

    #[test]
    fn test_parliament_panel_detail_popup() {
        let mut panel = ParliamentPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: "p1".into(),
            voter: "alice".into(),
            vote: true,
        }]);

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::EventDetail {
                title,
                event_type,
                payload_decoded,
                related_event_ids,
                ..
            })) => {
                assert!(title.contains("VoteCast"));
                assert_eq!(event_type, "VoteCast");
                assert!(payload_decoded.contains("alice"));
                assert!(related_event_ids.contains(&"p1".to_string()));
            }
            _ => panic!("expected EventDetail popup command, got {:?}", cmd),
        }
    }
}
