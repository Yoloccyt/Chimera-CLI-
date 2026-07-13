//! TUI 命令面板 — 底部命令/搜索输入栏
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 命令面板为无状态解析器:输入状态保存在 `TuiState` 中,
//!   便于面板与 `TuiApp` 统一访问。
//! - M1 仅支持面板切换与退出命令;搜索模式为占位,
//!   后续可在 `submit` 中接入真实搜索逻辑。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::types::{InputMode, PanelId, TuiCommand, TuiState};

/// 命令面板 — 解析并执行底部输入栏的命令
///
/// M1 为无状态结构体;未来可在此扩展命令历史、自动补全等状态。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CommandPalette;

impl CommandPalette {
    /// 创建新的命令面板
    pub fn new() -> Self {
        Self
    }

    /// 根据当前输入模式渲染底部输入栏
    pub fn render(&self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let (prefix, title) = match state.input_mode {
            InputMode::Command => (":", " Command "),
            InputMode::Search => ("/", " Search (M1 stub) "),
            InputMode::Normal => return,
        };

        let content = format!("{}{}", prefix, state.input_buffer);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().fg(Color::Yellow));
        let paragraph = Paragraph::new(Line::from(content)).block(block);
        paragraph.render(area, buf);
    }

    /// 处理命令/搜索模式下的按键
    ///
    /// - Esc:取消输入,返回 Normal 模式
    /// - Enter:提交当前输入并返回解析后的命令(如果有)
    /// - 可打印字符:追加到输入缓冲
    /// - Backspace:删除最后一个字符
    pub fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Esc => {
                state.input_mode = InputMode::Normal;
                state.input_buffer.clear();
                None
            }
            KeyCode::Enter => self.submit(state),
            KeyCode::Char(c) => {
                state.input_buffer.push(c);
                None
            }
            KeyCode::Backspace => {
                state.input_buffer.pop();
                None
            }
            _ => None,
        }
    }

    /// 提交当前输入并解析命令
    ///
    /// 解析完成后清空输入缓冲并恢复 Normal 模式。
    pub fn submit(&mut self, state: &mut TuiState) -> Option<TuiCommand> {
        let input = state.input_buffer.trim();
        let cmd = match state.input_mode {
            InputMode::Command => Self::parse_command(input),
            InputMode::Search => {
                // M1 搜索为占位:接受输入但不执行任何操作。
                None
            }
            InputMode::Normal => None,
        };

        state.input_mode = InputMode::Normal;
        state.input_buffer.clear();
        cmd
    }

    /// 解析命令字符串
    ///
    /// 支持的命令(冒号可省略,因为进入命令模式时已输入冒号):
    /// - `quest`/`parliament`/`budget`/`memory`/`security`/`health`/`log`/`help`:切换面板
    /// - `quit`:退出应用
    fn parse_command(input: &str) -> Option<TuiCommand> {
        let cmd = input.strip_prefix(':').unwrap_or(input).trim();
        match cmd {
            "quest" => Some(TuiCommand::SwitchPanel(PanelId::Quest)),
            "parliament" => Some(TuiCommand::SwitchPanel(PanelId::Parliament)),
            "budget" => Some(TuiCommand::SwitchPanel(PanelId::Budget)),
            "memory" => Some(TuiCommand::SwitchPanel(PanelId::Memory)),
            "security" => Some(TuiCommand::SwitchPanel(PanelId::Security)),
            "health" => Some(TuiCommand::SwitchPanel(PanelId::Health)),
            "log" => Some(TuiCommand::SwitchPanel(PanelId::Log)),
            "help" => Some(TuiCommand::SwitchPanel(PanelId::Help)),
            "quit" => Some(TuiCommand::Quit),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_state(input: &str) -> TuiState {
        let mut state = TuiState::new();
        state.input_mode = InputMode::Command;
        state.input_buffer = input.to_string();
        state
    }

    #[test]
    fn test_parse_panel_commands() {
        assert_eq!(
            CommandPalette::parse_command("quest"),
            Some(TuiCommand::SwitchPanel(PanelId::Quest))
        );
        assert_eq!(
            CommandPalette::parse_command("parliament"),
            Some(TuiCommand::SwitchPanel(PanelId::Parliament))
        );
        assert_eq!(
            CommandPalette::parse_command("budget"),
            Some(TuiCommand::SwitchPanel(PanelId::Budget))
        );
        assert_eq!(
            CommandPalette::parse_command("memory"),
            Some(TuiCommand::SwitchPanel(PanelId::Memory))
        );
        assert_eq!(
            CommandPalette::parse_command("security"),
            Some(TuiCommand::SwitchPanel(PanelId::Security))
        );
        assert_eq!(
            CommandPalette::parse_command("health"),
            Some(TuiCommand::SwitchPanel(PanelId::Health))
        );
        assert_eq!(
            CommandPalette::parse_command("log"),
            Some(TuiCommand::SwitchPanel(PanelId::Log))
        );
        assert_eq!(
            CommandPalette::parse_command("help"),
            Some(TuiCommand::SwitchPanel(PanelId::Help))
        );
    }

    #[test]
    fn test_parse_quit_command() {
        assert_eq!(
            CommandPalette::parse_command("quit"),
            Some(TuiCommand::Quit)
        );
    }

    #[test]
    fn test_parse_unknown_command() {
        assert_eq!(CommandPalette::parse_command("foo"), None);
    }

    #[test]
    fn test_submit_command_switches_panel() {
        let mut palette = CommandPalette::new();
        let mut state = command_state("budget");
        let cmd = palette.submit(&mut state);

        assert_eq!(cmd, Some(TuiCommand::SwitchPanel(PanelId::Budget)));
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_submit_unknown_command_clears_state() {
        let mut palette = CommandPalette::new();
        let mut state = command_state("unknown");
        let cmd = palette.submit(&mut state);

        assert_eq!(cmd, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_handle_key_appends_input() {
        let mut palette = CommandPalette::new();
        let mut state = TuiState::new();
        state.input_mode = InputMode::Command;

        palette.handle_key(
            KeyEvent::new(KeyCode::Char('q'), crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(state.input_buffer, "q");
    }

    #[test]
    fn test_handle_key_esc_cancels() {
        let mut palette = CommandPalette::new();
        let mut state = command_state("budget");
        let cmd = palette.handle_key(
            KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );

        assert_eq!(cmd, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
    }
}
