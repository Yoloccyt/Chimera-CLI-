//! 全局快捷键系统统一 — P3.3 集成测试
//!
//! 验证 `TuiApp::handle_global_key` 提取后，全局快捷键优先于面板键，
//! 且 `g` 前缀状态不会卡死。

#![forbid(unsafe_code)]

use chimera_tui::{McpNodeStatus, NodeStatus, PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 构造默认 TuiApp(无 event-bus，内存桩数据源)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 构造标准 KeyEvent(Press 状态，无修饰符)
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn global_keys_number_1_to_9_switch_first_9_panels() {
    let cases = [
        (KeyCode::Char('1'), PanelId::Quest),
        (KeyCode::Char('2'), PanelId::Parliament),
        (KeyCode::Char('3'), PanelId::Budget),
        (KeyCode::Char('4'), PanelId::Memory),
        (KeyCode::Char('5'), PanelId::Security),
        (KeyCode::Char('6'), PanelId::Health),
        (KeyCode::Char('7'), PanelId::Log),
        (KeyCode::Char('8'), PanelId::Help),
        (KeyCode::Char('9'), PanelId::Decay),
    ];

    for (code, expected) in cases {
        let mut app = make_app();
        app.handle_key_event(key(code));
        assert_eq!(
            app.current_panel(),
            expected,
            "key '{code:?}' should switch to {expected:?}"
        );
    }
}

#[test]
fn global_keys_g_prefix_switches_to_back_panels() {
    let cases = [
        (KeyCode::Char('1'), PanelId::EventStream),
        (KeyCode::Char('2'), PanelId::Router),
        (KeyCode::Char('3'), PanelId::McpNodes),
        (KeyCode::Char('4'), PanelId::Chtc),
    ];

    for (digit, expected) in cases {
        let mut app = make_app();
        // 先切换到 Log 面板，确保后续 g 前缀真正改变了焦点
        app.handle_key_event(key(KeyCode::Char('7')));
        assert_eq!(app.current_panel(), PanelId::Log);

        app.handle_key_event(key(KeyCode::Char('g')));
        assert!(
            app.state().g_prefix,
            "after pressing `g`, g_prefix should be true"
        );

        app.handle_key_event(key(digit));
        assert_eq!(
            app.current_panel(),
            expected,
            "g + '{digit:?}' should switch to {expected:?}"
        );
        assert!(
            !app.state().g_prefix,
            "after completing g-prefix combo, g_prefix should be reset"
        );
    }
}

#[test]
fn global_keys_g_prefix_abort_on_unknown_key_resets_state() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    assert!(app.state().g_prefix);

    // 按下一个非 g1-g5 的键，前缀状态应被重置，应用不卡死
    app.handle_key_event(key(KeyCode::Char('x')));
    assert!(
        !app.state().g_prefix,
        "g-prefix should reset on unknown follow-up key"
    );
    assert!(
        app.state().running,
        "app should still be running after abort"
    );
}

#[test]
fn global_keys_gg_scrolls_to_top() {
    let mut app = make_app();
    // 切换到 MCP 节点面板
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('3')));
    assert_eq!(app.current_panel(), PanelId::McpNodes);

    // 准备 3 个节点，并直接通过 state 注入
    app.state_mut().mcp_nodes = vec![
        McpNodeStatus {
            node_id: "node-a".into(),
            status: NodeStatus::Online,
            throughput: 100,
            last_seen: None,
        },
        McpNodeStatus {
            node_id: "node-b".into(),
            status: NodeStatus::Online,
            throughput: 200,
            last_seen: None,
        },
        McpNodeStatus {
            node_id: "node-c".into(),
            status: NodeStatus::Online,
            throughput: 300,
            last_seen: None,
        },
    ];

    // G 跳到底部(第 3 项，索引 2)
    app.handle_key_event(key(KeyCode::Char('G')));
    let panel_idx = app
        .state()
        .mcp_nodes
        .iter()
        .position(|n| n.node_id == "node-c")
        .unwrap();
    assert_eq!(panel_idx, 2, "setup has node-c at index 2");

    // gg 跳到顶部(索引 0)
    app.handle_key_event(key(KeyCode::Char('g')));
    assert!(app.state().g_prefix, "first g enters prefix state");
    app.handle_key_event(key(KeyCode::Char('g')));
    assert!(
        !app.state().g_prefix,
        "gg completes and resets prefix state"
    );
}

#[test]
fn global_keys_shift_g_scrolls_to_bottom() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('3')));
    assert_eq!(app.current_panel(), PanelId::McpNodes);

    app.state_mut().mcp_nodes = vec![
        McpNodeStatus {
            node_id: "node-a".into(),
            status: NodeStatus::Online,
            throughput: 100,
            last_seen: None,
        },
        McpNodeStatus {
            node_id: "node-b".into(),
            status: NodeStatus::Online,
            throughput: 200,
            last_seen: None,
        },
    ];

    // 默认选中索引 0，G 跳到底部索引 1
    app.handle_key_event(key(KeyCode::Char('G')));
    assert!(!app.state().g_prefix);
}

#[test]
fn global_keys_g_prefix_on_non_list_panel_does_not_panic() {
    let mut app = make_app();
    // Budget 不是列表面板，按 g 后按任意键不应 panic
    app.handle_key_event(key(KeyCode::Char('3')));
    assert_eq!(app.current_panel(), PanelId::Budget);

    app.handle_key_event(key(KeyCode::Char('g')));
    assert!(app.state().g_prefix);

    app.handle_key_event(key(KeyCode::Char('x')));
    assert!(!app.state().g_prefix);
}

#[test]
fn global_keys_tab_and_backtab_cycle_panels() {
    let mut app = make_app();
    assert_eq!(app.current_panel(), PanelId::Quest);

    app.handle_key_event(key(KeyCode::Tab));
    assert_eq!(app.current_panel(), PanelId::Parliament);

    app.handle_key_event(key(KeyCode::BackTab));
    assert_eq!(app.current_panel(), PanelId::Quest);
}

#[test]
fn global_keys_q_quits() {
    let mut app = make_app();
    assert!(app.state().running);
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(!app.state().running);
}

#[test]
fn global_keys_esc_quits() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.state().running);
}

#[test]
fn global_keys_question_mark_opens_help_overlay() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('?')));
    assert!(
        !app.state().popup_stack.is_empty(),
        "? should open help overlay"
    );
}

#[test]
fn global_keys_colon_enters_command_mode() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char(':')));
    assert_eq!(app.state().input_mode, chimera_tui::InputMode::Command);
}

#[test]
fn global_keys_slash_enters_search_mode() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('/')));
    assert_eq!(app.state().input_mode, chimera_tui::InputMode::Search);
}
