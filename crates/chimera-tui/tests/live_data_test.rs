//! TUI 实时数据集成测试 — Task M0
//!
//! 验证 `TuiApp` 通过 `DataPipeline` 消费 EventBus 事件，
//! 并将 Quest / Budget 数据渲染到面板上。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use chimera_tui::{
    BudgetMetrics, DataPipeline, DataSourceConfig, EventSubscriber, PanelId, TuiApp, TuiConfig,
};
use event_bus::{BudgetMetricsPayload, EventBus, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 构造测试用 Quest
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

/// 构造 QuestListUpdated 事件
fn quest_list_event(quests: Vec<Quest>) -> NexusEvent {
    NexusEvent::QuestListUpdated {
        metadata: EventMetadata::new("quest-engine"),
        quests,
        source: "quest-engine".into(),
    }
}

/// 构造 BudgetMetricsUpdated 事件
fn budget_metrics_event(metrics: BudgetMetrics) -> NexusEvent {
    NexusEvent::BudgetMetricsUpdated {
        metadata: EventMetadata::new("efficiency-monitor"),
        metrics: BudgetMetricsPayload {
            total_consumption: metrics.total_consumption,
            remaining_budget: metrics.remaining_budget,
            utilization_rate: metrics.utilization_rate,
            current_tier: metrics.current_tier,
            coefficient: metrics.coefficient,
            is_exceeded: metrics.is_exceeded,
            alert: metrics.alert,
        },
    }
}

/// 将 TestBackend 渲染内容转为字符串
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[tokio::test]
async fn tui_renders_live_event_bus_data() {
    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = std::sync::Arc::new(DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 50,
            ..Default::default()
        },
    ));

    // 发布 Quest 列表与 Budget 指标事件。
    let quest = sample_quest("live-q1", "Live Event Quest");
    bus.publish(quest_list_event(vec![quest.clone()]))
        .await
        .unwrap();
    bus.publish(budget_metrics_event(BudgetMetrics {
        total_consumption: 4200.0,
        remaining_budget: 5800.0,
        utilization_rate: 0.42,
        current_tier: "Medium".into(),
        coefficient: 0.9,
        is_exceeded: false,
        alert: None,
    }))
    .await
    .unwrap();

    // 轮询等待后台任务完成一次 tick 聚合,替代固定 sleep,避免 CI 抖动导致 flaky。
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let snap = pipeline.snapshot();
        let quest_ready = snap
            .quest_list
            .first()
            .map(|q| q.title == "Live Event Quest")
            .unwrap_or(false);
        let budget_ready = (snap.budget_metrics.total_consumption - 4200.0).abs() < f64::EPSILON;
        if quest_ready && budget_ready {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for pipeline snapshot; quest_ready={quest_ready}, budget_ready={budget_ready}"
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 使用实时数据管道创建 TUI 应用。
    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(std::sync::Arc::clone(&pipeline)),
    )
    .unwrap();

    // 拉取最新快照到状态。
    app.update();

    // 默认渲染 Quest 面板，应包含发布的 quest 标题。
    let mut content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Live Event Quest"),
        "Quest panel should render live quest title, got: {}",
        &content[..content.len().min(300)]
    );

    // 切换到 Budget 面板，应包含发布的预算指标。
    app.switch_panel_to(PanelId::Budget);
    content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Medium") && content.contains("4200.0"),
        "Budget panel should render live budget metrics, got: {}",
        &content[..content.len().min(300)]
    );

    // 事件也应出现在 Log 面板。
    app.switch_panel_to(PanelId::Log);
    content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("QuestListUpdated") && content.contains("BudgetMetricsUpdated"),
        "Log panel should render live events, got: {}",
        &content[..content.len().min(300)]
    );

    pipeline.shutdown().await;
}
