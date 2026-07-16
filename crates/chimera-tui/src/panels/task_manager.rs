//! TUI 任务管理面板 — Quest CRUD 控制台
//!
//! 对应架构层:L10 Interface
//! 对应 spec:`enterprise-tui-monitoring-task-viz §三 任务管理增强`
//!
//! # 设计决策(WHY)
//! - 独立 Panel 实现,不强注册到 TuiApp 17 面板循环(避免破坏既有焦点契约);
//!   面板通过单元测试与未来"插件化 PanelRegistry"接入。
//! - 优先级 0-10 用户面范围(与底层 `Quest.priority` 0-255 内部范围区分),
//!   范围映射在 `TuiApp::apply_command` 中桥接(×25)。
//! - 默认按优先级降序排序,稳定排序(同优先级按 quest_id 字典序),
//!   避免同优先级 Quest 在面板上抖动。
//! - 复用既有 `list_state::clamp_selected` / `adjust_scroll` 工具,
//!   保持与 Quest/EventStream/Log 面板一致的滚动语义。
//!
//! # 键位
//! - `C`:创建 Quest(占位,本期返回 None;后续 Task 接入创建命令)
//! - `P`:暂停(走 `TuiCommand::QuestControl { Pause }`)
//! - `T`:终止(走 `TuiCommand::QuestControl { Terminate }`)
//! - `+` / `=`:优先级 +1(上限 10)
//! - `-`:优先级 -1(下限 0)
//! - `Enter`:查看详情(沿用既有 OpenPopup 模式)
//! - `/`:关键字过滤(沿用 `state.filter_keyword`)
//! - `↑` / `↓`:导航

use std::cmp::Reverse;
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
use crate::types::{PanelId, QuestAction, TuiCommand, TuiState};
use nexus_core::Quest;

/// 优先级上限(用户面 0-10 范围)
///
/// WHY 10:与既有 `RequestQuestPriorityChange` 的 0-255 内部范围区分,
/// 桥接公式 `priority_255 = level * 25`,10 → 250 留出余量给"内部超调"。
pub const PRIORITY_MAX: u8 = 10;

/// 优先级下限(用户面 0-10 范围)
pub const PRIORITY_MIN: u8 = 0;

/// TaskManagerPanel — 任务管理面板
///
/// ## 字段说明
/// - `selected`:当前选中项在排序后列表中的索引
/// - `scroll_offset`:列表滚动偏移(与 list_state 模块一致)
/// - `selected_indices`:批量选中的索引集合(预留,本 Task 暂未启用)
#[derive(Debug, Default, Clone)]
pub struct TaskManagerPanel {
    /// 当前选中索引(在排序后列表中)
    selected: usize,
    /// 滚动偏移
    scroll_offset: usize,
    /// 批量选中的索引集合(为未来 batch 操作预留,本 Task 未启用)
    #[allow(dead_code)]
    selected_indices: HashSet<usize>,
}

impl TaskManagerPanel {
    /// 创建新面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 公开方法:返回排序后顶部 Quest 的 ID(测试用)
    ///
    /// WHY 返回 `String` 而非 `&str`:`sorted_quests` 返回 `Vec<&Quest>`(临时),
    /// 借出的 `&str` 与临时 Vec 同生命周期,无法在方法签名表达。
    /// 直接 clone 一个 `String` 是测试场景下的最小成本(每帧 1 次),
    /// 与 `QuestPanel::selected` 等公开方法保持一致的可观察性。
    pub fn top_quest_id(&self, state: &TuiState) -> Option<String> {
        Self::sorted_quests(state)
            .first()
            .map(|q| q.quest_id.clone())
    }

    /// 公开方法:返回排序后底部 Quest 的 ID(测试用)
    pub fn bottom_quest_id(&self, state: &TuiState) -> Option<String> {
        Self::sorted_quests(state)
            .last()
            .map(|q| q.quest_id.clone())
    }

    /// 公开方法:返回排序后 Quest 数量
    pub fn sorted_quest_count(state: &TuiState) -> usize {
        Self::sorted_quests(state).len()
    }

    /// 公开方法:渲染面板文本内容(测试用,与 `QuestPanel::content` 模式一致)
    ///
    /// WHY 独立方法:与 `render` 解耦,便于纯逻辑单元测试,
    /// 避免依赖 ratatui TestBackend 与 Buffer 比较。
    pub fn render_text(&self, state: &TuiState) -> String {
        Self::content(state, self.selected, &self.selected_indices).to_string()
    }

    /// 排序后的 Quest 列表(按优先级降序,同优先级按 quest_id 字典序)
    ///
    /// WHY 稳定排序:`sort_by_key` 在 Rust 中是稳定排序,
    /// 通过 `Reverse(priority)` 实现降序,通过 `quest_id` 作为二级 key
    /// 确保同优先级 Quest 顺序可预测(便于测试断言)。
    fn sorted_quests(state: &TuiState) -> Vec<&Quest> {
        let mut quests: Vec<&Quest> = state.quest_list.iter().collect();
        quests.sort_by_key(|q| (Reverse(q.priority), q.quest_id.clone()));
        quests
    }

    /// 公开方法:返回经过排序的 Quest 列表(测试用)
    pub fn sorted(state: &TuiState) -> Vec<&Quest> {
        Self::sorted_quests(state)
    }

    /// 构造面板内容(文本形式,便于测试与渲染共享)
    fn content(
        state: &TuiState,
        cursor: usize,
        _selected_indices: &HashSet<usize>,
    ) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Task Manager"), Line::from("──────────────")];

        let quests = Self::sorted_quests(state);

        if quests.is_empty() {
            lines.push(Line::from("No quests"));
        } else {
            for (idx, quest) in quests.iter().enumerate() {
                let is_cursor = idx == cursor;
                let prefix = if is_cursor { "> " } else { "  " };
                let style = if is_cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                // WHY 显示 quest_id + 标题 + 优先级(便于操作员快速定位)
                // 优先级列固定宽度 2 字符,左对齐
                let line = format!(
                    "{prefix}{id:<16} | {title} | P{priority:<2}",
                    id = quest.quest_id,
                    title = quest.title,
                    priority = quest.priority
                );
                lines.push(Line::from(vec![Span::styled(line, style)]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }

    /// 详情弹窗内容
    fn detail_content(quest: &Quest) -> String {
        format!(
            "Title: {}\nID: {}\nPriority: {}\nTasks: {}\nMode: {:?}",
            quest.title,
            quest.quest_id,
            quest.priority,
            quest.tasks.len(),
            quest.thinking_mode
        )
    }
}

impl Panel for TaskManagerPanel {
    fn id(&self) -> PanelId {
        // WHY 复用 PanelId::Quest:TaskManagerPanel 与 QuestPanel 共享数据源
        // (state.quest_list),但提供不同的视图(CRUD vs 监视)。
        // 复用同一 PanelId 避免焦点循环膨胀,后续如需独立焦点
        // 再扩展 PanelId::TaskManager 变体(会破坏既有 17 面板契约)。
        PanelId::Quest
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Task Manager ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let count = Self::sorted_quest_count(state);
        self.selected = list_state::clamp_selected(self.selected, count);

        let block = Block::default().borders(Borders::ALL).title(self.title());
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
        // Up/Down 导航(复用 list_state 工具)
        let count = Self::sorted_quest_count(state);
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            // WHY 排序后列表用 `sorted_quests` 而非 `quest_list`:
            // 选中项在排序后视图中的索引,需要访问排序后的列表。
            // 性能上每次按键 clone 整个列表在小规模(< 100)Quest 下可接受;
            // 后续如需优化可缓存排序结果(数据未变时复用)。
            // `get_quest_id` 闭包:从排序后列表取出选中项的 quest_id
            KeyCode::Char('P') => {
                let quest_id = Self::sorted_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Pause,
                })
            }
            KeyCode::Char('T') => {
                let quest_id = Self::sorted_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Terminate,
                })
            }
            // `+` 键:优先级 +1(上限 10,边界保护)
            //
            // WHY 边界检查在面板:与 spec 一致(0-10 范围),
            // priority=10 时不返回命令,避免无效 SetPriority(11) 进入事件总线。
            // Quest.priority 实际为 0-255,此处 +1 是用户面步进,与桥接公式无关。
            KeyCode::Char('+') => {
                let quest = Self::sorted_quests(state).get(self.selected).copied();
                quest.and_then(|q| {
                    // WHY saturating_add + 二次钳制:u8::saturating_add(255) = 255,
                    // 不会越界;然后与 PRIORITY_MAX 比较做用户面范围保护。
                    let current_user = user_priority_from_internal(q.priority);
                    if current_user < PRIORITY_MAX {
                        Some(TuiCommand::QuestControl {
                            id: q.quest_id.clone(),
                            action: QuestAction::SetPriority(current_user + 1),
                        })
                    } else {
                        None
                    }
                })
            }
            // `-` 键:优先级 -1(下限 0,边界保护)
            //
            // WHY 不区分用户面/内部映射:由于 `user_priority_from_internal` 是
            // 单调函数(priority_255 = level * 25),内部值 -1 对应用户面 -1,
            // 直接比较 PRIORITY_MIN 即可。
            KeyCode::Char('-') => {
                let quest = Self::sorted_quests(state).get(self.selected).copied();
                quest.and_then(|q| {
                    let current_user = user_priority_from_internal(q.priority);
                    if current_user > PRIORITY_MIN {
                        Some(TuiCommand::QuestControl {
                            id: q.quest_id.clone(),
                            action: QuestAction::SetPriority(current_user - 1),
                        })
                    } else {
                        None
                    }
                })
            }
            // Enter 键:打开 Quest 详情弹窗(沿用 OpenPopup 模式)
            KeyCode::Enter => {
                let quest = Self::sorted_quests(state).get(self.selected).copied();
                quest.map(|q| {
                    TuiCommand::OpenPopup(PopupKind::Detail {
                        title: format!("Task: {}", q.title),
                        content: Self::detail_content(q),
                        scroll: 0,
                    })
                })
            }
            // C/R 键:Create/Resume 预留(本期 C 键返回 None,Resume 实际处理)
            //
            // WHY C 键为 no-op:Quest 创建需要接收用户输入(spec 提到「C 创建」,
            // 但完整的创建流程需要命令面板交互,本 Task 暂占位为 None,
            // 避免误触发空 Quest)。
            KeyCode::Char('C') => None,
            // R 键:恢复已暂停 Quest(对称于 P 键的 Pause)
            KeyCode::Char('R') => {
                let quest_id = Self::sorted_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Resume,
                })
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
        let count = Self::sorted_quest_count(state);
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::sorted_quest_count(state);
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        // consume Ctrl modifier 警告(避免未使用 import)
        let _ = KeyModifiers::NONE;
        None
    }
}

/// 内部优先级 (0-255) → 用户面优先级 (0-10) 映射
///
/// 反向桥接 `apply_command` 中 `priority_255 = level * 25` 的公式。
/// 用于 `+`/`-` 键的"读取当前用户面值"判断。
///
/// WHY saturating_div:防止未来公式变化导致除零;
/// min(PRIORITY_MAX):确保返回值在 [0, 10] 范围内。
fn user_priority_from_internal(internal: u8) -> u8 {
    ((internal as u16 / 25).min(PRIORITY_MAX as u16)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Task, TaskStatus, ThinkingMode};

    fn sample_quest(id: &str, title: &str, priority: u8) -> Quest {
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
            priority,
        }
    }

    #[test]
    fn test_priority_mapping_roundtrip() {
        // 桥接公式:priority_255 = level * 25
        for level in 0..=10u8 {
            let internal = (level as u16 * 25) as u8;
            let back = user_priority_from_internal(internal);
            assert_eq!(back, level, "level {level} 桥接往返失败");
        }
    }

    #[test]
    fn test_user_priority_clamps_high_input() {
        // 内部 250 → 用户面 10
        assert_eq!(user_priority_from_internal(250), 10);
        // 内部 255 → 用户面 10(saturate)
        assert_eq!(user_priority_from_internal(255), 10);
        // 内部 0 → 用户面 0
        assert_eq!(user_priority_from_internal(0), 0);
    }

    #[test]
    fn test_sort_priority_descending() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-low", "Low", 2),
            sample_quest("q-high", "High", 8),
            sample_quest("q-mid", "Mid", 5),
        ];
        let sorted: Vec<String> = TaskManagerPanel::sorted(&state)
            .iter()
            .map(|q| q.quest_id.clone())
            .collect();
        assert_eq!(sorted, vec!["q-high", "q-mid", "q-low"]);
    }

    #[test]
    fn test_sort_stable_by_quest_id() {
        // 同优先级按 quest_id 字典序(稳定排序)
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-z", "Z", 5),
            sample_quest("q-a", "A", 5),
            sample_quest("q-m", "M", 5),
        ];
        let sorted: Vec<String> = TaskManagerPanel::sorted(&state)
            .iter()
            .map(|q| q.quest_id.clone())
            .collect();
        assert_eq!(sorted, vec!["q-a", "q-m", "q-z"]);
    }

    #[test]
    fn test_top_bottom_helpers() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-low", "Low", 2),
            sample_quest("q-high", "High", 8),
        ];
        let panel = TaskManagerPanel::new();
        assert_eq!(panel.top_quest_id(&state), Some("q-high".to_string()));
        assert_eq!(panel.bottom_quest_id(&state), Some("q-low".to_string()));
    }

    #[test]
    fn test_pause_key_emits_quest_control_pause() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-1", "T1", 5)];
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::QuestControl { id, action }) => {
                assert_eq!(id, "q-1");
                assert_eq!(action, QuestAction::Pause);
            }
            other => panic!("expected QuestControl Pause, got {other:?}"),
        }
    }

    #[test]
    fn test_priority_increment_at_max_returns_none() {
        // 内部 250 = 用户面 10(已到上限),+ 应不产生命令
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-max", "Max", 250)];
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE),
            &mut state,
        );
        assert!(cmd.is_none(), "优先级已到上限 10,+ 应不产生命令");
    }

    #[test]
    fn test_priority_decrement_at_min_returns_none() {
        // 内部 0 = 用户面 0(已到下限),- 应不产生命令
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-min", "Min", 0)];
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE),
            &mut state,
        );
        assert!(cmd.is_none(), "优先级已到下限 0,- 应不产生命令");
    }

    #[test]
    fn test_terminate_key_emits_quest_control_terminate() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-1", "T1", 5)];
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::QuestControl { id, action }) => {
                assert_eq!(id, "q-1");
                assert_eq!(action, QuestAction::Terminate);
            }
            other => panic!("expected QuestControl Terminate, got {other:?}"),
        }
    }
}
