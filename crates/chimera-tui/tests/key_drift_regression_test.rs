//! 键位漂移回归测试 — 面板绑定 vs 全局截获(Concord 键位治理)
//!
//! 对应架构层:L10 Interface
//!
//! # 背景(WHY)
//! InputRouter 在 Normal 模式先查全局 codegen 键位表,面板级 `handle_key`
//! 只收到路由器未截获的键。历史上多个面板声明了与全局键冲突的绑定(死键),
//! 且内联测试直调 `panel.handle_key` 绕过路由器形成假绿。本文件以黑盒事件流
//! (`TuiApp::handle_key_event`)锚定真实用户路径,防止键位漂移回归:
//! - TaskManager 增量搜索:`f`(原 `/` 被全局 EnterSlash 截获,功能曾丢失)
//! - MetricsDashboard:`l` 归全局 view.switch_layout(原面板 arm 为死键)
//! - `a` 动作菜单全链路(打开/移选/执行/关闭)
//! - ConfigMenu 经 Ctrl+P palette 触达(↑↓/Enter 就地循环)

#![forbid(unsafe_code)]

use chimera_tui::{InputMode, PanelId, TuiApp, TuiConfig, ViewMode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造默认 TuiApp(Dashboard 视图,无 event-bus,内存桩数据源)
fn make_app() -> TuiApp {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.state_mut().view_mode = ViewMode::Dashboard;
    app
}

/// 无修饰符按键
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Ctrl+<char> 组合键
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// 逐字符送入(palette / slash 检索输入)
fn type_chars(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
}

/// 经 Tab 焦点轮转把焦点切到目标面板(黑盒:只走公开 focus cycle)
fn focus_panel(app: &mut TuiApp, target: PanelId) {
    for _ in 0..app.panel_focus_order().len() {
        if app.current_panel() == target {
            return;
        }
        app.handle_key_event(key(KeyCode::Tab));
    }
    panic!("面板 {target:?} 无法经 Tab 轮转到达(焦点环不完整?)");
}

/// 在内存后端渲染一次,返回整帧文本(用于断言面板渲染内容)
///
/// WHY 宽字符感知(跳过 CJK 续格):CJK 宽字符占两格,续格 cell 会被朴素
/// 逐格收集替换为空格,导致 `过滤:` 等 zh contains 断言失配(批次-A 实测);
/// 与 osa_sparse/pvl_score 测试的 render_panel_to_string 同口径。
fn render_frame(app: &mut TuiApp) -> String {
    let backend = TestBackend::new(160, 48);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let mut out = String::new();
    let mut prev_wide = false;
    for cell in terminal.backend().buffer().content().iter() {
        if cell.skip {
            continue;
        }
        let s = cell.symbol();
        if s.is_empty() {
            continue;
        }
        if prev_wide {
            prev_wide = false;
            continue;
        }
        let ch = s.chars().next().unwrap_or(' ');
        prev_wide = unicode_width::UnicodeWidthStr::width(s) >= 2;
        out.push(ch);
    }
    out
}

// ============================================================
// 1. TaskManager 搜索:`f` 进面板搜索模式;`/` 归全局斜杠命令
// ============================================================

#[test]
fn task_manager_f_enters_panel_search_mode() {
    // 批次-A i18n 迁移后过滤标记走键表(zh=" (过滤: {})",en=" (filter: {})"),
    // 本测试断言 zh 渲染 → 钉 Zh locale(与面板 i18n 断言同范式)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let mut app = make_app();
    focus_panel(&mut app, PanelId::TaskManager);

    // 基线:标题无过滤标记
    let before = render_frame(&mut app);
    assert!(
        !before.contains("过滤:") && !before.contains("(filter:"),
        "基线渲染不应出现搜索态标题, got: {}",
        before
    );

    // `f`(原 `/` 被 EnterSlash 截获,面板搜索不可达)→ 面板进入搜索模式
    app.handle_key_event(key(KeyCode::Char('f')));
    let after = render_frame(&mut app);
    assert!(
        after.contains("过滤:"),
        "按 `f` 后 TaskManager 标题应追加 `(过滤: ...)` 搜索态标记, got: {after}"
    );
    // WHY 恢复默认 Zh:key_drift 其余测试未加锁,依赖边界处 locale=默认
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    drop(_locale_guard);
}

#[test]
fn task_manager_search_filters_rendered_list() {
    // 搜索态下输入字符应实时过滤列表(增量搜索语义可观测)
    // 批次-A i18n 迁移后过滤标记断言钉 Zh(同 task_manager_f_enters_panel_search_mode)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let mut app = make_app();
    focus_panel(&mut app, PanelId::TaskManager);
    app.handle_key_event(key(KeyCode::Char('f')));
    // 搜索模式下字符进入面板过滤关键字(非全局输入缓冲)
    app.handle_key_event(key(KeyCode::Char('z')));
    let frame = render_frame(&mut app);
    assert!(
        frame.contains("过滤: z"),
        "搜索态输入 'z' 应反映在标题过滤关键字中, got: {frame}"
    );
}

#[test]
fn task_manager_slash_stays_global_slash_mode() {
    let mut app = make_app();
    focus_panel(&mut app, PanelId::TaskManager);
    app.handle_key_event(key(KeyCode::Char('/')));
    assert_eq!(
        app.state().input_mode,
        InputMode::Slash,
        "`/` 应进入全局斜杠命令模式,不被面板消费"
    );
    let frame = render_frame(&mut app);
    assert!(
        !frame.contains("(filter:"),
        "`/` 不应让面板进入搜索态(原死键语义已移除)"
    );
}

// ============================================================
// 2. MetricsDashboard:`l` 归全局布局切换(面板 arm 已移除)
// ============================================================

#[test]
fn metrics_dashboard_l_switches_layout_globally() {
    let mut app = make_app();
    focus_panel(&mut app, PanelId::MetricsDashboard);
    let before = app.state().layout_mode;
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_ne!(
        app.state().layout_mode,
        before,
        "MetricsDashboard 聚焦时按 `l` 应触发全局 view.switch_layout"
    );
}

// ============================================================
// 3. `a` 动作菜单全链路(打开 / 移选 / 执行 / 关闭)
// ============================================================

#[test]
fn action_menu_opens_moves_and_executes_on_quest() {
    let mut app = make_app();
    assert_eq!(app.current_panel(), PanelId::Quest, "初始焦点应为 Quest");

    // bare `a` → 打开 Quest 上下文动作菜单
    app.handle_key_event(key(KeyCode::Char('a')));
    match app.state().popup_stack.current() {
        Some(chimera_tui::popup::PopupKind::ActionMenu { title, entries, .. }) => {
            assert_eq!(title, "Quest", "菜单标题应为焦点面板名");
            assert!(
                entries.iter().any(|(id, _)| id == "quest.pause"),
                "Quest 动作菜单应含 quest.pause, got: {entries:?}"
            );
        }
        other => panic!("按 `a` 应打开 ActionMenu 弹窗, got: {other:?}"),
    }
    // 初始选中第一项
    assert_eq!(
        app.state().popup_stack.action_menu_selected_id().as_deref(),
        Some("quest.pause")
    );

    // `j` 下移选择
    app.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(
        app.state().popup_stack.action_menu_selected_id().as_deref(),
        Some("quest.resume"),
        "`j` 应下移到第二项 quest.resume"
    );

    // Enter 执行选中动作:菜单关闭 + 编排域动作发布请求(pending deadline 置位)
    app.handle_key_event(key(KeyCode::Enter));
    assert!(
        app.state().popup_stack.is_empty(),
        "Enter 执行后动作菜单应关闭"
    );
    assert!(
        app.state().pending_action_deadline.is_some(),
        "执行 quest.resume 应经编排域派发(TuiActionRequested 兜底 deadline 置位)"
    );
}

#[test]
fn action_menu_esc_closes_without_executing() {
    let mut app = make_app();
    app.handle_key_event(key(KeyCode::Char('a')));
    assert!(!app.state().popup_stack.is_empty(), "`a` 应打开动作菜单");
    app.handle_key_event(key(KeyCode::Esc));
    assert!(
        app.state().popup_stack.is_empty(),
        "Esc 应关闭动作菜单且不执行任何动作"
    );
    assert!(
        app.state().pending_action_deadline.is_none(),
        "Esc 关闭不应触发动作派发"
    );
}

// ============================================================
// 4. ConfigMenu 经 Ctrl+P palette 触达(↑↓/Enter 就地循环)
// ============================================================

#[test]
fn config_menu_open_via_palette_and_cycle_ratio() {
    let mut app = make_app();

    // Ctrl+P 打开 palette,精确检索 config.edit 并执行
    app.handle_key_event(ctrl('p'));
    assert!(app.palette_is_open(), "Ctrl+P 应打开命令面板");
    type_chars(&mut app, "config.edit");
    app.handle_key_event(key(KeyCode::Enter));

    match app.state().popup_stack.current() {
        Some(chimera_tui::popup::PopupKind::ConfigMenu { .. }) => {}
        other => panic!("执行 config.edit 应打开 ConfigMenu 弹窗, got: {other:?}"),
    }

    // Down 移到第 2 项(主面板占比),Enter 就地循环:占比变化且菜单常驻
    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(
        app.state().popup_stack.config_menu_selected(),
        Some(1),
        "Down 应选中下标 1(主面板占比项)"
    );
    let ratio_before = app.main_panel_ratio();
    app.handle_key_event(key(KeyCode::Enter));
    assert_ne!(
        app.main_panel_ratio(),
        ratio_before,
        "Enter 应就地循环占比预设值"
    );
    assert!(
        !app.state().popup_stack.is_empty(),
        "ConfigMenu 编辑后菜单常驻(支持连续编辑)"
    );

    // Esc 关闭
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.state().popup_stack.is_empty(), "Esc 应关闭配置菜单");
}
