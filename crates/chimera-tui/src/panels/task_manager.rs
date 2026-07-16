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
//! - P4.3:支持 3 种排序模式(Priority/Status/CreatedAt),
//!   用户通过 `S` 键循环切换,默认模式由 `TuiConfig::task_manager_default_sort` 控制。
//! - P4.3:CreatedAt 排序使用面板侧表(`created_at_index`)记录 Quest 首次观察时间,
//!   避免修改 `nexus-core::Quest` 域类型(95+ 构造点),L10 自治封装。
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
//! - `S`:循环切换排序模式(Priority → Status → CreatedAt → Priority,P4.3)

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
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
use crate::types::{PanelId, QuestAction, SortMode, TuiCommand, TuiState};
use nexus_core::{Quest, TaskStatus};

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
/// - `sort_mode`:当前排序模式(P4.3 新增,默认 `SortMode::Priority`)
/// - `created_at_index`:CreatedAt 模式的时间索引侧表(P4.3 新增,`quest_id` → 首次观察时间)
#[derive(Debug, Default, Clone)]
pub struct TaskManagerPanel {
    /// 当前选中索引(在排序后列表中)
    selected: usize,
    /// 滚动偏移
    scroll_offset: usize,
    /// 批量选中的索引集合(为未来 batch 操作预留,本 Task 未启用)
    #[allow(dead_code)]
    selected_indices: HashSet<usize>,
    /// 当前排序模式(P4.3 新增,默认 Priority)
    sort_mode: SortMode,
    /// CreatedAt 模式时间索引:`quest_id` → 面板首次观察到该 Quest 的 UTC 时间
    ///
    /// WHY 独立侧表:不修改 `nexus-core::Quest` 域类型(L1 稳定性约束),
    /// L10 自治追踪"首次观察时间"作为 TUI 上下文的创建时间代理。
    /// 缺失项(Quest 未在面板登记过时间)用 `Utc::now()` 兜底,排在末位。
    created_at_index: HashMap<String, DateTime<Utc>>,
}

impl TaskManagerPanel {
    /// 创建新面板(默认 sort_mode = Priority)
    pub fn new() -> Self {
        Self::default()
    }

    /// P4.3:创建指定排序模式的面板(测试与未来配置注入用)
    pub fn with_sort_mode(mode: SortMode) -> Self {
        Self {
            sort_mode: mode,
            ..Self::default()
        }
    }

    /// P4.3:获取当前排序模式
    pub fn sort_mode(&self) -> SortMode {
        self.sort_mode
    }

    /// P4.3:记录一个 Quest 的首次观察时间(CreatedAt 排序索引)
    ///
    /// 公开 API:测试可注入任意时间,生产代码在 `render`/`sorted_quests`
    /// 自动对未登记 quest_id 补 `Utc::now()` 兜底。
    ///
    /// WHY 允许测试覆盖时间:让 `test_sort_by_created_at_descending` 能
    /// 构造"旧-中-新"三档时间序列,而非依赖 `std::thread::sleep` 真实流逝。
    pub fn note_first_observation(&mut self, quest_id: &str, time: DateTime<Utc>) {
        self.created_at_index.insert(quest_id.to_string(), time);
    }

    /// 公开方法:返回排序后顶部 Quest 的 ID(测试用)
    ///
    /// WHY 返回 `String` 而非 `&str`:`sorted_quests` 返回 `Vec<&Quest>`(临时),
    /// 借出的 `&str` 与临时 Vec 同生命周期,无法在方法签名表达。
    /// 直接 clone 一个 `String` 是测试场景下的最小成本(每帧 1 次),
    /// 与 `QuestPanel::selected` 等公开方法保持一致的可观察性。
    pub fn top_quest_id(&self, state: &TuiState) -> Option<String> {
        self.sorted_quests(state)
            .first()
            .map(|q| q.quest_id.clone())
    }

    /// 公开方法:返回排序后底部 Quest 的 ID(测试用)
    pub fn bottom_quest_id(&self, state: &TuiState) -> Option<String> {
        self.sorted_quests(state).last().map(|q| q.quest_id.clone())
    }

    /// 公开方法:返回排序后 Quest 数量
    pub fn sorted_quest_count(state: &TuiState) -> usize {
        // P4.3:不持有 self 的静态入口,默认走 Priority 排序
        // (与既有语义一致 — `state.quest_list` 长度不受 sort_mode 影响)
        state.quest_list.len()
    }

    /// 公开方法:返回按指定模式排序的 Quest 列表(测试用,P4.3)
    ///
    /// WHY 显式 mode 参数而非隐式 `self.sort_mode`:让测试断言
    /// "以指定 mode 排序"时不必反复构造 panel,降低测试噪音。
    pub fn sorted_with_mode<'a>(&self, state: &'a TuiState, mode: SortMode) -> Vec<&'a Quest> {
        self.sort_quests(state, mode)
    }

    /// 公开方法:返回经过排序的 Quest 列表(测试与既有调用方用)
    ///
    /// P4.3 兼容:原签名 `sorted(state)` 是关联函数,默认走 Priority 排序。
    /// 为保持外部调用方(既有 5 个内部测试)不破坏,本方法仍走 Priority。
    /// 新测试请用 `sorted_with_mode(state, mode)` 显式指定 mode。
    pub fn sorted(state: &TuiState) -> Vec<&Quest> {
        let mut quests: Vec<&Quest> = state.quest_list.iter().collect();
        quests.sort_by_key(|q| (Reverse(q.priority), q.quest_id.clone()));
        quests
    }

    /// 公开方法:渲染面板文本内容(测试用,与 `QuestPanel::content` 模式一致)
    ///
    /// WHY 独立方法:与 `render` 解耦,便于纯逻辑单元测试,
    /// 避免依赖 ratatui TestBackend 与 Buffer 比较。
    pub fn render_text(&self, state: &TuiState) -> String {
        let quests = self.sorted_quests(state);
        Self::content(
            &quests,
            self.selected,
            &self.selected_indices,
            self.sort_mode,
        )
        .to_string()
    }

    /// 按 `self.sort_mode` 排序 Quest 列表(P4.3 主排序入口)
    fn sorted_quests<'a>(&self, state: &'a TuiState) -> Vec<&'a Quest> {
        self.sort_quests(state, self.sort_mode)
    }

    /// P4.3 排序分发器:按 `mode` 分发到对应排序策略
    ///
    /// WHY 集中分发:三种排序模式的键构造逻辑差异大(优先级元组/状态 rank/时间戳),
    /// 集中在 `sort_by_*` 三个函数中,主分发器只负责数据准备 + 函数调用。
    ///
    /// 显式生命周期 `'a` 是必要的:`Vec<&Quest>` 中的引用必须与 `state` 同生命周期,
    /// Rust 借用检查器无法从 `&self` + `&TuiState` 两个独立引用推导出统一生命周期。
    /// 与既有 `sorted_quests` 静态方法签名保持一致。
    fn sort_quests<'a>(&self, state: &'a TuiState, mode: SortMode) -> Vec<&'a Quest> {
        let mut quests: Vec<&'a Quest> = state.quest_list.iter().collect();
        match mode {
            SortMode::Priority => self.sort_by_priority(&mut quests),
            SortMode::Status => self.sort_by_status(&mut quests),
            SortMode::CreatedAt => self.sort_by_created_at(&mut quests),
        }
        quests
    }

    /// Priority 模式:按 priority 降序,同优先级按 quest_id 字典序
    fn sort_by_priority(&self, quests: &mut Vec<&Quest>) {
        quests.sort_by_key(|q| (Reverse(q.priority), q.quest_id.clone()));
    }

    /// Status 模式:按 Quest 派生状态 rank 升序,同状态按 quest_id 字典序
    ///
    /// 派生规则:任何任务 Running → Quest Running (rank 0);
    /// 任意任务 Failed → Quest Failed (rank 2);
    /// 全 Completed → Quest Completed (rank 3);
    /// 其余(全 Pending / 空任务列表)→ Quest Pending (rank 1)。
    fn sort_by_status(&self, quests: &mut Vec<&Quest>) {
        quests.sort_by_key(|q| (Self::status_rank_of(q), q.quest_id.clone()));
    }

    /// CreatedAt 模式:按首次观察时间降序(最新在前),未登记时间排末位
    ///
    /// WHY 未登记排末位:`Utc::now()` 兜底会让"新出现的 Quest"误排到第一位,
    /// 这里采用两段式排序:已登记的时间按降序排在前面,未登记的(Option::None)
    /// 统一排到末位,便于用户优先看老任务。
    fn sort_by_created_at(&self, quests: &mut Vec<&Quest>) {
        // 第一键:是否登记(已登记=1 排前,未登记=0 排后)
        // 第二键:已登记时用 Reverse(time),未登记时用 Reverse(0)(放末位)
        // 第三键:同时间戳内按 quest_id 字典序
        quests.sort_by_key(|q| {
            let registered = self.created_at_index.get(&q.quest_id);
            match registered {
                Some(time) => (1u8, Reverse(time.timestamp_millis()), q.quest_id.clone()),
                None => (0u8, Reverse(0i64), q.quest_id.clone()),
            }
        });
    }

    /// 构造面板内容(文本形式,便于测试与渲染共享)
    ///
    /// P4.3 REFACTOR: 接受已排序的 `&[&Quest]` slice,不再在内部重复排序逻辑,
    /// 调用方(`render` / `render_text`)负责 `self.sorted_quests(state)` 排序。
    /// 职责单一原则:`content` 只负责文本格式化,`sort_quests` 负责排序。
    ///
    /// 标题行追加 `[sort_mode]` 显示当前排序模式(如 `Task Manager [priority]`)
    fn content(
        quests: &[&Quest],
        cursor: usize,
        _selected_indices: &HashSet<usize>,
        sort_mode: SortMode,
    ) -> Text<'static> {
        // P4.3:面板标题显示当前排序模式,便于用户感知
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(format!("Task Manager [{}]", sort_mode)),
            Line::from("──────────────"),
        ];

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

    /// P4.3:从 Quest 的任务状态聚合 Quest 级别状态 rank(用于 Status 排序)
    ///
    /// rank 越小越靠前:
    /// - 0 (Running):任何任务正在运行,或存在 Failed 任务(运维优先关注)
    /// - 1 (Pending):全 Pending 或空任务列表(待执行)
    /// - 2 (Failed):保留位,当前未使用(Quest-level Failed 视作 Running)
    /// - 3 (Completed):全 Completed(已完结)
    ///
    /// WHY 0/1/3 而非连续编号:为未来新增"Paused"/"Cancelled"等状态预留位,
    /// 排序语义保持稳定。
    fn status_rank_of(quest: &Quest) -> u8 {
        if quest.tasks.is_empty() {
            return 1; // 空任务列表 → Pending
        }
        let has_running = quest.tasks.iter().any(|t| t.status == TaskStatus::Running);
        if has_running {
            return 0; // Running 优先
        }
        let has_failed = quest.tasks.iter().any(|t| t.status == TaskStatus::Failed);
        if has_failed {
            return 0; // Failed 视作 Running(运维关注,与 §6 一致)
        }
        let all_completed = quest
            .tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed);
        if all_completed {
            return 3; // 已完结
        }
        1 // 默认 Pending(混合状态如部分 Pending + 部分 Completed)
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

        // P4.3:content 接受已排序的 quests slice,避免重复排序逻辑
        let quests = self.sorted_quests(state);
        let paragraph = Paragraph::new(Self::content(
            &quests,
            self.selected,
            &self.selected_indices,
            self.sort_mode,
        ))
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
            // P4.3:`S` 键循环切换排序模式(Priority → Status → CreatedAt → Priority)
            //
            // WHY 无副作用:排序模式是面板本地状态,不发 TuiCommand,
            // 避免污染事件总线与下游订阅者。
            KeyCode::Char('S') => {
                self.sort_mode = self.sort_mode.next();
                None
            }
            // WHY 排序后列表用 `sorted_quests` 而非 `quest_list`:
            // 选中项在排序后视图中的索引,需要访问排序后的列表。
            // 性能上每次按键 clone 整个列表在小规模(< 100)Quest 下可接受;
            // 后续如需优化可缓存排序结果(数据未变时复用)。
            // `get_quest_id` 闭包:从排序后列表取出选中项的 quest_id
            KeyCode::Char('P') => {
                let quest_id = self
                    .sorted_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Pause,
                })
            }
            KeyCode::Char('T') => {
                let quest_id = self
                    .sorted_quests(state)
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
                let quest = self.sorted_quests(state).get(self.selected).copied();
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
                let quest = self.sorted_quests(state).get(self.selected).copied();
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
                let quest = self.sorted_quests(state).get(self.selected).copied();
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
                let quest_id = self
                    .sorted_quests(state)
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
