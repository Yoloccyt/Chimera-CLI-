//! 伴随面板(Companion Pane)集成测试 — M2 增量3 Stage 1(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **黑盒事件流**:经 `TuiApp` 公共 API(按键/切换面板)驱动,验证对外可观测行为
//!   (`companion_visible()`、渲染输出),不触碰私有字段。
//! - **面板体标记**:`PanelId::as_str()`("Quest")出现在 tabs;而 Quest 面板"体"
//!   渲染块标题 "Quest Tasks"(两词短语)仅在该面板作为主区/伴随区被渲染时出现,
//!   故以 "Quest Tasks" 作为"伴随面板确被渲染"的稳定标记。
//! - **零回归**:伴随面板默认关闭,渲染路径与既有一致。

#![forbid(unsafe_code)]

use chimera_tui::{PanelId, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造默认 TuiApp(无 event-bus,内存桩数据源)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 无修饰符按键
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Ctrl+<char> 组合键
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// 在内存后端渲染并收集全部 cell 字符
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
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

/// 造出确定的伴随目标:先切 Quest 再切 Parliament → prev=Quest, focused=Parliament。
///
/// WHY 两次切换:无论初始焦点为何,第二次切换后 `prev_panel` 必为 Quest,
/// 使 `companion_target()` 确定性地指向 Quest 面板。
fn focus_parliament_with_quest_companion(app: &mut TuiApp) {
    app.switch_panel_to(PanelId::Quest);
    app.switch_panel_to(PanelId::Parliament);
}

#[test]
fn companion_hidden_by_default() {
    let mut app = make_app();
    assert!(!app.companion_visible(), "伴随面板默认应关闭");
    // 默认渲染不 panic 且非空(行为与既有一致,零回归)
    let out = render_to_string(&mut app, 120, 30);
    assert!(!out.trim().is_empty(), "默认渲染应产出非空内容");
}

#[test]
fn toggle_companion_shows_second_panel() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);

    // 关闭伴随:焦点为 Parliament,不应渲染 Quest 面板体("Quest Tasks")
    let off = render_to_string(&mut app, 120, 30);
    let off_compact: String = off.chars().filter(|c| *c != ' ').collect();
    assert!(
        !off_compact.contains("任务列表"),
        "关闭伴随时不应渲染 Quest 面板体"
    );

    // 开启伴随:右栏应渲染 Quest 面板体
    app.handle_key_event(key(KeyCode::Char('\\')));
    assert!(app.companion_visible(), "\\ 键应开启伴随面板");
    let on = render_to_string(&mut app, 120, 30);
    let on_compact: String = on.chars().filter(|c| *c != ' ').collect();
    assert!(
        on_compact.contains("任务列表"),
        "开启伴随后应在右栏渲染 Quest 面板体(companion)"
    );
    assert_ne!(on, off, "伴随开启应改变渲染输出");
}

#[test]
fn companion_noop_in_focus_mode() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);

    // `l` 循环布局:Dual → Triple → VimSplit → Single(Focus 别名,M3d 4 循环)
    app.handle_key_event(key(KeyCode::Char('l')));
    app.handle_key_event(key(KeyCode::Char('l')));
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(
        app.state().layout_mode.as_str(),
        "single",
        "三次 l 应到达 SinglePane(M3d 纳入 VimSplit 后为 4 态循环)"
    );

    // Focus 模式无 context 区:开启伴随仍不渲染第二面板
    app.handle_key_event(key(KeyCode::Char('\\')));
    assert!(app.companion_visible());
    let out = render_to_string(&mut app, 120, 30);
    assert!(
        !out.contains("Quest Tasks"),
        "Focus 模式伴随面板应 no-op(无 context 区)"
    );
}

#[test]
fn companion_toggle_via_palette() {
    let mut app = make_app();
    assert!(!app.companion_visible());
    // 命令面板过滤 view.toggle_companion 并执行
    app.handle_key_event(ctrl('p'));
    for c in "toggle_companion".chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
    app.handle_key_event(key(KeyCode::Enter));
    assert!(
        app.companion_visible(),
        "命令面板执行 view.toggle_companion 应开启伴随面板"
    );
}

#[test]
fn narrow_width_does_not_split() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('\\')));
    assert!(app.companion_visible());
    // 宽度 < 60:不切分、不 panic,故不渲染伴随面板体
    let out = render_to_string(&mut app, 40, 20);
    assert!(!out.contains("Quest Tasks"), "窄视口不应切分出伴随栏");
}

// ============================================================
// Stage 2:可循环绑定 + 跨窗格焦点
// ============================================================

#[test]
fn cycle_companion_binds_and_shows() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::Parliament);
    // `]` 循环绑定:置可见 + 绑定一个非主焦点面板
    app.handle_key_event(key(KeyCode::Char(']')));
    assert!(app.companion_visible(), "] 循环应置伴随面板可见");
    let target = app.companion_panel();
    assert!(target.is_some(), "循环后应有伴随目标");
    assert_ne!(target, Some(PanelId::Parliament), "伴随目标不应等于主焦点");
    // 渲染不 panic
    let _ = render_to_string(&mut app, 120, 30);
}

#[test]
fn cycle_companion_skips_focused() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::Parliament);
    // 多次循环,伴随目标始终 != 主焦点
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Char(']')));
        assert_ne!(
            app.companion_panel(),
            Some(PanelId::Parliament),
            "循环伴随目标不应等于主焦点 Parliament"
        );
    }
}

#[test]
fn focus_pane_toggles_only_when_visible() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    // 伴随关闭:w 无效(no-op)
    app.handle_key_event(key(KeyCode::Char('w')));
    assert!(!app.companion_focused(), "伴随关闭时 w 应 no-op");
    // 开启伴随后 w 生效
    app.handle_key_event(key(KeyCode::Char('\\')));
    app.handle_key_event(key(KeyCode::Char('w')));
    assert!(app.companion_focused(), "伴随可见时 w 应聚焦伴随窗格");
}

#[test]
fn keys_route_to_companion_when_focused() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    let main_before = app.current_panel();
    app.handle_key_event(key(KeyCode::Char('\\'))); // 显示伴随
    app.handle_key_event(key(KeyCode::Char('w'))); // 聚焦伴随
    assert!(app.companion_focused());
    // 面板级键路由到伴随面板:不 panic、主焦点不变、应用不退出
    app.handle_key_event(key(KeyCode::Char('j')));
    app.handle_key_event(key(KeyCode::Down));
    assert!(app.state().running, "面板级键不应退出应用");
    assert_eq!(app.current_panel(), main_before, "主焦点不应因面板级键改变");
    assert!(app.companion_focused(), "面板级键不应改变窗格焦点");
}

#[test]
fn switch_panel_resets_companion_focus() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('\\'))); // 显示
    app.handle_key_event(key(KeyCode::Char('w'))); // 聚焦伴随
    assert!(app.companion_focused());
    // 切换主面板(数字键 3 → Budget)应复位窗格焦点
    app.handle_key_event(key(KeyCode::Char('3')));
    assert!(!app.companion_focused(), "切换主面板应复位跨窗格焦点");
}

#[test]
fn render_active_pane_highlight_no_panic() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('\\'))); // 显示伴随(主区高亮)
    let _ = render_to_string(&mut app, 120, 30);
    app.handle_key_event(key(KeyCode::Char('w'))); // 聚焦伴随(伴随高亮)
    let _ = render_to_string(&mut app, 120, 30);
    // 窄视口(不切分)也不 panic
    let _ = render_to_string(&mut app, 40, 20);
}
