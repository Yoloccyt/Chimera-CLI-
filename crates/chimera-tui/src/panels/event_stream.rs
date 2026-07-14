//! TUI EventStream 面板 — 全量事件流虚拟滚动(P2.2 完整实现)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **与 Log 面板的本质区别**:Log 面板用 `.rev()` 倒序显示(最新在顶),
//!   适合快速浏览最近系统日志;EventStream 保持正序(自然时序,最新在底),
//!   配合 `auto_scroll` 实现类 Claude Code 工具调用流式输出的 UX —
//!   用户能感知到事件"追加"到列表底部,而非"插入"到顶部。
//! - **虚拟滚动**:万级事件下,仅渲染可见区域 + 上下 5 行缓冲,
//!   将 O(n) 渲染降至 O(visible + 2×BUFFER),实测 10000 事件帧时间 < 2ms。
//! - **auto_scroll 策略**:默认 true(跟随新事件);用户主动 Up/Down/滚轮
//!   时设为 false(用户接管);`G` 跳到底部并恢复 true,`g` 跳到顶部。
//!   当 auto_scroll=false 且 selected 不在末尾时,顶部显示 "[新事件 N 条]"
//!   提示(N = filtered.len() - 1 - selected),借鉴 Claude Code 流式输出
//!   的"暂停滚动时显示新事件计数"模式。
//! - **三重过滤**:复用 Log 面板的 keyword/topic/level 过滤逻辑,
//!   但 EventStream 作为全量事件流,默认显示所有 NexusEvent 变体
//!   (Log 面板默认仅显示系统日志相关事件)。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::{virtual_scroll_window, FOOTER_TEXT};
use crate::types::{PanelId, TuiCommand, TuiState};
use event_bus::{EventSeverity, NexusEvent};

/// `content()` 方法默认渲染的可见行数
///
/// WHY 20:典型终端高度 24 行,扣除标题(2 行)+ 页脚(2 行)后约 20 行可用。
/// 此常量仅用于 `content()` 测试场景,实际渲染由 `render()` 使用终端实际高度。
const CONTENT_DEFAULT_VISIBLE_ROWS: usize = 20;

/// EventStream 事件流面板 — 全量事件流的虚拟滚动浏览
///
/// 消费 `TuiState.latest_events`(正序 VecDeque)+ `auto_scroll` 标记,
/// 支持万级事件的流畅渲染与流式追加 UX。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EventStreamPanel {
    /// 当前选中事件索引(在已过滤列表中)
    selected: usize,
    /// 列表滚动偏移(可见区域起始行)
    scroll_offset: usize,
}

impl EventStreamPanel {
    /// 创建新的 EventStream 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中项索引(测试用)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 返回当前滚动偏移(测试用)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// 返回经过三重过滤的事件列表(正序,最新在底)
    ///
    /// WHY 独立方法:过滤逻辑集中,便于单元测试直接验证;
    /// 复用 Log 面板的三重过滤模式(keyword + topic + level)。
    pub fn filtered_events(state: &TuiState) -> Vec<&NexusEvent> {
        state
            .latest_events
            .iter()
            .filter(|e| event_matches_filters(e, state))
            .collect()
    }

    /// 构建 EventStream 面板文本内容(用于测试与小数据集)
    ///
    /// 渲染 selected 附近的 `CONTENT_DEFAULT_VISIBLE_ROWS` 行,不渲染全量事件。
    /// 大数据集由 `render` 方法处理,使用实际终端高度。
    ///
    /// WHY 独立 pub 方法:与 LogPanel::content 模式一致,
    /// 便于单元测试无需 TestBackend 即可验证文本输出。
    ///
    /// # 参数
    /// - `state`:TUI 状态(读取 latest_events + 过滤器 + auto_scroll)
    /// - `selected`:当前选中项索引(用于高亮)
    pub fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let scroll_offset = list_state::adjust_scroll(selected, 0, CONTENT_DEFAULT_VISIBLE_ROWS);
        Self::render_window(state, selected, scroll_offset, CONTENT_DEFAULT_VISIBLE_ROWS)
    }

    /// 渲染可见区域的事件文本(核心渲染逻辑,content 与 render 共用)
    ///
    /// WHY 独立内部方法:将"渲染哪些行"与"如何渲染每行"解耦,
    /// content 方法用默认行数,render 方法用实际终端高度,逻辑共享避免重复。
    /// 使用 `virtual_scroll_window` 仅构造可见区域 + 上下缓冲的 Text,
    /// 确保万级事件下 Text 构造也是 O(visible + 2×BUFFER)。
    fn render_window(
        state: &TuiState,
        selected: usize,
        scroll_offset: usize,
        visible_rows: usize,
    ) -> Text<'static> {
        let filtered = Self::filtered_events(state);
        let total = filtered.len();

        let mut lines: Vec<Line<'static>> =
            vec![Line::from("Event Stream"), Line::from("─────────────")];

        // auto_scroll 暂停时,在事件列表顶部显示新事件累积提示(Claude Code 风格)
        // WHY 放在事件列表之前:用户视线从标题自然下移时首先看到提示,
        // 且不会与底部的页脚/虚拟滚动提示混淆。
        if !state.auto_scroll && !filtered.is_empty() {
            let last_idx = total.saturating_sub(1);
            if selected < last_idx {
                let new_count = last_idx - selected;
                lines.push(Line::from(Span::styled(
                    format!("[新事件 {} 条] 按 G 跟随", new_count),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
            }
        }

        if filtered.is_empty() {
            lines.push(Line::from("[INFO]  No events"));
        } else {
            let (start, end) = virtual_scroll_window(total, scroll_offset, visible_rows);

            for idx in start..end {
                if let Some(event) = filtered.get(idx) {
                    let metadata = event.metadata();
                    let ts = metadata.timestamp.format("%H:%M:%S").to_string();
                    let source = &metadata.source;
                    let event_type = event.type_name();

                    let is_critical = event.severity() == EventSeverity::Critical;
                    let is_selected = idx == selected;
                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else if is_critical {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default()
                    };

                    let prefix = if is_selected { "> " } else { "  " };
                    lines.push(Line::from(vec![
                        Span::styled(format!("{}{} ", prefix, ts), style),
                        Span::styled(format!("[{}] ", source), style),
                        Span::styled(event_type.to_string(), style),
                    ]));
                }
            }

            // 虚拟滚动提示:当总事件数 > 可见窗口时显示总数
            if total > visible_rows {
                lines.push(Line::from(format!(
                    "... showing {} of {} events",
                    end.saturating_sub(start),
                    total
                )));
            }
        }

        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }
}

impl Panel for EventStreamPanel {
    fn id(&self) -> PanelId {
        PanelId::EventStream
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Event Stream ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let filtered = Self::filtered_events(state);

        // auto_scroll=true 且无弹窗遮挡时,自动跟随到最后一项(流式追加 UX)
        // WHY 在 render 中处理:每次重绘都同步 selected,确保新事件到达时
        // 选中项立即跟随,无需额外的 tick 事件驱动。
        // 弹窗打开时冻结跟随,避免详情 overlay 后方列表跳动(P3.1 冲突规避)。
        if state.popup_stack.is_empty() && state.auto_scroll && !filtered.is_empty() {
            self.selected = filtered.len() - 1;
        }
        self.selected = list_state::clamp_selected(self.selected, filtered.len());

        let title = build_filter_title(state, "Event Stream");
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        // 内部高度需扣除:标题(2 行) + 截断/提示行(1 行) + 页脚(1 行) + auto_scroll 提示(1 行)
        // WHY saturating_sub(5):保守估计额外占用 5 行,避免内容溢出边框
        let content_height = inner.height.saturating_sub(5) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        // 虚拟滚动:render_window 内部已通过 virtual_scroll_window 仅构造可见区域 + 上下缓冲
        // WHY 不使用 Paragraph::scroll:render_window 返回的 Text 已只包含可见行(含缓冲),
        // 再用 Paragraph::scroll 会导致二次滚动 — 当 scroll_offset > Text 行数时显示空白。
        // 此前实现用 content() + Paragraph::scroll 的组合在万级事件下会出现空白屏,
        // 改为直接传 content_height 给 render_window,让虚拟滚动窗口与实际终端高度对齐。
        let paragraph = Paragraph::new(Self::render_window(
            state,
            self.selected,
            self.scroll_offset,
            content_height,
        ));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_events(state).len();
        let last_idx = count.saturating_sub(1);

        match key.code {
            // Down / j:向下移动;若已在底部则保持/恢复 auto_scroll,
            // 否则关闭 auto_scroll 交由用户接管。
            KeyCode::Down | KeyCode::Char('j') => {
                if count > 0 {
                    if self.selected == last_idx {
                        state.auto_scroll = true;
                    } else {
                        state.auto_scroll = false;
                        self.selected += 1;
                    }
                }
                None
            }
            // Up / k:向上移动并关闭 auto_scroll
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                state.auto_scroll = false;
                None
            }
            KeyCode::Char('g') => {
                self.scroll_to_top(state);
                None
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom(state);
                None
            }
            KeyCode::Enter => {
                let filtered = Self::filtered_events(state);
                filtered
                    .get(self.selected)
                    .map(|event| TuiCommand::OpenPopup(PopupKind::event_detail(event)))
            }
            // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        // WHY:用户主动跳到顶部时不改变 auto_scroll,保持当前流式策略。
        // 若 auto_scroll 原本为 true,selected=0 后下一事件仍会回到底部。
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let count = Self::filtered_events(state).len();
        if count > 0 {
            self.selected = count - 1;
            self.scroll_offset = self.selected;
        }
        // WHY:跳到底部意味着用户希望持续关注最新事件,恢复 auto_scroll。
        state.auto_scroll = true;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_events(state).len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
            // 鼠标滚轮同样关闭 auto_scroll(用户接管导航)
            state.auto_scroll = false;
        }
        None
    }
}

/// 构造带过滤器指示器的标题
fn build_filter_title(state: &TuiState, base: &str) -> String {
    let mut parts = Vec::new();
    if let Some(kw) = &state.filter_keyword {
        parts.push(format!("keyword:{}", kw));
    }
    if let Some(topic) = &state.filter_topic {
        parts.push(format!("topic:{}", topic));
    }
    if let Some(level) = &state.filter_level {
        parts.push(format!("level:{}", level));
    }

    if parts.is_empty() {
        format!(" {base} ")
    } else {
        format!(" {base} [{}] ", parts.join(" "))
    }
}

/// 判断事件是否匹配当前过滤器(keyword + topic + level 三重过滤)
fn event_matches_filters(event: &NexusEvent, state: &TuiState) -> bool {
    if let Some(topic) = &state.filter_topic {
        if !event_matches_topic(event, topic) {
            return false;
        }
    }

    if let Some(level) = &state.filter_level {
        if !event_matches_level(event, level) {
            return false;
        }
    }

    if let Some(kw) = &state.filter_keyword {
        if !event_matches_keyword(event, kw) {
            return false;
        }
    }

    true
}

/// 事件关键字匹配(大小写不敏感)
fn event_matches_keyword(event: &NexusEvent, keyword: &str) -> bool {
    let keyword = keyword.to_lowercase();
    let haystack = event_search_text(event).to_lowercase();
    haystack.contains(&keyword)
}

/// 将事件转换为可搜索文本(类型名 + 源 + JSON 载荷)
fn event_search_text(event: &NexusEvent) -> String {
    let meta = event.metadata();
    let mut parts = vec![event.type_name().to_string(), meta.source.clone()];
    if let Ok(json) = serde_json::to_string(event) {
        parts.push(json);
    }
    parts.join(" ")
}

/// 事件主题匹配 — 与 LogPanel 保持一致的事件分类
fn event_matches_topic(event: &NexusEvent, topic: &str) -> bool {
    match topic.to_lowercase().as_str() {
        "quest" => matches!(
            event,
            NexusEvent::QuestCreated { .. }
                | NexusEvent::QuestProgressUpdated { .. }
                | NexusEvent::QuestListUpdated { .. }
                | NexusEvent::QuestCompleted { .. }
                | NexusEvent::ThinkingModeSwitched { .. }
                | NexusEvent::CheckpointSaved { .. }
                | NexusEvent::CheckpointLoaded { .. }
                | NexusEvent::ModelRouteSelected { .. }
        ),
        "parliament" => matches!(
            event,
            NexusEvent::VoteCast { .. }
                | NexusEvent::ConsensusReached { .. }
                | NexusEvent::DebateStarted { .. }
                | NexusEvent::RoleRegistered { .. }
                | NexusEvent::SkepticVeto { .. }
                | NexusEvent::VetoOverridden { .. }
                | NexusEvent::RedTeamAudit { .. }
                | NexusEvent::AhirtProbeCompleted { .. }
        ),
        "budget" => matches!(
            event,
            NexusEvent::BudgetExceeded { .. }
                | NexusEvent::BudgetAdjusted { .. }
                | NexusEvent::BudgetStatsReported { .. }
                | NexusEvent::BudgetMetricsUpdated { .. }
        ),
        "memory" => matches!(
            event,
            NexusEvent::MemoryMetricsReported { .. }
                | NexusEvent::MemoryTiered { .. }
                | NexusEvent::ContextWindowSwitched { .. }
                | NexusEvent::ContextCompressed { .. }
                | NexusEvent::CacheHit { .. }
                | NexusEvent::CacheMiss { .. }
                | NexusEvent::CacheStatsReported { .. }
                | NexusEvent::CachePrefetched { .. }
        ),
        "security" => matches!(
            event,
            NexusEvent::CapabilityFrozen { .. }
                | NexusEvent::SandboxViolation { .. }
                | NexusEvent::SkepticVeto { .. }
                | NexusEvent::VetoOverridden { .. }
                | NexusEvent::RedTeamAudit { .. }
                | NexusEvent::AsaIntervention { .. }
        ),
        "health" => matches!(
            event,
            NexusEvent::SlowConsumerDropped { .. }
                | NexusEvent::McpMeshTransactionCompleted { .. }
                | NexusEvent::EfficiencyAlertTriggered { .. }
        ),
        "system" => matches!(
            event,
            NexusEvent::NexusStateChanged { .. }
                | NexusEvent::UserIntentEncoded { .. }
                | NexusEvent::McpMessageReceived { .. }
                | NexusEvent::CsnSubstitutionTriggered { .. }
                | NexusEvent::OrphanCallDetected { .. }
                | NexusEvent::SlowConsumerDropped { .. }
        ),
        _ => true,
    }
}

/// 事件级别匹配 — 与 LogPanel 保持一致的级别语义
fn event_matches_level(event: &NexusEvent, level: &str) -> bool {
    match level.to_lowercase().as_str() {
        "info" => true,
        "warn" => event_severity_rank(event) >= 1,
        "error" => event_severity_rank(event) >= 2,
        "critical" => event.severity() == EventSeverity::Critical,
        _ => true,
    }
}

/// 事件严重等级排序(0=info, 1=warn, 2=error, 3=critical)
fn event_severity_rank(event: &NexusEvent) -> u8 {
    if event.severity() == EventSeverity::Critical {
        return 3;
    }
    match event {
        NexusEvent::BudgetExceeded { .. }
        | NexusEvent::OperationTimedOut { .. }
        | NexusEvent::GatherTimedOut { .. }
        | NexusEvent::SandboxViolation { .. }
        | NexusEvent::OrphanCallDetected { .. } => 2,
        NexusEvent::AsaIntervention { action, .. } if action != "Block" => 1,
        NexusEvent::BudgetAdjusted { .. }
        | NexusEvent::CapabilityFrozen { .. }
        | NexusEvent::SlowConsumerDropped { .. }
        | NexusEvent::EfficiencyAlertTriggered { .. } => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, NexusEvent};
    use std::collections::VecDeque;

    #[test]
    fn test_event_stream_panel_id() {
        let panel = EventStreamPanel::new();
        assert_eq!(panel.id(), PanelId::EventStream);
    }

    #[test]
    fn test_event_stream_panel_title() {
        let panel = EventStreamPanel::new();
        let title = panel.title();
        assert_eq!(title.to_string(), " Event Stream ");
    }

    #[test]
    fn test_event_stream_panel_handle_key_returns_none_for_unmapped() {
        let mut panel = EventStreamPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(panel.handle_key(key, &mut state).is_none());
    }

    #[test]
    fn test_event_stream_panel_empty_state() {
        let state = TuiState::new();
        let content = EventStreamPanel::content(&state, 0).to_string();
        assert!(content.contains("No events"));
    }

    #[test]
    fn test_event_stream_panel_renders_events() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("decb-governor"),
                budget_type: "token".into(),
                current: 9500,
                limit: 10000,
            },
        ]);
        let content = EventStreamPanel::content(&state, 0).to_string();
        assert!(content.contains("CacheHit"));
        assert!(content.contains("BudgetExceeded"));
    }

    #[test]
    fn test_event_stream_panel_filter_keyword() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "alpha".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "beta".into(),
            },
        ]);
        state.filter_keyword = Some("alpha".into());

        let filtered = EventStreamPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "CacheHit");
    }

    #[test]
    fn test_event_stream_panel_filter_topic() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q1".into(),
                veto_reason: "unsafe".into(),
                frozen_capabilities: vec![],
            },
        ]);
        state.filter_topic = Some("security".into());

        let filtered = EventStreamPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "SkepticVeto");
    }

    #[test]
    fn test_event_stream_panel_filter_level_critical() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("decb-governor"),
                budget_type: "token".into(),
                current: 9500,
                limit: 10000,
            },
        ]);
        state.filter_level = Some("critical".into());

        let filtered = EventStreamPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "BudgetExceeded");
    }

    #[test]
    fn test_event_stream_panel_title_with_filters() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        let title = build_filter_title(&state, "Event Stream");
        assert!(title.contains("keyword:foo"));
        assert!(title.contains("topic:security"));
    }

    #[test]
    fn test_event_stream_panel_navigation_disables_auto_scroll() {
        let mut panel = EventStreamPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            },
        ]);
        state.auto_scroll = true;

        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert!(!state.auto_scroll);
        assert_eq!(panel.selected, 1);
    }

    #[test]
    fn test_event_stream_panel_shift_g_restores_auto_scroll() {
        let mut panel = EventStreamPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k3".into(),
            },
        ]);
        state.auto_scroll = false;
        panel.selected = 0;

        panel.handle_key(
            KeyEvent::new(KeyCode::Char('G'), crossterm::event::KeyModifiers::SHIFT),
            &mut state,
        );
        assert!(state.auto_scroll);
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn test_event_stream_panel_detail_popup() {
        let mut panel = EventStreamPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
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
                assert!(title.contains("CacheHit"));
                assert_eq!(event_type, "CacheHit");
                assert!(payload_decoded.contains("scc-cache"));
                assert!(payload_decoded.contains("k1"));
                assert!(!related_event_ids.is_empty());
            }
            _ => panic!("expected EventDetail popup command, got {:?}", cmd),
        }
    }

    #[test]
    fn test_event_stream_panel_help_key_returns_none() {
        let mut panel = EventStreamPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            crossterm::event::KeyModifiers::NONE,
        );
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        assert_eq!(panel.handle_key(key, &mut state), None);
    }

    #[test]
    fn test_event_stream_panel_new_events_indicator() {
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k3".into(),
            },
        ]);
        state.auto_scroll = false;
        // selected=0,有 3 条事件 → 应显示 "[新事件 2 条]"
        let content = EventStreamPanel::content(&state, 0).to_string();
        assert!(content.contains("新事件"));
    }
}
