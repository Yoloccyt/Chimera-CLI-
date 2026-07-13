//! TUI Security 面板 — 显示 Skeptic 否决、红队审计、ASA 干预与冻结能力
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 数据驱动:从 `TuiState.security_state` 读取安全事件摘要。
//! - 键盘导航:Up/Down 选择事件,Enter 打开 Detail 弹窗展示完整内容。
//! - 严重级别着色:critical=red,warning=yellow,普通默认。

use chrono::SecondsFormat;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::FOOTER_TEXT;
use crate::types::{PanelId, TuiCommand, TuiState};

/// Security 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SecurityPanel {
    /// 当前选中事件的索引
    selected: usize,
}

impl SecurityPanel {
    /// 创建新的 Security 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前可选中的事件总数
    fn event_count(state: &TuiState) -> usize {
        state.security_state.active_vetoes.len()
            + state.security_state.recent_audits.len()
            + state.security_state.recent_interventions.len()
    }

    /// 将当前选中索引限制在有效范围内
    fn clamp_selected(&mut self, state: &TuiState) {
        let count = Self::event_count(state);
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    /// 构建所有安全事件行的统一视图
    fn build_rows(state: &TuiState, selected: usize) -> (Vec<Line<'static>>, usize) {
        let mut rows = Vec::new();
        let mut current_idx = 0usize;

        for v in &state.security_state.active_vetoes {
            let is_selected = current_idx == selected;
            let prefix = if is_selected { "> " } else { "  " };
            rows.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled("[VETO]", Style::default().fg(Color::Red)),
                Span::raw(format!(" {} | {}", v.quest_id, v.veto_reason)),
            ]));
            current_idx += 1;
        }

        for a in &state.security_state.recent_audits {
            let is_selected = current_idx == selected;
            let prefix = if is_selected { "> " } else { "  " };
            let color = if a.detection_rate > 0.0 {
                Color::Red
            } else {
                Color::Yellow
            };
            rows.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled("[AUDIT]", Style::default().fg(color)),
                Span::raw(format!(
                    " {} | risk={:.0}%",
                    a.vulnerability_type,
                    a.detection_rate * 100.0
                )),
            ]));
            current_idx += 1;
        }

        for i in &state.security_state.recent_interventions {
            let is_selected = current_idx == selected;
            let prefix = if is_selected { "> " } else { "  " };
            let color = match i.action.as_str() {
                "Block" => Color::Red,
                "Warn" => Color::Yellow,
                _ => Color::Green,
            };
            rows.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled("[ASA]", Style::default().fg(color)),
                Span::raw(format!(
                    " {} | {} | score={:.2}",
                    i.operation_id, i.action, i.safety_score
                )),
            ]));
            current_idx += 1;
        }

        (rows, current_idx)
    }

    /// 构造选中事件的详情文本
    fn detail_content(state: &TuiState, selected: usize) -> Option<(String, String)> {
        let mut offset = 0usize;

        for v in &state.security_state.active_vetoes {
            if offset == selected {
                let ts = v.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
                let content = format!(
                    "Type: Skeptic Veto\nQuest: {}\nReason: {}\nFrozen capabilities: {}\nTime: {}",
                    v.quest_id,
                    v.veto_reason,
                    v.frozen_capabilities.join(", "),
                    ts
                );
                return Some(("Skeptic Veto Detail".into(), content));
            }
            offset += 1;
        }

        for a in &state.security_state.recent_audits {
            if offset == selected {
                let ts = a.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
                let content = format!(
                    "Type: Red Team Audit\nVulnerability: {}\nFailed/Total: {}/{}\nDetection rate: {:.1}%\nSuggestion: {}\nTime: {}",
                    a.vulnerability_type,
                    a.failed_probes,
                    a.total_probes,
                    a.detection_rate * 100.0,
                    a.remediation_suggestion,
                    ts
                );
                return Some(("Red Team Audit Detail".into(), content));
            }
            offset += 1;
        }

        for i in &state.security_state.recent_interventions {
            if offset == selected {
                let ts = i.timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
                let block_reason = i.block_reason.as_deref().unwrap_or("(none)");
                let content = format!(
                    "Type: ASA Intervention\nOperation: {}\nAction: {}\nSafety score: {:.2}\nBlock reason: {}\nTime: {}",
                    i.operation_id,
                    i.action,
                    i.safety_score,
                    block_reason,
                    ts
                );
                return Some(("ASA Intervention Detail".into(), content));
            }
            offset += 1;
        }

        None
    }

    /// 构建冻结能力列表文本
    fn frozen_text(state: &TuiState) -> Text<'static> {
        let mut lines = vec![
            Line::from(vec![Span::styled(
                "Frozen Capabilities",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from("─────────────"),
        ];

        if state.security_state.frozen_capabilities.is_empty() {
            lines.push(Line::from("None"));
        } else {
            for cap in &state.security_state.frozen_capabilities {
                lines.push(Line::from(vec![
                    Span::styled("• ", Style::default().fg(Color::Red)),
                    Span::raw(cap.clone()),
                ]));
            }
        }
        Text::from(lines)
    }
}

impl Panel for SecurityPanel {
    fn id(&self) -> PanelId {
        PanelId::Security
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Security ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        self.clamp_selected(state);

        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 20 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(inner);

        // 左侧:事件列表
        let (rows, _count) = Self::build_rows(state, self.selected);
        let mut left_lines = vec![
            Line::from(vec![Span::styled(
                "Security Events",
                Style::default().add_modifier(Modifier::BOLD),
            )]),
            Line::from("─────────────"),
        ];
        if rows.is_empty() {
            left_lines.push(Line::from("No security events"));
        } else {
            left_lines.extend(rows);
        }
        left_lines.push(Line::from(""));
        left_lines.push(Line::from(FOOTER_TEXT));
        let left = Paragraph::new(Text::from(left_lines));
        left.render(chunks[0], buf);

        // 右侧:冻结能力
        let right = Paragraph::new(Self::frozen_text(state));
        right.render(chunks[1], buf);
    }

    fn focus(&mut self, focused: bool) {
        // 获得焦点时无需特殊处理;选择索引由状态维护。
        let _ = focused;
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::event_count(state);
        match key.code {
            KeyCode::Up => {
                if count > 0 && self.selected > 0 {
                    self.selected -= 1;
                }
                Some(TuiCommand::SwitchPanel(PanelId::Security))
            }
            KeyCode::Down => {
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
                Some(TuiCommand::SwitchPanel(PanelId::Security))
            }
            KeyCode::Enter => {
                if let Some((title, content)) = Self::detail_content(state, self.selected) {
                    Some(TuiCommand::OpenPopup(PopupKind::Detail { title, content }))
                } else {
                    None
                }
            }
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{
        AsaInterventionSummary, RedTeamAuditSummary, SecurityState, SkepticVetoSummary,
    };
    use chrono::Utc;

    fn sample_state() -> TuiState {
        let mut state = TuiState::new();
        state.security_state = SecurityState {
            active_vetoes: vec![SkepticVetoSummary {
                quest_id: "q1".into(),
                veto_reason: "unsafe".into(),
                frozen_capabilities: vec!["cap1".into()],
                timestamp: Utc::now(),
            }],
            recent_audits: vec![RedTeamAuditSummary {
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 2,
                total_probes: 10,
                detection_rate: 0.2,
                remediation_suggestion: "sanitize".into(),
                timestamp: Utc::now(),
            }],
            recent_interventions: vec![AsaInterventionSummary {
                operation_id: "op1".into(),
                action: "Block".into(),
                safety_score: 0.1,
                block_reason: Some("malicious".into()),
                timestamp: Utc::now(),
            }],
            frozen_capabilities: vec!["cap1".into()],
        };
        state
    }

    #[test]
    fn test_security_panel_id() {
        let panel = SecurityPanel::new();
        assert_eq!(panel.id(), PanelId::Security);
    }

    #[test]
    fn test_security_panel_navigation() {
        let mut panel = SecurityPanel::new();
        let mut state = sample_state();
        assert_eq!(panel.selected, 0);

        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 1);

        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 2);

        panel.handle_key(
            KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn test_security_panel_detail_popup() {
        let mut panel = SecurityPanel::new();
        let mut state = sample_state();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Detail { title, content })) => {
                assert!(title.contains("Veto"));
                assert!(content.contains("q1"));
            }
            _ => panic!("expected Detail popup command, got {:?}", cmd),
        }
    }

    #[test]
    fn test_security_panel_clamps_selection_when_empty() {
        let mut panel = SecurityPanel::new();
        panel.selected = 5;
        let state = TuiState::new();
        panel.clamp_selected(&state);
        assert_eq!(panel.selected, 0);
    }
}
