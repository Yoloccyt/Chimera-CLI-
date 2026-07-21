//! TUI Quest 面板 — 显示任务列表与进度
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移原有渲染逻辑,保持数据驱动行为不变。
//! - 使用 `Panel` trait 统一接口,便于 `TuiApp` 通过 `Box<dyn Panel>` 管理。
//! - M3 增加关键字过滤、滚动选择与详情弹窗。

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
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
use nexus_core::{Quest, TaskStatus};

/// Quest 面板
#[derive(Debug, Default, Clone)]
pub struct QuestPanel {
    /// 当前选中 Quest 的索引(在已过滤列表中)
    selected: usize,
    /// 列表滚动偏移
    scroll_offset: usize,
    /// 多选索引集合(批量操作用)
    selected_indices: HashSet<usize>,
}

/// 自定义 PartialEq:忽略 selected_indices,避免同一面板状态在不同选择集合下被判不等
///
/// WHY:多选索引是瞬时 UI 状态,不影响面板的功能等价性。
/// 两个 QuestPanel 若光标位置与滚动偏移相同即视为等价。
impl PartialEq for QuestPanel {
    fn eq(&self, other: &Self) -> bool {
        self.selected == other.selected && self.scroll_offset == other.scroll_offset
    }
}

impl QuestPanel {
    /// 创建新的 Quest 面板
    pub fn new() -> Self {
        Self {
            selected_indices: HashSet::new(),
            ..Default::default()
        }
    }

    /// 返回当前选中项索引(测试用,与 EventStreamPanel/LogPanel 模式一致)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 返回当前滚动偏移(测试用,与 EventStreamPanel 模式一致)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
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
    ///
    /// # 参数
    /// - `state`:TUI 共享状态
    /// - `cursor`:当前光标所在的 Quest 索引
    /// - `selected_indices`:批量选中的索引集合
    pub fn content(
        state: &TuiState,
        cursor: usize,
        selected_indices: &HashSet<usize>,
    ) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Quest Tasks"), Line::from("─────────────")];

        let quests = Self::filtered_quests(state);

        if quests.is_empty() {
            lines.push(Line::from("No active quests"));
        } else {
            let quest_count = quests.len();
            for (idx, quest) in quests.iter().enumerate() {
                let is_cursor = idx == cursor;
                let is_checked = selected_indices.contains(&idx);

                // 标题行:前缀 + 序号 + 标题 + mini gauge
                let title_prefix = match (is_cursor, is_checked) {
                    (true, true) => "●>",
                    (true, false) => "> ",
                    (false, true) => "● ",
                    (false, false) => "  ",
                };

                let title_style = if is_checked {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                // 任务进度百分比 mini gauge
                let gauge = if quest.tasks.is_empty() {
                    "          ".to_string()
                } else {
                    let total = quest.tasks.len();
                    let done = quest
                        .tasks
                        .iter()
                        .filter(|t| t.status == TaskStatus::Completed)
                        .count();
                    let pct = if total > 0 {
                        (done as f32 / total as f32 * 100.0) as usize
                    } else {
                        0
                    };
                    format!(
                        " {} {}/{} ({}%)",
                        render_mini_gauge(done, total),
                        done,
                        total,
                        pct
                    )
                };

                let index_str = format!("[{}]", idx + 1);
                // 用固定宽度标题区 + gauge 右对齐在 80 列内
                let title_text = format!(
                    "{title_prefix}{index_str} {title}{gauge}",
                    title = quest.title
                );

                lines.push(Line::from(vec![Span::styled(title_text, title_style)]));

                // 元信息行:灰色缩进显示 ID 与思考模式
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "    ID: {} | Mode: {:?} | Priority: {}",
                        quest.quest_id, quest.thinking_mode, quest.priority
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

/// 渲染 mini progress gauge(4 字符宽)
///
/// 用 Unicode block 字符 ▰/▱ 表示进度,固定 4 格宽度。
/// 当 total=0 时返回 `[    ]`。
fn render_mini_gauge(completed: usize, total: usize) -> String {
    if total == 0 {
        return "[    ]".to_string();
    }
    let ratio = completed as f32 / total as f32;
    let filled = (ratio * 4.0).round() as usize;
    let mut s = String::with_capacity(6);
    s.push('[');
    for i in 0..4 {
        if i < filled {
            s.push('▰');
        } else {
            s.push('▱');
        }
    }
    s.push(']');
    s
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
        self.selected = list_state::clamp_selected(self.selected, quests.len());

        let title = build_filter_title(state, "Quest Tasks");
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(Self::content(state, self.selected, &self.selected_indices))
            .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_quests(state).len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            // 空格键:切换当前项的批量选择状态
            //
            // WHY 空格:与文件管理器/邮件客户端的多选语义一致,
            // 操作员直觉式操作无需学习。不影响单行光标导航。
            KeyCode::Char(' ') => {
                let quests = Self::filtered_quests(state);
                if quests.is_empty() {
                    return None;
                }
                let idx = list_state::clamp_selected(self.selected, quests.len());
                if self.selected_indices.contains(&idx) {
                    self.selected_indices.remove(&idx);
                } else {
                    self.selected_indices.insert(idx);
                }
                None
            }
            // Ctrl+A:全选当前过滤列表中所有 Quest
            //
            // WHY Ctrl+A:与编辑器全选语义一致,减少逐项选择的操作成本。
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let quests = Self::filtered_quests(state);
                for i in 0..quests.len() {
                    self.selected_indices.insert(i);
                }
                None
            }
            // Ctrl+D:取消所有批量选择
            //
            // WHY Ctrl+D:对称于 Ctrl+A,同时不与现有 `d`(单条取消)冲突。
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.selected_indices.clear();
                None
            }
            // `p` 键:暂停 Quest(批量优先;无多选时暂停当前光标项)
            //
            // WHY 批量优先:若操作员已勾选多个 Quest,`p` 应暂停全部而非仅当前项,
            // 减少操作员的认知负担。多选时弹出批量确认弹窗;无多选时回退到单选暂停。
            KeyCode::Char('p') => {
                if !self.selected_indices.is_empty() {
                    let quests = Self::filtered_quests(state);
                    let quest_ids: Vec<String> = self
                        .selected_indices
                        .iter()
                        .filter_map(|&i| quests.get(i))
                        .map(|q| q.quest_id.clone())
                        .collect();
                    if quest_ids.is_empty() {
                        return None;
                    }
                    let batch_ids = quest_ids.join(",");
                    return Some(TuiCommand::OpenPopup(PopupKind::control_confirm(
                        &format!("Batch pause {} quests", quest_ids.len()),
                        &batch_ids,
                        format!("batch_pause:{batch_ids}"),
                    )));
                }
                // 单选暂停:委托给现有 RequestQuestPause 命令
                let quests = Self::filtered_quests(state);
                quests
                    .get(self.selected)
                    .map(|quest| TuiCommand::RequestQuestPause(quest.quest_id.clone()))
            }
            // `c` 键:取消 Quest(批量优先,破坏性操作需确认)
            //
            // WHY `c` 而非复用 `d`:单条取消(`d`)与批量取消(`c`)语义区分,
            // 避免操作员因肌肉记忆按 `d` 时意外触发批量操作。
            // 多选时弹出批量确认弹窗;无多选时 `c` 为 no-op(单条取消请用 `d`)。
            KeyCode::Char('c') => {
                if !self.selected_indices.is_empty() {
                    let quests = Self::filtered_quests(state);
                    let quest_ids: Vec<String> = self
                        .selected_indices
                        .iter()
                        .filter_map(|&i| quests.get(i))
                        .map(|q| q.quest_id.clone())
                        .collect();
                    if quest_ids.is_empty() {
                        return None;
                    }
                    let batch_ids = quest_ids.join(",");
                    return Some(TuiCommand::OpenPopup(PopupKind::control_confirm(
                        &format!("Batch cancel {} quests", quest_ids.len()),
                        &batch_ids,
                        format!("batch_cancel:{batch_ids}"),
                    )));
                }
                None
            }
            // P5 跨面板联动:Enter 跳转到 EventStream 面板并按 quest_id 筛选事件
            //
            // WHY Enter 改为跳转:Quest 面板的核心联动场景是"查看某 Quest 的
            // 关联事件流",Enter 作为最直接的动作键,应触发最高频的联动操作。
            // 原 detail popup 功能保留到 `d` 键,避免功能丢失。
            KeyCode::Enter => {
                let quests = Self::filtered_quests(state);
                quests
                    .get(self.selected)
                    .map(|quest| TuiCommand::JumpToEventStream {
                        quest_id: quest.quest_id.clone(),
                    })
            }
            // `i` 键打开 Quest 详情弹窗(原 `d` 键功能,P5 迁移到此键,M4 再迁移)
            //
            // WHY 迁移历史:Enter(detail) → `d`(P5) → `i`(M4)。
            // M4 将 `d` 重新分配给 cancel(破坏性操作需显眼键位),
            // `i` 是 "info" 的首字母,语义直观,且当前未被使用。
            KeyCode::Char('i') => {
                let quests = Self::filtered_quests(state);
                quests.get(self.selected).map(|quest| {
                    let content = Self::detail_content(quest);
                    TuiCommand::OpenPopup(PopupKind::Detail {
                        title: quest.title.clone(),
                        content,
                        scroll: 0,
                    })
                })
            }
            // `d` 键取消选中 Quest(破坏性操作,弹出确认弹窗)
            //
            // WHY `d` = "cancel/destroy":破坏性操作需高显眼键位,`d` 与 vim 的
            // delete 语义一致,操作员肌肉记忆强。返回 RequestQuestCancel 后由
            // TuiApp::apply_command 弹出 Confirm 弹窗,操作员确认(Enter)后
            // 才发布 QuestCancelRequested 事件,防误触导致任务丢失。
            KeyCode::Char('d') => {
                let quests = Self::filtered_quests(state);
                quests
                    .get(self.selected)
                    .map(|quest| TuiCommand::RequestQuestCancel(quest.quest_id.clone()))
            }
            // `+` 键:优先级 +1(上限 255,边界保护)
            //
            // WHY 直接返回命令而非发布事件:面板不持有 EventBus(L10 面板保持
            // 无状态),由 TuiApp::apply_command 统一发布。边界检查在面板完成,
            // 避免无效请求(priority=255 时 +1 溢出)进入事件总线。
            KeyCode::Char('+') => {
                let quests = Self::filtered_quests(state);
                quests.get(self.selected).and_then(|quest| {
                    if quest.priority < u8::MAX {
                        Some(TuiCommand::RequestQuestPriorityChange {
                            quest_id: quest.quest_id.clone(),
                            new_priority: quest.priority + 1,
                        })
                    } else {
                        None
                    }
                })
            }
            // `-` 键:优先级 -1(下限 0,边界保护)
            //
            // WHY 边界检查在面板:与 `+` 对称,priority=0 时不返回命令,
            // TuiApp 不会发布事件,操作员无感知(无弹窗、无状态栏错误)。
            KeyCode::Char('-') => {
                let quests = Self::filtered_quests(state);
                quests.get(self.selected).and_then(|quest| {
                    if quest.priority > 0 {
                        Some(TuiCommand::RequestQuestPriorityChange {
                            quest_id: quest.quest_id.clone(),
                            new_priority: quest.priority - 1,
                        })
                    } else {
                        None
                    }
                })
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
        let count = Self::filtered_quests(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑/↓", "导航"),
            ("Enter", "详情"),
            ("g g", "跳顶"),
            ("G", "跳底"),
        ]
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_quests(state).len();
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
            priority: 128,
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
        let content = QuestPanel::content(&state, 0, &HashSet::new()).to_string();
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
        let content = QuestPanel::content(&state, 0, &HashSet::new()).to_string();
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
                priority: 128,
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
                priority: 128,
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
        // P5→M4:detail popup 从 `d` 键迁移到 `i` 键(info)
        // WHY `d` 在 M4 重新分配给 cancel(破坏性操作),detail 改用 `i`
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q1", "Detail Quest")];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('i'), crossterm::event::KeyModifiers::NONE),
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
    fn test_quest_panel_enter_jumps_to_event_stream() {
        // P5 跨面板联动:Enter 键应返回 JumpToEventStream 命令
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q1", "Jump Quest")];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::JumpToEventStream { quest_id }) => {
                assert_eq!(quest_id, "q1");
            }
            _ => panic!("expected JumpToEventStream command, got {:?}", cmd),
        }
    }

    #[test]
    fn test_quest_panel_enter_no_quest_returns_none() {
        // P5:无 Quest 时 Enter 应返回 None
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert!(cmd.is_none());
    }

    #[test]
    fn test_quest_panel_handle_key_help_returns_none() {
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), crossterm::event::KeyModifiers::NONE);
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        assert_eq!(panel.handle_key(key, &mut state), None);
    }
}
