//! P4.2 Parliament 面板虚拟滚动集成测试
//!
//! 验证 Parliament 面板在 1000+ 条事件下的虚拟滚动行为
//! (F-09/F-14 加固:内容级断言 + 真实滚动键;旧版四用例仅断言边框标题,
//! 且 j/单 g 的"滚动动作"实际从未触达面板——j 不被面板处理、单 g 只进前缀态)

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use std::collections::VecDeque;

mod common;
use common::mock::MockDataSource;

use chimera_tui::config::TuiConfig;
use chimera_tui::data::DataSnapshot;
use chimera_tui::types::PanelId;
use chimera_tui::TuiApp;
use event_bus::{EventMetadata, NexusEvent};
use ratatui::Terminal;

/// 构造含 1000 条 VoteCast 事件的快照
fn snapshot_with_1000_votes() -> DataSnapshot {
    let mut events = VecDeque::new();
    for i in 0..1000u32 {
        let event = NexusEvent::VoteCast {
            metadata: EventMetadata::new("parliament"),
            proposal_id: format!("prop-{i}"),
            voter: format!("voter-{i}"),
            vote: i % 2 == 0,
        };
        events.push_back(event);
    }
    DataSnapshot {
        latest_events: std::sync::Arc::new(events),
        ..Default::default()
    }
}

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

/// 构造带 1000 事件的 app
fn app_with_1000_votes() -> TuiApp {
    let snapshot = snapshot_with_1000_votes();
    let ds = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(ds),
    )
    .unwrap();
    app.switch_panel_to(PanelId::Parliament);
    app
}

/// 1000 条事件渲染不 panic;顶部窗口最新在前且选中前缀可见
///
/// WHY 先 `app.update()`:`TuiApp::render` 不拉快照(仅事件循环 step 调 update),
/// 不同步则 `state.latest_events` 为空,任何内容级断言都无从谈起。
#[test]
fn parliament_1000_events_renders_without_panic() {
    let mut app = app_with_1000_votes();
    app.update();
    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("Parliament"));
    assert!(
        content.contains("prop-999"),
        "事件倒序展示,顶部窗口应含最新 prop-999, got: {content}"
    );
    assert!(
        content.contains("> [ParliamentVoteCast] prop-999"),
        "初始选中第 0 条应带 > 前缀, got: {content}"
    );
}

/// 1000 条事件下 Down 深滚(30 次)列表区不空白,且选中行带 > 前缀可见
///
/// F-09 回归门(双层):
/// - 双重滚动缺陷:scroll_offset(≈27)≥ Text 行数 → 整版空白,事件行全消失;
/// - 上缓冲无补偿残留:窗口含 +VIRTUAL_SCROLL_BUFFER(5) 上缓冲而渲染无
///   .scroll 补偿时,稳态下滚(adjust_scroll 钉 Δ=content_height-1)选中行
///   落在可视带之外 → 列表有内容但 "> " 前缀持续不可见,Enter 会作用于
///   不可见条目。故两层断言缺一不可。
///
/// WHY 用 Down 而非 j:Parliament 面板仅处理 Up/Down
/// (list_state::handle_key_navigation),j 转发到面板后不触发任何滚动。
#[test]
fn parliament_1000_events_down_scroll_keeps_list_rendered() {
    let mut app = app_with_1000_votes();
    app.update();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for _ in 0..30 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("[ParliamentVoteCast]"),
        "深滚 30 行后列表区仍应渲染事件行(双重滚动缺陷下整版空白), got: {content}"
    );
    assert!(
        content.contains("> [ParliamentVoteCast]"),
        "深滚 30 行后选中事件应带 > 前缀且可见(上缓冲无补偿缺陷下选中行被挤出可视带), got: {content}"
    );
}

/// 1000 条事件下 G 跳底:窗口末端回填贴底填满,末条 prop-0 可见且带选中前缀
///
/// F-09 主回归门:双重滚动下 scroll_to_bottom 置 scroll_offset=999,
/// `Paragraph::scroll((999, 0))` ≥ Text 行数(约 10 行)→ 零渲染 → prop-0 不可见。
/// WHY NONE 而非 SHIFT:global_keys_test 以 Char('G')(NONE)实证 ScrollBottom
/// 路由;crossterm 大写字符不依赖 SHIFT 修饰位。
#[test]
fn parliament_1000_events_g_jump_bottom() {
    let mut app = app_with_1000_votes();
    app.update();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    // 顶部帧的事件行数 = "满屏密度"基准:初始窗口 [0, content_height) 填满列表区
    let top = render_to_string(&mut app, 80, 24);
    let top_event_rows = top.matches("[ParliamentVoteCast]").count();
    assert!(
        top_event_rows >= 2,
        "前置:80x24 顶部窗口应渲染多条事件(密度基准), got: {top}"
    );
    app.handle_key_event(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("prop-0"),
        "G 跳底后应看到最早事件 prop-0, got: {content}"
    );
    assert!(
        content.contains("> [ParliamentVoteCast] prop-0"),
        "跳底后选中项应为 prop-0(带 > 前缀), got: {content}"
    );
    // 窗口末端回填回归门(批次一评审遗留):修复前跳底窗口 [999,1000) 仅 1 条
    // 事件,列表区大面积空白易误读为"列表断了";回填后应贴底填满、与顶部帧
    // 同密度(多条 prop-* 同帧可见)。密度计数不硬编码 prop 编号,对任意
    // content_height ≥ 2 的布局均成立(80x24 下窗口 6 条,prop-9 不在窗内)。
    let bottom_event_rows = content.matches("[ParliamentVoteCast]").count();
    assert!(
        bottom_event_rows >= 2,
        "G 跳底回填后应渲染多条事件而非单条(列表区不再大面积空白), got: {content}"
    );
    assert_eq!(
        bottom_event_rows, top_event_rows,
        "G 跳底回填后窗口应贴底填满,与顶部帧同密度, got top={top_event_rows} bottom={bottom_event_rows}"
    );
}

/// 1000 条事件下深滚后 gg 跳顶:回到最新事件且选中前缀可见
///
/// WHY 两次 g:单 g 仅进入 GPrefix 前缀态(router EnterMode),第二次 g 才
/// 触发 ScrollTop;旧用例 j×10 + 单 g 的"跳顶"从未发生,断言恒真(F-14)。
#[test]
fn parliament_1000_events_gg_jump_top() {
    let mut app = app_with_1000_votes();
    app.update();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for _ in 0..10 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("> [ParliamentVoteCast] prop-999"),
        "gg 跳顶后应回到最新事件 prop-999 且选中, got: {content}"
    );
}
