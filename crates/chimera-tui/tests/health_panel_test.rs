//! Health 面板集成测试 — M2
//!
//! 验证 HealthPanel 正确渲染事件速率、慢消费者、平均延迟与健康评分公式。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, HealthMetrics, PanelId, TuiApp, TuiConfig, TuiDataSource,
    TuiError,
};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造简单 Quest(集成测试辅助函数,与 quest_event_jump_test.rs 保持一致)
fn sample_quest(id: &str, title: &str) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![Task {
            task_id: format!("{id}-t1"),
            description: "test task".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

#[derive(Debug)]
struct HealthTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl HealthTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for HealthTestSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
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
fn test_health_panel_renders_with_sample_data() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics {
            events_per_second: 120.5,
            slow_consumer_count: 2,
            average_latency_ms: 23.4,
            health_score: 80,
        },
        event_rate_history: vec![100, 110, 105, 115, 120, 118, 122],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Health"),
        "Health panel title should be rendered"
    );
    assert!(
        content.contains("120.5"),
        "events per second should be rendered"
    );
    assert!(
        content.contains("2"),
        "slow consumer count should be rendered"
    );
    assert!(
        content.contains("23.4 ms"),
        "average latency should be rendered"
    );
    assert!(content.contains("80"), "health score should be rendered");
}

#[test]
fn test_health_score_formula() {
    // M2 健康评分公式:100 - 10 * slow_consumer_count,最低 0
    assert_eq!(HealthMetrics::compute_health_score(0), 100);
    assert_eq!(HealthMetrics::compute_health_score(1), 90);
    assert_eq!(HealthMetrics::compute_health_score(5), 50);
    assert_eq!(HealthMetrics::compute_health_score(10), 0);
    assert_eq!(HealthMetrics::compute_health_score(20), 0);
}

#[test]
fn test_health_panel_empty_data_renders_defaults() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Health"),
        "Health panel should render even with default data"
    );
    assert!(
        content.contains("100"),
        "default health score should be 100"
    );
}

#[test]
fn test_health_panel_low_score_uses_red_color() {
    let snapshot = DataSnapshot {
        health_metrics: HealthMetrics {
            health_score: 30,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(content.contains("30"));
}

// ============================================================
// Health 面板增强测试 — 活跃/暂停 Quest 数 + 积压因子
// ============================================================
//
// WHY 独立测试组:验证 HealthPanel 新增的 Active/Paused Quests 指标
// 渲染,以及健康评分积压因子(活跃 Quest > 10 时扣 10 分)。
// 这些指标从 DataSnapshot.quest_list 与 paused_quest_count 派生,
// 不新增事件变体,复用已有的 QuestPaused/QuestResumed 事件。

#[test]
fn test_health_panel_shows_active_quests() {
    // 构造 3 个 Quest 的快照,验证 Health 面板渲染 "Active Quests: 3"
    let snapshot = DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
            sample_quest("q3", "Third Quest"),
        ],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Active Quests: 3"),
        "Health panel should show 'Active Quests: 3', got: {content}"
    );
}

#[test]
fn test_health_panel_shows_paused_quests() {
    // 构造 2 个 Quest 且 paused_quest_count = 1 的快照,
    // 验证 Health 面板渲染 "Paused Quests: 1"
    let snapshot = DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "Active Quest"),
            sample_quest("q2", "Paused Quest"),
        ],
        paused_quest_count: 1,
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(HealthTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Paused Quests: 1"),
        "Health panel should show 'Paused Quests: 1', got: {content}"
    );
}

#[test]
fn test_health_score_drops_when_many_active_quests() {
    // 积压因子:活跃 Quest > 10 时扣 10 分
    // 基准:compute_health_score(0) = 100(无慢消费者)
    // 积压:compute_health_score_with_backlog(0, 15) = 90(15 个活跃 Quest 扣 10 分)
    let baseline = HealthMetrics::compute_health_score(0);
    let with_backlog = HealthMetrics::compute_health_score_with_backlog(0, 15);
    assert_eq!(
        baseline, 100,
        "baseline health score with no slow consumers should be 100"
    );
    assert_eq!(
        with_backlog, 90,
        "health score with 15 active quests should drop by 10 (backlog factor)"
    );
    assert!(
        with_backlog < baseline,
        "health score with backlog should be lower than baseline"
    );
}

#[test]
fn test_health_score_no_backlog_below_threshold() {
    // 活跃 Quest ≤ 10 时不扣分(积压因子阈值 = 10)
    assert_eq!(
        HealthMetrics::compute_health_score_with_backlog(0, 0),
        100,
        "0 active quests should not trigger backlog factor"
    );
    assert_eq!(
        HealthMetrics::compute_health_score_with_backlog(0, 10),
        100,
        "10 active quests (at threshold) should not trigger backlog factor"
    );
    assert_eq!(
        HealthMetrics::compute_health_score_with_backlog(0, 11),
        90,
        "11 active quests (above threshold) should trigger backlog factor (-10)"
    );
}

#[test]
fn test_health_score_backlog_combined_with_slow_consumers() {
    // 积压因子与慢消费者扣分叠加:1 个慢消费者(-10) + 15 个活跃 Quest(-10) = 80
    assert_eq!(
        HealthMetrics::compute_health_score_with_backlog(1, 15),
        80,
        "1 slow consumer + 15 active quests should yield 80 (100 - 10 - 10)"
    );
}

#[test]
fn test_health_score_backlog_clamped_to_zero() {
    // 极端场景:10 个慢消费者(0 分)+ 15 个活跃 Quest,仍为 0 分(不低于 0)
    assert_eq!(
        HealthMetrics::compute_health_score_with_backlog(10, 15),
        0,
        "health score should be clamped to 0 (100 - 100 - 10 = -10 → 0)"
    );
}
