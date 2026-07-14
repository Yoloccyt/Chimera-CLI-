//! P4.4 FPS 显示集成测试
//!
//! 验证 FPS 计算与状态栏显示

use chimera_tui::config::TuiConfig;
use chimera_tui::TuiApp;
use ratatui::Terminal;

/// 渲染并返回字符串
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
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

/// 验证连续 render 后 fps 被更新(非零)
#[test]
fn fps_updated_after_consecutive_renders() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    // 首帧 fps 可能为 0(无历史数据)
    terminal.draw(|f| app.render(f)).unwrap();

    // 模拟帧间隔
    std::thread::sleep(std::time::Duration::from_millis(16));
    terminal.draw(|f| app.render(f)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(16));
    terminal.draw(|f| app.render(f)).unwrap();

    // 经过 3 帧后 fps 应该被更新为非零值
    assert!(app.state().fps > 0, "FPS should be > 0 after 3 frames");
}

/// 验证 fps 不会超过显示上限 999
#[test]
fn fps_within_display_max() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    // 连续快速渲染(模拟高帧率)
    for _ in 0..10 {
        terminal.draw(|f| app.render(f)).unwrap();
    }

    assert!(
        app.state().fps <= 999,
        "FPS should not exceed display max 999, got {}",
        app.state().fps
    );
}

/// 验证状态栏包含 FPS 文本
#[test]
fn status_bar_contains_fps_text() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();

    // 渲染几帧让 FPS 有值
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let _ = render_to_string(&mut app, 80, 24);
    }

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("FPS") || content.contains("fps"),
        "Status bar should contain FPS text"
    );
}
