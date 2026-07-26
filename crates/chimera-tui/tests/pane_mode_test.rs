//! PaneMode 多窗格集成测试 — M3d(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **黑盒公共 API**:经 `TuiApp` 公共 API(按键 / 切面板 / `active_pane()` /
//!   `pane_count()`)驱动,验证对外可观测的窗格模型行为,不触碰私有字段。
//! - **窗格数与活跃索引**:M3d 把 Stage 2 的"主+单一伴随"2 窗格泛化为 PaneMode
//!   驱动的多窗格(Chat 2 / VimSplit 2 / IDE 3),用 `pane_count()` 断言窗格数、
//!   `active_pane()` 断言 `w` 环形循环,覆盖 IDE 三窗格与 VimSplit 双分屏两条新路径。
//! - **零回归 + 不 panic**:各 PaneMode × 宽/窄视口渲染均不 panic;窄视口收敛单窗格。

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

/// Ctrl+<char> 组合键(Ctrl+W 方向导航前缀)
fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// 在内存后端渲染一帧(仅验证不 panic)
fn render_once(app: &mut TuiApp, width: u16, height: u16) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
}

/// 造出确定的 context 目标:先切 Quest 再切 Parliament → prev=Quest, focused=Parliament。
///
/// WHY 两次切换:第二次切换后 `prev_panel` 必为 Quest,使 context 窗格确定性地指向 Quest,
/// 且 `active_pane` 经 `record_prev_panel` 复位为 0(主区)。
fn focus_parliament_with_quest_companion(app: &mut TuiApp) {
    app.switch_panel_to(PanelId::Quest);
    app.switch_panel_to(PanelId::Parliament);
}

// ============================================================
// l 循环纳入 VimSplit(第 4 布局态)
// ============================================================

#[test]
fn vim_split_reachable_via_l_cycle() {
    let mut app = make_app();
    // Dual → Triple → VimSplit(两次 l)
    app.handle_key_event(key(KeyCode::Char('l')));
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(
        app.state().layout_mode.as_str(),
        "vim",
        "两次 l 应从 Dual 经 Triple 到达 VimSplit(M3d 4 态循环)"
    );
}

// ============================================================
// IDE 三窗格(内在多窗格,与 companion_visible 无关)
// ============================================================

#[test]
fn ide_mode_has_three_panes_and_w_cycles() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    // Dual → Triple(Ide,一次 l):IDE 内在三窗格(主 + context + 侧栏)
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.state().layout_mode.as_str(), "triple");
    assert_eq!(app.pane_count(), 3, "IDE 模式应有 3 窗格");
    assert_eq!(app.active_pane(), 0, "默认活跃主窗格(索引 0)");

    // w 键环形循环:main(0) → context(1) → sidebar(2) → main(0)
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 1, "w 应切到 context 窗格");
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 2, "w 应切到侧栏窗格");
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 0, "w 应环形回到主窗格");
}

#[test]
fn ide_active_pane_key_routing_no_panic() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide)
    let main_before = app.current_panel();
    // 聚焦侧栏窗格后,面板级键路由到侧栏面板:不 panic、主焦点不变、应用不退出
    app.handle_key_event(key(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 2);
    app.handle_key_event(key(KeyCode::Char('j')));
    app.handle_key_event(key(KeyCode::Down));
    assert!(app.state().running, "面板级键不应退出应用");
    assert_eq!(app.current_panel(), main_before, "主焦点不应因面板级键改变");
    assert_eq!(app.active_pane(), 2, "面板级键不应改变活跃窗格");
}

// ============================================================
// VimSplit 双等分窗格
// ============================================================

#[test]
fn vim_split_has_two_panes_and_w_cycles() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    // Dual → Triple → VimSplit(两次 l)
    app.handle_key_event(key(KeyCode::Char('l')));
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.state().layout_mode.as_str(), "vim");
    assert_eq!(app.pane_count(), 2, "VimSplit 应有 2 等分窗格");

    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 1, "w 应切到右分屏");
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 0, "2 窗格环形回到主窗格");
}

// ============================================================
// Focus 单窗格(w no-op)
// ============================================================

#[test]
fn focus_mode_single_pane_w_noop() {
    let mut app = make_app();
    // Dual → Triple → VimSplit → Single(三次 l)
    for _ in 0..3 {
        app.handle_key_event(key(KeyCode::Char('l')));
    }
    assert_eq!(app.state().layout_mode.as_str(), "single");
    assert_eq!(app.pane_count(), 1, "Focus 模式仅主窗格");
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 0, "单窗格 w 应 no-op(索引不变)");
}

// ============================================================
// 切主面板复位活跃窗格 + 切布局钳制
// ============================================================

#[test]
fn switch_panel_resets_active_pane() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide),3 窗格
    app.handle_key_event(key(KeyCode::Char('w'))); // active = 1
    assert_eq!(app.active_pane(), 1);
    // 切主面板(数字键 3 → Budget)应复位活跃窗格回主区
    app.handle_key_event(key(KeyCode::Char('3')));
    assert_eq!(app.active_pane(), 0, "切主面板应复位活跃窗格");
}

#[test]
fn switch_layout_clamps_active_pane() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide),3 窗格
    app.handle_key_event(key(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('w'))); // active = 2(侧栏)
    assert_eq!(app.active_pane(), 2);
    // Triple → VimSplit(2 窗格):active=2 越界,应钳制回 0
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.state().layout_mode.as_str(), "vim");
    assert_eq!(
        app.active_pane(),
        0,
        "切到更少窗格的布局应钳制活跃窗格回主区"
    );
}

// ============================================================
// 各 PaneMode × 视口渲染不 panic(含窄视口收敛)
// ============================================================

#[test]
fn all_pane_modes_render_no_panic() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    // 遍历 4 布局 × (宽 120×30 / 窄 40×20)视口,渲染均不应 panic
    for _ in 0..4 {
        render_once(&mut app, 120, 30);
        render_once(&mut app, 40, 20);
        app.handle_key_event(key(KeyCode::Char('l'))); // 切下一布局
    }
}

#[test]
fn narrow_viewport_collapses_render_no_panic() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    // 切到 Triple(Ide);逻辑窗格数仍为 3(供键盘路由),
    // 窄视口(< 60)渲染收敛为单窗格,不 panic
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.pane_count(), 3);
    render_once(&mut app, 40, 20);
    // 活跃窗格可停留在越界前的逻辑索引,渲染兜底不 panic
    app.handle_key_event(key(KeyCode::Char('w')));
    render_once(&mut app, 40, 20);
}

// ============================================================
// Ctrl+W h/l 方向导航(M3 后续)
// ============================================================

#[test]
fn ctrl_w_directional_focus_in_ide() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide),3 窗格
    render_once(&mut app, 120, 30); // 建立 last_area(方向导航依赖窗格几何)
    assert_eq!(app.pane_count(), 3);
    assert_eq!(app.active_pane(), 0, "初始主窗格");

    // IDE 位置:sidebar(左) | 主(中) | context(右);循环序 [主=0, context=1, sidebar=2]
    // Ctrl+W l:主(中)→ 右侧 context
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.active_pane(), 1, "Ctrl+W l 从主窗格聚焦右侧 context");

    // Ctrl+W h:context(右)→ 最近左邻是主(中)
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.active_pane(), 0, "Ctrl+W h 从 context 回到主窗格");

    // Ctrl+W h:主(中)→ 左侧 sidebar
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.active_pane(), 2, "Ctrl+W h 从主窗格聚焦左侧 sidebar");
}

#[test]
fn ctrl_w_j_k_noop_in_horizontal_layout() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide)
    render_once(&mut app, 120, 30);
    assert_eq!(app.active_pane(), 0);
    // 横向布局无上下堆叠窗格:Ctrl+W j / k 应 no-op
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(app.active_pane(), 0, "横向布局 Ctrl+W j 应 no-op");
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('k')));
    assert_eq!(app.active_pane(), 0, "横向布局 Ctrl+W k 应 no-op");
}

#[test]
fn ctrl_w_w_cycles_like_plain_w() {
    let mut app = make_app();
    focus_parliament_with_quest_companion(&mut app);
    app.handle_key_event(key(KeyCode::Char('l'))); // Triple(Ide)
    render_once(&mut app, 120, 30);
    // Ctrl+W w → 循环(与 plain w 一致)
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('w')));
    assert_eq!(app.active_pane(), 1, "Ctrl+W w 应循环到下一窗格");
}

#[test]
fn ctrl_w_direction_single_pane_noop() {
    let mut app = make_app();
    // 默认 Dual + companion 关闭 → 单窗格
    render_once(&mut app, 120, 30);
    assert_eq!(app.pane_count(), 1);
    app.handle_key_event(ctrl(KeyCode::Char('w')));
    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.active_pane(), 0, "单窗格 Ctrl+W l 应 no-op");
}
