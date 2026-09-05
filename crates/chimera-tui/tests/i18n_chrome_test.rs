//! i18n chrome 集成测试 — 状态栏/提示栏随 Ctrl+L 中英切换(ADR-029,v3.1 M2 i18n Slice 1)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **断言英文侧**:中文默认渲染为 CJK 双宽字符(单元格间夹空,`contains("面板")` 不可靠),
//!   故以英文标签(ASCII 单宽、连续)作稳定断言:中文默认时英文标签**缺席**,
//!   Ctrl+L 切英文后**出现**。
//! - **locale 串行化**:界面语言为全局静态,涉及 locale 的测试用同一 `Mutex` 互斥。

#![forbid(unsafe_code)]

use std::sync::Mutex;

use chimera_tui::{set_locale, Locale, TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 串行化所有会读写全局 locale 的测试
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn make_app() -> TuiApp {
    {
        let mut __app = TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .unwrap();
        __app.state_mut().view_mode = chimera_tui::ViewMode::Dashboard;
        __app
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

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

#[test]
fn hint_bar_localizes_with_ctrl_l() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::Zh);
    let mut app = make_app();

    // 中文默认:提示栏为中文,英文复合标记("q:Quit"/"Tab:Next")缺席。
    // WHY 用提示栏复合标记:提示栏是纯 chrome、内容受控;面板体含大量英文
    // (slice 1 未 i18n 面板),故不能对整屏做英文缺席断言。
    let zh = render_to_string(&mut app, 80, 24);
    assert!(!zh.contains("q:Quit"), "中文默认提示栏不应出现英文 q:Quit");
    assert!(
        !zh.contains("Tab:Next"),
        "中文默认提示栏不应出现英文 Tab:Next"
    );

    // Ctrl+L 切英文:提示栏英文标记出现
    app.handle_key_event(ctrl('l'));
    let en = render_to_string(&mut app, 80, 24);
    assert!(en.contains("q:Quit"), "切英文后提示栏应出现 q:Quit");
    assert!(en.contains("Tab:Next"), "切英文后提示栏应出现 Tab:Next");

    set_locale(Locale::Zh); // 复位
}

#[test]
fn layout_and_theme_messages_localize() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::En);
    let mut app = make_app();

    // 布局切换反馈消息(英文)
    app.handle_key_event(key(KeyCode::Char('l')));
    let after_l = render_to_string(&mut app, 80, 24);
    assert!(
        after_l.contains("Layout"),
        "切英文后布局反馈应含 Layout 标签"
    );

    // 主题切换反馈消息(英文,覆盖上一条状态消息)
    app.handle_key_event(key(KeyCode::Char('t')));
    let after_t = render_to_string(&mut app, 80, 24);
    assert!(after_t.contains("Theme"), "切英文后主题反馈应含 Theme 标签");

    set_locale(Locale::Zh); // 复位
}
