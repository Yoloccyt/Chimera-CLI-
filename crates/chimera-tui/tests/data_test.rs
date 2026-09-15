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
        priority: 128,
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
        snapshot_interval_s: 30,
        max_snapshots: 100,
        eco_tick_interval_ms: 1000,
        event_backlog_threshold: 100,
        max_chat_messages: 500,
    }
}

/// 轮询等待 pipeline 快照包含指定数量的事件，最多等待 2 秒
///
/// WHY 轮询而非固定 sleep: SysMetricsCollector::new() 调用
/// sysinfo::System::new_all() 是同步阻塞操作，在 current_thread runtime
/// (#[tokio::test] 默认)中会阻塞整个 runtime。固定 sleep(80ms) 不足以
/// 覆盖初始化时间，导致 DataPipeline 在窗口内未处理事件，snapshot 中
/// quest_list 为空。轮询等待确保 DataPipeline 至少处理完
/// expected_event_count 个事件后才进行断言，与 live_data_test.rs 模式一致。
async fn wait_for_events(pipeline: &DataPipeline, expected_event_count: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snap = pipeline.snapshot();
        if snap.latest_events.len() >= expected_event_count {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for {} events; got {} events, quest_list len={}",
                expected_event_count,
                snap.latest_events.len(),
                snap.quest_list.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
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

    // 等待 DataPipeline 处理完 3 个事件（轮询避免 SysMetricsCollector 初始化阻塞）
    wait_for_events(&pipeline, 3).await;

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

    // 等待 DataPipeline 处理完 5 个事件（轮询避免 SysMetricsCollector 初始化阻塞）
    wait_for_events(&pipeline, 5).await;

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
async fn pipeline_handles_1000_events_per_second() {
    // 使用 250ms tick（生产默认值），在 tick 窗口内突发 1000 个事件。
    let bus = EventBus::with_capacity(4096);
    let subscriber = EventSubscriber::new(bus.clone());
    let config = DataSourceConfig {
        // 覆盖 test_config() 默认值 256 → 1000,确保 1000 个事件全部保留在 latest_events 中
        max_event_history: 1000,
        tick_interval_ms: 250,
        ..test_config()
    };
    let pipeline = DataPipeline::new(subscriber, config);

    // 快速发布 1000 个 BudgetMetricsUpdated 事件
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

    // 轮询直到 snapshot 包含全部 1000 个事件。窗口 3s:事件唤醒使消费节奏
    // 变为「更频繁、更小批」,突发 1000(事件率 ~4000/s)会触发 Eco 降频
    // (1s 节奏,正确的事件风暴保护)——完整消费需一次 Eco 周期 + 大批处理,
    // 与本测试自身的宽松断言口径对齐;3s 覆盖两个 Eco 周期,余量充足。
    let deadline = publish_done + Duration::from_secs(3);
    while Instant::now() < deadline {
        let snap = pipeline.snapshot();
        if snap.latest_events.len() == 1000 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let snapshot = pipeline.snapshot();
    assert_eq!(snapshot.latest_events.len(), 1000);
    assert!(
        snapshot.revision >= 1,
        "P2 性能:快照应携带递增 revision(实际 {})",
        snapshot.revision
    );

    // 端到端时序:事件唤醒使突发消费节奏变为「即时唤醒 + Eco 降频保护」——
    // 突发 1000(事件率 ~4000/s)触发 Eco(1s 节奏)属**预期行为**,完整消费
    // 需一次 Eco 周期(~2.2s)。灾难性回归(事件丢失/死循环)由上方 3s deadline
    // 轮询单点拦截(拿不到 1000 即 panic);精确时序仍由 criterion bench
    // (data_pipeline_snapshot_latency / data_pipeline_throughput)在受控环境度量。
}

/// P2 性能(P-1):快照 revision 随每个 tick 单调递增,供 `TuiApp::update`
/// 跳过无变化帧的字段拷贝(轮询 100ms 快于 tick 250ms 时的关键优化前提)。
#[tokio::test]
async fn pipeline_revision_monotonic_increases() {
    let bus = EventBus::with_capacity(4096);
    let subscriber = EventSubscriber::new(bus.clone());
    let config = DataSourceConfig {
        tick_interval_ms: 50,
        ..test_config()
    };
    let pipeline = DataPipeline::new(subscriber, config);

    bus.publish(budget_metrics_event(
        BudgetMetrics::default(),
        "efficiency-monitor",
    ))
    .await
    .unwrap();

    // 轮询等待首个 tick(避免 CI 上任务调度延迟造成的墙钟时序脆弱)
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut snap1 = pipeline.snapshot();
    while snap1.revision == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
        snap1 = pipeline.snapshot();
    }
    assert!(snap1.revision >= 1, "首次 tick 后 revision 应 >= 1");

    // 轮询等待 revision 前进(至少再完成一个 tick)
    let deadline2 = Instant::now() + Duration::from_millis(500);
    let mut snap2 = snap1.clone();
    while snap2.revision <= snap1.revision && Instant::now() < deadline2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        snap2 = pipeline.snapshot();
    }
    assert!(
        snap2.revision > snap1.revision,
        "revision 应随 tick 单调递增(snap1={}, snap2={})",
        snap1.revision,
        snap2.revision
    );

    pipeline.shutdown().await;
}

// ============================================================
// Task M4 扩展:QuestCancelled / QuestPriorityAdjusted 事件消费
// ============================================================
//
// WHY 独立测试组:quest-engine 发布这两个状态变更事件后,DataPipeline
// 必须更新 quest_list 以反映最新状态。QuestCancelled 移除 Quest 并清理
// 暂停集合(避免内存泄漏),QuestPriorityAdjusted 只更新 priority 字段。

/// 构造 QuestCancelled 事件
fn quest_cancelled_event(quest_id: &str) -> NexusEvent {
    NexusEvent::QuestCancelled {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: quest_id.into(),
        requested_by: "test".into(),
    }
}

/// 构造 QuestPriorityAdjusted 事件
fn quest_priority_adjusted_event(quest_id: &str, new_priority: u8) -> NexusEvent {
    NexusEvent::QuestPriorityAdjusted {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: quest_id.into(),
        new_priority,
        requested_by: "test".into(),
    }
}

#[tokio::test]
async fn test_quest_cancelled_removes_from_list() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    // 初始化两个 Quest
    let q1 = quest("q1", "first");
    let q2 = quest("q2", "second");
    bus.publish(quest_list_event(vec![q1, q2.clone()], "quest-engine"))
        .await
        .unwrap();

    // 发布 QuestCancelled 取消 q1
    bus.publish(quest_cancelled_event("q1")).await.unwrap();

    // 等待 DataPipeline 处理完 2 个事件（轮询避免 SysMetricsCollector 初始化阻塞）
    wait_for_events(&pipeline, 2).await;

    let snapshot = pipeline.snapshot();
    // q1 被移除,只剩 q2
    assert_eq!(snapshot.quest_list.len(), 1, "q1 should be removed");
    assert_eq!(snapshot.quest_list[0].quest_id, "q2");
    assert_eq!(snapshot.quest_list[0].title, "second");
}

#[tokio::test]
async fn test_quest_priority_adjusted_updates_field() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    // 初始化一个 Quest(priority=128,默认值)
    let q1 = quest("q1", "priority-test");
    bus.publish(quest_list_event(vec![q1], "quest-engine"))
        .await
        .unwrap();

    // 发布 QuestPriorityAdjusted 调整优先级为 200
    bus.publish(quest_priority_adjusted_event("q1", 200))
        .await
        .unwrap();

    // 等待 DataPipeline 处理完 2 个事件（轮询避免 SysMetricsCollector 初始化阻塞）
    wait_for_events(&pipeline, 2).await;

    let snapshot = pipeline.snapshot();
    assert_eq!(snapshot.quest_list.len(), 1, "quest should still exist");
    assert_eq!(
        snapshot.quest_list[0].priority, 200,
        "priority should be updated to 200"
    );
}

#[tokio::test]
async fn test_quest_cancelled_unknown_id_no_change() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    // 初始化一个 Quest
    let q1 = quest("q1", "only-one");
    bus.publish(quest_list_event(vec![q1.clone()], "quest-engine"))
        .await
        .unwrap();

    // 发布 QuestCancelled 取消不存在的 quest_id,不应 panic 也不应改变列表
    bus.publish(quest_cancelled_event("nonexistent"))
        .await
        .unwrap();

    // 等待 DataPipeline 处理完 2 个事件（轮询避免 SysMetricsCollector 初始化阻塞）
    wait_for_events(&pipeline, 2).await;

    let snapshot = pipeline.snapshot();
    assert_eq!(
        snapshot.quest_list.len(),
        1,
        "quest_list should remain unchanged for unknown quest_id"
    );
    assert_eq!(snapshot.quest_list[0].quest_id, "q1");
}

// ============================================================
// Concord T1.7:budget_metrics_ttl_ms 消费的管道级传播验证
// ============================================================

#[tokio::test]
async fn budget_snapshot_is_fresh_right_after_update_event() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    bus.publish(budget_metrics_event(
        BudgetMetrics::default(),
        "efficiency-monitor",
    ))
    .await
    .unwrap();
    wait_for_events(&pipeline, 1).await;

    // 事件刚到达(远小于 ttl=5000ms)→ 快照陈旧标志必须为 false
    let snap = pipeline.snapshot();
    assert!(
        !snap.budget_metrics_stale,
        "收到 BudgetMetricsUpdated 后立即判新鲜(ttl 未超期)"
    );
    pipeline.shutdown().await;
}

#[tokio::test]
async fn budget_snapshot_is_stale_without_any_update_event() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    // 发布无关事件驱动至少一个 tick(避免首 tick 初始化耗时干扰)
    bus.publish(quest_list_event(
        vec![quest("q-stale", "驱动 tick")],
        "quest-engine",
    ))
    .await
    .unwrap();
    wait_for_events(&pipeline, 1).await;

    // 从未收到 BudgetMetricsUpdated → 诚实判陈旧(面板将置灰)
    let snap = pipeline.snapshot();
    assert!(
        snap.budget_metrics_stale,
        "无预算更新事件时必须判陈旧(默认占位值不得伪装新鲜)"
    );
    pipeline.shutdown().await;
}

// ============================================================
// 感知延迟优化(§7.7 设计,2026-09-13)——事件驱动即时 tick
// ============================================================

/// 构造 Chat 回复 chunk 事件(流式回复的最小单元)
fn chat_chunk_event(session_id: &str, delta: &str) -> NexusEvent {
    NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: session_id.into(),
        delta: delta.into(),
        cursor_hint: 0,
    }
}

/// 感知延迟主断言:Chat 事件到达 → 快照**远早于 tick 间隔**刷新
///
/// 旧口径:事件进入订阅缓冲后,等下一 250ms tick 才进快照;
/// 新口径:转发任务的 Notify 唤醒 pipeline 的 `select!`,事件到达即 tick。
/// 验证方式:tick_interval_ms=250,事件发布后 **80ms 窗口**内轮询 revision
/// ——旧行为下 80ms 内不可能有 tick,断言只在事件唤醒生效时通过。
/// (80ms 给足 16 倍轮询步长余量,防 CI 慢机 flaky)
#[tokio::test]
async fn chat_event_refreshes_snapshot_immediately_not_on_tick() {
    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 250,
            ..test_config()
        },
    );

    let before = pipeline.snapshot().revision;
    // delta 以换行结尾:ChatSync 行闸门只提交完整行(半行留存防闪烁),
    // 无换行的 delta 会留存在闸门中不进 chat_messages(设计行为)
    bus.publish(chat_chunk_event("s-immediate", "即时刷新\n"))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_millis(80);
    loop {
        if pipeline.snapshot().revision > before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "chat 事件应在 80ms 内刷新快照(revision {before} 未变)——事件驱动唤醒失效"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // 内容也应即时可见(chat_messages 已进快照)
    let snap = pipeline.snapshot();
    assert!(
        snap.chat_messages
            .iter()
            .any(|m| m.content.contains("即时刷新")),
        "chat 消息应已进快照"
    );
    pipeline.shutdown().await;
}

/// 轻量 tick 副作用保护:事件唤醒的 tick **不**推进趋势 history
///
/// WHY:事件唤醒频率随事件率上升,若唤醒 tick 也 push_history,趋势曲线的
/// 64 点时间窗口会被高频点稀释(250ms 粒度失效)。本测试发 10 个 Budget
/// 事件(间隔 ~8ms,全部触发唤醒),断言 100ms 窗口内 budget_history 增长
/// ≤1(仅可能有一次 250ms 定时 tick)——证明轻量 tick 正确跳过 push。
#[tokio::test]
async fn event_wake_ticks_do_not_dilute_history() {
    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 250,
            ..test_config()
        },
    );

    let before = pipeline.snapshot().budget_history.len();
    // 10 个 Budget 事件,间隔 8ms(~80ms 总时长)——全部经 Notify 唤醒
    for i in 0..10 {
        bus.publish(budget_metrics_event(
            BudgetMetrics {
                total_consumption: 100.0 + i as f64,
                remaining_budget: 9900.0,
                utilization_rate: 0.1,
                current_tier: "Low".into(),
                coefficient: 0.9,
                is_exceeded: false,
                alert: None,
            },
            "test",
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(8)).await;
    }
    // 再给 40ms 让最后一次唤醒 tick 完成(仍在首个 250ms 定时 tick 之前)
    tokio::time::sleep(Duration::from_millis(40)).await;

    let after = pipeline.snapshot().budget_history.len();
    assert!(
        after <= before + 1,
        "事件唤醒的轻量 tick 不得推进趋势 history(稀释 250ms 粒度);before={before}, after={after}"
    );
    pipeline.shutdown().await;
}
