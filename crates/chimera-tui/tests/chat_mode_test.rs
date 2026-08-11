//! 会话模式端到端集成测试 — Concord W3(T3.1~T3.4 闭环验证)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - Chat 为第一默认视图(ADR-076);`\` 键与 /chat /dashboard 命令互切;
//! - Chat 模式渲染:composer 提示占位(Normal)、Insert 输入栏、状态栏;
//! - 会话流内嵌卡片:失败复盘卡(QuestCompleted{Failed})与计划卡派生渲染;
//! - 流式行闸门语义:半行暂存、完整行可见、Completed 冲刷残段;
//! - view_mode 持久化往返(save→load 保留)。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{InputMode, TuiApp, TuiConfig, TuiState, ViewMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent, QuestStatus};
use nexus_core::{Quest, Task, TaskStatus};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化涉及全局 locale 的测试(与 integration.rs 同范式,消除竞态 flaky)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 构造 Chat 默认视图的测试应用(禁用持久化,排除用户状态文件干扰)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

/// 构造 Dashboard 视图的测试应用(遗留语义对照组)
fn make_dashboard_app() -> TuiApp {
    TuiApp::new(TuiConfig {
        default_view_mode: ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap()
}

fn press(app: &mut TuiApp, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

/// 输入并提交一条斜杠命令(不含前导 `/`)
fn slash_submit(app: &mut TuiApp, cmd: &str) {
    press(app, KeyCode::Char('/'));
    type_str(app, cmd);
    press(app, KeyCode::Enter);
}

/// 渲染一帧并返回字符串快照(80x24 内存终端)
fn render_to_string(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(80, 24);
    let mut term = Terminal::new(backend).expect("memory terminal init");
    term.draw(|f| app.render(f)).expect("render frame");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

// ============================================================
// A. 第一默认视图与模式互切
// ============================================================

#[test]
fn default_view_is_chat() {
    let app = make_app();
    assert_eq!(
        app.state().view_mode,
        ViewMode::Chat,
        "ADR-076:Chat 为第一默认视图"
    );
}

#[test]
fn backslash_toggles_between_views() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('\\'));
    assert_eq!(
        app.state().view_mode,
        ViewMode::Dashboard,
        "\\ 应切到仪表盘"
    );
    press(&mut app, KeyCode::Char('\\'));
    assert_eq!(app.state().view_mode, ViewMode::Chat, "再按 \\ 应切回会话");
}

#[test]
fn slash_dashboard_and_chat_commands_switch_views() {
    let mut app = make_app();
    slash_submit(&mut app, "dashboard");
    assert_eq!(
        app.state().view_mode,
        ViewMode::Dashboard,
        "/dashboard 应切换视图模式(W3 改接)"
    );
    slash_submit(&mut app, "chat");
    assert_eq!(
        app.state().view_mode,
        ViewMode::Chat,
        "/chat 应切换视图模式(W3 改接)"
    );
}

#[test]
fn backslash_in_insert_mode_types_char_not_toggles() {
    // Insert 模式下 `\` 是文本字符,不应触发视图切换(模式隔离)
    let mut app = make_app();
    press(&mut app, KeyCode::Char('i'));
    press(&mut app, KeyCode::Char('\\'));
    assert_eq!(
        app.state().view_mode,
        ViewMode::Chat,
        "Insert 内 \\ 不应切视图"
    );
    assert!(
        app.state().input_buffer.contains('\\'),
        "Insert 内 \\ 应作为文本输入"
    );
}

// ============================================================
// B. Chat 模式渲染
// ============================================================

#[test]
fn chat_mode_renders_composer_hint_in_normal() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    let out = render_to_string(&mut app);
    // WHY ASCII 断言:宽字符(CJK)在 TestBackend 逐单元格重组时存在续位
    // 单元格歧义,En 文案断言对重建路径鲁棒(zh 键覆盖由 i18n 测试守护)
    assert!(
        out.contains("Press i to type"),
        "Normal 态应显示 composer 输入提示"
    );
}

#[test]
fn chat_mode_insert_shows_input_bar() {
    let mut app = make_app();
    press(&mut app, KeyCode::Char('i'));
    assert_eq!(app.state().input_mode, InputMode::Insert);
    type_str(&mut app, "hello");
    let out = render_to_string(&mut app);
    assert!(out.contains(">"), "Insert 态 composer 应显示输入前缀");
    assert!(out.contains("hello"), "composer 应回显输入内容");
}

#[test]
fn dashboard_mode_renders_panel_tabs() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_dashboard_app();
    let out = render_to_string(&mut app);
    // Dashboard 布局含面板边框标题(En 文案 ASCII),与 Chat 视图可区分
    assert!(out.contains("Quest Tasks"), "Dashboard 视图应渲染面板标题");
}

// ============================================================
// C. 会话流内嵌卡片(T3.3)
// ============================================================

fn failed_quest_event(quest_id: &str) -> NexusEvent {
    NexusEvent::QuestCompleted {
        metadata: EventMetadata::new("test"),
        quest_id: quest_id.into(),
        status: QuestStatus::Failed,
    }
}

#[test]
fn reflection_card_appears_after_quest_failure() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    app.state_mut()
        .latest_events
        .push_back(failed_quest_event("q-42"));
    let out = render_to_string(&mut app);
    assert!(
        out.contains("Failure Reflection"),
        "Quest 失败后应渲染复盘卡"
    );
    assert!(out.contains("q-42"), "复盘卡应含失败 Quest ID");
}

#[test]
fn plan_card_appears_with_quest_and_no_failure() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    app.state_mut().quest_list.push(Quest {
        quest_id: "q-1".into(),
        title: "Refactor Plan".into(),
        tasks: vec![Task {
            task_id: "t1".into(),
            description: "implement newline gate".into(),
            status: TaskStatus::Running,
            dependencies: vec![],
        }],
        ..Default::default()
    });
    let out = render_to_string(&mut app);
    assert!(out.contains("Plan:"), "有 Quest 时应渲染计划卡标题");
    assert!(out.contains("implement newline gate"), "计划卡应含任务描述");
}

#[test]
fn reflection_card_takes_priority_over_plan_card() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    app.state_mut().quest_list.push(Quest {
        quest_id: "q-1".into(),
        title: "Refactor Plan".into(),
        tasks: vec![Task {
            task_id: "t1".into(),
            description: "implement newline gate".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        ..Default::default()
    });
    app.state_mut()
        .latest_events
        .push_back(failed_quest_event("q-1"));
    let out = render_to_string(&mut app);
    // 失败告警优先:复盘卡存在
    assert!(out.contains("Failure Reflection"), "失败告警应优先渲染");
    assert!(
        !out.contains("implement newline gate"),
        "复盘卡存在时计划卡让位"
    );
}

// ============================================================
// D. 流式行闸门语义(T3.1 经 ChatSync)
// ============================================================

#[test]
fn chat_stream_holds_partial_line_until_newline() {
    // 经 ChatSync 验证行闸门接线:半行暂存、完整行可见、Completed 冲刷
    use chimera_tui::data::sync::ChatSync;
    let mut sync = ChatSync::new(100);
    sync.apply_event(&NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("test"),
        session_id: "s1".into(),
        query: "hi".into(),
        slash_command: None,
    });
    sync.apply_event(&NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("orchestrator"),
        session_id: "s1".into(),
        delta: "第一行\n半".into(),
        cursor_hint: 0,
    });
    let msgs = sync.messages();
    assert_eq!(msgs[1].content, "第一行\n", "完整行应立即可见");
    sync.apply_event(&NexusEvent::TuiChatCompleted {
        metadata: EventMetadata::new("orchestrator"),
        session_id: "s1".into(),
        tool_use: None,
    });
    assert_eq!(
        sync.messages()[1].content,
        "第一行\n半",
        "Completed 应冲刷残段不丢内容"
    );
}

#[test]
fn chat_stream_fence_block_commits_as_whole() {
    // fence 块稳态:未闭合围栏内行暂存,闭合后整块提交(避免代码块半块闪烁)
    use chimera_tui::data::sync::ChatSync;
    let mut sync = ChatSync::new(100);
    sync.apply_event(&NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("test"),
        session_id: "s1".into(),
        query: "hi".into(),
        slash_command: None,
    });
    sync.apply_event(&NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("orchestrator"),
        session_id: "s1".into(),
        delta: "before\n```\ncode1\n".into(),
        cursor_hint: 0,
    });
    // 围栏未闭合:before 行可见,fence 块暂存
    assert_eq!(sync.messages()[1].content, "before\n");
    sync.apply_event(&NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("orchestrator"),
        session_id: "s1".into(),
        delta: "code2\n```\n".into(),
        cursor_hint: 0,
    });
    // 围栏闭合:整块一次性提交
    assert_eq!(
        sync.messages()[1].content,
        "before\n```\ncode1\ncode2\n```\n",
        "fence 块应闭合后整块提交"
    );
}

#[test]
fn insert_submit_respects_view_mode_panel_policy() {
    // Concord W3 T3.2:Chat 模式提交后不再切面板(已全屏);
    // Dashboard 模式保持原行为(提交后自动切到 Chat 面板)
    let mut chat_app = make_app();
    let panel_before = chat_app.current_panel();
    press(&mut chat_app, KeyCode::Char('i'));
    type_str(&mut chat_app, "hello");
    press(&mut chat_app, KeyCode::Enter);
    assert_eq!(
        chat_app.current_panel(),
        panel_before,
        "Chat 模式提交不应改变面板焦点(会话流已全屏)"
    );

    let mut dash_app = make_dashboard_app();
    press(&mut dash_app, KeyCode::Char('i'));
    type_str(&mut dash_app, "hello");
    press(&mut dash_app, KeyCode::Enter);
    assert_eq!(
        dash_app.current_panel(),
        chimera_tui::PanelId::Chat,
        "Dashboard 模式提交后应自动切到 Chat 面板(原行为保留)"
    );
}

// ============================================================
// E. view_mode 持久化往返(T3.4)
// ============================================================

#[test]
fn view_mode_persistence_roundtrip() {
    let dir = std::env::temp_dir().join(format!("chimera_w3_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("view_mode_state.yaml");

    let mut state = TuiState::new();
    assert_eq!(state.view_mode, ViewMode::Chat, "新状态默认 Chat");
    state.view_mode = ViewMode::Dashboard;
    state.save_to_file(&path).expect("save state");

    let loaded = TuiState::load_from_file(&path);
    assert_eq!(
        loaded.view_mode,
        ViewMode::Dashboard,
        "持久化往返应保留视图模式"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn legacy_state_file_without_view_mode_defaults_chat() {
    // 旧状态文件无 view_mode 字段:serde 默认取 Chat(第一默认)
    let dir = std::env::temp_dir().join(format!("chimera_w3b_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("legacy_state.yaml");
    std::fs::write(
        &path,
        "running: true\ninput_mode: Normal\ninput_buffer: ''\nframe_count: 0\n",
    )
    .expect("write legacy state");
    let loaded = TuiState::load_from_file(&path);
    assert_eq!(
        loaded.view_mode,
        ViewMode::Chat,
        "旧状态文件缺 view_mode 时应得 Chat 默认"
    );
    std::fs::remove_file(&path).ok();
}

// ============================================================
// F. ModeBanner 常驻横幅(Concord W6 T6.1)
// ============================================================

fn shift_tab(app: &mut TuiApp) {
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
}

#[test]
fn banner_shown_in_plan_mode_chat_view() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    shift_tab(&mut app); // Normal → Plan
    let out = render_to_string(&mut app);
    assert!(out.contains("PLAN mode"), "Plan 态应渲染 ModeBanner 横幅");
}

#[test]
fn banner_shown_in_auto_mode_chat_view() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    shift_tab(&mut app); // → Plan
    shift_tab(&mut app); // → Auto
    let out = render_to_string(&mut app);
    assert!(out.contains("AUTO mode"), "Auto 态应渲染 ModeBanner 横幅");
}

#[test]
fn banner_hidden_in_normal_mode() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    let out = render_to_string(&mut app);
    assert!(
        !out.contains("PLAN mode") && !out.contains("AUTO mode"),
        "Normal 态不应渲染横幅"
    );
}

#[test]
fn banner_disappears_after_cycling_back_to_normal() {
    let _g = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app();
    shift_tab(&mut app); // → Plan(横幅出现)
    shift_tab(&mut app); // → Auto
    shift_tab(&mut app); // → Normal(横幅消失)
    let out = render_to_string(&mut app);
    assert!(
        !out.contains("PLAN mode") && !out.contains("AUTO mode"),
        "切回 Normal 后横幅应消失"
    );
}

// ============================================================
// G. composer 历史持久化(Concord W6 T6.2)
// ============================================================

#[test]
fn input_history_persistence_roundtrip() {
    let dir = std::env::temp_dir().join(format!("chimera_w6_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("history_state.yaml");

    let mut state = TuiState::new();
    state.input_history.push_back("first".to_string());
    state.input_history.push_back("second".to_string());
    state.save_to_file(&path).expect("save state");

    let loaded = TuiState::load_from_file(&path);
    let v: Vec<&str> = loaded.input_history.iter().map(|s| s.as_str()).collect();
    assert_eq!(v, vec!["first", "second"], "持久化往返应保留输入历史");
    assert_eq!(loaded.history_pos, None, "导航位置不跨会话延续");
    std::fs::remove_file(&path).ok();
}
