//! TUI Help 面板 — 显示快捷键说明
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 从 `app.rs` 迁移而来,保持 M1 行为不变。
//! - Help 面板支持上下文感知:当 `context_panel_id` 为 Some 时,
//!   在全局快捷键之后追加当前面板的专属快捷键章节。
//! - 上下文感知模式由 `with_context()` 构造器激活,`new()` 返回无上下文模式。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// Help 面板 — 支持上下文感知的快捷键说明
///
/// 当 `context_panel_id` 为 `None` 时仅显示全局快捷键;
/// 为 `Some(id)` 时在全局快捷键后追加该面板的专属快捷键。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct HelpPanel {
    /// 上下文面板 ID,None 表示仅显示全局快捷键
    context_panel_id: Option<PanelId>,
}

impl HelpPanel {
    /// 创建新的 Help 面板(无上下文,仅显示全局快捷键)
    pub fn new() -> Self {
        Self {
            context_panel_id: None,
        }
    }

    /// 创建带上下文的面板,显示指定面板的专属快捷键
    pub fn with_context(panel_id: PanelId) -> Self {
        Self {
            context_panel_id: Some(panel_id),
        }
    }

    /// 返回上下文面板 ID
    pub fn context(&self) -> Option<PanelId> {
        self.context_panel_id
    }

    /// 返回全局快捷键条目
    fn global_shortcuts() -> Vec<(&'static str, &'static str)> {
        vec![
            ("Tab", "Next panel"),
            ("Shift+Tab", "Previous panel"),
            ("1-8", "Jump to panel"),
            (":", "Command mode"),
            ("/", "Search mode (keyword filter)"),
            ("Enter", "Submit command/search"),
            ("Esc", "Cancel input / close popup"),
            ("Ctrl+Up", "Increase main panel ratio"),
            ("Ctrl+Down", "Decrease main panel ratio"),
            ("q / Esc", "Quit (normal mode)"),
            ("?", "Show this help"),
            ("t", "Switch theme"),
            ("l", "Switch layout"),
            ("g+1-6", "Jump to extended panels"),
            ("g g", "Scroll to top"),
            ("G", "Scroll to bottom"),
            ("F1-F8", "Jump to panel (F-keys)"),
        ]
    }

    /// 构建 Help 面板文本内容
    ///
    /// 如果 `context` 为 None,显示全局快捷键;
    /// 如果 `context` 为 Some,在全局快捷键之后追加"Panel Shortcuts"章节。
    pub fn content(
        context: Option<PanelId>,
        panel_shortcuts: &[(&'static str, &'static str)],
    ) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Help"),
            Line::from("────────────────────────────────────────"),
            Line::from(""),
            Line::from("Global Shortcuts"),
            Line::from("─────────────"),
        ];

        for (key, desc) in Self::global_shortcuts() {
            lines.push(Line::from(format!("  {:<16} - {}", key, desc)));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(
            "Commands: find <k>, filter <topic>, level <severity>,",
        ));
        lines.push(Line::from("          pause <quest>, resume <quest>,"));
        lines.push(Line::from(
            "          vote <yes|no|abstain> <proposal>, refresh, quit",
        ));

        // 如果提供了上下文,追加面板专属快捷键
        if let Some(panel_id) = context {
            if !panel_shortcuts.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Panel Shortcuts — {}",
                    panel_id.as_str()
                )));
                lines.push(Line::from("─────────────"));
                for (key, desc) in panel_shortcuts {
                    lines.push(Line::from(format!("  {:<16} - {}", key, desc)));
                }
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from("Chimera CLI NEXUS-OMEGA"));

        Text::from(lines)
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
        let content = Self::content(self.context_panel_id, &[]);
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(content).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Esc", "关闭"), ("?", "显示帮助")]
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
    fn test_help_panel_new_has_no_context() {
        let panel = HelpPanel::new();
        assert!(panel.context().is_none());
    }

    #[test]
    fn test_help_panel_with_context() {
        let panel = HelpPanel::with_context(PanelId::Quest);
        assert_eq!(panel.context(), Some(PanelId::Quest));
    }

    #[test]
    fn test_help_panel_content_no_context() {
        let content = HelpPanel::content(None, &[]).to_string();
        assert!(content.contains("Help"));
        assert!(content.contains("Global Shortcuts"));
        assert!(content.contains("Tab"));
        assert!(content.contains("Shift+Tab"));
        assert!(content.contains("q / Esc"));
        assert!(content.contains("F1-F8"));
        assert!(content.contains("Switch theme"));
        assert!(content.contains("Switch layout"));
        assert!(content.contains("Chimera CLI NEXUS-OMEGA"));
        // 无上下文时不应包含"Panel Shortcuts"
        assert!(!content.contains("Panel Shortcuts"));
    }

    #[test]
    fn test_help_panel_content_with_context() {
        let panel_shortcuts = vec![("↑/↓", "Navigate"), ("Enter", "Detail"), ("/", "Search")];
        let content = HelpPanel::content(Some(PanelId::Quest), &panel_shortcuts).to_string();
        assert!(content.contains("Help"));
        assert!(content.contains("Global Shortcuts"));
        assert!(content.contains("Panel Shortcuts"));
        assert!(content.contains("Quest"));
        assert!(content.contains("Navigate"));
        assert!(content.contains("Detail"));
        assert!(content.contains("Search"));
    }

    #[test]
    fn test_help_panel_content_with_context_empty_shortcuts() {
        // 有上下文但快捷键为空时,不应显示"Panel Shortcuts"章节
        let content = HelpPanel::content(Some(PanelId::Budget), &[]).to_string();
        assert!(content.contains("Global Shortcuts"));
        // 空快捷键列表不追加 Panel Shortcuts 章节
        assert!(!content.contains("Panel Shortcuts"));
    }

    #[test]
    fn test_help_panel_shortcuts() {
        let panel = HelpPanel::new();
        let shortcuts = panel.shortcuts();
        assert_eq!(shortcuts.len(), 2, "HelpPanel 应包含 2 条快捷键: Esc + ?");
        assert_eq!(shortcuts[0].0, "Esc");
        assert_eq!(shortcuts[0].1, "关闭");
        assert_eq!(shortcuts[1].0, "?");
        assert_eq!(shortcuts[1].1, "显示帮助");
    }
}
