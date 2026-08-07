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
    /// M4 二期:渲染内容缓存键(命中时复用 `cached_text`)
    cached_key: Option<QuestRenderKey>,
    /// M4 二期:缓存的渲染文本(命中时仅 clone 供渲染,跳过重建开销)
    cached_text: Text<'static>,
    /// M4 二期:缓存对应时刻的过滤列表长度(命中时免去 O(n) 过滤)
    cached_count: usize,
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

/// Quest 面板渲染缓存键(M4 二期)
///
/// 覆盖所有影响输出文本的状态:
/// - `revision`:快照数据(Quest 列表/任务进度)变化;
/// - `keyword`:关键字过滤器(同时影响标题与列表内容);
/// - `selected` / `selected_indices`:光标与多选(影响前缀符号与样式);
/// - `locale`:全局语言(`Ctrl+L` 切换中英)。
#[derive(Debug, Clone, PartialEq, Eq)]
struct QuestRenderKey {
    revision: u64,
    keyword: Option<String>,
    selected: usize,
    selected_indices: Vec<usize>,
    locale: u8,
}

impl QuestRenderKey {
    /// 以当前状态与面板选择构造缓存键
    fn new(state: &TuiState, selected: usize, selected_indices: &HashSet<usize>) -> Self {
        // 排序保证同一集合的不同插入顺序得到相同键
        let mut indices: Vec<usize> = selected_indices.iter().copied().collect();
        indices.sort_unstable();
        Self {
            revision: state.last_snapshot_revision,
            keyword: state.filter_keyword.clone(),
            selected,
            selected_indices: indices,
            // `as_u8` 为 i18n 模块私有,此处以匹配保持缓存键稳定(仅两种 locale)
            locale: match crate::i18n::current_locale() {
                crate::i18n::Locale::Zh => 0,
                crate::i18n::Locale::En => 1,
            },
        }
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
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.quest.body_title")),
            Line::from("─────────────"),
        ];

        let quests = Self::filtered_quests(state);

        if quests.is_empty() {
            lines.push(Line::from(crate::t!("panel.quest.no_active")));
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
                        "    ID: {} | {}: {:?} | {}: {}",
                        quest.quest_id,
                        crate::t!("panel.quest.mode"),
                        quest.thinking_mode,
                        crate::t!("panel.quest.priority"),
                        quest.priority
                    ),
                    Style::default().fg(Color::Gray),
                )]));

                // 任务摘要行:统计任务总数、已完成数、待处理数
                if quest.tasks.is_empty() {
                    lines.push(Line::from(vec![Span::styled(
                        crate::t!("panel.quest.no_tasks"),
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
                        Span::styled(
                            format!("{} ", crate::t!("panel.quest.tasks_label")),
                            Style::default().fg(Color::Gray),
                        ),
                        Span::from(format!("{} {}", total, crate::t!("panel.quest.total"))),
                        Span::from(", "),
                        Span::styled(
                            format!("{} {}", done, crate::t!("panel.quest.done")),
                            Style::default().fg(Color::Green),
                        ),
                        Span::from(", "),
                        Span::styled(
                            format!("{} {}", running, crate::t!("panel.quest.running")),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::from(", "),
                        Span::styled(
                            format!("{} {}", pending, crate::t!("panel.quest.pending")),
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
            format!("{} {}", crate::t!("panel.quest.detail_title"), quest.title),
            format!("{} {}", crate::t!("panel.quest.detail_id"), quest.quest_id),
            format!(
                "{} {:?}",
                crate::t!("panel.quest.detail_mode"),
                quest.thinking_mode
            ),
            format!(
                "{} {}",
                crate::t!("panel.quest.detail_checkpoint"),
                quest
                    .checkpoint_id
                    .as_deref()
                    .unwrap_or(crate::t!("common.none"))
            ),
            format!(
                "{} {}",
                crate::t!("panel.quest.detail_tasks"),
                quest.tasks.len()
            ),
        ];

        if !quest.tasks.is_empty() {
            lines.push("".into());
            lines.push(crate::t!("panel.quest.detail_task_list").to_string());
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
        // Task 3.10: 新增变体 — Cancelled 用 ⊘(禁止符号),Paused 用 ⏸(暂停符号)
        TaskStatus::Cancelled => "⊘",
        TaskStatus::Paused => "⏸",
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

    /// 返回选中 Quest 的 quest_id(§1.3b,供 quest.* 精确定位)
    ///
    /// 实时读过滤列表,selected 越界经 `.get()` 返回 None 安全。
    fn selected_context_id(&self, state: &TuiState) -> Option<String> {
        Self::filtered_quests(state)
            .get(self.selected)
            .map(|q| q.quest_id.clone())
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.quest"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let title = build_filter_title(state, crate::t!("panel.quest.body_title"));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize;

        // M4 二期:键未变时直接复用缓存(免过滤/格式化/i18n),仅把缓存文本
        // clone 给 Paragraph 写入本帧 buffer;revision == 0(测试桩)禁用缓存,
        // 避免状态被就地修改时读到陈旧内容。
        let mut key = QuestRenderKey::new(state, self.selected, &self.selected_indices);
        let enable_cache = state.last_snapshot_revision != 0;
        let hit = enable_cache && self.cached_key.as_ref() == Some(&key);
        let count = if hit {
            self.cached_count
        } else {
            Self::filtered_quests(state).len()
        };
        self.selected = list_state::clamp_selected(self.selected, count);
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        if !hit {
            key.selected = self.selected;
            self.cached_text = Self::content(state, self.selected, &self.selected_indices);
            self.cached_count = count;
            self.cached_key = if enable_cache { Some(key) } else { None };
        }
        let paragraph =
            Paragraph::new(self.cached_text.clone()).scroll((self.scroll_offset as u16, 0));
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
        if let Some(new_selected) =
            list_state::handle_key_page_navigation(key.code, self.selected, count)
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
                        &format!(
                            "{} {} {}",
                            crate::t!("panel.quest.batch_pause"),
                            quest_ids.len(),
                            crate::t!("panel.quest.quests")
                        ),
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
                        &format!(
                            "{} {} {}",
                            crate::t!("panel.quest.batch_cancel"),
                            quest_ids.len(),
                            crate::t!("panel.quest.quests")
                        ),
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
            // `v` 键打开 Quest 详情弹窗(view/info 首字母,未被全局路由占用)
            //
            // WHY 用 `v` 而非 `i`:`i` 已被 InputRouter Normal 表全局拦截为
            // Insert 模式(router.rs),面板级 `i` arm 不可达——原详情死路径
            // (I-1 高严重度)。`v` 未被全局路由占用,经 FocusPanel 委托可达。
            KeyCode::Char('v') => {
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
            // g/G 双路径:app 交互经 InputRouter 全局拦截(gg→ScrollTop、G→ScrollBottom),
            // 面板直接 API(测试/嵌入调用)仍保留同名 arm,语义一致。
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
        // Task 2.5:QuestPanel 手写 UI 键位(导航/详情/跳顶/跳底)保留为面板内交互提示。
        // 功能动作快捷键(agent.chat / quest.pause 等)经 `shortcuts_with_registry`
        // 从 ActionRegistry::by_domain(Quest) 自动派生,新增 quest.* 动作零手写接入。
        vec![
            ("↑/↓", "导航"),
            ("PgUp/PgDn", "翻页"),
            ("Enter", "跳转事件流"),
            ("v", "详情"),
            ("g g", "跳顶"),
            ("G", "跳底"),
        ]
    }

    fn action_domain(&self) -> Option<crate::actions::ActionDomain> {
        // 声明 Quest 域:shortcuts_with_registry 默认实现会合并本域 action 的快捷键
        Some(crate::actions::ActionDomain::Quest)
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
    use proptest::prelude::*;

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
        assert!(content.contains("任务列表"));
        assert!(content.contains("暂无进行中的任务"));
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
        // I-1 修复:detail popup 键从 `i` 改绑 `v`(`i` 被 InputRouter 拦截为 Insert)
        let mut panel = QuestPanel::new();
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q1", "Detail Quest")];

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('v'), crossterm::event::KeyModifiers::NONE),
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
    fn selected_context_id_returns_selected_quest_id() {
        // §1.3b:selected_context_id 供 quest.* 精确定位
        let panel = QuestPanel::new();
        let mut state = TuiState::new();
        // 空列表:无选中上下文
        assert_eq!(panel.selected_context_id(&state), None);
        // 多 Quest:默认 selected=0 → 首个 quest_id
        state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
        assert_eq!(panel.selected_context_id(&state).as_deref(), Some("q1"));
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

    /// M4 二期:相同键(数据/选中/过滤/语言均未变)连续渲染应复用缓存文本。
    ///
    /// 通过把缓存文本替换为哨兵串验证:若渲染仍输出哨兵,说明确实复用了
    /// 上次构建结果而非每帧重建。
    #[test]
    fn quest_render_reuses_cached_text_when_key_unchanged() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.quest_list = vec![sample_quest("q1", "Cached Quest")];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        // 篡改缓存:若下一次渲染命中缓存,输出应为哨兵串
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            rendered.contains("SENTINEL-CACHE-HIT"),
            "缓存未命中: 内容被重建(rendered={rendered})"
        );
    }

    /// M4 二期:快照 revision 变化(数据更新)必须使缓存失效。
    #[test]
    fn quest_render_invalidates_on_revision_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.quest_list = vec![sample_quest("q1", "Cached Quest")];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        state.last_snapshot_revision = 2;
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT") && rendered.contains("Cached Quest"),
            "revision 变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:选中索引变化(光标移动)必须使缓存失效。
    #[test]
    fn quest_render_invalidates_on_selection_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.quest_list = vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
        ];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT") && rendered.contains("Second Quest"),
            "选中变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:关键字过滤器变化必须使缓存失效。
    #[test]
    fn quest_render_invalidates_on_filter_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.quest_list = vec![
            sample_quest("q1", "Alpha Quest"),
            sample_quest("q2", "Beta Quest"),
        ];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        state.filter_keyword = Some("alpha".into());
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT")
                && rendered.contains("Alpha Quest")
                && !rendered.contains("Beta Quest"),
            "过滤器变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:全局语言切换必须使缓存失效。
    #[test]
    fn quest_render_invalidates_on_locale_change() {
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.quest_list = vec![sample_quest("q1", "Cached Quest")];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        // 不切换全局 locale(避免与未加锁的既有测试并行竞态),而是直接篡改
        // 缓存键的 locale 分量模拟"另一语言下构建的缓存"。
        if let Some(mut simulated) = panel.cached_key.take() {
            simulated.locale ^= 1;
            panel.cached_key = Some(simulated);
        }
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT"),
            "语言切换未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:revision == 0(测试桩/未同步)时禁用缓存,避免就地修改状态读到陈旧内容。
    #[test]
    fn quest_render_disables_cache_at_revision_zero() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.quest_list = vec![sample_quest("q1", "Cached Quest")];
        let mut panel = QuestPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT"),
            "revision==0 时不应使用缓存(rendered={rendered})"
        );
    }

    // M4 二期属性测试:缓存键与多选集合的迭代顺序无关,且对选中项敏感。
    // WHY locale guard:QuestRenderKey 含全局 locale 字段,并行测试切换语言会
    // 在 key_a/key_b 构造之间改变 locale 导致断言偶发失败(flaky,2026-08-07)。
    proptest! {
        #[test]
        fn quest_render_key_ignores_hashset_iteration_order(
            indices in proptest::collection::vec(0usize..50, 0..20),
        ) {
            let _locale_guard = crate::i18n::locale_test_guard();
            let mut state = TuiState::new();
            state.last_snapshot_revision = 7;
            state.filter_keyword = Some("kw".into());

            let forward: HashSet<usize> = indices.iter().copied().collect();
            let mut reversed = indices.clone();
            reversed.reverse();
            let backward: HashSet<usize> = reversed.into_iter().collect();

            let key_a = QuestRenderKey::new(&state, 3, &forward);
            let key_b = QuestRenderKey::new(&state, 3, &backward);
            prop_assert_eq!(&key_a, &key_b, "同一集合的不同迭代顺序应得到相同缓存键");

            let key_other_selected = QuestRenderKey::new(&state, 4, &forward);
            prop_assert_ne!(
                key_a, key_other_selected,
                "选中索引变化必须改变缓存键"
            );
        }
    }
}
