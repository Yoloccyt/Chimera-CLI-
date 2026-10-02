//! TUI 黑板浏览面板 — MLC 协作态投影的按认识类型过滤浏览（v2.3 §(b.6)/§(b.2)）
//!
//! 对应架构层:L10 Interface
//! 对应设计源:`Chimera架构重构与优化方案_v2.3` §(b.6)「新增面板 黑板浏览(认识类型过滤)」
//!           与 §(b.2)「黑板=MLC 协作态投影」(投影落库见 mlc-engine::blackboard_projector)
//!
//! # 核心职责
//! **零管道侵入**地从 `state.latest_events` 派生(与 SelfAssessment/ReviewQueue 同范式):
//! 扫描 `BlackboardEntry` 事件,按 `key` 去重(最新覆盖),再按**认识类型(epistemic)**
//! 稳定分组浏览,使跨 agent 共享的 Fact/Episode/Observation/MindModel 一目了然。
//!
//! # 消费点(供 event_coverage_drift 声明一致性校验)
//! 本文件匹配 `NexusEvent::BlackboardEntry`,是该事件在 TUI 侧的真实消费点。

use std::collections::HashMap;

use crossterm::event::KeyEvent;
use event_bus::NexusEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 黑板协作态条目 — 从 `BlackboardEntry` 事件派生的展示快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardItem {
    /// 黑板条目键(检索/去重)
    pub key: String,
    /// 条目摘要
    pub summary: String,
    /// TTL 秒(存活时长)
    pub ttl_secs: u64,
    /// 认识类型标签(Fact/Episode/Observation/MindModel)
    pub epistemic: String,
}

/// 从 `latest_events` 聚合黑板协作态条目(按 key 去重,最新事件覆盖)。
#[must_use]
pub fn collect_board_entries(state: &TuiState) -> Vec<BoardItem> {
    let mut by_key: HashMap<String, BoardItem> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for event in state.latest_events.iter() {
        if let NexusEvent::BlackboardEntry {
            key,
            summary,
            ttl_secs,
            epistemic,
            ..
        } = event
        {
            if !by_key.contains_key(key) {
                order.push(key.clone());
            }
            by_key.insert(
                key.clone(),
                BoardItem {
                    key: key.clone(),
                    summary: summary.clone(),
                    ttl_secs: *ttl_secs,
                    epistemic: epistemic.clone(),
                },
            );
        }
    }
    let mut items: Vec<BoardItem> = order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect();
    // 按 epistemic 标签稳定分组(认识类型过滤),组内按 key 升序
    items.sort_by(|a, b| (&a.epistemic, &a.key).cmp(&(&b.epistemic, &b.key)));
    items
}

/// 黑板浏览面板(v2.3 §(b.6)/§(b.2))
#[derive(Debug, Default)]
pub struct BlackboardPanel {}

impl BlackboardPanel {
    /// 创建面板(数据从 `state.latest_events` 派生,无需外部注入)
    pub fn new() -> Self {
        Self {}
    }

    /// 构建面板文本内容:按 epistemic 标签分组列出协作态条目。
    #[must_use]
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.blackboard.subtitle")),
            Line::from(crate::t!("panel.blackboard.filter")),
            Line::from("──────────────────────────────────────"),
        ];
        let items = collect_board_entries(state);
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                crate::t!("panel.blackboard.none"),
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        }
        for item in items {
            lines.push(Line::from(Span::styled(
                format!("[{}] {} (ttl={}s)", item.epistemic, item.key, item.ttl_secs),
                Style::default().fg(Color::Cyan),
            )));
        }
        Text::from(lines)
    }
}

impl Panel for BlackboardPanel {
    fn id(&self) -> PanelId {
        PanelId::Blackboard
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.blackboard"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        if crate::panels::degenerate(area) {
            crate::panels::render_too_small(area, buf);
            return;
        }
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 ReviewQueue/SelfAssessment)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", crate::t!("shortcut.switch_panel"))]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn board(key: &str, epistemic: &str, summary: &str) -> NexusEvent {
        NexusEvent::BlackboardEntry {
            metadata: EventMetadata::new("t"),
            key: key.into(),
            summary: summary.into(),
            ttl_secs: 60,
            epistemic: epistemic.into(),
            provenance: None,
        }
    }

    fn push(state: &mut TuiState, ev: NexusEvent) {
        std::sync::Arc::make_mut(&mut state.latest_events).push_back(ev);
    }

    #[test]
    fn panel_id_semantics() {
        assert_eq!(BlackboardPanel::new().id(), PanelId::Blackboard);
    }

    #[test]
    fn empty_when_no_events() {
        let state = TuiState::new();
        assert!(collect_board_entries(&state).is_empty());
        let content = BlackboardPanel::content(&state).to_string();
        assert!(
            content.contains("无黑板条目") || content.contains("No board entries"),
            "空应诚实展示无条目, got: {content}"
        );
    }

    #[test]
    fn entries_grouped_by_epistemic() {
        let mut state = TuiState::new();
        push(&mut state, board("z", "Observation", "s1"));
        push(&mut state, board("a", "Fact", "s2"));
        let items = collect_board_entries(&state);
        assert_eq!(items.len(), 2);
        // epistemic 升序:Fact < Observation
        assert_eq!(items[0].epistemic, "Fact");
        assert_eq!(items[1].epistemic, "Observation");
    }

    #[test]
    fn same_key_latest_wins() {
        let mut state = TuiState::new();
        push(&mut state, board("k", "Fact", "v1"));
        push(&mut state, board("k", "Episode", "v2"));
        let items = collect_board_entries(&state);
        assert_eq!(items.len(), 1, "同 key 去重");
        assert_eq!(items[0].epistemic, "Episode", "最新事件覆盖");
    }
}
