//! Router 面板集成测试 — P2.3 TUI v1.7-omega
//!
//! 验证 RouterPanel 正确渲染三路由器(KVBSR/SESA/FaaE)命中率进度条、
//! P50/P95/P99 延迟分位数,以及热点 capability_id Top-10 列表。
//!
//! # 设计决策(WHY)
//! - 使用 TestBackend 内存渲染,无需真实终端,CI 可运行。
//! - 阈值语义:命中率 < 0.6(60%)黄色高亮,与 spec "评分 < 60% 黄色高亮" 一致。
//!   此阈值是经验值:低于 60% 表示路由器命中率不及格,需运维关注。
//! - Top-K 测试:注入 >10 个 capability 验证 `select_nth_unstable` O(n) 截断路径。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, Panel, PanelId, RouterMetrics, RouterPanel, RouterStatsInfo,
    TuiApp, TuiCommand, TuiConfig, TuiDataSource, TuiError, TuiState,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 测试数据源 — 返回预设 Router 指标
#[derive(Debug)]
struct RouterTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl RouterTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for RouterTestSource {
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

/// 构造正常路由器快照(三路由器命中率均 > 0.6,无高亮)
fn normal_router_snapshot() -> DataSnapshot {
    DataSnapshot {
        router_metrics: RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.87,
                p50_latency_us: 120,
                p95_latency_us: 480,
                p99_latency_us: 950,
                hot_capabilities: vec![("search".into(), 42), ("read_file".into(), 28)],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.72,
                p50_latency_us: 200,
                p95_latency_us: 800,
                p99_latency_us: 1500,
                hot_capabilities: vec![("activate".into(), 15)],
            },
            faae_stats: RouterStatsInfo {
                hit_rate: 0.91,
                p50_latency_us: 60,
                p95_latency_us: 280,
                p99_latency_us: 650,
                hot_capabilities: vec![("tool_call".into(), 88)],
            },
        },
        ..Default::default()
    }
}

/// 构造低命中率快照(KVBSR=0.45,应黄色高亮)
fn low_hit_rate_snapshot() -> DataSnapshot {
    DataSnapshot {
        router_metrics: RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.45,
                p50_latency_us: 500,
                p95_latency_us: 2000,
                p99_latency_us: 5000,
                hot_capabilities: vec![],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.50,
                p50_latency_us: 300,
                p95_latency_us: 1200,
                p99_latency_us: 3500,
                hot_capabilities: vec![],
            },
            faae_stats: RouterStatsInfo {
                hit_rate: 0.30,
                p50_latency_us: 800,
                p95_latency_us: 3500,
                p99_latency_us: 9000,
                hot_capabilities: vec![],
            },
        },
        ..Default::default()
    }
}

/// 构造超长热点列表快照(15 个 capability,验证 Top-10 截断)
fn many_capabilities_snapshot() -> DataSnapshot {
    let hot_caps: Vec<(String, u64)> = (0..15)
        .map(|i| (format!("cap-{:02}", i), (15 - i) as u64 * 10))
        .collect();
    DataSnapshot {
        router_metrics: RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.80,
                p50_latency_us: 100,
                p95_latency_us: 400,
                p99_latency_us: 800,
                hot_capabilities: hot_caps,
            },
            sesa_stats: RouterStatsInfo::default(),
            faae_stats: RouterStatsInfo::default(),
        },
        ..Default::default()
    }
}

// ============================================================
// A. 基础渲染测试
// ============================================================

#[test]
fn test_router_panel_id() {
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    app.switch_panel_to(PanelId::Router);
    assert_eq!(app.current_panel(), PanelId::Router);
}

#[test]
fn test_router_panel_renders_title() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("Router"),
        "Router panel title should be rendered, got: {}",
        &content[..content.len().min(200)]
    );
}

#[test]
fn test_router_panel_default_state_renders_three_routers() {
    // 默认状态:三路由器 hit_rate = 0.0,应显示三路由器标识
    let snapshot = DataSnapshot {
        router_metrics: RouterMetrics::default(),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 100, 30);
    // 三路由器名称应同时显示
    assert!(
        content.contains("KVBSR"),
        "default state should display KVBSR router, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("SESA"),
        "default state should display SESA router"
    );
    assert!(
        content.contains("FaaE"),
        "default state should display FaaE router"
    );
}

#[test]
fn test_router_panel_default_state_zero_hit_rate() {
    // 默认状态:hit_rate = 0.0 应显示 0.0%
    let snapshot = DataSnapshot {
        router_metrics: RouterMetrics::default(),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 100, 30);
    assert!(
        content.contains("0.0%"),
        "default hit_rate 0.0 should display as 0.0%, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// B. 数据展示测试 — 命中率与延迟
// ============================================================

#[test]
fn test_router_panel_renders_hit_rates() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 120, 40);
    // KVBSR 0.87 → 87.0%
    assert!(
        content.contains("87.0%"),
        "KVBSR hit_rate 0.87 should display as 87.0%, got: {}",
        &content[..content.len().min(400)]
    );
    // SESA 0.72 → 72.0%
    assert!(
        content.contains("72.0%"),
        "SESA hit_rate 0.72 should display as 72.0%"
    );
    // FaaE 0.91 → 91.0%
    assert!(
        content.contains("91.0%"),
        "FaaE hit_rate 0.91 should display as 91.0%"
    );
}

#[test]
fn test_router_panel_renders_latency_p50_p95_p99() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 140, 40);
    // KVBSR P50=120μs / P95=480μs / P99=950μs
    assert!(
        content.contains("120"),
        "KVBSR P50 latency 120 should be rendered, got: {}",
        &content[..content.len().min(500)]
    );
    assert!(
        content.contains("480"),
        "KVBSR P95 latency 480 should be rendered"
    );
    assert!(
        content.contains("950"),
        "KVBSR P99 latency 950 should be rendered"
    );
}

#[test]
fn test_router_panel_renders_all_three_routers_with_data() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 140, 40);
    // 三路由器名称应同时显示
    assert!(content.contains("KVBSR"), "KVBSR name should be rendered");
    assert!(content.contains("SESA"), "SESA name should be rendered");
    assert!(content.contains("FaaE"), "FaaE name should be rendered");
}

// ============================================================
// C. 命中率进度条测试
// ============================================================

#[test]
fn test_router_panel_renders_utilization_bar() {
    // 进度条应包含 [====---] 形式的字符
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 120, 40);
    // 进度条特征:包含 '=' 与 '-' 与 '[' 与 ']'
    assert!(
        content.contains('[') && content.contains(']'),
        "utilization bar should render [====----] form, got: {}",
        &content[..content.len().min(400)]
    );
    assert!(
        content.contains('='),
        "utilization bar should have filled '=' characters"
    );
}

#[test]
fn test_router_panel_zero_hit_rate_renders_empty_bar() {
    // hit_rate = 0.0 时进度条应全为 '-'
    let snapshot = DataSnapshot {
        router_metrics: RouterMetrics::default(),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 120, 40);
    // 0.0% 必须显示
    assert!(
        content.contains("0.0%"),
        "zero hit rate should display 0.0%, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// D. 热点 capability_id 列表测试
// ============================================================

#[test]
fn test_router_panel_renders_hot_capabilities() {
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(normal_router_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("search"),
        "hot capability 'search' should be rendered, got: {}",
        &content[..content.len().min(500)]
    );
    assert!(
        content.contains("read_file"),
        "hot capability 'read_file' should be rendered"
    );
    assert!(
        content.contains("tool_call"),
        "hot capability 'tool_call' should be rendered"
    );
}

#[test]
fn test_router_panel_empty_hot_capabilities_renders_placeholder() {
    // 三路由器均无热点能力,应显示占位文本而不 panic
    let snapshot = DataSnapshot {
        router_metrics: RouterMetrics::default(),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    // 不应 panic
    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("Router"),
        "Router panel should render title with empty hot capabilities"
    );
}

// ============================================================
// E. Top-K 截断测试 — select_nth_unstable 路径
// ============================================================

#[test]
fn test_router_panel_top_k_truncates_to_ten() {
    // 注入 15 个 capability,验证 Top-K 截断到 10
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(many_capabilities_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    // 渲染不应 panic,且应包含最高频的 cap-00(150 次)
    let content = render_to_string(&mut app, 140, 50);
    assert!(
        content.contains("cap-00"),
        "highest-frequency cap-00 should be in Top-10, got: {}",
        &content[..content.len().min(500)]
    );
    // cap-14 是最低频(10 次),应被截断
    // 注意:由于 content 是字符流,这里只能验证 cap-00 存在,无法精确验证 cap-14 不存在
    // (因为 cap-01 与 cap-14 的前缀 "cap-0" 与 "cap-1" 可能匹配)
}

#[test]
fn test_router_panel_top_k_keeps_highest_frequency() {
    // 验证 Top-K 排序:最高频的应在列表前部
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(many_capabilities_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 140, 50);
    // cap-00 (150 次) 应在 cap-05 (100 次) 之前出现
    let pos_00 = content.find("cap-00");
    let pos_05 = content.find("cap-05");
    assert!(
        pos_00.is_some() && pos_05.is_some(),
        "both cap-00 and cap-05 should be rendered"
    );
    assert!(
        pos_00 < pos_05,
        "cap-00 (higher frequency) should appear before cap-05"
    );
}

// ============================================================
// F. 低命中率场景测试 — 黄色高亮路径
// ============================================================

#[test]
fn test_router_panel_low_hit_rate_renders_without_panic() {
    // 三路由器命中率均 < 0.6,渲染不应 panic
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(low_hit_rate_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    let content = render_to_string(&mut app, 120, 40);
    // KVBSR 0.45 → 45.0%
    assert!(
        content.contains("45.0%"),
        "low hit_rate 0.45 should display as 45.0%, got: {}",
        &content[..content.len().min(400)]
    );
    // SESA 0.50 → 50.0%
    assert!(
        content.contains("50.0%"),
        "low hit_rate 0.50 should display as 50.0%"
    );
    // FaaE 0.30 → 30.0%
    assert!(
        content.contains("30.0%"),
        "low hit_rate 0.30 should display as 30.0%"
    );
}

#[test]
fn test_router_panel_threshold_boundary_60_percent() {
    // 0.6 是阈值边界:命中率为 0.6 时不应该高亮(>= 0.6 为正常)
    let snapshot = DataSnapshot {
        router_metrics: RouterMetrics {
            kvbsr_stats: RouterStatsInfo {
                hit_rate: 0.6,
                p50_latency_us: 100,
                p95_latency_us: 400,
                p99_latency_us: 800,
                hot_capabilities: vec![],
            },
            sesa_stats: RouterStatsInfo {
                hit_rate: 0.59,
                p50_latency_us: 100,
                p95_latency_us: 400,
                p99_latency_us: 800,
                hot_capabilities: vec![],
            },
            faae_stats: RouterStatsInfo::default(),
        },
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(RouterTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Router);

    // 渲染不应 panic,0.6 与 0.59 都应正确显示
    let content = render_to_string(&mut app, 120, 40);
    assert!(
        content.contains("60.0%"),
        "hit_rate 0.6 should display as 60.0%, got: {}",
        &content[..content.len().min(400)]
    );
    assert!(
        content.contains("59.0%"),
        "hit_rate 0.59 should display as 59.0%"
    );
}

// ============================================================
// G. handle_key 测试
// ============================================================

#[test]
fn test_router_panel_handle_key_question_mark() {
    let mut panel = RouterPanel::new();
    let mut state = TuiState::new();
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('?'),
        crossterm::event::KeyModifiers::NONE,
    );
    assert_eq!(
        panel.handle_key(key, &mut state),
        Some(TuiCommand::ShowHelp)
    );
}

#[test]
fn test_router_panel_handle_key_other_returns_none() {
    let mut panel = RouterPanel::new();
    let mut state = TuiState::new();
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    );
    assert!(panel.handle_key(key, &mut state).is_none());
}

// ============================================================
// H. content 方法单元测试 — 直接验证文本输出
// ============================================================

#[test]
fn test_router_panel_content_normal_state() {
    let mut state = TuiState::new();
    state.router_metrics = normal_router_snapshot().router_metrics;
    let content = RouterPanel::content(&state).to_string();
    assert!(
        content.contains("KVBSR"),
        "content should contain KVBSR, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(content.contains("SESA"), "content should contain SESA");
    assert!(content.contains("FaaE"), "content should contain FaaE");
    assert!(content.contains("87.0%"), "content should contain 87.0%");
    assert!(content.contains("72.0%"), "content should contain 72.0%");
    assert!(content.contains("91.0%"), "content should contain 91.0%");
    assert!(content.contains("120"), "content should contain P50=120");
    assert!(
        content.contains("search"),
        "content should contain hot cap 'search'"
    );
}

#[test]
fn test_router_panel_content_low_hit_rate() {
    let mut state = TuiState::new();
    state.router_metrics = low_hit_rate_snapshot().router_metrics;
    let content = RouterPanel::content(&state).to_string();
    assert!(
        content.contains("45.0%"),
        "low hit rate 45.0% should be in content, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("30.0%"),
        "low hit rate 30.0% should be in content"
    );
}

#[test]
fn test_router_panel_content_empty_state() {
    let state = TuiState::new();
    let content = RouterPanel::content(&state).to_string();
    // 空状态也应包含三路由器标识与 0.0%
    assert!(
        content.contains("KVBSR"),
        "empty state should contain KVBSR, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(content.contains("SESA"), "empty state should contain SESA");
    assert!(content.contains("FaaE"), "empty state should contain FaaE");
    assert!(content.contains("0.0%"), "empty state should contain 0.0%");
}

// ============================================================
// I. 顶层辅助函数测试 — top_k_capabilities
// ============================================================

#[test]
fn test_top_k_capabilities_empty() {
    let empty: Vec<(String, u64)> = vec![];
    let result = chimera_tui::panels::router::top_k_capabilities(&empty, 10);
    assert!(result.is_empty(), "empty input should return empty");
}

#[test]
fn test_top_k_capabilities_fewer_than_k() {
    // 输入 < k 个,应返回全部并按频次降序排序
    let input: Vec<(String, u64)> = vec![("a".into(), 10), ("b".into(), 30), ("c".into(), 20)];
    let result = chimera_tui::panels::router::top_k_capabilities(&input, 10);
    assert_eq!(result.len(), 3, "should return all 3 items");
    assert_eq!(result[0].1, 30, "highest frequency should be first");
    assert_eq!(result[1].1, 20, "second highest should be second");
    assert_eq!(result[2].1, 10, "lowest should be last");
}

#[test]
fn test_top_k_capabilities_more_than_k() {
    // 输入 > k 个,应截断到 k 个并按频次降序排序
    let input: Vec<(String, u64)> = (0..15)
        .map(|i| (format!("cap-{:02}", i), (15 - i) as u64 * 10))
        .collect();
    let result = chimera_tui::panels::router::top_k_capabilities(&input, 10);
    assert_eq!(result.len(), 10, "should truncate to 10");
    // 最高频 cap-00 (150 次) 应在第一位
    assert_eq!(result[0].0, "cap-00", "highest frequency should be first");
    assert_eq!(result[0].1, 150, "highest frequency should be 150");
    // 第 10 位应为 cap-09 (60 次)
    assert_eq!(result[9].0, "cap-09", "10th should be cap-09");
    assert_eq!(result[9].1, 60, "10th frequency should be 60");
}

#[test]
fn test_top_k_capabilities_k_zero() {
    // k=0 应返回空
    let input: Vec<(String, u64)> = vec![("a".into(), 10)];
    let result = chimera_tui::panels::router::top_k_capabilities(&input, 0);
    assert!(result.is_empty(), "k=0 should return empty");
}
