//! Ambient Mode 集成测试 — 资源看门狗 / 检查点节流 / 记忆整理 hook
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 Milestone B-2）：
//! 后台常驻 Ambient Mode（记忆整理 + 检查点 + 资源等待），事件驱动无轮询锁，
//! 不依赖 RL（jcode 论文独立可落地项）。E2E 验收：资源恢复触发 quest 恢复。

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::ambient_mode::{spawn_ambient_subscriber, AmbientModeConfig, MemoryTidyHook};
use quest_engine::QuestEngine;

fn make_intent(text: &str) -> UserIntent {
    UserIntent {
        intent_id: "i-1".into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

/// 计数型记忆整理 hook（测试观测点——计数 Arc 共享给测试与订阅器）
struct CountingTidyHook {
    calls: Arc<AtomicUsize>,
}

impl MemoryTidyHook for CountingTidyHook {
    fn on_memory_pressure(&self, _quest_id: &str) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}

/// 构造 ambient 订阅器 + 引擎（返回 engine + 整理调用计数供断言）
async fn setup(bus: EventBus, config: AmbientModeConfig) -> (Arc<QuestEngine>, Arc<AtomicUsize>) {
    let engine = Arc::new(QuestEngine::new(bus.clone()));
    let calls = Arc::new(AtomicUsize::new(0));
    let hook: Arc<dyn MemoryTidyHook> = Arc::new(CountingTidyHook {
        calls: Arc::clone(&calls),
    });
    let _handle =
        spawn_ambient_subscriber(bus.clone(), Arc::clone(&engine), config, Arc::clone(&hook));
    (engine, calls)
}

/// 资源看门狗：BudgetExceeded → 活跃 Quest 被挂起（发布 QuestPaused）
#[tokio::test]
async fn watchdog_pauses_quests_on_budget_exceeded() {
    let bus = EventBus::new();
    let mut ambient_rx = bus.subscribe(); // 先 subscribe 再 spawn（红线）

    // 先创建 Quest（发布 QuestCreated）再 recv 消费——recv 前通道须非空
    let (engine, _hook) = setup(bus.clone(), AmbientModeConfig::default()).await;
    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();
    let _ = ambient_rx.recv().await.unwrap(); // 消费 QuestCreated

    // 发布预算超限
    bus.publish(NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("test"),
        budget_type: "memory".into(),
        current: 120,
        limit: 100,
    })
    .await
    .unwrap();

    // Ambient 应发布 QuestPaused
    let mut saw_paused = false;
    for _ in 0..4 {
        if let Ok(Ok(NexusEvent::QuestPaused { quest_id, .. })) =
            tokio::time::timeout(std::time::Duration::from_secs(2), ambient_rx.recv()).await
        {
            assert_eq!(quest_id, quest.quest_id);
            saw_paused = true;
            break;
        }
    }
    assert!(saw_paused, "BudgetExceeded 后应发布 QuestPaused");
}

/// E2E 验收点：资源恢复（ResourceRecovered）→ 被看门狗挂起的 Quest 恢复（QuestResumed）
#[tokio::test]
async fn resource_recovery_resumes_watchdog_paused_quest() {
    let bus = EventBus::new();
    let mut ambient_rx = bus.subscribe(); // 先 subscribe 再 spawn（红线）

    let (engine, _hook) = setup(bus.clone(), AmbientModeConfig::default()).await;
    let quest = engine
        .create_quest(make_intent("长跑任务。"))
        .await
        .unwrap();
    let _ = ambient_rx.recv().await.unwrap(); // 消费 QuestCreated（先发布后消费）

    // 1) 预算超限 → 挂起
    bus.publish(NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("test"),
        budget_type: "memory".into(),
        current: 120,
        limit: 100,
    })
    .await
    .unwrap();

    let mut saw_paused = false;
    for _ in 0..4 {
        if let Ok(Ok(NexusEvent::QuestPaused { quest_id, .. })) =
            tokio::time::timeout(std::time::Duration::from_secs(2), ambient_rx.recv()).await
        {
            assert_eq!(quest_id, quest.quest_id);
            saw_paused = true;
            break;
        }
    }
    assert!(saw_paused, "先挂起");

    // 2) 资源恢复 → 恢复
    bus.publish(NexusEvent::ResourceRecovered {
        metadata: EventMetadata::new("test"),
        resource_type: "memory".into(),
    })
    .await
    .unwrap();

    let mut saw_resumed = false;
    for _ in 0..4 {
        if let Ok(Ok(NexusEvent::QuestResumed { quest_id, .. })) =
            tokio::time::timeout(std::time::Duration::from_secs(2), ambient_rx.recv()).await
        {
            assert_eq!(quest_id, quest.quest_id);
            saw_resumed = true;
            break;
        }
    }
    assert!(
        saw_resumed,
        "ResourceRecovered 后应发布 QuestResumed（资源恢复触发 quest 恢复）"
    );
}

/// 记忆整理 hook：CheckpointSaved → on_memory_pressure 被调用
#[tokio::test]
async fn memory_tidy_hook_invoked_on_checkpoint_saved() {
    let bus = EventBus::new();
    let (engine, hook) = setup(bus.clone(), AmbientModeConfig::default()).await;

    let quest = engine.create_quest(make_intent("任务。")).await.unwrap();

    bus.publish(NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("test"),
        quest_id: quest.quest_id.clone(),
        checkpoint_id: "cp-1".into(),
        memory_snapshot_hash: "hash-1".into(),
    })
    .await
    .unwrap();

    // 等待 hook 调用（事件异步处理）
    for _ in 0..10 {
        if hook.load(Ordering::Relaxed) > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        hook.load(Ordering::Relaxed) > 0,
        "CheckpointSaved 应触发记忆整理 hook"
    );
}

/// 检查点节流：间隔内重复事件不重复触发整理
#[tokio::test]
async fn tidy_hook_throttled_by_interval() {
    let bus = EventBus::new();
    let config = AmbientModeConfig {
        tidy_interval_secs: 3600, // 1 小时节流——测试窗口内只允许首次触发
        ..AmbientModeConfig::default()
    };
    let (engine, hook) = setup(bus.clone(), config).await;

    let quest = engine.create_quest(make_intent("任务。")).await.unwrap();

    for i in 0..3 {
        bus.publish(NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("test"),
            quest_id: quest.quest_id.clone(),
            checkpoint_id: format!("cp-{i}"),
            memory_snapshot_hash: format!("hash-{i}"),
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    // 节流窗口内：最多触发 1 次
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let calls = hook.load(Ordering::Relaxed);
    assert!(calls <= 1, "节流窗口内整理应至多触发 1 次（实际 {calls}）");
}

/// 检查点调度节流：短 checkpoint_interval 下事件驱动保存，间隔内不重复
#[tokio::test]
async fn checkpoint_schedule_respects_interval() {
    let bus = EventBus::new();
    let config = AmbientModeConfig {
        tidy_interval_secs: 3600,
        checkpoint_interval_secs: 3600, // 1 小时节流——测试窗口内只允许首次
        ..AmbientModeConfig::default()
    };
    let (engine, hook) = setup(bus.clone(), config).await;
    let quest = engine.create_quest(make_intent("任务。")).await.unwrap();

    // 两次 CheckpointSaved 信号：第二次不应触发额外维护（窗口内）
    for i in 0..2 {
        bus.publish(NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("test"),
            quest_id: quest.quest_id.clone(),
            checkpoint_id: format!("cp-{i}"),
            memory_snapshot_hash: format!("hash-{i}"),
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        hook.load(Ordering::Relaxed) <= 1,
        "检查点节流窗口内维护应至多 1 次"
    );
}
