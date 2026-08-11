//! TUI Budget 面板 — 显示预算级别、消耗与利用率
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移原有渲染逻辑,保持进度条与超限高亮行为不变。
//! - 使用 `Panel` trait 统一接口,便于 `TuiApp` 通过 `Box<dyn Panel>` 管理。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// Budget 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BudgetPanel;

impl BudgetPanel {
    /// 创建新的 Budget 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Budget 面板文本内容
    pub fn content(state: &TuiState) -> Text<'static> {
        let budget = &state.budget;
        let total = budget.total_consumption + budget.remaining_budget;
        let utilization_pct = budget.utilization_rate * 100.0;

        // 基础信息行
        let mut lines: Vec<Line<'static>> = vec![
            Line::from(crate::t!("panel.budget.title")),
            Line::from("─────────────"),
            Line::from(format!(
                "{}: {}",
                crate::t!("panel.budget.tier"),
                budget.current_tier
            )),
            Line::from(format!(
                "{}:  {:.1}",
                crate::t!("panel.budget.coefficient"),
                budget.coefficient
            )),
            Line::from(format!(
                "{}:  {:.1} / {:.1}",
                crate::t!("panel.budget.consumption"),
                budget.total_consumption,
                total
            )),
            Line::from(format!(
                "{}:    {:.1}",
                crate::t!("panel.budget.remaining"),
                budget.remaining_budget
            )),
            Line::from(format!(
                "{}:  {:.1}%",
                crate::t!("panel.budget.utilization"),
                utilization_pct
            )),
        ];

        // Status 行:超限时红色加粗,否则默认样式
        let status_text = if budget.is_exceeded { "EXCEEDED" } else { "OK" };
        let status_style = if budget.is_exceeded {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!(
                "{}:       {}",
                crate::t!("panel.budget.status"),
                status_text
            ),
            status_style,
        )));

        // 利用率进度条:宽度 30,已用 = Cyan,剩余 = Gray
        const BAR_WIDTH: usize = 30;
        let clamped_rate = budget.utilization_rate.clamp(0.0, 1.0);
        let used_chars = ((clamped_rate * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH);
        let remaining_chars = BAR_WIDTH - used_chars;
        let bar_label = format!("{:.1}%", utilization_pct);
        lines.push(Line::from(vec![
            Span::from("["),
            Span::styled("=".repeat(used_chars), Style::default().fg(Color::Cyan)),
            Span::styled(
                "-".repeat(remaining_chars),
                Style::default().fg(Color::Gray),
            ),
            Span::from(format!("] {}", bar_label)),
        ]));

        // Alert 行:存在时显示;超限时同样红色加粗
        if let Some(ref alert) = budget.alert {
            let alert_style = if budget.is_exceeded {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{}:        {}", crate::t!("panel.budget.alert"), alert),
                alert_style,
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(crate::t!("panel.budget.hint")));

        // Concord T1.7(消费 budget_metrics_ttl_ms):指标陈旧时整体置灰 +
        // 标题下方插入过期提示——对过期数据给诚实反馈而非伪造新鲜度。
        if state.budget_metrics_stale {
            let stale_style = Style::default().fg(Color::DarkGray);
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style = stale_style;
                }
            }
            lines.insert(
                2,
                Line::from(Span::styled(
                    format!("⚠ {}", crate::t!("panel.budget.stale")),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )),
            );
        }

        Text::from(lines)
    }
}

impl Panel for BudgetPanel {
    fn id(&self) -> PanelId {
        PanelId::Budget
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.budget"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // `R` 刷新:发布 RequestRefresh(与 `:refresh` 命令同语义,由上游决定
        // 是否重载/清空过滤器)。WHY 补齐 shortcuts() 声明但无实现的按键
        // (快捷键诚实性:声明即可达)。
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
    use crate::data::BudgetMetrics;

    #[test]
    fn test_budget_panel_id() {
        let panel = BudgetPanel::new();
        assert_eq!(panel.id(), PanelId::Budget);
    }

    #[test]
    fn handle_key_r_returns_request_refresh() {
        // 快捷键诚实性:R 刷新声明即可达(此前 handle_key 恒 None)
        let mut panel = BudgetPanel::new();
        let mut state = TuiState::new();
        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Char('R'), crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
    }

    #[test]
    fn test_budget_panel_default_state() {
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let state = TuiState::new();
        let content = BudgetPanel::content(&state).to_string();
        assert!(content.contains("Budget"));
        assert!(content.contains("High"));
        assert!(content.contains("OK"));
    }

    #[test]
    fn test_budget_panel_exceeded_state() {
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let mut state = TuiState::new();
        state.budget = BudgetMetrics {
            total_consumption: 9500.0,
            remaining_budget: 500.0,
            utilization_rate: 0.95,
            current_tier: "Critical".into(),
            coefficient: 1.2,
            is_exceeded: true,
            alert: Some("Budget cap exceeded".into()),
        };
        let content = BudgetPanel::content(&state).to_string();
        assert!(content.contains("EXCEEDED"));
        assert!(content.contains("Budget cap exceeded"));
        assert!(content.contains("Critical"));
    }

    #[test]
    fn test_budget_panel_zh_body_labels() {
        // U-3:面板正文随 locale 切换(中文标签),不再硬编码英文
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let state = TuiState::new();
        let content = BudgetPanel::content(&state).to_string();
        assert!(content.contains("当前档位"));
        assert!(content.contains("利用率"));
    }
}
