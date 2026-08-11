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
            ("Tab", crate::t!("help.sc.next")),
            ("Shift+Tab", crate::t!("help.sc.prev")),
            ("1-8", crate::t!("help.sc.jump")),
            (":", crate::t!("help.sc.command")),
            ("/", crate::t!("help.sc.search")),
            ("Enter", crate::t!("help.sc.submit")),
            ("Esc", crate::t!("help.sc.cancel")),
            ("Ctrl+Up", crate::t!("help.sc.ratio_up")),
            ("Ctrl+Down", crate::t!("help.sc.ratio_down")),
            ("q / Esc", crate::t!("help.sc.quit")),
            ("?", crate::t!("help.sc.help")),
            ("t", crate::t!("help.sc.theme")),
            ("l", crate::t!("help.sc.layout")),
            ("a", crate::t!("help.sc.panel_actions")),
            ("g+1-6", crate::t!("help.sc.gjump")),
            ("g g", crate::t!("help.sc.top")),
            ("G", crate::t!("help.sc.bottom")),
            ("F1-F8", crate::t!("help.sc.fkeys")),
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
            Line::from(crate::t!("help.header")),
            Line::from("────────────────────────────────────────"),
            Line::from(""),
            Line::from(crate::t!("help.section.global")),
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
                    "{} — {}",
                    crate::t!("help.section.panel"),
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
        Line::from(crate::t!("panel.border.help"))
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
        // WHY 移除 "Esc 关闭":Help 是面板而非弹窗,Normal 下 Esc 是全局退出键;
        // 帮助浮层的关闭由弹窗层 Esc 处理(open_help_action 弹出的 overlay)。
        vec![("?", crate::t!("shortcut.show_help"))]
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
        let _locale_guard = crate::i18n::locale_test_guard();
        // i18n:content 已本地化;固定英文捕获后复位,断言 ASCII 文案。
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let content = HelpPanel::content(None, &[]).to_string();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
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
        let _locale_guard = crate::i18n::locale_test_guard();
        // i18n:content 已本地化;固定英文捕获后复位,断言 ASCII 文案。
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let content = HelpPanel::content(Some(PanelId::Quest), &panel_shortcuts).to_string();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
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
        let _locale_guard = crate::i18n::locale_test_guard();
        // i18n:content 已本地化;固定英文捕获后复位,断言 ASCII 文案。
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let content = HelpPanel::content(Some(PanelId::Budget), &[]).to_string();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        assert!(content.contains("Global Shortcuts"));
        // 空快捷键列表不追加 Panel Shortcuts 章节
        assert!(!content.contains("Panel Shortcuts"));
    }

    #[test]
    fn test_help_panel_shortcuts() {
        let panel = HelpPanel::new();
        let shortcuts = panel.shortcuts();
        // 快捷键诚实性:Help 是面板而非弹窗,Normal 下 Esc 是全局退出键;
        // 移除虚假的 "Esc 关闭" 提示(帮助浮层关闭由弹窗层 Esc 处理)。
        assert_eq!(shortcuts.len(), 1, "HelpPanel 应只声明 `?` 一条快捷键");
        assert_eq!(shortcuts[0].0, "?");
        assert_eq!(shortcuts[0].1, "显示帮助");
    }
}
