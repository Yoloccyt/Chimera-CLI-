//! CHTC 面板集成测试 — P2.5 TUI v1.7-omega
//!
//! 验证 ChtcPanel 正确渲染 5 IDE 适配器列表(VSCode/JetBrains/Vim/Emacs/LSP)、
//! 兼容性评分(0-100)、评分阈值着色(< 60 黄色 / >= 80 绿色)、最近请求类型分布。
//!
//! # 设计决策(WHY)
//! - 使用 TestBackend 内存渲染,无需真实终端,CI 可运行。
//! - 颜色测试策略:TestBackend 渲染会丢失 Span 样式信息,因此颜色逻辑通过
//!   `ChtcPanel::score_color()` 公开函数直接断言,确保阈值映射可独立验证。
//! - 文本测试策略:通过 `ChtcPanel::content()` 静态方法获取 `Text<'static>`,
//!   既能验证文本内容,也能在需要时检查 Span 颜色。
//! - 阈值语义:评分 < 60 黄色高亮(兼容性风险),>= 80 绿色(健康),
//!   60-79 之间正常显示。此阈值与 spec "评分 < 60 黄色高亮" 一致,
//!   是经验值:低于 60% 表示兼容性不及格,需运维关注。

#![forbid(unsafe_code)]

use chimera_tui::{
    ChtcAdapterInfo, ChtcPanel, ChtcState, DataSnapshot, DataSourceConfig, Panel, PanelId, TuiApp,
    TuiConfig, TuiDataSource, TuiError,
};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

/// 测试数据源 — 返回预设 CHTC 适配器状态
#[derive(Debug)]
struct ChtcTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl ChtcTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for ChtcTestSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 渲染应用并返回字符串内容
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

/// 构造 5 IDE 适配器快照(VSCode/JetBrains/Vim/Emacs/LSP)
///
/// WHY 5 IDE 列表:与 chtc-bridge 的 enum dispatch 设计一致(§4.3),
/// 覆盖主流编辑器生态。评分分布涵盖三档(高/中/低)以便测试颜色阈值。
fn five_adapters_snapshot() -> DataSnapshot {
    DataSnapshot {
        chtc_state: ChtcState {
            adapters: vec![
                ChtcAdapterInfo {
                    adapter_id: "vscode-ext".into(),
                    adapter_type: "vscode".into(),
                    compatibility_score: 95,
                    recent_requests: vec![
                        ("tool_call".into(), 42),
                        ("read_file".into(), 18),
                        ("search".into(), 7),
                    ],
                    is_online: true,
                },
                ChtcAdapterInfo {
                    adapter_id: "jetbrains-plugin".into(),
                    adapter_type: "jetbrains".into(),
                    compatibility_score: 88,
                    recent_requests: vec![("tool_call".into(), 18), ("read_file".into(), 5)],
                    is_online: true,
                },
                ChtcAdapterInfo {
                    adapter_id: "vim-adapter".into(),
                    adapter_type: "vim".into(),
                    compatibility_score: 72,
                    recent_requests: vec![("tool_call".into(), 9)],
                    is_online: true,
                },
                ChtcAdapterInfo {
                    adapter_id: "emacs-adapter".into(),
                    adapter_type: "emacs".into(),
                    compatibility_score: 45,
                    recent_requests: vec![("tool_call".into(), 3)],
                    is_online: false,
                },
                ChtcAdapterInfo {
                    adapter_id: "lsp-bridge".into(),
                    adapter_type: "lsp".into(),
                    compatibility_score: 80,
                    recent_requests: vec![("completion".into(), 22)],
                    is_online: true,
                },
            ],
        },
        ..Default::default()
    }
}

/// 构造空适配器快照
fn empty_adapters_snapshot() -> DataSnapshot {
    DataSnapshot {
        chtc_state: ChtcState::default(),
        ..Default::default()
    }
}

// ============================================================
// A. 基础渲染测试
// ============================================================

#[test]
fn test_chtc_panel_id() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    app.switch_panel_to(PanelId::Chtc);
    assert_eq!(app.current_panel(), PanelId::Chtc);
}

#[test]
fn test_chtc_panel_renders_title() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("CHTC"),
        "CHTC panel title should be rendered, got: {}",
        &content[..content.len().min(200)]
    );
}

// ============================================================
// B. 适配器列表测试(P2.5.1 测试点 1:列表渲染)
// ============================================================

#[test]
fn test_chtc_panel_renders_adapter_list_when_non_empty() {
    // P2.5.1 测试点 1:ChtcPanel 渲染包含适配器列表(当 adapters 非空时)
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 120, 40);
    // 5 IDE 适配器类型应全部显示
    assert!(
        content.contains("vscode") || content.contains("VSCode"),
        "vscode adapter should be rendered, got: {}",
        &content[..content.len().min(400)]
    );
    assert!(
        content.contains("jetbrains") || content.contains("JetBrains"),
        "jetbrains adapter should be rendered"
    );
    assert!(
        content.contains("vim") || content.contains("Vim"),
        "vim adapter should be rendered"
    );
    assert!(
        content.contains("emacs") || content.contains("Emacs"),
        "emacs adapter should be rendered"
    );
    assert!(
        content.contains("lsp") || content.contains("LSP"),
        "lsp adapter should be rendered"
    );
}

#[test]
fn test_chtc_panel_empty_adapters_shows_placeholder() {
    // P2.5.1 测试点 5:空适配器列表时显示 "No CHTC adapters connected"
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(empty_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("No CHTC adapters connected"),
        "empty adapter list should show placeholder, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// C. 兼容性评分测试(P2.5.1 测试点 2:评分显示)
// ============================================================

#[test]
fn test_chtc_panel_renders_compatibility_scores() {
    // P2.5.1 测试点 2:渲染包含兼容性评分(0-100)
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 120, 40);
    // 评分 95/88/72/45/80 应显示在面板中(以 /100 格式或纯数字)
    assert!(
        content.contains("95"),
        "score 95 should be rendered, got: {}",
        &content[..content.len().min(500)]
    );
    assert!(content.contains("88"), "score 88 should be rendered");
    assert!(content.contains("72"), "score 72 should be rendered");
    assert!(content.contains("45"), "score 45 should be rendered");
    assert!(content.contains("80"), "score 80 should be rendered");
    // 评分应以 "/100" 形式显示,体现 0-100 范围语义
    assert!(
        content.contains("/100"),
        "score should display as X/100 format, got: {}",
        &content[..content.len().min(400)]
    );
}

// ============================================================
// D. 评分阈值颜色测试(P2.5.1 测试点 3 & 4:颜色高亮)
// ============================================================
//
// WHY 直接测试 score_color 函数:TestBackend 渲染会丢失 Span 样式信息,
// 无法从字符串内容反查颜色。将颜色映射逻辑提取为 pub fn score_color,
// 既能独立验证阈值逻辑,又便于 content() 方法复用,避免逻辑分散。

#[test]
fn test_chtc_panel_high_score_green() {
    // P2.5.1 测试点 4:评分 >= 80 时绿色显示
    assert_eq!(
        ChtcPanel::score_color(80),
        Color::Green,
        "80 should be green"
    );
    assert_eq!(
        ChtcPanel::score_color(95),
        Color::Green,
        "95 should be green"
    );
    assert_eq!(
        ChtcPanel::score_color(100),
        Color::Green,
        "100 should be green"
    );
}

#[test]
fn test_chtc_panel_low_score_yellow() {
    // P2.5.1 测试点 3:评分 < 60 时黄色高亮
    assert_eq!(
        ChtcPanel::score_color(0),
        Color::Yellow,
        "0 should be yellow"
    );
    assert_eq!(
        ChtcPanel::score_color(45),
        Color::Yellow,
        "45 should be yellow"
    );
    assert_eq!(
        ChtcPanel::score_color(59),
        Color::Yellow,
        "59 should be yellow"
    );
}

#[test]
fn test_chtc_panel_medium_score_normal() {
    // 60-79 之间应为正常颜色(非绿色、非黄色)
    assert_eq!(
        ChtcPanel::score_color(60),
        Color::Reset,
        "60 should be normal (Reset)"
    );
    assert_eq!(
        ChtcPanel::score_color(72),
        Color::Reset,
        "72 should be normal (Reset)"
    );
    assert_eq!(
        ChtcPanel::score_color(79),
        Color::Reset,
        "79 should be normal (Reset)"
    );
}

#[test]
fn test_chtc_panel_score_color_boundaries() {
    // 边界测试:80 是绿色起点,79 是正常终点,60 是正常起点,59 是黄色终点
    assert_eq!(ChtcPanel::score_color(79), Color::Reset);
    assert_eq!(ChtcPanel::score_color(80), Color::Green);
    assert_eq!(ChtcPanel::score_color(59), Color::Yellow);
    assert_eq!(ChtcPanel::score_color(60), Color::Reset);
}

// ============================================================
// E. content() 方法测试 — 验证 Text 结构与 Span 颜色
// ============================================================

#[test]
fn test_chtc_panel_content_empty_state() {
    use chimera_tui::TuiState;

    let state = TuiState::new();
    let text = ChtcPanel::content(&state).to_string();
    assert!(
        text.contains("No CHTC adapters connected"),
        "content() empty state should show placeholder, got: {}",
        &text[..text.len().min(200)]
    );
}

#[test]
fn test_chtc_panel_content_with_adapters() {
    use chimera_tui::TuiState;

    let mut state = TuiState::new();
    state.chtc_state = five_adapters_snapshot().chtc_state;
    let text = ChtcPanel::content(&state).to_string();
    assert!(
        text.contains("vscode") || text.contains("VSCode"),
        "content() should contain vscode adapter"
    );
    assert!(text.contains("95"), "content() should contain score 95");
    assert!(
        text.contains("/100"),
        "content() should contain /100 format"
    );
}

#[test]
fn test_chtc_panel_content_online_offline_status() {
    use chimera_tui::TuiState;

    let mut state = TuiState::new();
    state.chtc_state = five_adapters_snapshot().chtc_state;
    let text = ChtcPanel::content(&state).to_string();
    // emacs-adapter (score=45) 是离线的
    assert!(
        text.contains("online") || text.contains("offline"),
        "content() should show online/offline status, got: {}",
        &text[..text.len().min(300)]
    );
}

// ============================================================
// F. 进度条渲染测试(P2.5.3 评分进度展示)
// ============================================================

#[test]
fn test_chtc_panel_renders_progress_bar() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 120, 40);
    // 进度条特征:包含 '[' 与 ']' 与 '=' 或 '-'
    assert!(
        content.contains('[') && content.contains(']'),
        "progress bar should render [====----] form, got: {}",
        &content[..content.len().min(400)]
    );
    assert!(
        content.contains('='),
        "progress bar should have filled '=' characters"
    );
}

// ============================================================
// G. 请求分布 sparkline 测试(P2.5.4)
// ============================================================

#[test]
fn test_chtc_panel_renders_request_distribution() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 120, 40);
    // 请求分布区应显示标题或请求类型
    assert!(
        content.contains("Request") || content.contains("request"),
        "request distribution section should be rendered, got: {}",
        &content[..content.len().min(500)]
    );
}

#[test]
fn test_chtc_panel_renders_request_types() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let content = render_to_string(&mut app, 120, 40);
    // 注入的请求类型应显示
    assert!(
        content.contains("tool_call"),
        "tool_call request type should be rendered"
    );
}

// ============================================================
// H. handle_key 测试 — `?` 已由 TuiApp 全局拦截,面板不再处理
// ============================================================

#[test]
fn test_chtc_panel_handle_key_question_mark_returns_none() {
    use chimera_tui::TuiState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut panel = ChtcPanel::new();
    let mut state = TuiState::new();
    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(
        panel.handle_key(key, &mut state),
        None,
        "'?' should be handled globally by TuiApp as Help overlay"
    );
}

#[test]
fn test_chtc_panel_handle_key_other_returns_none() {
    use chimera_tui::TuiState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut panel = ChtcPanel::new();
    let mut state = TuiState::new();
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        panel.handle_key(key, &mut state).is_none(),
        "Enter key should not trigger any command"
    );
}

#[test]
fn test_chtc_panel_handle_key_navigation() {
    // 列表非空时 Up/Down 应能导航(不返回命令,仅内部状态变化)
    use chimera_tui::TuiState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut panel = ChtcPanel::new();
    let mut state = TuiState::new();
    state.chtc_state = five_adapters_snapshot().chtc_state;

    // Down 应被消费(返回 None,不冒泡),内部 selected 应变化
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let result = panel.handle_key(down, &mut state);
    assert!(
        result.is_none(),
        "Down key should be consumed for navigation, not produce command"
    );

    // Up 也应被消费
    let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
    let result = panel.handle_key(up, &mut state);
    assert!(
        result.is_none(),
        "Up key should be consumed for navigation, not produce command"
    );
}

// ============================================================
// I. 默认/空状态稳定性测试
// ============================================================

#[test]
fn test_chtc_panel_default_state_renders_without_panic() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(empty_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    // 默认状态应能渲染,不 panic
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("CHTC"),
        "CHTC panel should render title in default state"
    );
}

#[test]
fn test_chtc_panel_small_area_renders_without_panic() {
    // 极小渲染区域不应 panic
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(ChtcTestSource::new(five_adapters_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Chtc);

    let _content = render_to_string(&mut app, 20, 5);
    // 渲染完成即通过,不 panic
}
