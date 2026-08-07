//! OverWindow 面板 — 超窗兜底/RAG 检索链的结构化展示(P1,ADR-072)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **零管道侵入**:数据从 `TuiState.latest_events` 过滤 `OverWindowFallbackTriggered`
//!   派生(复刻 SelfAssessment/DagViz 面板的零侵入模式)——超窗触发事件经
//!   `commands/tui.rs` 注入的 OverWindowBridge 发布到 TUI 会话总线,再由既有
//!   EventSubscriber → DataPipeline 进入 latest_events,本面板只读展示,不改动
//!   DataPipeline/TuiState 任何字段。
//! - **字面量标题**:暂不新增 i18n SEED_KEYS(面板级 i18n 收口由 P3 统一推进),
//!   避免与并发 i18n 改动冲突;后续随面板级 i18n 收口统一迁移(边框标题 key
//!   走 i18n 表,与既有 panel.border.* 键族对齐)。

use crossterm::event::KeyEvent;
use event_bus::NexusEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 面板最多展示的兜底触发事件数(最新在前;事件流上限 256,此处截断防撑爆)
const MAX_TRIGGERS_SHOWN: usize = 8;

/// OverWindow 面板 — 展示超窗兜底触发记录(语料规模/有效窗口/候选数/装窗数)
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct OverWindowPanel;

impl OverWindowPanel {
    /// 创建面板
    pub fn new() -> Self {
        Self
    }

    /// 构建面板文本 — 从 latest_events 过滤超窗触发事件(最新在前)
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.overwindow.body_title")),
            Line::from("────────────────────────────────────────"),
        ];

        // 反向过滤:最新触发事件在前(与 EventStream 一致的时间序)
        let triggers: Vec<_> = state
            .latest_events
            .iter()
            .rev()
            .filter_map(|ev| match ev {
                NexusEvent::OverWindowFallbackTriggered {
                    corpus_tokens,
                    effective_window,
                    candidate_count,
                    loaded_count,
                    ..
                } => Some((
                    *corpus_tokens,
                    *effective_window,
                    *candidate_count,
                    *loaded_count,
                )),
                _ => None,
            })
            .collect();

        if triggers.is_empty() {
            lines.push(Line::from(Span::styled(
                crate::t!("panel.overwindow.empty"),
                Style::default().fg(Color::Gray),
            )));
            lines.push(Line::from(Span::styled(
                crate::t!("panel.overwindow.hint"),
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        }

        for (i, (corpus_tokens, effective_window, candidate_count, loaded_count)) in
            triggers.iter().take(MAX_TRIGGERS_SHOWN).enumerate()
        {
            lines.push(Line::from(vec![
                Span::styled(format!("#{} ", i + 1), Style::default().fg(Color::Cyan)),
                Span::from(format!(
                    "{}={} tok, {}={} tok",
                    crate::t!("panel.overwindow.corpus"),
                    corpus_tokens,
                    crate::t!("panel.overwindow.window"),
                    effective_window
                )),
            ]));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}={}, {}={}",
                    crate::t!("panel.overwindow.candidates"),
                    candidate_count,
                    crate::t!("panel.overwindow.loaded"),
                    loaded_count
                ),
                Style::default().fg(Color::Yellow),
            )));
        }

        if triggers.len() > MAX_TRIGGERS_SHOWN {
            lines.push(Line::from(Span::styled(
                format!(
                    "... {} {}",
                    triggers.len() - MAX_TRIGGERS_SHOWN,
                    crate::t!("panel.overwindow.more")
                ),
                Style::default().fg(Color::Gray),
            )));
        }

        Text::from(lines)
    }
}

impl Panel for OverWindowPanel {
    fn id(&self) -> PanelId {
        PanelId::OverWindow
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.overwindow.title"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 SelfAssessment/DagViz)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", "切换面板")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn trigger(
        corpus_tokens: u64,
        effective_window: u64,
        candidate_count: u32,
        loaded_count: u32,
    ) -> NexusEvent {
        NexusEvent::OverWindowFallbackTriggered {
            metadata: EventMetadata::new("test"),
            corpus_tokens,
            effective_window,
            candidate_count,
            loaded_count,
        }
    }

    #[test]
    fn panel_id_is_overwindow() {
        assert_eq!(OverWindowPanel::new().id(), PanelId::OverWindow);
    }

    #[test]
    fn empty_state_shows_awaiting_hint() {
        // WHY locale 锁:断言依赖中文文案“尚未触发超窗兜底”,并行测试切换语言会偶发失败
        // (既有 flaky,2026-08-07 修复)
        let _locale_guard = crate::i18n::locale_test_guard();
        let state = TuiState::new();
        let text = OverWindowPanel::content(&state);
        let joined = text.lines.iter().map(|l| l.to_string()).collect::<String>();
        assert!(joined.contains("尚未触发超窗兜底"), "空态应展示等待提示");
    }

    #[test]
    fn renders_trigger_details_latest_first() {
        // 与其它 locale 测试互斥,避免并行竞态把全局语言切到 En 导致断言失败
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut state = TuiState::new();
        state
            .latest_events
            .push_back(trigger(100_000, 131_072, 0, 0));
        state
            .latest_events
            .push_back(trigger(600_000, 131_072, 42, 128));
        let text = OverWindowPanel::content(&state);
        let joined = text.lines.iter().map(|l| l.to_string()).collect::<String>();
        assert!(joined.contains("语料=600000 tok"), "最新触发应显示语料规模");
        assert!(joined.contains("候选=42"), "候选数应展示");
        assert!(joined.contains("装窗=128"), "装窗数应展示");
        assert!(
            joined.find("#1").unwrap() < joined.find("#2").unwrap(),
            "最新触发应排在最前"
        );
    }
}
