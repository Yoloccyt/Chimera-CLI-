//! CommandPalette 集成测试 — 验证命令解析与模式切换

#![forbid(unsafe_code)]

use chimera_tui::{CommandPalette, InputMode, PanelId, TuiCommand, TuiState};
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
}

#[test]
fn command_palette_search_mode_is_stub() {
    let mut palette = CommandPalette::new();
    let mut state = TuiState::new();
    state.input_mode = InputMode::Search;
    state.input_buffer = "query".into();

    let cmd = palette.submit(&mut state);
    assert_eq!(cmd, None);
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
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
