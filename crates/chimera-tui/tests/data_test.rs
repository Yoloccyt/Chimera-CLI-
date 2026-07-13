//! DataPipeline 集成测试 — Task P1.3
//!
//! 验证 `DataPipeline` 能把多源 NexusEvent 对齐为单一 `DataSnapshot`,
//! 支持同一 tick 内状态事件去重，并保留完整事件日志流。

use chimera_tui::{BudgetMetrics, DataPipeline, DataSourceConfig, EventSubscriber};
use event_bus::{BudgetMetricsPayload, EventBus, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use std::time::{Duration, Instant};

/// 构造测试用 Quest
fn quest(id: &str, title: &str) -> Quest {
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
    }
}

/// 构造 QuestListUpdated 事件
fn quest_list_event(quests: Vec<Quest>, source: &str) -> NexusEvent {
    NexusEvent::QuestListUpdated {
        metadata: EventMetadata::new(source),
        quests,
        source: source.into(),
    }
}

/// 构造 BudgetMetricsUpdated 事件
fn budget_metrics_event(metrics: BudgetMetrics, source: &str) -> NexusEvent {
    NexusEvent::BudgetMetricsUpdated {
        metadata: EventMetadata::new(source),
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

/// 构造 SkepticVeto 事件（Parliament 相关）
fn skeptic_veto_event(quest_id: &str) -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.into(),
        veto_reason: "unsafe shell injection detected".into(),
        frozen_capabilities: vec!["shell.exec".into()],
    }
}

/// 默认测试配置，tick 间隔 50ms 便于快速验证
fn test_config() -> DataSourceConfig {
    DataSourceConfig {
        max_event_history: 256,
        max_quest_list_size: 64,
        budget_metrics_ttl_ms: 5000,
        tick_interval_ms: 50,
        max_history_len: 64,
        max_security_summaries: 10,
        max_frozen_capabilities: 20,
    }
}

#[tokio::test]
async fn pipeline_aligns_multi_source_events_into_single_snapshot() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    let q = quest("q1", "对齐测试");
    bus.publish(quest_list_event(vec![q.clone()], "quest-engine"))
        .await
        .unwrap();
    bus.publish(budget_metrics_event(
        BudgetMetrics {
            total_consumption: 7500.0,
            remaining_budget: 2500.0,
            utilization_rate: 0.75,
            current_tier: "Medium".into(),
            coefficient: 0.9,
            is_exceeded: false,
            alert: None,
        },
        "efficiency-monitor",
    ))
    .await
    .unwrap();
    bus.publish(skeptic_veto_event("q1")).await.unwrap();

    // 等待一次 tick，让 DataPipeline 处理事件
    tokio::time::sleep(Duration::from_millis(80)).await;

    let snapshot = pipeline.snapshot();
    assert_eq!(snapshot.quest_list, vec![q]);
    assert!((snapshot.budget_metrics.utilization_rate - 0.75).abs() < f32::EPSILON);
    assert_eq!(snapshot.latest_events.len(), 3);
}

#[tokio::test]
async fn pipeline_deduplicates_repeated_state_events() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    // 同一 tick 窗口内发布多个 QuestListUpdated / BudgetMetricsUpdated
    let q1 = quest("q1", "first");
    let q2 = quest("q2", "second");
    let q3 = quest("q3", "third");
    bus.publish(quest_list_event(vec![q1.clone()], "quest-engine"))
        .await
        .unwrap();
    bus.publish(budget_metrics_event(
        BudgetMetrics {
            total_consumption: 1000.0,
            remaining_budget: 9000.0,
            utilization_rate: 0.1,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        },
        "efficiency-monitor",
    ))
    .await
    .unwrap();
    bus.publish(quest_list_event(
        vec![q1.clone(), q2.clone()],
        "quest-engine",
    ))
    .await
    .unwrap();
    bus.publish(budget_metrics_event(
        BudgetMetrics {
            total_consumption: 2000.0,
            remaining_budget: 8000.0,
            utilization_rate: 0.2,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        },
        "efficiency-monitor",
    ))
    .await
    .unwrap();
    bus.publish(quest_list_event(
        vec![q1.clone(), q2.clone(), q3.clone()],
        "quest-engine",
    ))
    .await
    .unwrap();

    // 等待一次 tick 完成批量处理
    tokio::time::sleep(Duration::from_millis(80)).await;

    let snapshot = pipeline.snapshot();
    // 去重后 quest_list 应为最后一个 QuestListUpdated 的内容 [q1, q2, q3]
    assert_eq!(snapshot.quest_list, vec![q1, q2.clone(), q3.clone()]);
    // budget_metrics 应为最后一个 BudgetMetricsUpdated 的内容
    assert!((snapshot.budget_metrics.utilization_rate - 0.2).abs() < f32::EPSILON);
    assert_eq!(snapshot.budget_metrics.total_consumption, 2000.0);
    // 日志流保留所有 5 个事件，不去重
    assert_eq!(snapshot.latest_events.len(), 5);
}

#[tokio::test]
#[ignore = "性能测试：请在 release 模式运行，验证 1000 事件/秒处理延迟"]
async fn pipeline_handles_1000_events_per_second() {
    // 使用 250ms tick（生产默认值），在 tick 窗口内突发 1000 个事件。
    let bus = EventBus::with_capacity(4096);
    let subscriber = EventSubscriber::new(bus.clone());
    let config = DataSourceConfig {
        tick_interval_ms: 250,
        ..test_config()
    };
    let pipeline = DataPipeline::new(subscriber, config);

    // 快速发布 1000 个 BudgetMetricsUpdated 事件
    let publish_start = Instant::now();
    for i in 0..1000 {
        bus.publish(budget_metrics_event(
            BudgetMetrics {
                total_consumption: i as f64 * 10.0,
                remaining_budget: 10000.0 - i as f64 * 10.0,
                utilization_rate: (i as f32 / 1000.0).clamp(0.0, 1.0),
                current_tier: "High".into(),
                coefficient: 1.0,
                is_exceeded: false,
                alert: None,
            },
            "efficiency-monitor",
        ))
        .await
        .unwrap();
    }
    let publish_done = Instant::now();

    // 轮询直到 snapshot 包含全部 1000 个事件，最多等待 1 秒
    let deadline = publish_done + Duration::from_secs(1);
    let mut snapshot_ready = publish_done;
    while Instant::now() < deadline {
        let snap = pipeline.snapshot();
        if snap.latest_events.len() == 1000 {
            snapshot_ready = Instant::now();
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let snapshot = pipeline.snapshot();
    assert_eq!(snapshot.latest_events.len(), 1000);

    // 端到端延迟包含一次 tick 等待（最大 250ms）+ 处理时间。
    // 目标：处理延迟 P95 < 100ms，因此端到端应 < 350ms，留足余量断言 < 400ms。
    let elapsed = snapshot_ready.duration_since(publish_start);
    assert!(
        elapsed < Duration::from_millis(400),
        "1000 events processing took {:?}, expected < 400ms",
        elapsed
    );
}
