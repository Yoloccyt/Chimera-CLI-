//! CommandPalette 集成测试 — 验证命令解析、过滤器命令与搜索模式

#![forbid(unsafe_code)]

use chimera_tui::{CommandPalette, InputMode, PanelId, PopupKind, Severity, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn command_state(input: &str) -> TuiState {
    let mut state = TuiState::new();
    state.input_mode = InputMode::Command;
    state.input_buffer = input.to_string();
    state
}

#[test]
fn command_palette_parses_panel_switch_commands() {
    let mut palette = CommandPalette::new();

    let cases = [
        ("quest", PanelId::Quest),
        ("parliament", PanelId::Parliament),
        ("budget", PanelId::Budget),
        ("log", PanelId::Log),
        ("help", PanelId::Help),
    ];

    for (input, expected) in cases {
        let mut state = command_state(input);
        let cmd = palette.submit(&mut state);
        assert_eq!(
            cmd,
            Some(TuiCommand::SwitchPanel(expected)),
            "input '{}'",
            input
        );
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
    }
}

#[test]
fn command_palette_parses_quit_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quit");

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, Some(TuiCommand::Quit));
}

#[test]
fn command_palette_ignores_unknown_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("unknown");

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, None);
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
    assert!(
        state
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("unknown command"),
        "status should report unknown command"
    );
    assert_eq!(state.status_message.as_ref().unwrap().1, Severity::Error);
}

#[test]
fn command_palette_search_mode_sets_keyword() {
    let mut palette = CommandPalette::new();
    let mut state = TuiState::new();
    state.input_mode = InputMode::Search;
    state.input_buffer = "query".into();

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, None);
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
    assert_eq!(state.filter_keyword, Some("query".into()));
}

#[test]
fn command_palette_find_command_sets_keyword() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("find error");

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, None);
    assert_eq!(state.filter_keyword, Some("error".into()));
}

#[test]
fn command_palette_filter_command_sets_topic() {
    let cases = [
        "quest",
        "parliament",
        "budget",
        "memory",
        "security",
        "health",
        "system",
    ];
    for topic in cases {
        let mut palette = CommandPalette::new();
        let mut state = command_state(&format!("filter {topic}"));
        let cmd = palette.submit(&mut state);
        assert_eq!(cmd, None, "topic '{}' should not produce command", topic);
        assert_eq!(
            state.filter_topic,
            Some(topic.into()),
            "topic '{}' should be set",
            topic
        );
    }
}

#[test]
fn command_palette_filter_command_rejects_invalid_topic() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("filter invalid-topic");

    let cmd = palette.submit(&mut state);
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
fn command_palette_level_command_sets_level() {
    let cases = ["info", "warn", "error", "critical"];
    for level in cases {
        let mut palette = CommandPalette::new();
        let mut state = command_state(&format!("level {level}"));
        let cmd = palette.submit(&mut state);
        assert_eq!(cmd, None, "level '{}' should not produce command", level);
        assert_eq!(
            state.filter_level,
            Some(level.into()),
            "level '{}' should be set",
            level
        );
    }
}

#[test]
fn command_palette_level_command_rejects_invalid_level() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("level verbose");

    let cmd = palette.submit(&mut state);
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
fn command_palette_refresh_returns_request() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("refresh");
    state.filter_keyword = Some("old".into());
    state.filter_topic = Some("security".into());
    state.filter_level = Some("error".into());

    // M4:refresh 现在作为控制请求发布,由上游订阅者处理过滤器清空。
    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
    assert!(state.filter_keyword.is_some());
    assert!(state.filter_topic.is_some());
    assert!(state.filter_level.is_some());
}

#[test]
fn command_palette_missing_argument_reports_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("find");

    let cmd = palette.submit(&mut state);
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
fn command_palette_handle_key_appends_characters() {
    let mut palette = CommandPalette::new();
    let mut state = TuiState::new();
    state.input_mode = InputMode::Command;

    for c in "budget".chars() {
        palette.handle_key(
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            &mut state,
        );
    }
    assert_eq!(state.input_buffer, "budget");
}

#[test]
fn command_palette_handle_key_backspace_removes_last_char() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("bud");

    palette.handle_key(
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(state.input_buffer, "bu");
}

#[test]
fn command_palette_handle_key_esc_cancels_input() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quit");

    let cmd = palette.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
    assert_eq!(cmd, None);
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
}

#[test]
fn command_palette_handle_key_esc_in_search_clears_keyword() {
    let mut palette = CommandPalette::new();
    let mut state = TuiState::new();
    state.input_mode = InputMode::Search;
    state.filter_keyword = Some("old".into());
    state.input_buffer = "new".into();

    let cmd = palette.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
    assert_eq!(cmd, None);
    assert!(state.filter_keyword.is_none());
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
}

#[test]
fn command_palette_handle_key_enter_submits() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("help");

    let cmd = palette.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(cmd, Some(TuiCommand::SwitchPanel(PanelId::Help)));
    assert_eq!(state.input_mode, InputMode::Normal);
}

#[test]
fn command_palette_parses_pause_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("pause q-1");

    let cmd = palette.submit(&mut state);
    match cmd {
        Some(TuiCommand::OpenPopup(PopupKind::Confirm {
            prompt,
            on_confirm,
            confirmed,
        })) => {
            assert!(prompt.contains("Pause quest"));
            assert!(prompt.contains("q-1"));
            assert_eq!(on_confirm, "pause:q-1");
            assert!(confirmed, "control confirm should default to Yes");
        }
        other => panic!("expected Confirm popup, got {other:?}"),
    }
}

#[test]
fn command_palette_parses_resume_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("resume q-2");

    let cmd = palette.submit(&mut state);
    match cmd {
        Some(TuiCommand::OpenPopup(PopupKind::Confirm {
            prompt,
            on_confirm,
            confirmed,
        })) => {
            assert!(prompt.contains("Resume quest"));
            assert!(prompt.contains("q-2"));
            assert_eq!(on_confirm, "resume:q-2");
            assert!(confirmed, "control confirm should default to Yes");
        }
        other => panic!("expected Confirm popup, got {other:?}"),
    }
}

#[test]
fn command_palette_parses_vote_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("vote yes p-1");

    let cmd = palette.submit(&mut state);
    match cmd {
        Some(TuiCommand::OpenPopup(PopupKind::Confirm {
            prompt,
            on_confirm,
            confirmed,
        })) => {
            assert!(prompt.contains("Vote yes on proposal"));
            assert!(prompt.contains("p-1"));
            assert_eq!(on_confirm, "vote:yes:p-1");
            assert!(confirmed, "control confirm should default to Yes");
        }
        other => panic!("expected Confirm popup, got {other:?}"),
    }
}

#[test]
fn command_palette_vote_command_rejects_invalid_vote() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("vote maybe p-1");

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, None);
    assert!(
        state
            .status_message
            .as_ref()
            .unwrap()
            .0
            .contains("invalid vote"),
        "status should report invalid vote"
    );
}

#[test]
fn command_palette_refresh_command_returns_request() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("refresh");
    state.filter_keyword = Some("old".into());

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, Some(TuiCommand::RequestRefresh));
    // refresh 现在直接发布请求,不再清空过滤器;清空逻辑留待上游处理
    assert_eq!(state.filter_keyword, Some("old".into()));
}

#[test]
fn command_palette_control_commands_require_arguments() {
    let cases = [
        ("pause", "pause requires a quest id"),
        ("resume", "resume requires a quest id"),
        ("vote", "vote requires a vote value and proposal id"),
    ];

    for (input, expected) in cases {
        let mut palette = CommandPalette::new();
        let mut state = command_state(input);
        let cmd = palette.submit(&mut state);
        assert_eq!(cmd, None, "input '{}' should not produce command", input);
        assert!(
            state.status_message.as_ref().unwrap().0.contains(expected),
            "input '{}' should report missing argument",
            input
        );
    }
}
