//! TUI Quest 面板 — 显示任务列表与进度
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移原有渲染逻辑,保持数据驱动行为不变。
//! - 使用 `Panel` trait 统一接口,便于 `TuiApp` 通过 `Box<dyn Panel>` 管理。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::FOOTER_TEXT;
use crate::types::{PanelId, TuiCommand, TuiState};
use nexus_core::TaskStatus;

/// Quest 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct QuestPanel;

impl QuestPanel {
    /// 创建新的 Quest 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Quest 面板文本内容
    ///
    /// WHY 独立方法:与 `render` 解耦,便于单元测试直接验证文本输出。
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Quest Tasks"), Line::from("─────────────")];

        if state.quest_list.is_empty() {
            lines.push(Line::from("No active quests"));
        } else {
            let quest_count = state.quest_list.len();
            for (idx, quest) in state.quest_list.iter().enumerate() {
                // 标题行:加粗显示 Quest 序号与标题
                lines.push(Line::from(vec![Span::styled(
                    format!("[{}] {}", idx + 1, quest.title),
                    Style::default().add_modifier(Modifier::BOLD),
                )]));

                // 元信息行:灰色缩进显示 ID 与思考模式
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "    ID: {} | Mode: {:?}",
                        quest.quest_id, quest.thinking_mode
                    ),
                    Style::default().fg(Color::Gray),
                )]));

                // 任务摘要行:统计任务总数、已完成数、待处理数
                if quest.tasks.is_empty() {
                    lines.push(Line::from(vec![Span::styled(
                        "    (no tasks)",
                        Style::default().fg(Color::Gray),
                    )]));
                } else {
                    let total = quest.tasks.len();
                    let done = quest
                        .tasks
                        .iter()
                        .filter(|&t| t.status == TaskStatus::Completed)
                        .count();
                    let running = quest
                        .tasks
                        .iter()
                        .filter(|&t| t.status == TaskStatus::Running)
                        .count();
                    let pending = quest
                        .tasks
                        .iter()
                        .filter(|&t| t.status == TaskStatus::Pending)
                        .count();

                    lines.push(Line::from(vec![
                        Span::styled("    Tasks: ", Style::default().fg(Color::Gray)),
                        Span::from(format!("{total} total")),
                        Span::from(", "),
                        Span::styled(format!("{done} done"), Style::default().fg(Color::Green)),
                        Span::from(", "),
                        Span::styled(
                            format!("{running} running"),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::from(", "),
                        Span::styled(
                            format!("{pending} pending"),
                            Style::default().fg(Color::Gray),
                        ),
                    ]));
                }

                // 除最后一个 Quest 外,每个 Quest 后空一行,提升可读性
                if idx + 1 < quest_count {
                    lines.push(Line::from(""));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }
}

impl Panel for QuestPanel {
    fn id(&self) -> PanelId {
        PanelId::Quest
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Quest Tasks ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // M1:Quest 面板不处理专属按键;后续可在此扩展任务展开/折叠等交互。
        match key.code {
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

    fn sample_quest(id: &str, title: &str) -> Quest {
        Quest {
            quest_id: id.into(),
            title: title.into(),
            tasks: vec![Task {
                task_id: format!("{id}-t1"),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    #[test]
    fn test_quest_panel_id() {
        let panel = QuestPanel::new();
        assert_eq!(panel.id(), PanelId::Quest);
    }

    #[test]
    fn test_quest_panel_empty_state() {
        let state = TuiState::new();
        let content = QuestPanel::content(&state).to_string();
        assert!(content.contains("Quest Tasks"));
        assert!(content.contains("No active quests"));
    }

    #[test]
    fn test_quest_panel_with_quests() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
        ];
        let content = QuestPanel::content(&state).to_string();
        assert!(content.contains("First Quest"));
        assert!(content.contains("Second Quest"));
        assert!(content.contains("[1]"));
        assert!(content.contains("[2]"));
    }

    #[test]
    fn test_quest_panel_handle_key_help() {
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), crossterm::event::KeyModifiers::NONE);
        assert_eq!(
            panel.handle_key(key, &mut state),
            Some(TuiCommand::ShowHelp)
        );
    }
}
