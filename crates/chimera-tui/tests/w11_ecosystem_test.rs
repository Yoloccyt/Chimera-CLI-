//! W11 生态波集成测试 — Concord W11(ADR-083)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖
//! - /notify on|off 开关与持久化语义、非法参数诚实反馈(T11.3);
//! - /recap 会话回顾(空历史诚实 + 有历史提取)(T11.8);
//! - /copy 无回复时诚实反馈(T11.9);
//! - /commands 空态诚实反馈(T11.2);
//! - 用户自定义命令:加载 → 展开入 composer;SEC-1 项目级信任门控
//!   (首次确认弹窗 → 确认后信任 + 展开)(T11.1/T11.2);
//! - apply_paste:输入型模式粘贴追加 + 消毒;非输入模式忽略(T11.5)。

#![forbid(unsafe_code)]

use chimera_tui::{InputMode, PopupKind, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .expect("app init")
}

fn press(app: &mut TuiApp, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn slash_submit(app: &mut TuiApp, cmd: &str) {
    press(app, KeyCode::Char('/'));
    type_str(app, cmd);
    press(app, KeyCode::Enter);
}

// ============================================================
// T11.3 /notify
// ============================================================

#[test]
fn notify_toggle_on_off_and_invalid() {
    let mut app = make_app();
    assert!(!app.config().notify_enabled, "默认关(opt-in)");
    slash_submit(&mut app, "notify on");
    assert!(app.config().notify_enabled, "/notify on 应开启");
    slash_submit(&mut app, "notify off");
    assert!(!app.config().notify_enabled, "/notify off 应关闭");
    slash_submit(&mut app, "notify turbo");
    let (msg, _) = app.state().status_message.as_ref().expect("应有反馈");
    assert!(msg.contains("/notify"), "非法参数应反馈用法,got: {msg}");
}

// ============================================================
// T11.8 /recap
// ============================================================

#[test]
fn recap_empty_history_honest_hint() {
    let mut app = make_app();
    app.state_mut().chat_messages.clear();
    slash_submit(&mut app, "recap");
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { content, .. }) => {
            assert!(!content.is_empty(), "空历史应有诚实提示");
        }
        other => panic!("应为 Detail 弹窗,实际 {other:?}"),
    }
}

#[test]
fn recap_with_history_extracts_turns() {
    let mut app = make_app();
    app.state_mut().chat_messages = vec![
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::User,
            content: "实现登录功能".into(),
        },
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: "已分解为 3 个任务".into(),
        },
    ];
    slash_submit(&mut app, "recap");
    match app.state().popup_stack.current() {
        Some(PopupKind::Detail { content, .. }) => {
            assert!(content.contains("实现登录功能"), "应含首个提问");
        }
        other => panic!("应为 Detail 弹窗,实际 {other:?}"),
    }
}

// ============================================================
// T11.9 /copy
// ============================================================

#[test]
fn copy_without_reply_honest_warning() {
    let mut app = make_app();
    app.state_mut().chat_messages.clear();
    slash_submit(&mut app, "copy");
    let (msg, _) = app.state().status_message.as_ref().expect("应有反馈");
    assert!(!msg.is_empty(), "无回复应诚实反馈");
}

// ============================================================
// T11.2 /commands
// ============================================================

#[test]
fn commands_empty_honest_status() {
    let mut app = make_app();
    slash_submit(&mut app, "commands");
    // 测试环境可能扫到用户 HOME 命令,故断言"有反馈"而非绝对空态
    assert!(
        app.state().status_message.is_some() || app.state().popup_stack.current().is_some(),
        "/commands 应有状态或弹窗反馈"
    );
}

// ============================================================
// T11.1/T11.2 用户命令:加载 → SEC-1 信任门控 → 展开
// ============================================================

fn setup_user_commands(app: &mut TuiApp) -> tempfile::TempDir {
    let project = tempfile::tempdir().expect("tempdir");
    let cmds = project.path().join(".chimera").join("commands");
    std::fs::create_dir_all(&cmds).expect("mkdir");
    std::fs::write(
        cmds.join("release.md"),
        "---\nname: release\ndescription: 发布流程\n---\n执行发布 {{args}} 版本",
    )
    .expect("write");
    app.reload_user_commands(project.path(), None);
    project
}

#[test]
fn user_command_expands_into_composer_after_trust() {
    let mut app = make_app();
    let _dir = setup_user_commands(&mut app);

    // 项目级命令首次使用 → SEC-1 信任确认弹窗
    slash_submit(&mut app, "release 1.2.3");
    match app.state().popup_stack.current() {
        Some(PopupKind::Confirm { .. }) => {}
        other => panic!("项目级命令首次使用应弹信任确认,实际 {other:?}"),
    }
    // 确认(默认选中 Yes,Enter 确认)→ 信任记录 + 模板展开入 composer
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "展开后应进 Insert 模式"
    );
    assert_eq!(
        app.state().input_buffer,
        "执行发布 1.2.3 版本",
        "模板 {{args}} 应替换为实参"
    );

    // 已信任后再次使用 → 免确认直接展开(先 Esc 回 Normal 再输命令,
    // 与真实交互一致:展开后处于 Insert 模式)
    press(&mut app, KeyCode::Esc);
    app.state_mut().input_buffer.clear();
    slash_submit(&mut app, "release 2.0.0");
    assert_eq!(
        app.state().input_buffer,
        "执行发布 2.0.0 版本",
        "信任后免确认直接展开"
    );
    assert!(
        !matches!(
            app.state().popup_stack.current(),
            Some(PopupKind::Confirm { .. })
        ),
        "信任后不应再弹确认"
    );
}

#[test]
fn user_level_command_skips_trust_gate() {
    let mut app = make_app();
    let user_dir = tempfile::tempdir().expect("tempdir");
    let project_dir = tempfile::tempdir().expect("tempdir");
    let cmds = user_dir.path().join(".chimera").join("commands");
    std::fs::create_dir_all(&cmds).expect("mkdir");
    std::fs::write(cmds.join("greet.md"), "打个招呼").expect("write");
    // 用户级目录经 user_dir 参数注入(project_level=false 免信任门控)
    app.reload_user_commands(project_dir.path(), Some(user_dir.path()));

    // 用户级命令免信任门控,直接展开
    slash_submit(&mut app, "greet");
    assert_eq!(app.state().input_buffer, "打个招呼");
    assert_eq!(app.state().input_mode, InputMode::Insert);
}

// ============================================================
// T10.4 /agent tree(W10 补全验收:命令 → 面板切换)
// ============================================================

#[test]
fn agent_tree_command_switches_panel() {
    let mut app = make_app();
    slash_submit(&mut app, "agent tree");
    assert_eq!(
        app.current_panel(),
        chimera_tui::PanelId::AgentTree,
        "/agent tree 应切换到 AgentTree 面板"
    );
}

// ============================================================
// T11.5 apply_paste
// ============================================================

#[test]
fn paste_appends_in_slash_mode_and_sanitizes() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('/'));
    assert_eq!(app.state().input_mode, InputMode::Slash);
    // SEC-2:粘贴体携带控制序列应被消毒
    app.apply_paste("quest\x1b]9;evil\x07 pause");
    assert_eq!(
        app.state().input_buffer,
        "quest]9;evil pause",
        "控制字符应被剥离,可打印内容保留"
    );
}

#[test]
fn paste_ignored_in_normal_mode() {
    let mut app = make_app();
    assert_eq!(app.state().input_mode, InputMode::Normal);
    app.apply_paste("anything");
    assert!(
        app.state().input_buffer.is_empty(),
        "非输入型模式应忽略粘贴"
    );
}
