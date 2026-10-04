//! Chat 视图按键/鼠标语义测试(评估报告 I-2 / I-6 修复验收)
//!
//! # 背景(WHY)
//! Chat 视图全屏渲染会话流,但 Tab/数字键仍隐式切换不可见面板("按了没反应"),
//! 鼠标命中仍按 Dashboard 三块布局切分(composer 区点击会误入遗留命令模式)。
//! 修复后:面板切换键在 Chat 视图给出状态栏诚实提示且不切换;鼠标仅保留
//! 会话流滚轮滚动。
//!
//! R5 补充(F-01/F-02):Chat 视图 Ctrl+P 打开的命令面板必须真被渲染
//! (黑盒帧断言);Dashboard 下 palette 打开时鼠标点击底栏/tab 须一并关闭
//! palette,消除"键盘全进不可见 palette"的状态双轨。
//!
//! R5 补充(F-03):Chat 视图键盘滚动(gg/G/j/k)与滚轮均直取 Chat 面板——
//! 委托焦点/活跃窗格会把滚动落在被会话流遮挡的不可见面板(静默无反馈)。
//!
//! 契约:Dashboard 视图行为零回归(数字键/F 键/Tab 正常切换,由
//! m3a_input_routing_test 既有锚点覆盖)。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{PanelId, TuiApp, TuiConfig, ViewMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造 Dashboard 视图 TuiApp(无 event-bus,内存桩数据源)
fn make_app(view: ViewMode) -> TuiApp {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: view,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.state_mut().view_mode = view;
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Ctrl+<char> 组合键(打开命令面板的唯一入口 Ctrl+P)
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// 在内存后端渲染一帧并返回缓冲文本(黑盒渲染断言用)
fn render_frame_text(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(80, 24);
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

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// 滚轮事件(kind 指定上/下滚,滚动方向语义断言需要区分)
fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// 注入 n 条单行消息(msg-00..msg-<n-1>,内容唯一可断言)
///
/// 长对话使会话流行数超出可视区,滚动键/滚轮的生效与否可经渲染帧判别。
fn seed_long_conversation(app: &mut TuiApp, n: usize) {
    app.state_mut().chat_messages = (0..n)
        .map(|i| chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: format!("msg-{i:02}"),
        })
        .collect();
}

// ============================================================
// I-2:Chat 视图数字键 → 状态栏提示,不切换面板
// ============================================================

#[test]
fn chat_view_digit_key_shows_hint_not_switch() {
    let mut app = make_app(ViewMode::Chat);
    let before = app.current_panel();

    app.handle_key_event(key(KeyCode::Char('2')));

    assert_eq!(
        app.current_panel(),
        before,
        "Chat 视图数字键不应切换面板(切换不可见,属语义空洞)"
    );
    let (msg, severity) = app
        .state()
        .status_message
        .clone()
        .expect("Chat 视图按数字键应给出状态栏提示");
    assert_eq!(
        severity,
        chimera_tui::Severity::Info,
        "提示应为 Info 级(诚实告知而非警告)"
    );
    assert!(!msg.is_empty());
}

#[test]
fn chat_view_tab_shows_hint_not_switch() {
    let mut app = make_app(ViewMode::Chat);
    let before = app.current_panel();

    app.handle_key_event(key(KeyCode::Tab));

    assert_eq!(app.current_panel(), before, "Chat 视图 Tab 不应切换面板");
    assert!(
        app.state().status_message.is_some(),
        "Chat 视图 Tab 应给出状态栏提示"
    );
}

// ============================================================
// 零回归:Dashboard 视图数字键切换行为保持
// ============================================================

#[test]
fn dashboard_digit_key_still_switches() {
    let mut app = make_app(ViewMode::Dashboard);
    app.handle_key_event(key(KeyCode::Char('2')));
    assert_ne!(
        app.current_panel(),
        PanelId::Quest,
        "Dashboard 数字键切换行为零回归"
    );
}

// ============================================================
// I-6:Chat 视图鼠标点击不再误入遗留命令模式,滚轮滚动会话流
// ============================================================

#[test]
fn chat_view_mouse_click_does_not_enter_command_mode() {
    let mut app = make_app(ViewMode::Chat);

    // 点击任意位置(含底部 composer 区域):Chat 视图无 Dashboard 布局可命中
    app.handle_mouse_event(click(10, 28));
    app.handle_mouse_event(click(10, 0));

    assert_eq!(
        app.state().input_mode,
        chimera_tui::InputMode::Normal,
        "Chat 视图鼠标点击不得进入遗留 Command 模式"
    );
}

#[test]
fn chat_view_mouse_scroll_routes_to_chat_panel() {
    let mut app = make_app(ViewMode::Chat);
    seed_long_conversation(&mut app, 40);
    // 先渲染一帧建立贴底基线(follow=true 时 render 把偏移钳到内容底)
    let baseline = render_frame_text(&mut app);
    assert!(
        baseline.contains("msg-39") && !baseline.contains("msg-00"),
        "贴底基线应显示最新消息 msg-39 且最早消息 msg-00 不可见(40 行 > 可视区)"
    );

    // 滚轮上滚应路由到 Chat 面板(会话流):退出贴底跟随,最新消息移出可视区
    app.handle_mouse_event(wheel(MouseEventKind::ScrollUp, 10, 10));
    let scrolled = render_frame_text(&mut app);
    assert!(
        !scrolled.contains("msg-39") && scrolled.contains("msg-38"),
        "滚轮上滚应真滚动会话流(修复前 ChatPanel 未覆盖 handle_mouse,滚轮静默无操作)"
    );
}

// ============================================================
// R5 F-03:Chat 视图键盘滚动(gg/G/j/k)直取 Chat 面板
// ============================================================

/// 键盘滚动语义守护(修复前 gg/G 委托焦点面板 Quest、j/k 委托活跃窗格,
/// 滚动落在被会话流全屏遮挡的不可见面板,Chat 帧恒贴底零变化)
#[test]
fn chat_view_keyboard_scroll_routes_to_chat_panel() {
    let mut app = make_app(ViewMode::Chat);
    seed_long_conversation(&mut app, 40);
    let baseline = render_frame_text(&mut app);
    assert!(
        baseline.contains("msg-39") && !baseline.contains("msg-00"),
        "贴底基线应显示最新消息 msg-39 且最早消息 msg-00 不可见"
    );

    // gg:跳到最早消息(单 g 进入 GPrefix,次键 g → ScrollTop)
    app.handle_key_event(key(KeyCode::Char('g')));
    app.handle_key_event(key(KeyCode::Char('g')));
    let top = render_frame_text(&mut app);
    assert!(
        top.contains("msg-00") && !top.contains("msg-39"),
        "gg 应滚动 Chat 会话流到最早消息(修复前落在焦点面板 Quest,帧恒贴底)"
    );

    // j:下滚一行,msg-01 进入窗口顶(msg-00 移出)
    app.handle_key_event(key(KeyCode::Char('j')));
    let down_one = render_frame_text(&mut app);
    assert!(
        !down_one.contains("msg-00") && down_one.contains("msg-01"),
        "j 应下滚一行(msg-01 可见;修复前帧仍贴底,msg-01 不可见)"
    );

    // k:上滚一行回到窗口顶,msg-00 重新可见
    app.handle_key_event(key(KeyCode::Char('k')));
    let up_one = render_frame_text(&mut app);
    assert!(
        up_one.contains("msg-00"),
        "k 应上滚一行使 msg-00 重新可见(修复前帧恒贴底,恒不可见)"
    );

    // G:恢复贴底跟随,显示最新消息
    app.handle_key_event(key(KeyCode::Char('G')));
    let bottom = render_frame_text(&mut app);
    assert!(
        bottom.contains("msg-39") && !bottom.contains("msg-00"),
        "G 应恢复贴底跟随显示最新消息 msg-39"
    );
}

// ============================================================
// R5 F-26:贴底按 ↓(滚轮向下同语义)保持跟随 —— 修复前无条件解除
// follow,新回复到达后视口滞留原地、新消息在视口外且无未读提示
// ============================================================

#[test]
fn chat_view_bottom_down_keeps_following() {
    let mut app = make_app(ViewMode::Chat);
    seed_long_conversation(&mut app, 40);
    let baseline = render_frame_text(&mut app);
    assert!(
        baseline.contains("msg-39") && !baseline.contains("msg-00"),
        "贴底基线应显示最新消息 msg-39 且最早消息 msg-00 不可见"
    );

    // 贴底按 ↓:画面保持贴底(修复后 follow 不被解除)
    app.handle_key_event(key(KeyCode::Down));
    let after_down = render_frame_text(&mut app);
    assert!(
        after_down.contains("msg-39") && !after_down.contains("msg-00"),
        "贴底按 ↓ 应保持贴底画面不变"
    );

    // 流式新回复到达:跟随语义下新消息应立即可见。
    // 判别力:修复前按 ↓ 已解除 follow,且中间渲染帧把偏移钳回旧底部,
    // 新消息行恰在视口外 → contains 必败。
    app.state_mut()
        .chat_messages
        .push(chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: "msg-40-new-reply".into(),
        });
    let after_new = render_frame_text(&mut app);
    assert!(
        after_new.contains("msg-40-new-reply"),
        "贴底按 ↓ 后新回复到达应仍贴底可见(修复前 follow 已静默解除,新消息在视口外)"
    );
}

#[test]
fn chat_view_bottom_wheel_down_keeps_following() {
    // 滚轮向下与 Down/j 同语义(R5 F-26:滚轮 arm 同病同修)
    let mut app = make_app(ViewMode::Chat);
    seed_long_conversation(&mut app, 40);
    let baseline = render_frame_text(&mut app);
    assert!(
        baseline.contains("msg-39") && !baseline.contains("msg-00"),
        "贴底基线应显示最新消息 msg-39 且最早消息 msg-00 不可见"
    );

    app.handle_mouse_event(wheel(MouseEventKind::ScrollDown, 10, 10));
    let after_wheel = render_frame_text(&mut app);
    assert!(
        after_wheel.contains("msg-39") && !after_wheel.contains("msg-00"),
        "贴底向下滚轮应保持贴底画面不变"
    );

    app.state_mut()
        .chat_messages
        .push(chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: "msg-40-new-reply".into(),
        });
    let after_new = render_frame_text(&mut app);
    assert!(
        after_new.contains("msg-40-new-reply"),
        "贴底向下滚轮后新回复到达应仍贴底可见(修复前 follow 已静默解除)"
    );
}

// ============================================================
// R5 F-01:Chat 视图 Ctrl+P 后命令面板必须真被渲染
// ============================================================

#[test]
fn chat_view_ctrl_p_renders_palette_overlay() {
    // WHY pin En 断言(照 i18n_chrome_test.rs:6 / chat_mode_test.rs:155 先例):
    // CJK 宽字符在 TestBackend 逐单元格重组时续位格被 reset 为空格,
    // contains(zh 文案) 永假;断言文案经 t! 键表取当前 locale 值(En 侧
    // "Command Palette",ASCII 鲁棒),zh 键覆盖由 i18n 测试守护。
    // guard 防并行测试污染全局 locale。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = make_app(ViewMode::Chat);

    // 基线:palette 未开时 Chat 帧不含面板标题 —— 证明标题非背景常驻,
    // 使下方主断言对"palette 是否真被渲染"有判别力(修复前此断言必败:
    // render_chat_mode 全函数无 render_palette,键盘却被全量接管)
    let baseline = render_frame_text(&mut app);
    assert!(
        !baseline.contains(chimera_tui::t!("palette.title")),
        "palette 未打开时 Chat 帧不应含命令面板标题"
    );

    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "Ctrl+P 应打开命令面板");

    let frame = render_frame_text(&mut app);
    assert!(
        frame.contains(chimera_tui::t!("palette.title")),
        "Chat 视图打开命令面板后渲染帧应含面板标题(修复前为打开即失明)"
    );
}

// ============================================================
// R5 F-02:palette 打开时鼠标点击底栏/tab 须一并关闭 palette
// ============================================================

#[test]
fn palette_open_bottom_click_closes_palette_and_enters_slash() {
    let mut app = make_app(ViewMode::Dashboard);
    render_frame_text(&mut app); // 设置 pane_manager.last_area(鼠标命中前置)

    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "前置:Ctrl+P 打开 palette");

    // 点击底栏(y=18 落在 DualPane bottom 区,同 mouse_interaction_test 的
    // 命中坐标):应同时关闭 palette 并进入 Slash。修复前 palette 继续全量
    // 接管键盘,形成"Slash 输入栏可见、按键全进不可见 palette"的状态双轨。
    app.handle_mouse_event(click(10, 18));
    assert!(!app.palette_is_open(), "点击底栏应关闭 palette");
    assert_eq!(
        app.state().input_mode,
        chimera_tui::InputMode::Slash,
        "点击底栏应进入 Slash 模式"
    );

    // 修复后 palette 已关,单次 Esc 直接退出 Slash 回 Normal
    //(修复前该 Esc 只用于关 palette,需第二次 Esc 才能离开 Slash)
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(
        app.state().input_mode,
        chimera_tui::InputMode::Normal,
        "单次 Esc 应回到 Normal"
    );
    assert!(!app.palette_is_open(), "Esc 后 palette 保持关闭");
}

#[test]
fn palette_open_tab_click_closes_palette_and_switches_panel() {
    let mut app = make_app(ViewMode::Dashboard);
    render_frame_text(&mut app); // 设置 pane_manager.last_area(鼠标命中前置)
    let before = app.current_panel();

    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "前置:Ctrl+P 打开 palette");

    // 任务书②:handle_tab_click 与底栏点击同语义收口 —— 点击 tab 切换面板时
    // palette 一并关闭,避免面板已切而键盘仍被 palette 接管
    app.handle_mouse_event(click(3, 1));
    assert!(!app.palette_is_open(), "点击 tab 切换面板应关闭 palette");
    assert_ne!(app.current_panel(), before, "点击 tab 仍应正常切换面板");
}
