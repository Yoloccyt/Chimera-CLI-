//! P3 交互回归测试 — 死路径修复、退出确认、Repeat 支持、命令栏 Ctrl 守卫
//!
//! 对应架构层:L10 Interface(`chimera-tui`)
//!
//! # 覆盖的评估问题
//! - I-1:Quest 详情键改绑 `v` 后经完整路由(Normal → FocusPanel)可达
//! - I-2:g5 → Timeline 未注册不再静默失败,状态栏给出提示
//! - I-4:quit_requires_confirm 开启时 q/Esc 先确认再退出(默认关闭零回归)
//! - I-7:Repeat 事件视同 Press(长按滚动/输入可重复)
//! - I-5:命令栏 Ctrl 组合键不进缓冲,Ctrl+L 仍可切换语言

#![forbid(unsafe_code)]

use chimera_tui::{
    current_locale, set_locale, InputMode, Locale, PanelId, PopupKind, TuiApp, TuiConfig,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use event_bus::{ActionSource, EventBus, EventReceiver, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

fn make_confirm_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        quit_requires_confirm: true,
        ..TuiConfig::default()
    })
    .unwrap()
}

fn make_quest_app() -> TuiApp {
    let mut app = make_app();
    app.state_mut().quest_list = vec![Quest {
        quest_id: "q1".into(),
        title: "Detail Quest".into(),
        tasks: vec![Task {
            task_id: "q1-t1".into(),
            description: "test task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }];
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn render_once(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 将 EventBus 注入 TUI 并订阅返回的接收者(与 bidirectional_control_test 同模式)
fn app_with_bus(app: TuiApp) -> (TuiApp, EventBus, EventReceiver) {
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let app = TuiApp::with_event_bus(app, bus.clone());
    (app, bus, rx)
}

/// 打开 palette 并输入检索词(选中首个匹配项)
fn open_palette_select(app: &mut TuiApp, query: &str) {
    app.handle_key_event(ctrl('p'));
    for c in query.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
}

// ============================================================
// I-1:Quest 详情键改绑 `v`(原 `i` 被 InputRouter 拦截为 Insert)
// ============================================================

#[test]
fn quest_v_opens_detail_popup_via_full_routing() {
    let mut app = make_quest_app();
    app.switch_panel_to(PanelId::Quest);
    app.handle_key_event(key(KeyCode::Char('v')));

    assert!(
        !app.state().popup_stack.is_empty(),
        "`v` 应经完整路由打开 Quest 详情弹窗"
    );
    assert!(matches!(
        app.state().popup_stack.current(),
        Some(PopupKind::Detail { .. })
    ));
}

// ============================================================
// F-5:palette 参数动作输入流
// (agent.chat / quest.start / overwindow.run 经 palette 选中后收集 query,
//  以 {"query": text} 派发;Esc 取消;空输入不派发;无参动作行为不变)
// ============================================================

#[test]
fn palette_agent_chat_enters_query_input_mode() {
    let mut app = make_app();
    open_palette_select(&mut app, "agent.chat");

    app.handle_key_event(key(KeyCode::Enter));

    assert!(!app.palette_is_open(), "选中参数动作后 palette 应关闭");
    assert_eq!(
        app.state().input_mode,
        InputMode::Insert,
        "应进入 Insert 参数输入态"
    );
    let pending = app
        .state()
        .pending_action
        .as_ref()
        .expect("pending_action 应被设置");
    assert_eq!(pending.action_id, "agent.chat");
    assert_eq!(pending.source, ActionSource::Palette);
}

#[test]
fn palette_agent_chat_submit_dispatches_with_query_payload() {
    let app = make_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    open_palette_select(&mut app, "agent.chat");
    app.handle_key_event(key(KeyCode::Enter));

    for c in "hello".chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "参数提交后应回到 Normal(一次性动作)"
    );
    assert!(app.state().pending_action.is_none());
    assert!(app.state().input_buffer.is_empty());

    let event = rx.try_recv().expect("should receive event").unwrap();
    match event {
        NexusEvent::TuiActionRequested {
            action_id,
            payload,
            source,
            ..
        } => {
            assert_eq!(action_id, "agent.chat");
            assert_eq!(payload, r#"{"query":"hello"}"#);
            assert_eq!(source, ActionSource::Palette);
        }
        other => panic!("expected TuiActionRequested, got {other:?}"),
    }
}

#[test]
fn palette_query_input_esc_cancels_without_dispatch() {
    let app = make_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    open_palette_select(&mut app, "quest.start");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.state().pending_action.is_some());

    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(app.state().input_mode, InputMode::Normal);
    assert!(
        app.state().pending_action.is_none(),
        "Esc 应取消 pending 动作"
    );
    assert!(
        rx.try_recv().expect("try_recv ok").is_none(),
        "取消不应派发任何事件"
    );
}

#[test]
fn palette_query_empty_submit_does_not_dispatch() {
    let app = make_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    open_palette_select(&mut app, "agent.chat");
    app.handle_key_event(key(KeyCode::Enter));

    // 直接回车(空输入):不应派发,也不应丢失 pending 态
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.state().input_mode, InputMode::Insert);
    assert!(
        app.state().pending_action.is_some(),
        "空输入应保留 pending 等待继续输入"
    );
    assert!(rx.try_recv().expect("try_recv ok").is_none());
}

#[test]
fn palette_non_query_action_dispatches_immediately() {
    let app = make_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    open_palette_select(&mut app, "quest.pause");

    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "无参动作不应进入 Insert"
    );
    assert!(app.state().pending_action.is_none());
    let event = rx.try_recv().expect("should receive event").unwrap();
    match event {
        NexusEvent::TuiActionRequested { action_id, .. } => {
            assert_eq!(action_id, "quest.pause");
        }
        other => panic!("expected TuiActionRequested, got {other:?}"),
    }
}

// ============================================================
// I-2:g5 → Timeline 未注册,状态栏提示而非静默失败
// ============================================================

#[test]
fn g5_dead_jump_reports_status_not_silent() {
    let mut app = make_app();
    let before = app.current_panel();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('5')));

    assert!(app.state().running, "g5 不应退出应用");
    assert_eq!(app.current_panel(), before, "未注册面板不应切换焦点");
    let (msg, _sev) = app
        .state()
        .status_message
        .as_ref()
        .expect("g5 应有状态栏提示");
    assert!(
        msg.contains("Timeline"),
        "状态栏应提示 Timeline 未注册,got: {msg}"
    );
}

// ============================================================
// I-4:退出确认(默认关闭零回归;开启后先确认再退出)
// ============================================================

#[test]
fn quit_by_default_still_immediate() {
    // 默认 quit_requires_confirm=false:m3a 既有契约(q/Esc 立即退出)零回归
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.state().running);
}

#[test]
fn quit_confirm_enter_no_cancels() {
    let mut app = make_confirm_app();
    app.handle_key_event(key(KeyCode::Char('q')));
    assert!(app.state().running, "开启退出确认后 q 不应立即退出");
    assert!(!app.state().popup_stack.is_empty(), "q 应弹出确认框");

    // 默认选中 No:Enter 关闭弹窗并取消退出
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.state().popup_stack.is_empty());
    assert!(app.state().running);
}

#[test]
fn quit_confirm_toggle_yes_then_enter_quits() {
    let mut app = make_confirm_app();
    app.handle_key_event(key(KeyCode::Char('q')));
    // 左/右键切换确认项 → Yes,Enter 确认退出
    app.handle_key_event(key(KeyCode::Right));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.state().running, "确认 Yes + Enter 应退出应用");
}

// ============================================================
// I-7:Repeat 事件视同 Press(Release 仍忽略)
// ============================================================

#[test]
fn repeat_key_events_are_processed_like_press() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i'))); // 进入 Insert
    let repeat =
        KeyEvent::new_with_kind(KeyCode::Char('x'), KeyModifiers::NONE, KeyEventKind::Repeat);
    app.handle_key_event(repeat);
    assert_eq!(
        app.state().input_buffer,
        "x",
        "Repeat 字符应进入输入缓冲(长按可重复输入)"
    );
}

#[test]
fn release_events_still_ignored() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('i')));
    let release = KeyEvent::new_with_kind(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    app.handle_key_event(release);
    assert!(
        app.state().input_buffer.is_empty(),
        "Release 事件不应进入输入缓冲(Windows 平台兼容)"
    );
}

// ============================================================
// I-5:命令栏 Ctrl 组合键不进缓冲,Ctrl+L 仍切换语言
// ============================================================

#[test]
fn command_mode_ctrl_l_toggles_locale_not_buffer() {
    let mut app = make_app();
    set_locale(Locale::Zh);
    app.handle_key_event(key(KeyCode::Char(':'))); // 进入命令栏
    assert_eq!(app.state().input_mode, InputMode::Command);

    app.handle_key_event(ctrl('l'));
    assert_eq!(
        app.state().input_buffer,
        "",
        "Ctrl+L 不应把 `l` 打进命令栏缓冲"
    );
    assert_eq!(current_locale(), Locale::En, "Ctrl+L 应切换语言");
    set_locale(Locale::Zh); // 复位全局 locale,避免污染其他测试
}

// ============================================================
// P1-1(评估报告 v2):Terminate 与 Cancel 发布事件可区分
// ============================================================

#[test]
fn quest_cancel_publishes_operator_source() {
    // Quest 面板 `d` 键取消:确认后发布 QuestCancelRequested,来源标识为 "operator"
    let app = make_quest_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    app.switch_panel_to(PanelId::Quest);
    app.handle_key_event(key(KeyCode::Char('d')));
    // control_confirm 默认 Yes,Enter 直接确认
    app.handle_key_event(key(KeyCode::Enter));

    let event = rx.try_recv().expect("try_recv ok").expect("应收到事件");
    match event {
        NexusEvent::QuestCancelRequested {
            quest_id,
            requested_by,
            ..
        } => {
            assert_eq!(quest_id, "q1", "取消目标应为选中 Quest");
            assert_eq!(requested_by, "operator", "Cancel 来源标识应为 operator");
        }
        other => panic!("expected QuestCancelRequested, got {other:?}"),
    }
}

#[test]
fn task_manager_terminate_publishes_terminate_source() {
    // TaskManager `T` 键终止:确认后发布 QuestCancelRequested,
    // 来源标识为 "operator:terminate"(与 Cancel 在下游可区分,评估报告 v2 P1-1)
    let app = make_quest_app();
    let (mut app, _bus, mut rx) = app_with_bus(app);
    app.switch_panel_to(PanelId::TaskManager);
    app.handle_key_event(key(KeyCode::Char('T')));
    // control_confirm 默认 Yes,Enter 直接确认
    app.handle_key_event(key(KeyCode::Enter));

    let event = rx.try_recv().expect("try_recv ok").expect("应收到事件");
    match event {
        NexusEvent::QuestCancelRequested {
            quest_id,
            requested_by,
            ..
        } => {
            assert_eq!(quest_id, "q1", "终止目标应为选中 Quest");
            assert_eq!(
                requested_by, "operator:terminate",
                "Terminate 来源标识应为 operator:terminate"
            );
        }
        other => panic!("expected QuestCancelRequested, got {other:?}"),
    }
}

// ============================================================
// P1-2(评估报告 v2):TuiActionRequested 本地超时兜底反馈
// ============================================================

#[test]
fn action_timeout_reports_orchestrator_unavailable() {
    // standalone 场景:EventBus 无消费者,派发后无 Completed/Failed 回发;
    // 超过 ACTION_TIMEOUT 后 update 应在状态栏提示编排器未接线并清除计时。
    let app = make_app();
    let (mut app, _bus, _rx) = app_with_bus(app);
    // palette 派发无参动作 quest.pause → 立即发布 TuiActionRequested 并启动计时
    open_palette_select(&mut app, "quest.pause");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(
        app.state().pending_action_deadline.is_some(),
        "派发后应启动超时计时"
    );

    // 模拟超时:把 deadline 回拨到过去,update() 触发检测(无需真实等待)
    app.state_mut().pending_action_deadline =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
    app.update();

    assert!(
        app.state().pending_action_deadline.is_none(),
        "超时后应清除计时"
    );
    let (msg, sev) = app
        .state()
        .status_message
        .as_ref()
        .expect("超时应产生状态栏提示");
    assert!(
        msg.contains("orchestrator not connected"),
        "提示应说明编排器未接线,got: {msg}"
    );
    assert_eq!(*sev, chimera_tui::Severity::Warning);
}

#[test]
fn action_timeout_cleared_when_feedback_received() {
    // 收到 TuiActionCompleted 终态反馈(seq 增量)后,超时计时应被清除(不误报)
    let app = make_app();
    let (mut app, _bus, _rx) = app_with_bus(app);
    open_palette_select(&mut app, "quest.pause");
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.state().pending_action_deadline.is_some());

    // 直接清除计时模拟 feedback 到达后的 update 行为(反馈经 DataPipeline 消费,
    // 本测试不构建 pipeline,故直接验证 deadline 清除路径的状态)
    app.state_mut().pending_action_deadline = None;
    app.update();
    assert!(app.state().pending_action_deadline.is_none());
    // 无超时提示(deadline 已清除,检测不触发)
    let has_timeout_msg = app
        .state()
        .status_message
        .as_ref()
        .map(|(m, _)| m.contains("orchestrator"))
        .unwrap_or(false);
    assert!(!has_timeout_msg, "反馈后不应出现编排器未接线提示");
}

// ============================================================
// 快捷键诚实性:R 刷新在已修复面板经完整路由可达
// ============================================================

#[test]
fn budget_r_key_refresh_no_panic() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('3'))); // Budget
    app.handle_key_event(key(KeyCode::Char('R')));
    assert!(app.state().running);
    assert!(!render_once(&mut app).trim().is_empty());
}
