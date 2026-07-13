//! TUI Quest 面板 — 显示任务列表与进度
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移原有渲染逻辑,保持数据驱动行为不变。
//! - 使用 `Panel` trait 统一接口,便于 `TuiApp` 通过 `Box<dyn Panel>` 管理。
//! - M3 增加关键字过滤、滚动选择与详情弹窗。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::FOOTER_TEXT;
use crate::types::{PanelId, TuiCommand, TuiState};
use nexus_core::{Quest, TaskStatus};

/// Quest 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuestPanel {
    /// 当前选中 Quest 的索引(在已过滤列表中)
    selected: usize,
    /// 列表滚动偏移
    scroll_offset: usize,
}

impl QuestPanel {
    /// 创建新的 Quest 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回经过关键字过滤的 Quest 列表
    pub fn filtered_quests(state: &TuiState) -> Vec<&Quest> {
        if let Some(kw) = &state.filter_keyword {
            let kw = kw.to_lowercase();
            state
                .quest_list
                .iter()
                .filter(|q| quest_matches_keyword(q, &kw))
                .collect()
        } else {
            state.quest_list.iter().collect()
        }
    }

    /// 构建 Quest 面板文本内容
    ///
    /// WHY 独立方法:与 `render` 解耦,便于单元测试直接验证文本输出。
    pub fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Quest Tasks"), Line::from("─────────────")];

        let quests = Self::filtered_quests(state);

        if quests.is_empty() {
            lines.push(Line::from("No active quests"));
        } else {
            let quest_count = quests.len();
            for (idx, quest) in quests.iter().enumerate() {
                let is_selected = idx == selected;
                let selected_style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::BOLD)
                };
                let prefix = if is_selected { "> " } else { "  " };

                // 标题行:加粗显示 Quest 序号与标题
                lines.push(Line::from(vec![Span::styled(
                    format!("{}[{}] {}", prefix, idx + 1, quest.title),
                    selected_style,
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
                        .filter(|t| t.status == TaskStatus::Completed)
                        .count();
                    let running = quest
                        .tasks
                        .iter()
                        .filter(|t| t.status == TaskStatus::Running)
                        .count();
                    let pending = quest
                        .tasks
                        .iter()
                        .filter(|t| t.status == TaskStatus::Pending)
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

    /// 构建 Quest 详情弹窗内容
    fn detail_content(quest: &Quest) -> String {
        let mut lines = vec![
            format!("Title: {}", quest.title),
            format!("ID: {}", quest.quest_id),
            format!("Mode: {:?}", quest.thinking_mode),
            format!(
                "Checkpoint: {}",
                quest.checkpoint_id.as_deref().unwrap_or("(none)")
            ),
            format!("Tasks: {}", quest.tasks.len()),
        ];

        if !quest.tasks.is_empty() {
            lines.push("".into());
            lines.push("Task list:".into());
            for task in &quest.tasks {
                lines.push(format!(
                    "  - [{}] {}: {}",
                    task_status_symbol(&task.status),
                    task.task_id,
                    task.description
                ));
            }
        }

        lines.join("\n")
    }

    /// 根据选中项与可见行数调整滚动偏移
    fn adjust_scroll(selected: usize, scroll_offset: usize, visible_rows: usize) -> usize {
        if visible_rows == 0 {
            return scroll_offset;
        }
        if selected < scroll_offset {
            selected
        } else if selected >= scroll_offset + visible_rows {
            selected.saturating_sub(visible_rows - 1)
        } else {
            scroll_offset
        }
    }

    /// 将选中索引限制在有效范围内
    fn clamp_selected(&mut self, max: usize) {
        if max == 0 {
            self.selected = 0;
        } else if self.selected >= max {
            self.selected = max - 1;
        }
    }
}

fn task_status_symbol(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "✓",
        TaskStatus::Running => "●",
        TaskStatus::Pending => "○",
        TaskStatus::Failed => "✗",
    }
}

/// Quest 关键字匹配(标题 + 任务描述)
fn quest_matches_keyword(quest: &Quest, keyword: &str) -> bool {
    let keyword = keyword.to_lowercase();
    if quest.title.to_lowercase().contains(&keyword) {
        return true;
    }
    quest.tasks.iter().any(|t| {
        t.description.to_lowercase().contains(&keyword)
            || t.task_id.to_lowercase().contains(&keyword)
    })
}

/// 构造带过滤器指示器的标题
fn build_filter_title(state: &TuiState, base: &str) -> String {
    if let Some(kw) = &state.filter_keyword {
        format!(" {base} [keyword:{}] ", kw)
    } else {
        format!(" {base} ")
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
        let quests = Self::filtered_quests(state);
        self.clamp_selected(quests.len());

        let title = build_filter_title(state, "Quest Tasks");
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset = Self::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(Self::content(state, self.selected))
            .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_quests(state).len();
        match key.code {
            KeyCode::Up => {
                if count > 0 && self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            KeyCode::Down => {
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Enter => {
                let quests = Self::filtered_quests(state);
                if let Some(quest) = quests.get(self.selected) {
                    let content = Self::detail_content(quest);
                    Some(TuiCommand::OpenPopup(PopupKind::Detail {
                        title: quest.title.clone(),
                        content,
                        scroll: 0,
                    }))
                } else {
                    None
                }
            }
            KeyCode::Char('?') => Some(TuiCommand::ShowHelp),
            _ => None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            MouseEventKind::ScrollDown => {
                let count = Self::filtered_quests(state).len();
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
                None
            }
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
        let content = QuestPanel::content(&state, 0).to_string();
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
        let content = QuestPanel::content(&state, 0).to_string();
        assert!(content.contains("First Quest"));
        assert!(content.contains("Second Quest"));
        assert!(content.contains("[1]"));
        assert!(content.contains("[2]"));
    }

    #[test]
    fn test_quest_panel_filter_keyword_title() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q1", "Alpha Quest"),
            sample_quest("q2", "Beta Quest"),
        ];
        state.filter_keyword = Some("alpha".into());

        let filtered = QuestPanel::filtered_quests(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].quest_id, "q1");
    }

    #[test]
    fn test_quest_panel_filter_keyword_task() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            Quest {
                quest_id: "q1".into(),
                title: "First".into(),
                tasks: vec![Task {
                    task_id: "t1".into(),
                    description: "special task".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                }],
                thinking_mode: ThinkingMode::Standard,
                checkpoint_id: None,
            },
            Quest {
                quest_id: "q2".into(),
                title: "Second".into(),
                tasks: vec![Task {
                    task_id: "t2".into(),
                    description: "other task".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                }],
                thinking_mode: ThinkingMode::Standard,
                checkpoint_id: None,
            },
        ];
        state.filter_keyword = Some("special".into());

        let filtered = QuestPanel::filtered_quests(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].quest_id, "q1");
    }

    #[test]
    fn test_quest_panel_title_with_filter() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        let title = build_filter_title(&state, "Quest Tasks");
        assert!(title.contains("keyword:foo"));
    }

    #[test]
    fn test_quest_panel_navigation() {
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
        ];

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
    fn test_quest_panel_detail_popup() {
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q1", "Detail Quest")];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Detail { title, content, .. })) => {
                assert_eq!(title, "Detail Quest");
                assert!(content.contains("q1"));
                assert!(content.contains("test task"));
            }
            _ => panic!("expected Detail popup command, got {:?}", cmd),
        }
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
