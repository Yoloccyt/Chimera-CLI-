//! CommandPalette quest 子命令集成测试
//!
//! 验证 `quest cancel <id>` 与 `quest priority <id> <level>` 命令解析,
//! 包括非法 level(>255 / 非数字)与缺失参数的错误处理。
//!
//! 对应架构层:L10 Interface
//! 对应任务:Task 5 — TUI CommandPalette 新增 quest 管理命令

use chimera_tui::command_palette::CommandPalette;
use chimera_tui::popup::Severity;
use chimera_tui::types::{InputMode, PanelId, TuiCommand, TuiState};

/// 构造 Command 模式的 TuiState,输入缓冲已填入给定命令字符串
fn command_state(input: &str) -> TuiState {
    let mut state = TuiState::new();
    state.input_mode = InputMode::Command;
    state.input_buffer = input.to_string();
    state
}

// ============================================================
// quest cancel <id> — 正向解析
// ============================================================

#[test]
fn test_parse_quest_cancel_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest cancel quest-001");
    let cmd = palette.submit(&mut state);

    assert_eq!(
        cmd,
        Some(TuiCommand::RequestQuestCancel("quest-001".to_string()))
    );
    // 提交后输入模式应恢复 Normal,缓冲清空
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
}

#[test]
fn test_parse_quest_cancel_with_complex_id() {
    // 验证含连字符/数字的 quest_id 正常解析
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest cancel q-abc-123-xyz");
    let cmd = palette.submit(&mut state);

    assert_eq!(
        cmd,
        Some(TuiCommand::RequestQuestCancel("q-abc-123-xyz".to_string()))
    );
}

#[test]
fn test_quest_cancel_missing_id_shows_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest cancel");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    let (msg, sev) = state
        .status_message
        .expect("error status should be set for missing quest id");
    assert_eq!(sev, Severity::Error);
    assert!(
        msg.contains("quest id") || msg.contains("requires"),
        "status should report missing quest id, got: {msg}"
    );
}

// ============================================================
// quest priority <id> <level> — 正向解析
// ============================================================

#[test]
fn test_parse_quest_priority_command() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001 200");
    let cmd = palette.submit(&mut state);

    assert_eq!(
        cmd,
        Some(TuiCommand::RequestQuestPriorityChange {
            quest_id: "quest-001".to_string(),
            new_priority: 200,
        })
    );
    assert_eq!(state.input_mode, InputMode::Normal);
    assert!(state.input_buffer.is_empty());
}

#[test]
fn test_parse_quest_priority_boundary_zero() {
    // 边界值 0(u8 下限)应接受
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001 0");
    let cmd = palette.submit(&mut state);

    assert_eq!(
        cmd,
        Some(TuiCommand::RequestQuestPriorityChange {
            quest_id: "quest-001".to_string(),
            new_priority: 0,
        })
    );
}

#[test]
fn test_parse_quest_priority_boundary_max() {
    // 边界值 255(u8 上限)应接受
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001 255");
    let cmd = palette.submit(&mut state);

    assert_eq!(
        cmd,
        Some(TuiCommand::RequestQuestPriorityChange {
            quest_id: "quest-001".to_string(),
            new_priority: 255,
        })
    );
}

// ============================================================
// quest priority — 非法 level 错误处理
// ============================================================

#[test]
fn test_quest_priority_invalid_level_shows_error() {
    // 999 > 255(u8 上限),parse::<u8>() 必然失败
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001 999");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    let (msg, sev) = state
        .status_message
        .expect("error status should be set for invalid level");
    assert_eq!(sev, Severity::Error);
    assert!(
        msg.contains("invalid priority") || msg.contains("0-255"),
        "status should report invalid priority, got: {msg}"
    );
}

#[test]
fn test_quest_priority_non_numeric_level_shows_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001 abc");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    let (msg, sev) = state
        .status_message
        .expect("error status should be set for non-numeric level");
    assert_eq!(sev, Severity::Error);
    assert!(
        msg.contains("invalid priority") || msg.contains("0-255"),
        "status should report invalid priority, got: {msg}"
    );
}

#[test]
fn test_quest_priority_missing_level_shows_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority quest-001");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    let (msg, sev) = state
        .status_message
        .expect("error status should be set for missing level");
    assert_eq!(sev, Severity::Error);
    assert!(
        msg.contains("level") || msg.contains("requires"),
        "status should report missing level, got: {msg}"
    );
}

#[test]
fn test_quest_priority_missing_all_args_shows_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest priority");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    assert_eq!(
        state.status_message.as_ref().map(|(_, sev)| *sev),
        Some(Severity::Error)
    );
}

// ============================================================
// 回归测试 — quest 子命令不破坏现有 quest 面板切换
// ============================================================

#[test]
fn test_quest_alone_still_switches_panel() {
    // `quest` 单独仍应切换到 Quest 面板,不被子命令逻辑拦截
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, Some(TuiCommand::SwitchPanel(PanelId::Quest)));
}

#[test]
fn test_quest_unknown_subcommand_shows_error() {
    let mut palette = CommandPalette::new();
    let mut state = command_state("quest frobnicate quest-001");
    let cmd = palette.submit(&mut state);

    assert_eq!(cmd, None);
    let (msg, sev) = state
        .status_message
        .expect("error status should be set for unknown subcommand");
    assert_eq!(sev, Severity::Error);
    assert!(
        msg.contains("unknown quest subcommand") || msg.contains("unknown command"),
        "status should report unknown subcommand, got: {msg}"
    );
}
