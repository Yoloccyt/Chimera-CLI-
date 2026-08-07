//! TUI Parliament 面板 — 显示议会相关事件(Skeptic 否决、红队审计、ASA 干预、投票、共识)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变,同时修复 `unreachable!()` 的安全隐患。
//! - 对未识别事件使用安全回退(skip)而非 panic,符合 §4 编码红线。
//! - M3 增加滚动选择与详情弹窗。

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
use event_bus::NexusEvent;
use parliament::immune_system_status;

/// Parliament 面板
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParliamentPanel {
    /// 当前选中事件的索引
    selected: usize,
    /// 事件列表的滚动偏移
    scroll_offset: usize,
    /// M4 二期:渲染内容缓存键(命中时复用 `cached_text`)
    cached_key: Option<ParliamentRenderKey>,
    /// M4 二期:缓存的渲染文本(命中时仅 clone 供渲染,跳过重建开销)
    cached_text: Text<'static>,
    /// M4 二期:缓存对应时刻的过滤事件数(命中时免去 O(n) 过滤)
    cached_count: usize,
    /// M4 二期:缓存对应时刻的虚拟滚动窗口(面积/滚动变化时失效)
    cached_window: Option<(usize, usize)>,
}

/// Parliament 面板渲染缓存键(M4 二期)
///
/// 覆盖所有影响输出文本的状态:
/// - `revision`:快照数据(事件流)变化;
/// - `selected`:光标位置(影响前缀与高亮样式);
/// - `window`:虚拟滚动窗口(依赖滚动偏移与可用高度);
/// - `locale`:全局语言(`Ctrl+L` 切换中英)。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParliamentRenderKey {
    revision: u64,
    selected: usize,
    window: (usize, usize),
    locale: u8,
}

impl ParliamentPanel {
    /// 创建新的 Parliament 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回 Parliament 相关事件列表(最新在前)
    fn parliament_events(state: &TuiState) -> Vec<&NexusEvent> {
        state
            .latest_events
            .iter()
            .rev()
            .filter(|e| {
                matches!(
                    e,
                    NexusEvent::VoteCast { .. }
                        | NexusEvent::ConsensusReached { .. }
                        | NexusEvent::SkepticVeto { .. }
                        | NexusEvent::RedTeamAudit { .. }
                        | NexusEvent::AsaIntervention { .. }
                        | NexusEvent::VetoOverridden { .. }
                        | NexusEvent::DebateStarted { .. }
                        | NexusEvent::AhirtProbeCompleted { .. }
                )
            })
            .collect()
    }

    /// 构建 Parliament 面板文本内容
    ///
    /// P4.2 虚拟滚动:`window` 为 `(start, end)` 半开区间,仅渲染窗口内事件,
    /// 避免 1000+ 条事件时全量构建 `Text` 的性能开销。
    fn content(state: &TuiState, selected: usize, window: (usize, usize)) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.parliament.body_title")),
            Line::from("─────────────"),
        ];

        let parliament_events = Self::parliament_events(state);

        if parliament_events.is_empty() {
            lines.push(Line::from(crate::t!("panel.parliament.no_events")));
        } else {
            let (start, end) = window;
            for (idx, event) in parliament_events.iter().enumerate() {
                // 虚拟滚动:跳过窗口外的事件
                if idx < start || idx >= end {
                    continue;
                }
                let is_selected = idx == selected;
                let prefix = if is_selected { "> " } else { "  " };
                let (label, summary, style) = match event {
                    NexusEvent::SkepticVeto {
                        quest_id,
                        veto_reason,
                        ..
                    } => (
                        "SkepticVeto",
                        format!("{} | {}", quest_id, veto_reason),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                    NexusEvent::VetoOverridden {
                        quest_id,
                        proposal_id,
                        override_reason,
                        ..
                    } => (
                        "VetoOverridden",
                        format!("{} | {} | {}", quest_id, proposal_id, override_reason),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Red)
                        },
                    ),
                    NexusEvent::AsaIntervention {
                        operation_id,
                        action,
                        block_reason,
                        ..
                    } => {
                        let detail = block_reason
                            .as_deref()
                            .filter(|r| !r.is_empty())
                            .unwrap_or(action);
                        (
                            "AsaIntervention",
                            format!("{} | {}", operation_id, detail),
                            if is_selected {
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Yellow)
                            },
                        )
                    }
                    NexusEvent::RedTeamAudit {
                        vulnerability_type,
                        detection_rate,
                        remediation_suggestion,
                        ..
                    } => (
                        "RedTeamAudit",
                        format!(
                            "{} | {}={:.0}% | {}",
                            vulnerability_type,
                            crate::t!("panel.parliament.detection"),
                            detection_rate * 100.0,
                            remediation_suggestion
                        ),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::LightYellow)
                        },
                    ),
                    NexusEvent::ConsensusReached {
                        quest_id,
                        decision_hash,
                        ..
                    } => (
                        "ParliamentConsensusReached",
                        format!("{} | {}", quest_id, decision_hash),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Green)
                        },
                    ),
                    NexusEvent::VoteCast {
                        proposal_id,
                        voter,
                        vote,
                        ..
                    } => (
                        "ParliamentVoteCast",
                        format!(
                            "{} | {}: {}",
                            proposal_id,
                            voter,
                            if *vote {
                                crate::t!("panel.parliament.for")
                            } else {
                                crate::t!("panel.parliament.against")
                            }
                        ),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    NexusEvent::DebateStarted {
                        quest_id,
                        proposal_id,
                        ..
                    } => (
                        "DebateStarted",
                        format!("{} | {}", quest_id, proposal_id),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Cyan)
                        },
                    ),
                    NexusEvent::AhirtProbeCompleted {
                        probe_type,
                        total,
                        failed,
                        ..
                    } => (
                        "AhirtProbeCompleted",
                        format!(
                            "{} | {}={}/{}",
                            probe_type,
                            crate::t!("panel.parliament.failed"),
                            failed,
                            total
                        ),
                        if is_selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Yellow)
                        },
                    ),
                    // 安全回退:过滤条件之外的 Parliament 事件直接跳过,
                    // 避免 `unreachable!()` 在新增事件变体时 panic。
                    _ => continue,
                };

                lines.push(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(format!("[{}] ", label), style),
                    Span::styled(summary, style),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));
        Text::from(lines)
    }
}

impl Panel for ParliamentPanel {
    fn id(&self) -> PanelId {
        PanelId::Parliament
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.parliament"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        // Task 3.8:L10 → L8 向下依赖 — 免疫系统状态摘要（三探针 + 级联风险 + 膜厚）
        let status = immune_system_status();
        let immune_header = vec![
            Line::from(vec![
                Span::styled(
                    crate::t!("panel.parliament.immune"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "Mem={:.0}%  Reason={:.0}%  Evol={:.0}%  Cascade={:.0}%  Membrane={}/7",
                        status.memory_paradox_rate * 100.0,
                        status.reasoning_trap_rate * 100.0,
                        status.evolution_hack_rate * 100.0,
                        status.cascade_risk * 100.0,
                        status.membrane_thickness,
                    ),
                    Style::default().fg(if status.cascade_risk > 0.7 {
                        Color::Red
                    } else if status.cascade_risk > 0.3 {
                        Color::Yellow
                    } else {
                        Color::Green
                    }),
                ),
            ]),
            Line::from(""),
        ];
        let header_height = immune_header.len() as u16;

        // 垂直切分:免疫状态摘要 + 事件列表
        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: header_height.min(inner.height),
        };
        let list_area = Rect {
            x: inner.x,
            y: inner.y.saturating_add(header_height),
            width: inner.width,
            height: inner.height.saturating_sub(header_height),
        };

        let header_p = Paragraph::new(Text::from(immune_header));
        Widget::render(header_p, header_area, buf);

        let content_height = list_area.height.saturating_sub(3) as usize;

        // M4 二期:先按"上一窗口"试探缓存命中,命中则用缓存计数免去 O(n)
        // 事件过滤;随后按真实窗口二次校验,滚动/面积变化仍会正确失效。
        let candidate = ParliamentRenderKey {
            revision: state.last_snapshot_revision,
            selected: self.selected,
            window: self.cached_window.unwrap_or((0, 0)),
            // `as_u8` 为 i18n 模块私有,此处以匹配保持缓存键稳定(仅两种 locale)
            locale: match crate::i18n::current_locale() {
                crate::i18n::Locale::Zh => 0,
                crate::i18n::Locale::En => 1,
            },
        };
        let enable_cache = state.last_snapshot_revision != 0;
        let hit = enable_cache && self.cached_key.as_ref() == Some(&candidate);
        let count = if hit {
            self.cached_count
        } else {
            Self::parliament_events(state).len()
        };
        self.selected = list_state::clamp_selected(self.selected, count);
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, content_height);

        // P4.2 虚拟滚动:仅构建窗口内事件的 Text,减少内存与 CPU 开销
        let window = virtual_scroll_window(count, self.scroll_offset, content_height);
        let key = ParliamentRenderKey {
            selected: self.selected,
            window,
            ..candidate
        };
        if !hit || self.cached_key.as_ref() != Some(&key) {
            self.cached_text = Self::content(state, self.selected, window);
            self.cached_count = count;
            self.cached_window = Some(window);
            self.cached_key = if enable_cache { Some(key) } else { None };
        }
        let paragraph =
            Paragraph::new(self.cached_text.clone()).scroll((self.scroll_offset as u16, 0));
        paragraph.render(list_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::parliament_events(state).len();
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        match key.code {
            KeyCode::Enter => {
                let events = Self::parliament_events(state);
                events
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
        let count = Self::parliament_events(state).len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = Self::parliament_events(state).len();
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
            // I-9/快捷键诚实性:Enter 实际打开事件详情弹窗;V/Y/N/A 投票
            // 无面板内按键分支(需 proposal_id,当前只能 `:vote` 命令),移除虚假提示。
            ("Enter", "详情"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{EventMetadata, NexusEvent};
    use std::collections::VecDeque;

    #[test]
    fn test_parliament_panel_id() {
        let panel = ParliamentPanel::new();
        assert_eq!(panel.id(), PanelId::Parliament);
    }

    #[test]
    fn test_parliament_panel_empty_state() {
        let state = TuiState::new();
        let content = ParliamentPanel::content(&state, 0, (0, 50)).to_string();
        assert!(content.contains("议会"));
        assert!(content.contains("暂无近期议会事件"));
    }

    #[test]
    fn test_parliament_panel_no_panic_on_unknown_event() {
        // 即使过滤条件意外包含未处理变体,也不应 panic。
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("test"),
                cache_key: "k1".into(),
            },
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p1".into(),
                voter: "alice".into(),
                vote: true,
            },
        ]);
        let content = ParliamentPanel::content(&state, 0, (0, 50)).to_string();
        assert!(content.contains("ParliamentVoteCast"));
        assert!(!content.contains("CacheHit"));
    }

    #[test]
    fn test_parliament_panel_navigation() {
        let mut panel = ParliamentPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p1".into(),
                voter: "alice".into(),
                vote: true,
            },
            NexusEvent::VoteCast {
                metadata: EventMetadata::new("parliament"),
                proposal_id: "p2".into(),
                voter: "bob".into(),
                vote: false,
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
    fn test_parliament_panel_detail_popup() {
        let mut panel = ParliamentPanel::new();
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: "p1".into(),
            voter: "alice".into(),
            vote: true,
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
                assert!(title.contains("VoteCast"));
                assert_eq!(event_type, "VoteCast");
                assert!(payload_decoded.contains("alice"));
                assert!(related_event_ids.contains(&"p1".to_string()));
            }
            _ => panic!("expected EventDetail popup command, got {:?}", cmd),
        }
    }

    /// 构造含 N 条 VoteCast 事件的测试状态(revision=1)
    fn parliament_state_with_votes(count: usize) -> TuiState {
        let mut state = TuiState::new();
        state.last_snapshot_revision = 1;
        state.latest_events = VecDeque::from(
            (0..count)
                .map(|i| NexusEvent::VoteCast {
                    metadata: EventMetadata::new("test"),
                    proposal_id: format!("p{i}"),
                    voter: "alice".into(),
                    vote: i % 2 == 0,
                })
                .collect::<Vec<_>>(),
        );
        state
    }

    /// M4 二期:相同键(数据/选中/窗口/语言均未变)连续渲染应复用缓存文本。
    #[test]
    fn parliament_render_reuses_cached_text_when_key_unchanged() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let state = parliament_state_with_votes(2);
        let mut panel = ParliamentPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            rendered.contains("SENTINEL-CACHE-HIT"),
            "缓存未命中: 内容被重建(rendered={rendered})"
        );
    }

    /// M4 二期:快照 revision 变化(事件流更新)必须使缓存失效。
    #[test]
    fn parliament_render_invalidates_on_revision_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = parliament_state_with_votes(2);
        let mut panel = ParliamentPanel::new();
        let area = Rect::new(0, 0, 80, 24);

        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        state.last_snapshot_revision = 2;
        let mut buf2 = Buffer::empty(area);
        panel.render(&state, area, &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT") && rendered.contains("ParliamentVoteCast"),
            "revision 变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:选中索引变化(光标移动)必须使缓存失效。
    #[test]
    fn parliament_render_invalidates_on_selection_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = parliament_state_with_votes(2);
        let mut panel = ParliamentPanel::new();
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
            !rendered.contains("SENTINEL-CACHE-HIT"),
            "选中变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:可用高度变化(虚拟滚动窗口变化)必须使缓存失效。
    #[test]
    fn parliament_render_invalidates_on_window_change() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let state = parliament_state_with_votes(20);
        let mut panel = ParliamentPanel::new();

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        panel.render(&state, Rect::new(0, 0, 80, 24), &mut buf);
        panel.cached_text = Text::from("SENTINEL-CACHE-HIT");

        let mut buf2 = Buffer::empty(Rect::new(0, 0, 80, 10));
        panel.render(&state, Rect::new(0, 0, 80, 10), &mut buf2);
        let rendered: String = buf2.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains("SENTINEL-CACHE-HIT"),
            "窗口变化未使缓存失效(rendered={rendered})"
        );
    }

    /// M4 二期:全局语言切换必须使缓存失效。
    #[test]
    fn parliament_render_invalidates_on_locale_change() {
        let state = parliament_state_with_votes(2);
        let mut panel = ParliamentPanel::new();
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
    fn parliament_render_disables_cache_at_revision_zero() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state.latest_events = VecDeque::from([NexusEvent::VoteCast {
            metadata: EventMetadata::new("test"),
            proposal_id: "p1".into(),
            voter: "alice".into(),
            vote: true,
        }]);
        let mut panel = ParliamentPanel::new();
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
}
