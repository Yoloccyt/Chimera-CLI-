//! TUI 命令面板 — 底部命令/搜索输入栏
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 命令面板为无状态解析器:输入状态保存在 `TuiState` 中,
//!   便于面板与 `TuiApp` 统一访问。
//! - M3 扩展命令解析,支持 `:find`/`:filter`/`:level`/`:refresh` 等
//!   过滤器命令;搜索模式提交后设置全局关键字过滤器。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::popup::Severity;
use crate::types::{InputMode, PanelId, TuiCommand, TuiState};
use event_bus::VoteValue;

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
            InputMode::Search => ("/", " Search "),
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
                // WHY:搜索模式 Esc 需清除过滤器,避免残留关键字导致面板为空
                if state.input_mode == InputMode::Search {
                    state.filter_keyword = None;
                }
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
        let input = state.input_buffer.trim().to_string();
        let cmd = match state.input_mode {
            InputMode::Command => Self::parse_command(&input, state),
            InputMode::Search => {
                // M3:搜索输入作为全局关键字过滤器
                if input.is_empty() {
                    state.filter_keyword = None;
                } else {
                    state.filter_keyword = Some(input.to_lowercase());
                }
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
    /// - `find <keyword>`:设置关键字过滤器
    /// - `filter <topic>`:设置主题过滤器
    /// - `level <severity>`:设置级别过滤器
    /// - `refresh`:发布 `RefreshStateRequested` 控制请求事件,由上游决定是否重载/清空过滤器
    fn parse_command(input: &str, state: &mut TuiState) -> Option<TuiCommand> {
        let cmd = input.strip_prefix(':').unwrap_or(input).trim();
        if cmd.is_empty() {
            return None;
        }

        // 先处理无参数命令,避免被下面的 split 逻辑覆盖
        match cmd {
            "quest" => return Some(TuiCommand::SwitchPanel(PanelId::Quest)),
            "parliament" => return Some(TuiCommand::SwitchPanel(PanelId::Parliament)),
            "budget" => return Some(TuiCommand::SwitchPanel(PanelId::Budget)),
            "memory" => return Some(TuiCommand::SwitchPanel(PanelId::Memory)),
            "security" => return Some(TuiCommand::SwitchPanel(PanelId::Security)),
            "health" => return Some(TuiCommand::SwitchPanel(PanelId::Health)),
            "log" => return Some(TuiCommand::SwitchPanel(PanelId::Log)),
            "help" => return Some(TuiCommand::SwitchPanel(PanelId::Help)),
            "quit" => return Some(TuiCommand::Quit),
            "refresh" => return Some(TuiCommand::RequestRefresh),
            _ => {}
        }

        // 处理带参数命令
        let mut parts = cmd.splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();

        match name {
            "find" => {
                if arg.is_empty() {
                    state.set_status("find requires an argument", Severity::Error);
                    return None;
                }
                state.filter_keyword = Some(arg.to_lowercase());
                None
            }
            "filter" => {
                if arg.is_empty() {
                    state.set_status("filter requires an argument", Severity::Error);
                    return None;
                }
                if is_valid_topic(arg) {
                    state.filter_topic = Some(arg.to_lowercase());
                    None
                } else {
                    state.set_status(
                        format!("invalid topic '{}': expected quest|security|memory|health|parliament|budget|system", arg),
                        Severity::Error,
                    );
                    None
                }
            }
            "level" => {
                if arg.is_empty() {
                    state.set_status("level requires an argument", Severity::Error);
                    return None;
                }
                let level = arg.to_lowercase();
                if matches!(level.as_str(), "info" | "warn" | "error" | "critical") {
                    state.filter_level = Some(level);
                    None
                } else {
                    state.set_status(
                        format!("invalid level '{}': expected info|warn|error|critical", arg),
                        Severity::Error,
                    );
                    None
                }
            }
            "pause" => {
                if arg.is_empty() {
                    state.set_status("pause requires a quest id", Severity::Error);
                    return None;
                }
                // M4 review fix:统一走 TuiCommand::RequestQuestPause,
                // 由 TuiApp::apply_command 负责弹出确认框,避免两条控制路径并存。
                Some(TuiCommand::RequestQuestPause(arg.to_string()))
            }
            "resume" => {
                if arg.is_empty() {
                    state.set_status("resume requires a quest id", Severity::Error);
                    return None;
                }
                Some(TuiCommand::RequestQuestResume(arg.to_string()))
            }
            "vote" => Self::parse_vote_command(arg, state),
            _ => {
                state.set_status(format!("unknown command '{}'", cmd), Severity::Error);
                None
            }
        }
    }

    /// 解析 `:vote <yes|no|abstain> <proposal-id>` 命令
    ///
    /// 返回 `TuiCommand::RequestVote`;若参数非法则设置状态消息并返回 None。
    fn parse_vote_command(arg: &str, state: &mut TuiState) -> Option<TuiCommand> {
        if arg.trim().is_empty() {
            state.set_status(
                "vote requires a vote value and proposal id",
                Severity::Error,
            );
            return None;
        }

        let mut parts = arg.splitn(2, ' ');
        let vote_str = parts.next().unwrap_or("").trim();
        let proposal_id = parts.next().unwrap_or("").trim();

        let vote = match vote_str.parse::<VoteValue>() {
            Ok(v) => v,
            Err(()) => {
                state.set_status(
                    format!("invalid vote '{}': expected yes|no|abstain", vote_str),
                    Severity::Error,
                );
                return None;
            }
        };

        if proposal_id.is_empty() {
            state.set_status("vote requires a proposal id", Severity::Error);
            return None;
        }

        // M4 review fix:统一走 TuiCommand::RequestVote,由 TuiApp 负责确认弹窗。
        Some(TuiCommand::RequestVote {
            proposal_id: proposal_id.to_string(),
            vote,
        })
    }
}

/// 校验主题参数是否合法
fn is_valid_topic(topic: &str) -> bool {
    matches!(
        topic.to_lowercase().as_str(),
        "quest" | "security" | "memory" | "health" | "parliament" | "budget" | "system"
    )
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
            CommandPalette::parse_command("quest", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Quest))
        );
        assert_eq!(
            CommandPalette::parse_command("parliament", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Parliament))
        );
        assert_eq!(
            CommandPalette::parse_command("budget", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Budget))
        );
        assert_eq!(
            CommandPalette::parse_command("memory", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Memory))
        );
        assert_eq!(
            CommandPalette::parse_command("security", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Security))
        );
        assert_eq!(
            CommandPalette::parse_command("health", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Health))
        );
        assert_eq!(
            CommandPalette::parse_command("log", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Log))
        );
        assert_eq!(
            CommandPalette::parse_command("help", &mut TuiState::new()),
            Some(TuiCommand::SwitchPanel(PanelId::Help))
        );
    }

    #[test]
    fn test_parse_quit_command() {
        assert_eq!(
            CommandPalette::parse_command("quit", &mut TuiState::new()),
            Some(TuiCommand::Quit)
        );
    }

    #[test]
    fn test_parse_unknown_command() {
        let mut state = TuiState::new();
        assert_eq!(CommandPalette::parse_command("foo", &mut state), None);
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("unknown command"),
            "status should report unknown command"
        );
    }

    #[test]
    fn test_parse_find_command() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("find error", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_keyword, Some("error".into()));
    }

    #[test]
    fn test_parse_filter_command_valid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("filter security", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_topic, Some("security".into()));
    }

    #[test]
    fn test_parse_filter_command_invalid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("filter foo", &mut state);
        assert_eq!(cmd, None);
        assert!(state.filter_topic.is_none());
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("invalid topic"),
            "status should report invalid topic"
        );
    }

    #[test]
    fn test_parse_level_command_valid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("level critical", &mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.filter_level, Some("critical".into()));
    }

    #[test]
    fn test_parse_level_command_invalid() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("level foo", &mut state);
        assert_eq!(cmd, None);
        assert!(state.filter_level.is_none());
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("invalid level"),
            "status should report invalid level"
        );
    }

    #[test]
    fn test_parse_refresh_command_returns_request() {
        let mut state = TuiState::new();
        state.filter_keyword = Some("foo".into());
        state.filter_topic = Some("security".into());
        state.filter_level = Some("critical".into());

        // M4:refresh 现在作为控制请求发布,由上游订阅者决定是否清空过滤器,
        // 命令面板本身不再直接修改过滤器状态。
        let cmd = CommandPalette::parse_command("refresh", &mut state);
        assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
        assert!(state.filter_keyword.is_some());
        assert!(state.filter_topic.is_some());
        assert!(state.filter_level.is_some());
    }

    #[test]
    fn test_parse_missing_argument() {
        let mut state = TuiState::new();
        let cmd = CommandPalette::parse_command("find", &mut state);
        assert_eq!(cmd, None);
        assert!(
            state
                .status_message
                .as_ref()
                .unwrap()
                .0
                .contains("requires an argument"),
            "status should report missing argument"
        );
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
    fn test_submit_search_sets_keyword() {
        let mut palette = CommandPalette::new();
        let mut state = TuiState::new();
        state.input_mode = InputMode::Search;
        state.input_buffer = "Error".into();

        let cmd = palette.submit(&mut state);
        assert_eq!(cmd, None);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.filter_keyword, Some("error".into()));
    }

    #[test]
    fn test_submit_empty_search_clears_keyword() {
        let mut palette = CommandPalette::new();
        let mut state = TuiState::new();
        state.input_mode = InputMode::Search;
        state.filter_keyword = Some("old".into());
        state.input_buffer = "   ".into();

        let cmd = palette.submit(&mut state);
        assert_eq!(cmd, None);
        assert!(state.filter_keyword.is_none());
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

    #[test]
    fn test_handle_key_esc_in_search_clears_keyword() {
        let mut palette = CommandPalette::new();
        let mut state = TuiState::new();
        state.input_mode = InputMode::Search;
        state.filter_keyword = Some("old".into());
        state.input_buffer = "new".into();

        let cmd = palette.handle_key(
            KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );

        assert_eq!(cmd, None);
        assert!(state.filter_keyword.is_none());
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
    }
}
