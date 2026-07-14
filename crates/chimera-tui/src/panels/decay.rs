//! TUI Decay 面板 — 显示衰减系数、历史 sparkline 与最近衰减事件
//!
//! 对应架构层:L10 Interface
//! 对应创新点:Ω-Evolve(能力衰减可视化)
//!
//! # 设计决策(WHY)
//! - 数据来源:L4 decay-engine 通过 `NexusEvent::DecayMetricsReported` 发布,
//!   经 `DecaySync` 同步到 `DataSnapshot.decay_metrics` + `decay_history`。
//! - 布局:垂直两区 — 上方文本信息(系数 + 周期 + 事件列表),
//!   下方 sparkline 历史曲线。信息密度低,垂直排列便于阅读。
//! - 高衰减判定:`coefficient < 0.3` 时标记 `[HIGH DECAY]` 并红色高亮。
//!
//! # 阈值语义说明
//! spec 中"衰减系数 > 0.7(高衰减)"指的是"衰减量 > 0.7",
//! 即 `1 - coefficient > 0.7`,等价于 `coefficient < 0.3`。
//! `coefficient = 1.0` 表示无衰减(满血),`0.0` 表示完全衰减。
//! 这样默认值 1.0(无衰减)不会误显示为红色高亮。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::{self, FOOTER_TEXT};
use crate::types::{PanelId, TuiCommand, TuiState};

/// 高衰减阈值:当 coefficient 低于此值时,表示衰减量 > 0.7(高衰减)。
///
/// WHY 0.3:spec "衰减系数 > 0.7(高衰减)" 指的是衰减量(1 - coefficient)> 0.7,
/// 即 coefficient < 0.3。此阈值确保默认值 1.0(无衰减)不会触发高亮。
const DECAY_CRITICAL_THRESHOLD: f32 = 0.3;

/// Decay 衰减面板 — 可视化能力衰减曲线与最近触发事件
///
/// 消费 `TuiState.decay_metrics`(当前系数 + 事件列表)与
/// `TuiState.decay_history`(sparkline 数据点)。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DecayPanel;

impl DecayPanel {
    /// 创建新的 Decay 面板
    pub fn new() -> Self {
        Self
    }

    /// 判断当前是否为高衰减状态
    ///
    /// WHY 提取为方法:阈值语义在多处使用(系数着色、sparkline 着色、标记显示),
    /// 统一入口避免散落的多处魔法数字。
    fn is_high_decay(coefficient: f32) -> bool {
        coefficient < DECAY_CRITICAL_THRESHOLD
    }

    /// 构建面板文本内容(系数 + 周期 + 事件列表)
    ///
    /// WHY 独立 pub 方法:与 BudgetPanel::content 模式一致,
    /// 便于单元测试无需 TestBackend 即可验证文本输出。
    pub fn content(state: &TuiState) -> Text<'static> {
        let dm = &state.decay_metrics;
        let high_decay = Self::is_high_decay(dm.coefficient);
        let coeff_pct = dm.coefficient * 100.0;

        let mut lines: Vec<Line<'static>> = Vec::new();

        // 系数行:高衰减时红色加粗 + [HIGH DECAY] 标记
        let coeff_style = if high_decay {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let coeff_text = if high_decay {
            format!("Coefficient:  {:.1}%  [HIGH DECAY]", coeff_pct)
        } else {
            format!("Coefficient:  {:.1}%", coeff_pct)
        };
        lines.push(Line::from(Span::styled(coeff_text, coeff_style)));

        // 周期开始时间
        let cycle_text = match dm.cycle_start {
            Some(ts) => format!("Cycle Start:  {}", ts.format("%Y-%m-%d %H:%M:%S UTC")),
            None => "Cycle Start:  (no cycle active)".to_string(),
        };
        lines.push(Line::from(cycle_text));

        lines.push(Line::from(""));

        // 最近衰减事件列表
        lines.push(Line::from(Span::styled(
            "Recent Events:",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        if dm.recent_events.is_empty() {
            lines.push(Line::from("  (no recent decay events)"));
        } else {
            for event in &dm.recent_events {
                lines.push(Line::from(format!("  {event}")));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(FOOTER_TEXT));

        Text::from(lines)
    }
}

impl Panel for DecayPanel {
    fn id(&self) -> PanelId {
        PanelId::Decay
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Decay ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        // 空间不足时仅渲染边框,避免内容溢出
        if inner.height < 8 || inner.width < 20 {
            return;
        }

        // 垂直布局:文本区(系数+周期+事件)+ sparkline 历史区
        // WHY Min(6) + Length(5):文本区至少 6 行(系数+周期+空行+标题+1事件+footer),
        // sparkline 固定 5 行(边框+标题+数据+边框)。
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(5)])
            .split(inner);

        // 上:文本内容(系数 + 周期 + 事件列表)
        Paragraph::new(Self::content(state)).render(chunks[0], buf);

        // 下:衰减历史 sparkline
        // WHY 颜色随阈值变化:高衰减时红色,正常时青色,与系数文本着色保持一致
        let high_decay = Self::is_high_decay(state.decay_metrics.coefficient);
        let sparkline_color = if high_decay { Color::Red } else { Color::Cyan };
        let sparkline = render::sparkline(&state.decay_history, "Decay History", sparkline_color);
        sparkline.render(chunks[1], buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DecayMetrics;
    use chrono::Utc;

    #[test]
    fn test_decay_panel_id() {
        let panel = DecayPanel::new();
        assert_eq!(panel.id(), PanelId::Decay);
    }

    #[test]
    fn test_decay_panel_title() {
        let panel = DecayPanel::new();
        let title = panel.title();
        assert_eq!(title.to_string(), " Decay ");
    }

    #[test]
    fn test_decay_panel_handle_key_returns_none() {
        let mut panel = DecayPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(panel.handle_key(key, &mut state).is_none());
    }

    #[test]
    fn test_decay_panel_handle_key_question_mark_returns_none() {
        let mut panel = DecayPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(
            crossterm::event::KeyCode::Char('?'),
            crossterm::event::KeyModifiers::NONE,
        );
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        assert_eq!(panel.handle_key(key, &mut state), None);
    }

    #[test]
    fn test_decay_panel_content_normal() {
        let mut state = TuiState::new();
        state.decay_metrics = DecayMetrics {
            coefficient: 0.85,
            recent_events: vec!["capability_frozen:cap-1".into()],
            cycle_start: Some(Utc::now()),
        };
        let content = DecayPanel::content(&state).to_string();
        assert!(
            content.contains("85.0%"),
            "normal coefficient should display"
        );
        assert!(
            !content.contains("HIGH DECAY"),
            "normal decay should not show HIGH DECAY marker"
        );
        assert!(
            content.contains("capability_frozen:cap-1"),
            "recent event should be listed"
        );
        assert!(
            content.contains("Cycle Start"),
            "cycle start label should be present"
        );
    }

    #[test]
    fn test_decay_panel_content_high_decay() {
        let mut state = TuiState::new();
        state.decay_metrics = DecayMetrics {
            coefficient: 0.25,
            recent_events: vec!["critical_decay_event".into()],
            cycle_start: Some(Utc::now()),
        };
        let content = DecayPanel::content(&state).to_string();
        assert!(
            content.contains("25.0%"),
            "high decay coefficient should display"
        );
        assert!(
            content.contains("HIGH DECAY"),
            "high decay should show HIGH DECAY marker"
        );
    }

    #[test]
    fn test_decay_panel_content_default_state() {
        let state = TuiState::new();
        let content = DecayPanel::content(&state).to_string();
        // 默认 coefficient = 1.0,应显示 100.0%
        assert!(
            content.contains("100.0%"),
            "default coefficient 1.0 should display as 100.0%"
        );
        assert!(
            !content.contains("HIGH DECAY"),
            "default state (1.0) should not show HIGH DECAY"
        );
        assert!(
            content.contains("no recent decay events"),
            "empty events should show placeholder"
        );
        assert!(
            content.contains("no cycle active"),
            "None cycle_start should show placeholder"
        );
    }

    #[test]
    fn test_decay_panel_content_empty_events() {
        let mut state = TuiState::new();
        state.decay_metrics = DecayMetrics {
            coefficient: 0.5,
            recent_events: vec![],
            cycle_start: Some(Utc::now()),
        };
        let content = DecayPanel::content(&state).to_string();
        assert!(
            content.contains("no recent decay events"),
            "empty events list should show placeholder"
        );
    }

    #[test]
    fn test_decay_threshold_logic() {
        // 阈值边界测试:0.3 是临界点(< 0.3 为高衰减)
        assert!(!DecayPanel::is_high_decay(1.0), "1.0 = no decay");
        assert!(!DecayPanel::is_high_decay(0.5), "0.5 = moderate decay");
        assert!(!DecayPanel::is_high_decay(0.3), "0.3 = boundary, not high");
        assert!(DecayPanel::is_high_decay(0.29), "0.29 < 0.3 = high decay");
        assert!(DecayPanel::is_high_decay(0.0), "0.0 = fully decayed");
    }
}
