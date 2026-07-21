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
//! - `P`:暂停(单选,走 `TuiCommand::QuestControl { Pause }`)
//! - `B`:批量暂停(多选时批量弹窗确认;无多选时回退单选暂停)
//! - `R`:恢复(多选时批量弹窗确认;无多选时回退单选恢复)
//! - `T`:终止(多选时批量弹窗确认;无多选时回退单选终止)
//! - `+` / `=`:优先级 +1(上限 10)
//! - `-`:优先级 -1(下限 0)
//! - `Enter`:查看详情(沿用既有 OpenPopup 模式)
//! - `/`:关键字过滤(沿用 `state.filter_keyword`)
//! - `↑` / `↓`:导航
//! - `S`:循环切换排序模式(Priority → Status → CreatedAt → Priority,P4.3)
//! - `Space`:切换当前项的多选状态
//! - `Ctrl+A`:全选当前过滤列表中所有 Quest
//! - `Esc`:清空所有多选(测试场景;生产环境 Esc 由全局拦截为 quit)

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
    /// 批量选中的索引集合(多选模式,Space 切换/Ctrl+A 全选/Esc 清除)
    selected_indices: HashSet<usize>,
    /// 当前排序模式(P4.3 新增,默认 Priority)
    sort_mode: SortMode,
    /// CreatedAt 模式时间索引:`quest_id` → 面板首次观察到该 Quest 的 UTC 时间
    ///
    /// WHY 独立侧表:不修改 `nexus-core::Quest` 域类型(L1 稳定性约束),
    /// L10 自治追踪"首次观察时间"作为 TUI 上下文的创建时间代理。
    /// 缺失项(Quest 未在面板登记过时间)用 `Utc::now()` 兜底,排在末位。
    created_at_index: HashMap<String, DateTime<Utc>>,
    /// 过滤关键字(实时搜索)
    ///
    /// WHY 面板本地状态:过滤是视图层概念,与 `TuiState.filter_keyword`
    /// (Log/Quest 面板全局过滤)职责不同,本字段仅影响 TaskManagerPanel 的
    /// Quest 列表渲染,不污染全局状态。
    filter_keyword: String,
    /// 是否处于搜索模式
    ///
    /// WHY 独立标志:搜索模式下抑制所有非搜索键(P/R/T/+/-/S/Space/Ctrl+A),
    /// 避免误触发 Quest 控制操作。
    is_searching: bool,
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
        let quests = self.filtered_quests(state);
        self.content(
            state,
            &quests,
            self.selected,
            &self.selected_indices,
            self.sort_mode,
            self.selected_indices.len(),
        )
        .to_string()
    }

    /// 按 `self.sort_mode` 排序 Quest 列表(P4.3 主排序入口)
    fn sorted_quests<'a>(&self, state: &'a TuiState) -> Vec<&'a Quest> {
        self.sort_quests(state, self.sort_mode)
    }

    /// 返回按过滤关键字筛选后的 Quest 列表
    ///
    /// WHY 独立方法:排序 + 过滤是两个正交维度,先排序再过滤保证
    /// 过滤后的列表仍保持排序顺序,且过滤逻辑集中在一处,便于测试。
    /// 大小写不敏感匹配 quest_id 和 title。
    fn filtered_quests<'a>(&self, state: &'a TuiState) -> Vec<&'a Quest> {
        let quests = self.sorted_quests(state);
        if self.filter_keyword.is_empty() {
            return quests;
        }
        let keyword = self.filter_keyword.to_lowercase();
        quests
            .into_iter()
            .filter(|q| {
                q.quest_id.to_lowercase().contains(&keyword)
                    || q.title.to_lowercase().contains(&keyword)
            })
            .collect()
    }

    /// 统计 Quest 列表中各状态的数量
    ///
    /// WHY 关联函数(不依赖 self):统计逻辑仅依赖 `TuiState` 的
    /// `quest_list` 和 `paused_quest_count`,与面板本地状态无关,
    /// 保持纯函数语义便于测试。
    ///
    /// 返回 (pending, running, paused, completed)
    fn compute_status_counts(state: &TuiState) -> (usize, usize, usize, usize) {
        let mut pending = 0usize;
        let mut running = 0usize;
        let mut completed = 0usize;
        for quest in &state.quest_list {
            if quest.tasks.is_empty() {
                pending += 1;
                continue;
            }
            let has_running = quest.tasks.iter().any(|t| t.status == TaskStatus::Running);
            let has_failed = quest.tasks.iter().any(|t| t.status == TaskStatus::Failed);
            let all_completed = quest
                .tasks
                .iter()
                .all(|t| t.status == TaskStatus::Completed);
            if has_running || has_failed {
                running += 1;
            } else if all_completed {
                completed += 1;
            } else {
                pending += 1;
            }
        }
        (pending, running, state.paused_quest_count, completed)
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
    /// 标题行:普通模式 `Task Manager [sort_mode]`,搜索模式追加 `(filter: keyword)`
    ///
    /// 底部统计行:显示 Pending/Running/Paused/Completed 计数 + 批量选中数。
    fn content(
        &self,
        state: &TuiState,
        quests: &[&Quest],
        cursor: usize,
        selected_indices: &HashSet<usize>,
        sort_mode: SortMode,
        selected_count: usize,
    ) -> Text<'static> {
        // P4.3:面板标题显示当前排序模式;搜索模式下追加过滤关键字
        let title = if self.is_searching {
            format!(
                "Task Manager [{}] (filter: {})",
                sort_mode, self.filter_keyword
            )
        } else {
            format!("Task Manager [{}]", sort_mode)
        };
        let mut lines: Vec<Line<'static>> = vec![Line::from(title), Line::from("──────────────")];

        if quests.is_empty() {
            // WHY "No matching quests":过滤后列表为空说明关键字无匹配,
            // 与"没有任何 Quest"的语义不同,提示用户调整过滤关键字。
            lines.push(Line::from("No matching quests"));
        } else {
            for (idx, quest) in quests.iter().enumerate() {
                let is_cursor = idx == cursor;
                let is_selected = selected_indices.contains(&idx);
                // 多选视觉:选中行用 [*] 前缀 + 青色背景,光标行用 > 前缀 + 黄色背景
                let (prefix, style) = match (is_cursor, is_selected) {
                    (true, true) => (
                        "[*]",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    (true, false) => (
                        "> ",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    (false, true) => (
                        "[*]",
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    (false, false) => ("  ", Style::default()),
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
        // 状态统计行:显示各状态 Quest 计数 + 批量选中数
        let (pending, running, paused, completed) = Self::compute_status_counts(state);
        let mut status_line = format!(
            "Pending:{} | Running:{} | Paused:{} | Completed:{}",
            pending, running, paused, completed
        );
        if selected_count > 0 {
            status_line.push_str(&format!(" | Selected:{}", selected_count));
        }
        lines.push(Line::from(status_line));
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
        let quests = self.filtered_quests(state);
        let count = quests.len();
        self.selected = list_state::clamp_selected(self.selected, count);

        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let selected_count = self.selected_indices.len();
        let paragraph = Paragraph::new(self.content(
            state,
            &quests,
            self.selected,
            &self.selected_indices,
            self.sort_mode,
            selected_count,
        ))
        .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        // 搜索模式:仅处理搜索专用键,抑制所有其他键以避免误触 Quest 控制
        if self.is_searching {
            match key.code {
                // Esc 退出搜索模式并清除关键字
                KeyCode::Esc => {
                    self.is_searching = false;
                    self.filter_keyword.clear();
                    return None;
                }
                // Enter 退出搜索模式但保留关键字
                KeyCode::Enter => {
                    self.is_searching = false;
                    return None;
                }
                // Backspace 删除最后一个字符
                KeyCode::Backspace => {
                    self.filter_keyword.pop();
                    return None;
                }
                // 普通字符追加到过滤关键字
                KeyCode::Char(c) => {
                    self.filter_keyword.push(c);
                    return None;
                }
                // 搜索模式下抑制所有其他键
                _ => return None,
            }
        }

        // Up/Down 导航(复用 list_state 工具,使用过滤后列表长度)
        let filtered_count = self.filtered_quests(state).len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, filtered_count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            // Space 键:切换当前高亮项的多选状态
            //
            // WHY Space:与文件管理器/邮件客户端的多选语义一致,
            // 操作员直觉式操作无需学习。不影响单行光标导航。
            KeyCode::Char(' ') => {
                let quests = self.filtered_quests(state);
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
                let quests = self.filtered_quests(state);
                for i in 0..quests.len() {
                    self.selected_indices.insert(i);
                }
                None
            }
            // Esc:清空所有批量选择,退出多选模式
            //
            // 注意:生产环境中 Esc 由 app.rs 全局拦截为 quit,
            // 面板级 Esc 仅在测试场景中生效。
            KeyCode::Esc => {
                self.selected_indices.clear();
                None
            }
            // `/` 键:进入搜索模式,不清除已有关键字(支持增量搜索)
            //
            // WHY 不清除关键字:用户可能在过滤后调整排序模式,
            // 然后按 `/` 继续在同一关键字上追加搜索,清除会丢失上下文。
            KeyCode::Char('/') => {
                self.is_searching = true;
                None
            }
            // P4.3:`S` 键循环切换排序模式(Priority → Status → CreatedAt → Priority)
            //
            // WHY 无副作用:排序模式是面板本地状态,不发 TuiCommand,
            // 避免污染事件总线与下游订阅者。
            KeyCode::Char('S') => {
                self.sort_mode = self.sort_mode.next();
                None
            }
            // P 键:单选暂停(始终走确认弹窗)
            KeyCode::Char('P') => {
                let quest_id = self
                    .filtered_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Pause,
                })
            }
            // B 键:批量暂停(多选时批量;无多选时回退单选暂停,与 P 键一致)
            //
            // WHY B 键独立:操作员多选后按 B 批量暂停,无多选时 B 退化为单选暂停,
            // 与 P 键语义一致,互不冲突。
            KeyCode::Char('B') => {
                if !self.selected_indices.is_empty() {
                    let quests = self.filtered_quests(state);
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
                // 无多选时回退单选暂停
                let quest_id = self
                    .filtered_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Pause,
                })
            }
            // T 键:终止 Quest(多选时批量;无多选时回退单选终止)
            KeyCode::Char('T') => {
                if !self.selected_indices.is_empty() {
                    let quests = self.filtered_quests(state);
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
                        &format!("Batch terminate {} quests", quest_ids.len()),
                        &batch_ids,
                        format!("batch_terminate:{batch_ids}"),
                    )));
                }
                // 无多选时回退单选终止
                let quest_id = self
                    .filtered_quests(state)
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
            KeyCode::Char('+') => {
                let quest = self.filtered_quests(state).get(self.selected).copied();
                quest.and_then(|q| {
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
            KeyCode::Char('-') => {
                let quest = self.filtered_quests(state).get(self.selected).copied();
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
                let quest = self.filtered_quests(state).get(self.selected).copied();
                quest.map(|q| {
                    TuiCommand::OpenPopup(PopupKind::Detail {
                        title: format!("Task: {}", q.title),
                        content: Self::detail_content(q),
                        scroll: 0,
                    })
                })
            }
            // C 键:创建 Quest 预留(本期返回 None)
            KeyCode::Char('C') => None,
            // R 键:恢复 Quest(多选时批量;无多选时回退单选恢复)
            KeyCode::Char('R') => {
                if !self.selected_indices.is_empty() {
                    let quests = self.filtered_quests(state);
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
                        &format!("Batch resume {} quests", quest_ids.len()),
                        &batch_ids,
                        format!("batch_resume:{batch_ids}"),
                    )));
                }
                // 无多选时回退单选恢复
                let quest_id = self
                    .filtered_quests(state)
                    .get(self.selected)
                    .map(|q| q.quest_id.clone());
                quest_id.map(|id| TuiCommand::QuestControl {
                    id,
                    action: QuestAction::Resume,
                })
            }
            // E: 导出任务数据
            KeyCode::Char('E') => Some(TuiCommand::Export),
            // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let count = self.filtered_quests(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = self.filtered_quests(state).len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        // consume Ctrl modifier 警告(避免未使用 import)
        let _ = KeyModifiers::NONE;
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑/↓", "导航"),
            ("/", "过滤搜索"),
            ("P", "暂停"),
            ("B", "批量暂停"),
            ("R", "恢复"),
            ("T", "终止"),
            ("+/-", "优先级"),
            ("S", "排序"),
            ("E", "导出"),
            ("Enter", "详情"),
            ("Esc", "清除选择"),
            ("Space", "多选"),
            ("Ctrl+A", "全选"),
        ]
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

    // ── 辅助函数 ──

    /// 创建带自定义任务列表的 Quest（测试用）
    fn quest_with_tasks(id: &str, title: &str, priority: u8, tasks: Vec<Task>) -> Quest {
        Quest {
            quest_id: id.into(),
            title: title.into(),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority,
        }
    }

    /// 创建单个 Task（测试用）
    fn make_task(id: &str, status: TaskStatus) -> Task {
        Task {
            task_id: id.into(),
            description: "test task".into(),
            status,
            dependencies: vec![],
        }
    }

    // ═══════════════════════════════════════════════════════════
    // 多选模式测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_space_toggles_selection() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];

        // 第一次 Space:选中索引 0
        panel.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut state,
        );
        assert!(panel.selected_indices.contains(&0), "Space 应选中当前索引");
        assert_eq!(panel.selected_indices.len(), 1);

        // 第二次 Space:取消选中索引 0
        panel.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut state,
        );
        assert!(
            !panel.selected_indices.contains(&0),
            "再次 Space 应取消选中"
        );
        assert!(panel.selected_indices.is_empty());
    }

    #[test]
    fn test_ctrl_a_selects_all() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        // 3 个 Quest:关键字 "a" 匹配 quest_id 含 "a" 的 2 个
        state.quest_list = vec![
            sample_quest("q-apple", "Apple", 5),
            sample_quest("q-banana", "Banana", 5),
            sample_quest("q-cherry", "Cherry", 5),
        ];
        // 过滤:quest_id 含 "a" → q-apple、q-banana（q-cherry 不含）
        panel.filter_keyword = "a".to_string();

        panel.handle_key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &mut state,
        );

        assert_eq!(
            panel.selected_indices.len(),
            2,
            "Ctrl+A 应全选过滤后的 2 个 Quest"
        );
        assert!(panel.selected_indices.contains(&0));
        assert!(panel.selected_indices.contains(&1));
    }

    #[test]
    fn test_esc_clears_selection() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        panel.selected_indices.insert(0);
        panel.selected_indices.insert(2);
        panel.selected_indices.insert(5);
        assert_eq!(panel.selected_indices.len(), 3);

        panel.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);

        assert!(panel.selected_indices.is_empty(), "Esc 应清空所有选中");
    }

    // ═══════════════════════════════════════════════════════════
    // 批量操作测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_batch_pause_generates_confirm_popup() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];

        // 先选中第 1 个 Quest
        panel.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut state,
        );

        // 按 B 触发批量暂停
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Confirm {
                prompt, on_confirm, ..
            })) => {
                assert!(
                    prompt.contains("Batch pause"),
                    "弹窗提示应包含 'Batch pause': {prompt}"
                );
                assert!(
                    on_confirm.contains("batch_pause:"),
                    "on_confirm 应包含 'batch_pause:': {on_confirm}"
                );
            }
            other => panic!("批量暂停应生成 OpenPopup(Confirm), 实际: {other:?}"),
        }
    }

    #[test]
    fn test_batch_terminate_generates_confirm_popup() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];

        // 先选中第 1 个 Quest
        panel.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut state,
        );

        // 按 T 触发批量终止
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Confirm {
                prompt, on_confirm, ..
            })) => {
                assert!(
                    prompt.contains("Batch terminate"),
                    "弹窗提示应包含 'Batch terminate': {prompt}"
                );
                assert!(
                    on_confirm.contains("batch_terminate:"),
                    "on_confirm 应包含 'batch_terminate:': {on_confirm}"
                );
            }
            other => panic!("批量终止应生成 OpenPopup(Confirm), 实际: {other:?}"),
        }
    }

    #[test]
    fn test_empty_selection_batch_does_nothing() {
        // 空选中时 B 键回退为单选暂停，不生成批量弹窗
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-1", "Alpha", 5)];

        // selected_indices 为空，按 B 应回退单选暂停
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::QuestControl { id, action }) => {
                assert_eq!(id, "q-1");
                assert_eq!(action, QuestAction::Pause);
            }
            Some(TuiCommand::OpenPopup(_)) => {
                panic!("空选中时不应生成批量弹窗, 应回退单选暂停");
            }
            other => panic!("空选中按 B 应回退单选暂停, 实际: {other:?}"),
        }
    }

    // ═══════════════════════════════════════════════════════════
    // 过滤搜索测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_filter_search_matches_quest_id() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("alpha", "A", 5),
            sample_quest("beta", "B", 3),
            sample_quest("gamma", "C", 4),
        ];

        panel.filter_keyword = "bet".to_string();
        let filtered = panel.filtered_quests(&state);

        assert_eq!(filtered.len(), 1, "关键字 'bet' 应只匹配 'beta'");
        assert_eq!(filtered[0].quest_id, "beta");
    }

    #[test]
    fn test_filter_search_case_insensitive() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("alpha", "A", 5), sample_quest("beta", "B", 3)];

        panel.filter_keyword = "ALPHA".to_string();
        let filtered = panel.filtered_quests(&state);

        assert_eq!(filtered.len(), 1, "大小写不敏感: 'ALPHA' 应匹配 'alpha'");
        assert_eq!(filtered[0].quest_id, "alpha");
    }

    #[test]
    fn test_filter_search_no_match() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("alpha", "A", 5), sample_quest("beta", "B", 3)];

        panel.filter_keyword = "xyz".to_string();
        let filtered = panel.filtered_quests(&state);

        assert!(filtered.is_empty(), "关键字 'xyz' 不应匹配任何 Quest");
    }

    #[test]
    fn test_filter_search_matches_title() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Deploy Production", 5),
            sample_quest("q-2", "Run Tests", 3),
            sample_quest("q-3", "Deploy Backup", 4),
        ];

        panel.filter_keyword = "Deploy".to_string();
        let filtered = panel.filtered_quests(&state);

        assert_eq!(
            filtered.len(),
            2,
            "关键字 'Deploy' 应匹配 title 含 'Deploy' 的 2 个 Quest"
        );
        let ids: Vec<&str> = filtered.iter().map(|q| q.quest_id.as_str()).collect();
        assert!(ids.contains(&"q-1"));
        assert!(ids.contains(&"q-3"));
    }

    // ═══════════════════════════════════════════════════════════
    // 状态统计测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_status_counts_correct() {
        let mut state = TuiState::new();
        state.quest_list = vec![
            // Pending:空任务列表
            Quest {
                quest_id: "q-pending-empty".into(),
                title: "PE".into(),
                tasks: vec![],
                thinking_mode: ThinkingMode::Standard,
                checkpoint_id: None,
                priority: 5,
            },
            // Running:有 Running 任务
            quest_with_tasks(
                "q-running",
                "R",
                5,
                vec![
                    make_task("t1", TaskStatus::Running),
                    make_task("t2", TaskStatus::Pending),
                ],
            ),
            // Completed:全 Completed
            quest_with_tasks(
                "q-completed",
                "C",
                5,
                vec![
                    make_task("t1", TaskStatus::Completed),
                    make_task("t2", TaskStatus::Completed),
                ],
            ),
            // Pending:有 Pending 任务
            sample_quest("q-pending", "P", 5),
        ];

        let (pending, running, paused, completed) = TaskManagerPanel::compute_status_counts(&state);

        assert_eq!(pending, 2, "2 个 Pending(q-pending-empty + q-pending)");
        assert_eq!(running, 1, "1 个 Running(q-running)");
        assert_eq!(paused, 0, "paused_quest_count 默认为 0");
        assert_eq!(completed, 1, "1 个 Completed(q-completed)");
    }

    #[test]
    fn test_status_counts_with_paused() {
        let mut state = TuiState::new();
        state.paused_quest_count = 3;

        let (pending, running, paused, completed) = TaskManagerPanel::compute_status_counts(&state);

        assert_eq!(pending, 0);
        assert_eq!(running, 0);
        assert_eq!(paused, 3, "paused 字段应反映 state.paused_quest_count");
        assert_eq!(completed, 0);
    }

    // ═══════════════════════════════════════════════════════════
    // 多选视觉前缀测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_multiselect_visual_prefix() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];
        // 选中索引 0,验证渲染输出包含 [*] 前缀
        panel.selected_indices.insert(0);
        let text = panel.render_text(&state);
        assert!(text.contains("[*]"), "选中行应渲染 [*] 前缀:\n{text}");
        // 验证未选中行不含 [*]
        let star_count = text.matches("[*]").count();
        assert_eq!(star_count, 1, "应只有 1 个 [*] 前缀(选中 1 项)");
    }

    // ═══════════════════════════════════════════════════════════
    // 批量操作补充测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_batch_resume_generates_confirm_popup() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];

        // 先选中第 1 个 Quest
        panel.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut state,
        );

        // 按 R 触发批量恢复
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Confirm {
                prompt, on_confirm, ..
            })) => {
                assert!(
                    prompt.contains("Batch resume"),
                    "弹窗提示应包含 'Batch resume': {prompt}"
                );
                assert!(
                    on_confirm.contains("batch_resume:"),
                    "on_confirm 应包含 'batch_resume:': {on_confirm}"
                );
            }
            other => panic!("批量恢复应生成 OpenPopup(Confirm), 实际: {other:?}"),
        }
    }

    #[test]
    fn test_empty_selection_r_falls_back_to_single_resume() {
        // 空选中时 R 键回退为单选恢复,不生成批量弹窗
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-1", "Alpha", 5)];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::QuestControl { id, action }) => {
                assert_eq!(id, "q-1");
                assert_eq!(action, QuestAction::Resume);
            }
            Some(TuiCommand::OpenPopup(_)) => {
                panic!("空选中时不应生成批量弹窗, 应回退单选恢复");
            }
            other => panic!("空选中按 R 应回退单选恢复, 实际: {other:?}"),
        }
    }

    #[test]
    fn test_empty_selection_t_falls_back_to_single_terminate() {
        // 空选中时 T 键回退为单选终止,不生成批量弹窗
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q-1", "Alpha", 5)];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
            &mut state,
        );

        match cmd {
            Some(TuiCommand::QuestControl { id, action }) => {
                assert_eq!(id, "q-1");
                assert_eq!(action, QuestAction::Terminate);
            }
            Some(TuiCommand::OpenPopup(_)) => {
                panic!("空选中时不应生成批量弹窗, 应回退单选终止");
            }
            other => panic!("空选中按 T 应回退单选终止, 实际: {other:?}"),
        }
    }

    // ═══════════════════════════════════════════════════════════
    // 过滤搜索补充测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_filter_search_slash_enters_mode() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &mut state,
        );

        assert!(cmd.is_none(), "/ 键不应返回命令");
        assert!(panel.is_searching, "/ 键应进入搜索模式");
    }

    #[test]
    fn test_filter_search_backspace() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        // 模拟已进入搜索模式并输入了关键字
        panel.is_searching = true;
        panel.filter_keyword = "test".to_string();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut state,
        );

        assert!(cmd.is_none());
        assert_eq!(panel.filter_keyword, "tes", "Backspace 应删除最后一个字符");
    }

    #[test]
    fn test_filter_search_esc_clears() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        // 模拟已进入搜索模式并输入了关键字
        panel.is_searching = true;
        panel.filter_keyword = "test".to_string();

        let cmd = panel.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);

        assert!(cmd.is_none());
        assert!(!panel.is_searching, "Esc 应退出搜索模式");
        assert!(panel.filter_keyword.is_empty(), "Esc 应清除过滤关键字");
    }

    #[test]
    fn test_filter_search_enter_exits_mode_keeps_keyword() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        // 模拟已进入搜索模式并输入了关键字
        panel.is_searching = true;
        panel.filter_keyword = "alpha".to_string();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
        );

        assert!(cmd.is_none());
        assert!(!panel.is_searching, "Enter 应退出搜索模式");
        assert_eq!(panel.filter_keyword, "alpha", "Enter 应保留过滤关键字");
    }

    #[test]
    fn test_filter_search_char_append() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        panel.is_searching = true;
        panel.filter_keyword = "ab".to_string();

        panel.handle_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
            &mut state,
        );

        assert_eq!(panel.filter_keyword, "abc", "搜索模式下字符应追加到关键字");
    }

    // ═══════════════════════════════════════════════════════════
    // 状态统计补充测试
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn test_status_counts_empty() {
        let state = TuiState::new();
        let (pending, running, paused, completed) = TaskManagerPanel::compute_status_counts(&state);

        assert_eq!(pending, 0, "空列表 pending 应为 0");
        assert_eq!(running, 0, "空列表 running 应为 0");
        assert_eq!(paused, 0, "空列表 paused 应为 0");
        assert_eq!(completed, 0, "空列表 completed 应为 0");
    }

    #[test]
    fn test_status_counts_selected_display() {
        let mut panel = TaskManagerPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![
            sample_quest("q-1", "Alpha", 5),
            sample_quest("q-2", "Beta", 3),
        ];

        // 无选中时不显示 Selected
        let text_no_selection = panel.render_text(&state);
        assert!(
            !text_no_selection.contains("Selected:"),
            "无选中时不应显示 Selected"
        );

        // 选中 1 项
        panel.selected_indices.insert(0);
        let text_one = panel.render_text(&state);
        assert!(
            text_one.contains("Selected:1"),
            "选中 1 项时统计行应显示 'Selected:1':\n{text_one}"
        );

        // 选中 2 项
        panel.selected_indices.insert(1);
        let text_two = panel.render_text(&state);
        assert!(
            text_two.contains("Selected:2"),
            "选中 2 项时统计行应显示 'Selected:2':\n{text_two}"
        );
    }
}
