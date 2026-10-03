//! P3 交互回归测试 — 死路径修复、退出确认、Repeat 支持、命令栏 Ctrl 守卫
//!
//! 对应架构层:L10 Interface(`chimera-tui`)
//!
//! # 覆盖的评估问题
//! - I-1:Quest 详情键改绑 `v` 后经完整路由(Normal → FocusPanel)可达
//! - I-2:g5 → Timeline 跳转(Concord T1.4 接线后为有效面板)
//! - I-4:quit_requires_confirm 开启时 q/Esc 先确认再退出(默认关闭零回归)
//! - I-7:Repeat 事件视同 Press(长按滚动/输入可重复)
//! - I-5:命令栏 Ctrl 组合键不进缓冲,Ctrl+L 仍可切换语言

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{
    current_locale, set_locale, DataPipeline, DataSourceConfig, EventSubscriber, InputMode, Locale,
    PanelId, PopupKind, TuiApp, TuiConfig,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use event_bus::{ActionSource, EventBus, EventReceiver, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

fn make_confirm_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        quit_requires_confirm: true,
        ..TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        }
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
// I-2:g5 → Timeline 跳转(Concord T1.4 接线后为有效面板)
// ============================================================

#[test]
fn g5_jump_to_timeline_after_wiring() {
    // Concord T1.4:Timeline 已接线注册(§7.3),g5 从"未注册死键"变为有效跳转;
    // 原"未注册状态栏提示"路径由 event_loop 兑底保留(未来新增未注册
    // PanelId 时仍生效),此处验证接线后的正向行为。
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('5')));

    assert!(app.state().running, "g5 不应退出应用");
    assert_eq!(
        app.current_panel(),
        PanelId::Timeline,
        "Timeline 已接线,g5 应成功跳转"
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
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    // Concord W2:`:` 进入斜杠命令模式;Ctrl+L 仍被拦截为语言切换不进缓冲
    let mut app = make_app();
    set_locale(Locale::Zh);
    app.handle_key_event(key(KeyCode::Char(':'))); // 进入斜杠命令模式
    assert_eq!(app.state().input_mode, InputMode::Slash);

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
    // B5-G(评审 #2):状态文案断言经 t! 取值仍有 locale 翻转窗口,持 guard 互斥
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let app = make_app();
    let (mut app, _bus, _rx) = app_with_bus(app);
    // palette 派发无参动作 quest.pause → 立即发布 TuiActionRequested 并启动计时
    open_palette_select(&mut app, "quest.pause");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.state().pending_actions.len(), 1, "派发后应启动超时计时");

    // 模拟超时:把该请求键的 deadline 回拨到过去,update() 触发检测(无需真实等待)
    for deadline in app.state_mut().pending_actions.values_mut() {
        *deadline = std::time::Instant::now() - std::time::Duration::from_secs(3);
    }
    app.update();

    assert!(app.state().pending_actions.is_empty(), "超时后应清除计时");
    let (msg, sev) = app
        .state()
        .status_message
        .as_ref()
        .expect("超时应产生状态栏提示");
    assert!(
        // B4-I(F-04):断言经 t! 取值;读值仍随当前 locale,非与 locale 无关
        // (B5-G 已在测试入口补 locale_test_guard 消除翻转窗口)
        msg.contains(chimera_tui::t!("status.action_timeout")),
        "提示应说明编排器未接线,got: {msg}"
    );
    assert_eq!(*sev, chimera_tui::Severity::Warning);
}

#[tokio::test]
async fn action_timeout_cleared_when_feedback_received() {
    // B5-V(F-63)重写为真实回执链(旧实现手工 pending_actions.clear() 是
    // 同义反复):palette 派发 → TuiActionRequested(bus)→ 测试扮演编排器
    // 回发 TuiActionFailed(同 request_id,is_error)→ DataPipeline 消费 →
    // app.update() → 按 request_id 精准移除 + 错误级状态栏反馈,未回执请求
    // 保留计时。与 app/tests.rs 强不变量 feedback_removes_only_matching_request
    // 互补:此处走 event→bus→pipeline→app 真管道,并补上 app 级
    // is_error=true→Severity::Error 分支的集成覆盖(R5 收窄的净缺口)。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let bus = EventBus::new();
    // WHY subscribe 先于 publish:broadcast 不缓存历史消息(§4.4 #3 红线)
    let mut rx = bus.subscribe();
    let pipeline = std::sync::Arc::new(DataPipeline::new(
        EventSubscriber::new(bus.clone()),
        DataSourceConfig {
            tick_interval_ms: 25,
            ..Default::default()
        },
    ));
    // pipeline 兼作 app 数据源(impl TuiDataSource for Arc<DataPipeline>),
    // app.update() 拉取的快照即真管道产物(而非手工构造的 snapshot)
    let app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(pipeline.clone()),
    )
    .unwrap();
    let mut app = TuiApp::with_event_bus(app, bus.clone());

    // palette 黑盒派发两个动作(不硬编码 request_id 计数器实现)
    open_palette_select(&mut app, "quest.pause");
    app.handle_key_event(key(KeyCode::Enter));
    open_palette_select(&mut app, "quest.pause");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.state().pending_actions.len(),
        2,
        "两次派发应各挂起超时计时"
    );
    let mut request_ids: Vec<String> = Vec::new();
    while let Ok(Some(ev)) = rx.try_recv() {
        if let NexusEvent::TuiActionRequested { request_id, .. } = ev {
            request_ids.push(request_id);
        }
    }
    assert_eq!(
        request_ids.len(),
        2,
        "两次派发应发布两个 TuiActionRequested"
    );

    // 测试扮演编排器:仅对第一个请求回发失败回执
    let failed_rid = request_ids[0].clone();
    bus.publish(NexusEvent::TuiActionFailed {
        metadata: event_bus::EventMetadata::new("test-orchestrator"),
        request_id: failed_rid.clone(),
        action_id: "quest.pause".into(),
        error: "orchestrator rejected: quest.pause".into(),
    })
    .await
    .unwrap();

    // 轮询等待 DataPipeline 异步消费(同 compact_closure_test 真管道范式)
    // WHY 1.5s 必须短于 ACTION_TIMEOUT(2s,key_dispatch.rs):update() 的
    // check_action_timeout() 先于回执同步执行,若派发→update 之间进程停滞
    // ≥2s,超时路径会把幸存请求一并移除,下文 contains_key(surviving_rid)
    // 对正确实现误报红(假阳性 flake);正常回执消费为 ms 级,收窄后"超时
    // 路径可达"在本测试域内不可能,管线真卡死时本循环确定性变红。
    let deadline = Instant::now() + Duration::from_millis(1500);
    loop {
        if pipeline.snapshot().action_feedback_request_id.as_deref() == Some(failed_rid.as_str()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "等待 DataPipeline 消费失败回执超时"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    app.update();
    assert!(
        !app.state().pending_actions.contains_key(&failed_rid),
        "已回执请求应按 request_id 精准移除超时计时"
    );
    let surviving_rid = &request_ids[1];
    assert!(
        app.state().pending_actions.contains_key(surviving_rid),
        "未回执请求必须保留超时兜底计时(否则静默失败)"
    );
    let (msg, sev) = app
        .state()
        .status_message
        .as_ref()
        .expect("失败回执应产生状态栏反馈");
    assert_eq!(
        *sev,
        chimera_tui::Severity::Error,
        "is_error=true 应映射 Error 级反馈"
    );
    assert!(
        msg.contains("orchestrator rejected"),
        "状态栏应展示回执错误原文, got: {msg}"
    );
    // 旧测试语义保留:回执到达不误报超时提示(deadline 随精准移除而消失)
    assert!(
        !msg.contains(chimera_tui::t!("status.action_timeout")),
        "反馈后不应出现编排器未接线提示, got: {msg}"
    );

    pipeline.shutdown().await;
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
