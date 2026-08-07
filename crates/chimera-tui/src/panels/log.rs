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

use crate::panels::filter_cache::FilterCache;
use crate::panels::list_state;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::{virtual_scroll_window, FOOTER_TEXT};
use crate::types::{PanelId, TuiCommand, TuiState};
use event_bus::{EventSeverity, NexusEvent};

/// `content()` 方法默认渲染的可见行数(测试场景;实际渲染由终端高度决定)
const LOG_CONTENT_DEFAULT_VISIBLE_ROWS: usize = 20;

/// Log 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LogPanel {
    /// 当前选中事件的索引(在已过滤事件列表中)
    selected: usize,
    /// 事件列表的滚动偏移
    scroll_offset: usize,
    /// 过滤结果缓存(键 = revision + 三过滤器;production + 关键字时启用)
    filter_cache: FilterCache,
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

    /// 带缓存的过滤(供 render / handle_key / handle_mouse 等 `&mut self` 调用点使用)
    ///
    /// WHY M4 v1:Log 过滤为 O(n) 谓词遍历,万级事件 + 关键字下每帧重建代价高;
    /// revision 未变时复用索引缓存,跨帧零过滤开销;revision == 0(测试桩)或
    /// 未设置关键字时走无缓存路径(与 EventStream 缓存策略一致,防陈旧结果)。
    pub fn filtered_events_cached<'a>(&mut self, state: &'a TuiState) -> Vec<&'a NexusEvent> {
        let cache_enabled = FilterCache::enabled(state);
        if cache_enabled && self.filter_cache.matches(state) {
            return self
                .filter_cache
                .indices()
                .iter()
                .map(|&idx| &state.latest_events[idx])
                .collect();
        }
        // Log 展示顺序为最新在前(与 filtered_events 一致):索引按下标降序收集
        let indices: Vec<usize> = state
            .latest_events
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, event)| event_matches_filters(event, state))
            .map(|(idx, _)| idx)
            .collect();
        if cache_enabled {
            self.filter_cache.update(state, indices.clone());
        }
        indices
            .iter()
            .map(|&idx| &state.latest_events[idx])
            .collect()
    }

    /// 构建 Log 面板文本内容(窗口化:仅构建可见区域 + 上下缓冲)
    ///
    /// WHY P-4 性能:原实现 `.take(50)` + `Paragraph::scroll` 在过滤结果超过
    /// 50 条时滚入空白(滚动偏移超过 Text 行数),且每帧全量构建事件文本;
    /// 改为复用 EventStream 的 `virtual_scroll_window` 模式,
    /// Text 构造复杂度 O(visible + 2×BUFFER),万级事件下不随总量线性膨胀。
    pub fn content(state: &TuiState, selected: usize) -> Text<'static> {
        let scroll_offset =
            list_state::adjust_scroll(selected, 0, LOG_CONTENT_DEFAULT_VISIBLE_ROWS);
        let filtered = Self::filtered_events(state);
        Self::render_window(
            state,
            &filtered,
            selected,
            scroll_offset,
            LOG_CONTENT_DEFAULT_VISIBLE_ROWS,
        )
    }

    /// 渲染可见区域的事件文本(核心渲染逻辑,content 与 render 共用)
    fn render_window(
        _state: &TuiState,
        filtered: &[&NexusEvent],
        selected: usize,
        scroll_offset: usize,
        visible_rows: usize,
    ) -> Text<'static> {
        let total = filtered.len();
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.log.body_title")),
            Line::from("─────────────"),
        ];

        if filtered.is_empty() {
            lines.push(Line::from(crate::t!("panel.log.no_matching")));
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
            // 虚拟滚动提示:总量大于可见窗口时显示计数,避免用户误以为数据被截断
            if total > visible_rows {
                lines.push(Line::from(format!(
                    "... {} {} {} {} {}",
                    crate::t!("panel.log.showing"),
                    end.saturating_sub(start),
                    crate::t!("panel.log.of"),
                    total,
                    crate::t!("panel.log.events")
                )));
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

    /// 返回当前滚动偏移(测试用,与 EventStreamPanel 模式一致)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
}

impl Panel for LogPanel {
    fn id(&self) -> PanelId {
        PanelId::Log
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.log"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let filtered = self.filtered_events_cached(state);
        self.selected = list_state::clamp_selected(self.selected, filtered.len());

        let title = build_filter_title(state, crate::t!("panel.log.body_title"));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        // 内部高度扣除:标题(2 行) + 页脚(2 行) + 可能的"showing"提示行(1 行)
        // WHY saturating_sub(5):与 EventStream 一致,避免内容溢出边框
        let content_height = inner.height.saturating_sub(5) as usize;
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        // P-4 性能:虚拟滚动仅构造可见区域 + 缓冲;不再全量构建 50 行后二次滚动
        // (原实现 Paragraph::scroll 在 scroll_offset > Text 行数时显示空白)
        let paragraph = Paragraph::new(Self::render_window(
            state,
            &filtered,
            self.selected,
            self.scroll_offset,
            content_height,
        ));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = self.filtered_events_cached(state).len();
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
            KeyCode::Enter => {
                let filtered = self.filtered_events_cached(state);
                filtered
                    .get(self.selected)
                    .map(|event| TuiCommand::OpenPopup(PopupKind::event_detail(event)))
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
        let count = self.filtered_events_cached(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = self.filtered_events_cached(state).len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑/↓", "导航"),
            ("PgUp/PgDn", "翻页"),
            ("g g", "跳顶"),
            ("G", "跳底"),
            ("/", "过滤"),
        ]
    }
}

/// 构造带过滤器指示器的标题
fn build_filter_title(state: &TuiState, base: &str) -> String {
    let mut parts = Vec::new();
    if let Some(kw) = &state.filter_keyword {
        parts.push(format!("{}:{}", crate::t!("panel.log.keyword"), kw));
    }
    if let Some(topic) = &state.filter_topic {
        parts.push(format!("{}:{}", crate::t!("panel.log.topic"), topic));
    }
    if let Some(level) = &state.filter_level {
        parts.push(format!("{}:{}", crate::t!("panel.log.level"), level));
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
///
/// P1-3(评估报告 v2):高频变体经 `filter_fast::event_keyword_hit_fast` 快速路径
/// (免 JSON 全量序列化);其余变体回退 `event_search_text` JSON 兜底。
fn event_matches_keyword(event: &NexusEvent, keyword: &str) -> bool {
    if let Some(hit) = crate::panels::filter_fast::event_keyword_hit_fast(event, keyword) {
        return hit;
    }
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
        assert!(content.contains("系统日志"));
        assert!(content.contains("暂无匹配事件"));
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
        assert!(title.contains("关键字:foo"));
        assert!(title.contains("主题:security"));
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

    // P-4:虚拟滚动窗口化后,content() 只构建可见区域 + 缓冲行,
    // 窗口外事件不得出现,且超窗时显示总量提示(取代原 take(50) 全量构建)。
    #[test]
    fn test_log_panel_windowed_content_skips_out_of_window_events() {
        let mut state = TuiState::new();
        state.latest_events = (0..200)
            .map(|i| NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: format!("key-{i}"),
            })
            .collect();

        let content = LogPanel::content(&state, 0).to_string();
        // 窗口 = [0, 20 + VIRTUAL_SCROLL_BUFFER(5)) → key-0..key-24
        assert!(!content.contains("key-50"), "窗口外事件不应被构建进 Text");
        assert!(content.contains("显示"), "超窗时应显示虚拟滚动总量提示");
        assert!(content.contains("200"));
    }

    /// M4 v1:过滤缓存命中、过滤器变化失效、revision 变化失效
    #[test]
    fn test_log_filter_cache_hits_and_invalidates() {
        let mut panel = LogPanel::new();
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.filter_keyword = Some("alpha".into());
        state.latest_events = [
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "alpha-1".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "beta-2".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "alpha-3".into(),
            },
        ]
        .into_iter()
        .collect();

        // 冷路径:2 条命中(最新在前:alpha-3, alpha-1)
        let first = panel.filtered_events_cached(&state);
        assert_eq!(first.len(), 2);
        let first_key = match &first[0] {
            NexusEvent::CacheHit { cache_key, .. } => cache_key.as_str(),
            other => panic!("expected CacheHit, got {other:?}"),
        };
        assert_eq!(first_key, "alpha-3", "Log 顺序应最新在前");

        // 命中:相同状态返回一致结果
        let second = panel.filtered_events_cached(&state);
        assert_eq!(second.len(), 2);

        // 过滤器变化 → 缓存失效重算
        state.filter_keyword = Some("beta".into());
        let third = panel.filtered_events_cached(&state);
        assert_eq!(third.len(), 1);

        // revision 变化(事件流内容变化)→ 缓存失效重算
        state.filter_keyword = Some("alpha".into());
        state.last_snapshot_revision = 2;
        state.latest_events.push_back(NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha-4".into(),
        });
        let fourth = panel.filtered_events_cached(&state);
        assert_eq!(fourth.len(), 3);
    }
}
