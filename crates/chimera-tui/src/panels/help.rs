//! TUI Help 面板 — 显示快捷键说明
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变。
//! - Help 面板不处理按键,仅作为静态说明页。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// Help 面板
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct HelpPanel;

impl HelpPanel {
    /// 创建新的 Help 面板
    pub fn new() -> Self {
        Self
    }

    /// 构建 Help 面板文本内容
    fn content() -> Text<'static> {
        Text::from(
            "Help\n─────────────\nTab       - Next panel\nShift+Tab - Previous panel\n1-8       - Jump to panel\nF1        - Quest\nF2        - Parliament\nF3        - Budget\nF6        - Memory\nF7        - Security\nF8        - Health\n:         - Command mode\n/         - Search mode (M1 stub)\nq / Esc   - Quit\n?         - Show help\n\nChimera CLI NEXUS-OMEGA",
        )
    }
}

impl Panel for HelpPanel {
    fn id(&self) -> PanelId {
        PanelId::Help
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Help ")
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content()).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_panel_id() {
        let panel = HelpPanel::new();
        assert_eq!(panel.id(), PanelId::Help);
    }

    #[test]
    fn test_help_panel_content() {
        let content = HelpPanel::content().to_string();
        assert!(content.contains("Help"));
        assert!(content.contains("Tab"));
        assert!(content.contains("q / Esc"));
        assert!(content.contains("F1        - Quest"));
        assert!(content.contains("F6        - Memory"));
        assert!(content.contains("F7        - Security"));
        assert!(content.contains("F8        - Health"));
        assert!(content.contains("Chimera CLI NEXUS-OMEGA"));
    }
}
