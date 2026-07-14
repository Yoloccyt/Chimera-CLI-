//! TUI Log 面板 — 显示系统事件流
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变。
//! - 关键事件(Critical severity)使用红色高亮。
//! - M3 增加过滤(关键字/主题/级别)、滚动选择与详情弹窗。

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
use event_bus::{EventSeverity, NexusEvent};

/// Log 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LogPanel {
    /// 当前选中事件的索引(在已过滤事件列表中)
    selected: usize,
    /// 事件列表的滚动偏移
    scroll_offset: usize,
}

impl LogPanel {
    /// 创建新的 Log 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回经过过滤的事件列表
    ///
    /// WHY 独立方法:过滤逻辑集中,便于单元测试直接验证。
    pub fn filtered_events(state: &TuiState) -> Vec<&NexusEvent> {
        state
            .latest_events
            .iter()
            .rev()
            .filter(|e| event_matches_filters(e, state))
            .collect()
    }

    /// 构建 Log 面板文本内容
    pub fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let mut lines: Vec<Line<'static>> =
            vec![Line::from("System Log"), Line::from("─────────────")];

        let filtered = Self::filtered_events(state);

        if filtered.is_empty() {
            lines.push(Line::from("[INFO]  No matching events"));
        } else {
            for (idx, event) in filtered.iter().enumerate().take(50) {
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

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }

    /// 返回当前选中项索引(测试用)
    pub fn selected(&self) -> usize {
        self.selected
    }
}

impl Panel for LogPanel {
    fn id(&self) -> PanelId {
        PanelId::Log
    }

    fn title(&self) -> Line<'static> {
        Line::from(" System Log ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let filtered = Self::filtered_events(state);
        self.selected = list_state::clamp_selected(self.selected, filtered.len());

        let title = build_filter_title(state, "System Log");
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        let content_height = inner.height.saturating_sub(3) as usize; // 标题 + 分隔线 + 页脚
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        let paragraph = Paragraph::new(Self::content(state, self.selected))
            .scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_events(state).len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            KeyCode::Enter => {
                let filtered = Self::filtered_events(state);
                filtered
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
        let count = Self::filtered_events(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::filtered_events(state).len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
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

/// 判断事件是否匹配当前过滤器
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

/// 将事件转换为可搜索文本
fn event_search_text(event: &NexusEvent) -> String {
    let meta = event.metadata();
    let mut parts = vec![event.type_name().to_string(), meta.source.clone()];
    if let Ok(json) = serde_json::to_string(event) {
        parts.push(json);
    }
    parts.join(" ")
}

/// 事件主题匹配
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

/// 事件级别匹配
fn event_matches_level(event: &NexusEvent, level: &str) -> bool {
    match level.to_lowercase().as_str() {
        "info" => true,
        "warn" => event_severity_rank(event) >= 1,
        "error" => event_severity_rank(event) >= 2,
        "critical" => event.severity() == EventSeverity::Critical,
        _ => true,
    }
}

/// 事件严重等级排序(0=info,1=warn,2=error,3=critical)
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
    fn test_log_panel_id() {
        let panel = LogPanel::new();
        assert_eq!(panel.id(), PanelId::Log);
    }

    #[test]
    fn test_log_panel_empty_state() {
        let state = TuiState::new();
        let content = LogPanel::content(&state, 0).to_string();
        assert!(content.contains("System Log"));
        assert!(content.contains("No matching events"));
    }

    #[test]
    fn test_log_panel_renders_events() {
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
        let content = LogPanel::content(&state, 0).to_string();
        assert!(content.contains("CacheHit"));
        assert!(content.contains("BudgetExceeded"));
    }

    #[test]
    fn test_log_panel_filter_keyword() {
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

        let filtered = LogPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "CacheHit");
    }

    #[test]
    fn test_log_panel_filter_topic() {
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

        let filtered = LogPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "SkepticVeto");
    }

    #[test]
    fn test_log_panel_filter_level() {
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

        let filtered = LogPanel::filtered_events(&state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].type_name(), "BudgetExceeded");
    }

    #[test]
    fn test_log_panel_title_with_filters() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        let title = build_filter_title(&state, "System Log");
        assert!(title.contains("keyword:foo"));
        assert!(title.contains("topic:security"));
    }

    #[test]
    fn test_log_panel_navigation() {
        let mut panel = LogPanel::new();
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
    fn test_log_panel_detail_popup() {
        let mut panel = LogPanel::new();
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
}
