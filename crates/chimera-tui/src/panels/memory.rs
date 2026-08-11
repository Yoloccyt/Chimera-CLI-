//! TUI Memory 面板 — 显示缓存命中率、上下文窗口、压缩率与层级
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 纯数据驱动:从 `TuiState.memory_metrics` 与 `memory_history` 读取,
//!   不直接依赖 L2/L3 crate。
//! - 使用 `render.rs` 中的 `sparkline`/`gauge`/`utilization_bar` 统一视觉风格。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::{self, FOOTER_TEXT};
use crate::types::{PanelId, TuiCommand, TuiState};

/// Memory 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MemoryPanel;

impl MemoryPanel {
    /// 创建新的 Memory 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建左侧信息文本
    fn info_text(state: &TuiState) -> Text<'static> {
        let mm = &state.memory_metrics;
        let lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.cache_hit_rate")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1}%", mm.hit_rate_percent)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.evictions")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(mm.evictions.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.context_window")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{} bytes", mm.context_window_size)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.compressed_ratio")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1}%", mm.compressed_ratio * 100.0)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.cache_hits")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(mm.cache_hits.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.cache_misses")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(mm.cache_misses.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::t!("panel.memory.tier")),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(mm.tier.clone()),
            ]),
            Line::from(""),
            // Task 3.3: L3 Storage 协同 — 显示四层存储分布(全局快照)
            // 调用 cmt_tiering::tier_distribution() 获取 CMT 四级存储层字节数,
            // 实现 L10 Panel ↔ L3 Storage 真实数据闭环。
            {
                let dist = cmt_tiering::tier_distribution();
                let to_mb = |bytes: u64| -> f64 { bytes as f64 / 1_048_576.0 };
                Line::from(format!(
                    "{}: Hot:{:.1}MB | Warm:{:.1}MB | Cold:{:.1}MB | Frozen:{:.1}MB",
                    crate::t!("panel.memory.storage"),
                    to_mb(dist.hot),
                    to_mb(dist.warm),
                    to_mb(dist.cold),
                    to_mb(dist.frozen)
                ))
            },
            Line::from(""),
            Line::from(FOOTER_TEXT),
        ];
        Text::from(lines)
    }
}

impl Panel for MemoryPanel {
    fn id(&self) -> PanelId {
        PanelId::Memory
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.memory"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 20 {
            // 空间不足时仅渲染标题
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // 左:文字指标
        let info = Paragraph::new(Self::info_text(state));
        info.render(chunks[0], buf);

        // 右:命中率 Gauge + 历史 Sparkline
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(5)])
            .split(chunks[1]);

        let gauge = render::gauge(
            state.memory_metrics.hit_rate_percent as f64,
            100.0,
            &format!("{:.1}%", state.memory_metrics.hit_rate_percent),
            Color::Green,
        );
        gauge.render(right_chunks[0], buf);

        let sparkline = render::sparkline(
            &state.memory_history,
            crate::t!("panel.memory.hit_rate_history"),
            Color::Cyan,
        );
        sparkline.render(right_chunks[1], buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // `R` 刷新:发布 RequestRefresh(与 `:refresh` 命令同语义)。
        // WHY 补齐 shortcuts() 声明但无实现的按键(快捷键诚实性)。
        if key.code == KeyCode::Char('R') {
            return Some(TuiCommand::RequestRefresh);
        }
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("R", crate::t!("shortcut.refresh"))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::MemoryMetrics;
    use crate::types::TuiState;

    #[test]
    fn test_memory_panel_id() {
        let panel = MemoryPanel::new();
        assert_eq!(panel.id(), PanelId::Memory);
    }

    #[test]
    fn handle_key_r_returns_request_refresh() {
        // 快捷键诚实性:R 刷新声明即可达(此前 handle_key 恒 None)
        let mut panel = MemoryPanel::new();
        let mut state = TuiState::new();
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('R'), crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
    }

    #[test]
    fn test_memory_panel_renders_metrics() {
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let mut state = TuiState::new();
        state.memory_metrics = MemoryMetrics {
            hit_rate_percent: 87.5,
            evictions: 12,
            context_window_size: 4096,
            compressed_ratio: 0.72,
            cache_hits: 120,
            cache_misses: 18,
            tier: "L1".into(),
        };
        state.memory_history = vec![80, 82, 85, 83, 86, 88, 87];

        let panel = MemoryPanel::new();
        let content = MemoryPanel::info_text(&state).to_string();
        assert!(content.contains("87.5%"));
        assert!(content.contains("4096 bytes"));
        assert!(content.contains("72.0%"));
        assert!(content.contains("L1"));
        assert_eq!(panel.id(), PanelId::Memory);
    }
}
