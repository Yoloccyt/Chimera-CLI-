//! LHQP 检查点持久化集成测试 — 覆盖保存/加载/恢复/事件/保留策略
//!
//! 测试矩阵:
//! 1. 检查点保存与加载往返一致性(Quest 字段完整保留)
//! 2. SHA-256 完整性校验(篡改文件后 load 失败)
//! 3. 保留最近 N 个检查点(超出 max_keep 删除最旧)
//! 4. 崩溃恢复场景(完成 Task → 保存 → 丢弃内存 → 恢复 → 验证状态)
//! 5. CheckpointSaved 事件发布
//! 6. CheckpointLoaded 事件发布
//! 7. load_latest 返回最新检查点
//! 8. 无检查点时 load_latest 返回 None
//! 9. 自动检查点触发(checkpoint_interval 达阈值自动保存)
//! 10. 禁用检查点时 save/restore 返回明确错误

use std::path::PathBuf;
use std::time::Duration;

use event_bus::{EventBus, NexusEvent};
use nexus_core::{MultimodalInput, Quest, TaskStatus, UserIntent};
use quest_engine::{CheckpointManager, QuestConfig, QuestEngine, QuestError};
use tempfile::tempdir;

/// 构造测试用 UserIntent
fn make_intent(id: &str, text: &str) -> UserIntent {
    UserIntent {
        intent_id: id.into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

/// 接收事件带超时,避免死锁
async fn recv_event(rx: &mut event_bus::EventReceiver) -> NexusEvent {
    tokio::time::timeout(Duration::from_millis(1000), rx.recv())
        .await
        .expect("接收事件超时")
        .expect("接收事件失败")
}

/// 跳过非目标事件,等待指定类型
async fn skip_until_checkpoint_saved(
    rx: &mut event_bus::EventReceiver,
) -> (String, String, String) {
    loop {
        let event = recv_event(rx).await;
        if let NexusEvent::CheckpointSaved {
            quest_id,
            checkpoint_id,
            memory_snapshot_hash,
            ..
        } = event
        {
            return (quest_id, checkpoint_id, memory_snapshot_hash);
        }
        // 其他事件跳过
    }
}

async fn skip_until_checkpoint_loaded(rx: &mut event_bus::EventReceiver) -> (String, String) {
    loop {
        let event = recv_event(rx).await;
        if let NexusEvent::CheckpointLoaded {
            quest_id,
            checkpoint_id,
            ..
        } = event
        {
            return (quest_id, checkpoint_id);
        }
        // 其他事件跳过
    }
}

// ============================================================
// 测试 1:检查点保存与加载往返一致性
// ============================================================
#[tokio::test]
async fn test_checkpoint_save_load_roundtrip() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent = make_intent("i-1", "第一步。第二步。第三步。");
    let quest = engine.create_quest(intent).await.unwrap();

    let checkpoint = engine.save_checkpoint(&quest.quest_id).await.unwrap();
    assert_eq!(checkpoint.quest_id, quest.quest_id);
    assert!(!checkpoint.serialized_state.is_empty());
    assert_eq!(checkpoint.memory_snapshot_hash.len(), 64); // SHA-256 hex

    // 恢复
    let restored = engine
        .restore_from_checkpoint(&quest.quest_id)
        .await
        .unwrap();
    assert_eq!(restored.quest_id, quest.quest_id);
    assert_eq!(restored.tasks.len(), quest.tasks.len());
    assert_eq!(restored.title, quest.title);
    assert_eq!(restored.thinking_mode, quest.thinking_mode);
}

// ============================================================
// 测试 2:SHA-256 完整性校验 — 篡改文件后 load 失败
// ============================================================
#[tokio::test]
async fn test_integrity_check_corrupted_file() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent = make_intent("i-1", "第一步。第二步。");
    let quest = engine.create_quest(intent).await.unwrap();
    let checkpoint = engine.save_checkpoint(&quest.quest_id).await.unwrap();

    // 直接篡改磁盘文件(翻转末尾字节)
    let cm = engine.checkpoint_manager().unwrap();
    let file_path = cm
        .checkpoint_dir()
        .join(&quest.quest_id)
        .join(format!("{}.bin", checkpoint.checkpoint_id));
    let mut bytes = std::fs::read(&file_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&file_path, bytes).unwrap();

    // load 应失败(反序列化错误或哈希不匹配)
    let result = cm.load(&quest.quest_id, &checkpoint.checkpoint_id).await;
    assert!(result.is_err(), "篡改文件后 load 应失败,实际: {result:?}");
}

// ============================================================
// 测试 3:保留最近 5 个检查点
// ============================================================
#[tokio::test]
async fn test_prune_keeps_latest_five() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    // max_keep=5
    let engine =
        QuestEngine::with_checkpoints_and_max_keep(bus, config, tmp.path().to_path_buf(), 5);

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 创建 7 个检查点(每次保存间隔 5ms 确保 created_at 不同)
    for _ in 0..7 {
        std::thread::sleep(Duration::from_millis(5));
        engine.save_checkpoint(&quest.quest_id).await.unwrap();
    }

    let cm = engine.checkpoint_manager().unwrap();
    let remaining = cm.list_checkpoints(&quest.quest_id).unwrap();
    assert_eq!(
        remaining.len(),
        5,
        "应保留最近 5 个检查点,实际: {}",
        remaining.len()
    );
}

// ============================================================
// 测试 4:崩溃恢复场景 — 完成 Task 后保存,丢弃内存,恢复验证状态
// ============================================================
#[tokio::test]
async fn test_crash_recovery_scenario() {
    let tmp = tempdir().unwrap();
    let checkpoint_dir: PathBuf = tmp.path().to_path_buf();

    // 阶段 1:创建 Quest,完成 2 个 Task,保存检查点
    let quest_id = {
        let bus = EventBus::new();
        let config = QuestConfig::new(16, 100); // interval=100 避免自动触发
        let engine = QuestEngine::with_checkpoints(bus, config, checkpoint_dir.clone());

        let intent = make_intent("i-1", "第一步。第二步。第三步。");
        let quest = engine.create_quest(intent).await.unwrap();
        let qid = quest.quest_id.clone();

        // 完成 task-0 和 task-1
        engine
            .update_task_status(&qid, "task-0", TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&qid, "task-0", TaskStatus::Completed)
            .await
            .unwrap();
        engine
            .update_task_status(&qid, "task-1", TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&qid, "task-1", TaskStatus::Completed)
            .await
            .unwrap();

        // 保存检查点(模拟崩溃前持久化)
        engine.save_checkpoint(&qid).await.unwrap();
        qid
    }; // engine drop,模拟进程崩溃

    // 阶段 2:新进程从检查点恢复
    let bus2 = EventBus::new();
    let config2 = QuestConfig::default();
    let engine2 = QuestEngine::with_checkpoints(bus2, config2, checkpoint_dir);

    let restored = engine2.restore_from_checkpoint(&quest_id).await.unwrap();
    assert_eq!(restored.quest_id, quest_id);
    assert_eq!(restored.tasks.len(), 3);

    // 验证 Task 状态:task-0 和 task-1 为 Completed,task-2 为 Pending
    assert_eq!(restored.tasks[0].status, TaskStatus::Completed);
    assert_eq!(restored.tasks[1].status, TaskStatus::Completed);
    assert_eq!(restored.tasks[2].status, TaskStatus::Pending);
}

// ============================================================
// 测试 5:CheckpointSaved 事件发布
// ============================================================
#[tokio::test]
async fn test_checkpoint_saved_event_published() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated 事件
    let _created = rx.recv().await.unwrap();

    engine.save_checkpoint(&quest.quest_id).await.unwrap();

    let (qid, cid, hash) = skip_until_checkpoint_saved(&mut rx).await;
    assert_eq!(qid, quest.quest_id);
    assert!(!cid.is_empty());
    assert_eq!(hash.len(), 64); // SHA-256 hex
}

// ============================================================
// 测试 6:CheckpointLoaded 事件发布
// ============================================================
#[tokio::test]
async fn test_checkpoint_loaded_event_published() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated 事件
    let _created = rx.recv().await.unwrap();

    // 保存检查点(会发布 CheckpointSaved,跳过)
    engine.save_checkpoint(&quest.quest_id).await.unwrap();
    let _ = skip_until_checkpoint_saved(&mut rx).await;

    // 恢复(会发布 CheckpointLoaded)
    engine
        .restore_from_checkpoint(&quest.quest_id)
        .await
        .unwrap();

    let (qid, cid) = skip_until_checkpoint_loaded(&mut rx).await;
    assert_eq!(qid, quest.quest_id);
    assert!(!cid.is_empty());
}

// ============================================================
// 测试 7:load_latest 返回最新检查点
// ============================================================
#[tokio::test]
async fn test_load_latest_returns_most_recent() {
    let tmp = tempdir().unwrap();
    let cm = CheckpointManager::new(tmp.path().to_path_buf());

    // 直接构造 Quest(不经 engine,避免事件干扰)
    let quest = Quest {
        quest_id: "q-test".into(),
        title: "测试".into(),
        tasks: vec![],
        thinking_mode: nexus_core::ThinkingMode::Standard,
        checkpoint_id: None,
    };

    let mut newest_time = chrono::DateTime::<chrono::Utc>::MIN_UTC;
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(5));
        let cp = cm.save(&quest).await.unwrap();
        newest_time = cp.created_at;
    }

    let latest = cm.load_latest("q-test").await.unwrap().unwrap();
    assert_eq!(latest.created_at, newest_time);
}

// ============================================================
// 测试 8:无检查点时 load_latest 返回 None
// ============================================================
#[tokio::test]
async fn test_load_latest_returns_none_when_empty() {
    let tmp = tempdir().unwrap();
    let cm = CheckpointManager::new(tmp.path().to_path_buf());

    let result = cm.load_latest("nonexistent-quest").await.unwrap();
    assert!(result.is_none());
}

// ============================================================
// 测试 9:自动检查点触发 — checkpoint_interval 达阈值自动保存
// ============================================================
#[tokio::test]
async fn test_auto_checkpoint_on_interval() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    // interval=1:每个 Task 完成触发检查点
    let config = QuestConfig::new(16, 1);
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent = make_intent("i-1", "第一步。第二步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated
    let _created = rx.recv().await.unwrap();

    // 完成 task-0:Pending → Running → Completed
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .unwrap();
    // 跳过 ProgressUpdated (Running, completed=0)
    let _ = recv_event(&mut rx).await;

    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
        .await
        .unwrap();
    // 此时 completed=1, interval=1, 应触发自动检查点
    // 事件顺序:ProgressUpdated(completed=1) → CheckpointSaved

    // 接收 ProgressUpdated
    let progress = recv_event(&mut rx).await;
    assert!(matches!(
        progress,
        NexusEvent::QuestProgressUpdated { completed: 1, .. }
    ));

    // 接收 CheckpointSaved
    let (qid, _, _) = skip_until_checkpoint_saved(&mut rx).await;
    assert_eq!(qid, quest.quest_id);

    // 验证检查点文件已落盘
    let cm = engine.checkpoint_manager().unwrap();
    let ids = cm.list_checkpoints(&quest.quest_id).unwrap();
    assert_eq!(ids.len(), 1, "应自动保存 1 个检查点");
}

// ============================================================
// 测试 10:禁用检查点时 save/restore 返回明确错误
// ============================================================
#[tokio::test]
async fn test_disabled_checkpoints_return_error() {
    let bus = EventBus::new();
    // 使用 new(不启用检查点)
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // save_checkpoint 应返回 CheckpointSaveFailed
    let result = engine.save_checkpoint(&quest.quest_id).await;
    assert!(
        matches!(result, Err(QuestError::CheckpointSaveFailed(_))),
        "禁用检查点时 save 应返回 CheckpointSaveFailed,实际: {result:?}"
    );

    // restore_from_checkpoint 应返回 CheckpointNotFound
    let result = engine.restore_from_checkpoint(&quest.quest_id).await;
    assert!(
        matches!(result, Err(QuestError::CheckpointNotFound(_))),
        "禁用检查点时 restore 应返回 CheckpointNotFound,实际: {result:?}"
    );
}

// ============================================================
// 测试 11:恢复后可继续推进 Task(模拟崩溃后继续执行)
// ============================================================
#[tokio::test]
async fn test_resume_execution_after_recovery() {
    let tmp = tempdir().unwrap();
    let checkpoint_dir: PathBuf = tmp.path().to_path_buf();

    // 阶段 1:创建 Quest,完成 task-0,保存检查点
    let quest_id = {
        let bus = EventBus::new();
        let config = QuestConfig::new(16, 100);
        let engine = QuestEngine::with_checkpoints(bus, config, checkpoint_dir.clone());

        let intent = make_intent("i-1", "第一步。第二步。");
        let quest = engine.create_quest(intent).await.unwrap();
        let qid = quest.quest_id.clone();

        engine
            .update_task_status(&qid, "task-0", TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&qid, "task-0", TaskStatus::Completed)
            .await
            .unwrap();

        engine.save_checkpoint(&qid).await.unwrap();
        qid
    };

    // 阶段 2:新引擎恢复,继续完成 task-1
    let bus2 = EventBus::new();
    let mut rx = bus2.subscribe();
    let config2 = QuestConfig::new(16, 100);
    let engine2 = QuestEngine::with_checkpoints(bus2, config2, checkpoint_dir);

    let restored = engine2.restore_from_checkpoint(&quest_id).await.unwrap();
    assert_eq!(restored.tasks[0].status, TaskStatus::Completed);
    assert_eq!(restored.tasks[1].status, TaskStatus::Pending);

    // 继续完成 task-1
    engine2
        .update_task_status(&quest_id, "task-1", TaskStatus::Running)
        .await
        .unwrap();
    engine2
        .update_task_status(&quest_id, "task-1", TaskStatus::Completed)
        .await
        .unwrap();

    // 验证 Quest 已完成(应收到 ExecutionCompleted)
    // 跳过 CheckpointLoaded 和 ProgressUpdated 事件
    loop {
        let event = recv_event(&mut rx).await;
        if let NexusEvent::ExecutionCompleted { quest_id: qid, .. } = event {
            assert_eq!(qid, quest_id);
            break;
        }
    }
}

// ============================================================
// 测试 12:多个 Quest 的检查点互不影响
// ============================================================
#[tokio::test]
async fn test_multiple_quests_checkpoints_isolated() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let intent1 = make_intent("i-1", "任务一。");
    let quest1 = engine.create_quest(intent1).await.unwrap();

    let intent2 = make_intent("i-2", "任务二。");
    let quest2 = engine.create_quest(intent2).await.unwrap();

    engine.save_checkpoint(&quest1.quest_id).await.unwrap();
    engine.save_checkpoint(&quest2.quest_id).await.unwrap();

    let cm = engine.checkpoint_manager().unwrap();
    let ids1 = cm.list_checkpoints(&quest1.quest_id).unwrap();
    let ids2 = cm.list_checkpoints(&quest2.quest_id).unwrap();

    assert_eq!(ids1.len(), 1);
    assert_eq!(ids2.len(), 1);
    assert_ne!(ids1[0], ids2[0]);

    // 恢复 quest1 不影响 quest2
    let restored1 = engine
        .restore_from_checkpoint(&quest1.quest_id)
        .await
        .unwrap();
    assert_eq!(restored1.quest_id, quest1.quest_id);

    let ids2_after = cm.list_checkpoints(&quest2.quest_id).unwrap();
    assert_eq!(ids2_after.len(), 1, "恢复 quest1 不应影响 quest2 的检查点");
}

// ============================================================
// 测试 13:restore_from_checkpoint 不存在的 Quest 返回错误
// ============================================================
#[tokio::test]
async fn test_restore_nonexistent_quest_returns_error() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let result = engine.restore_from_checkpoint("nonexistent").await;
    assert!(
        matches!(result, Err(QuestError::CheckpointNotFound(_))),
        "应返回 CheckpointNotFound,实际: {result:?}"
    );
}

// ============================================================
// 测试 14:save_checkpoint 不存在的 Quest 返回错误
// ============================================================
#[tokio::test]
async fn test_save_checkpoint_nonexistent_quest_returns_error() {
    let tmp = tempdir().unwrap();
    let bus = EventBus::new();
    let config = QuestConfig::default();
    let engine = QuestEngine::with_checkpoints(bus, config, tmp.path().to_path_buf());

    let result = engine.save_checkpoint("nonexistent").await;
    assert!(
        matches!(result, Err(QuestError::QuestNotFound(_))),
        "应返回 QuestNotFound,实际: {result:?}"
    );
}

// ============================================================
// 回归测试:CheckpointManager 的 I/O 不阻塞 async runtime
// ============================================================

use std::sync::Arc;

use nexus_core::{Task, ThinkingMode};
use tokio::task::JoinSet;

/// 构造较大体积的 Quest,使 MessagePack 序列化 + 磁盘 I/O 非平凡
fn make_large_quest(id: &str, task_count: usize) -> Quest {
    let long_desc = "中".repeat(1024);
    let tasks = (0..task_count)
        .map(|i| Task {
            task_id: format!("task-{i}"),
            description: format!("任务 {i} 的详细描述: {long_desc}"),
            status: TaskStatus::Pending,
            dependencies: if i == 0 {
                vec![]
            } else {
                vec![format!("task-{}", i - 1)]
            },
        })
        .collect();

    Quest {
        quest_id: id.into(),
        title: format!("大 Quest {id}"),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    }
}

/// 轻量任务:仅连续 yield 10 次,用于探测 runtime 是否被阻塞
async fn yield_ten_times() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

// ------------------------------------------------------------
// 测试 A:save() 不阻塞 runtime
// ------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_save_load_not_blocking_runtime() {
    let tmp = tempdir().unwrap();
    let cm = CheckpointManager::new(tmp.path().to_path_buf());
    let quest = make_large_quest("q-save-blocking", 50);

    // 将 save 放到独立任务,与轻量任务并发执行
    let save_handle = tokio::spawn(async move { cm.save(&quest).await });

    let lightweight = yield_ten_times();
    let light_result = tokio::time::timeout(Duration::from_millis(100), lightweight).await;

    let save_result = save_handle
        .await
        .expect("save 任务不应 panic")
        .expect("save 应成功");

    assert!(
        light_result.is_ok(),
        "save 期间轻量任务应在 100ms 内完成,说明 runtime 未被阻塞"
    );
    assert_eq!(save_result.quest_id, "q-save-blocking");
}

// ------------------------------------------------------------
// 测试 B:load_latest() 不阻塞 runtime
// ------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_load_latest_not_blocking_runtime() {
    let tmp = tempdir().unwrap();
    let cm = CheckpointManager::new(tmp.path().to_path_buf());
    let quest = make_large_quest("q-load-blocking", 50);

    // 先保存一个检查点,使 load_latest 有文件可读
    let _ = cm.save(&quest).await.expect("前置 save 应成功");

    let cm = Arc::new(cm);
    let cm2 = Arc::clone(&cm);
    let load_handle = tokio::spawn(async move { cm2.load_latest("q-load-blocking").await });

    let lightweight = yield_ten_times();
    let light_result = tokio::time::timeout(Duration::from_millis(100), lightweight).await;

    let load_result = load_handle
        .await
        .expect("load_latest 任务不应 panic")
        .expect("load_latest 应成功");

    assert!(
        light_result.is_ok(),
        "load_latest 期间轻量任务应在 100ms 内完成,说明 runtime 未被阻塞"
    );
    assert!(load_result.is_some(), "load_latest 应返回已保存的检查点");
}

// ------------------------------------------------------------
// 测试 C:并发 save/load 无数据丢失或 ID 冲突
// ------------------------------------------------------------
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_save_load_correctness() {
    let tmp = tempdir().unwrap();
    let cm = Arc::new(CheckpointManager::new(tmp.path().to_path_buf()));
    let quest = make_large_quest("q-concurrent", 30);

    // 并发保存 5 个检查点
    let mut save_set = JoinSet::new();
    for _ in 0..5 {
        let cm = Arc::clone(&cm);
        let quest = quest.clone();
        save_set.spawn(async move { cm.save(&quest).await });
    }

    let mut checkpoint_ids = Vec::new();
    while let Some(result) = save_set.join_next().await {
        let cp = result.expect("save 任务不应 panic").expect("save 应成功");
        checkpoint_ids.push(cp.checkpoint_id);
    }
    assert_eq!(checkpoint_ids.len(), 5, "应保存 5 个检查点");

    // 并发按 ID 加载并校验完整性
    let mut load_set = JoinSet::new();
    for id in &checkpoint_ids {
        let cm = Arc::clone(&cm);
        let id = id.clone();
        load_set.spawn(async move { cm.load("q-concurrent", &id).await });
    }

    let mut loaded_count = 0;
    while let Some(result) = load_set.join_next().await {
        let cp = result.expect("load 任务不应 panic").expect("load 应成功");
        let restored: Quest =
            rmp_serde::from_slice(&cp.serialized_state).expect("反序列化 Quest 应成功");
        assert_eq!(restored.quest_id, quest.quest_id);
        assert_eq!(restored.tasks.len(), quest.tasks.len());
        loaded_count += 1;
    }
    assert_eq!(loaded_count, 5, "应成功加载全部 5 个检查点");

    // list_checkpoints 应恰好看到 5 个文件
    let listed = cm.list_checkpoints("q-concurrent").expect("list 应成功");
    assert_eq!(listed.len(), 5, "磁盘上应保留 5 个检查点文件");

    // load_latest 应返回一个有效的 Quest
    let latest = cm
        .load_latest("q-concurrent")
        .await
        .expect("load_latest 应成功")
        .expect("应存在最新检查点");
    let latest_quest: Quest =
        rmp_serde::from_slice(&latest.serialized_state).expect("反序列化最新 Quest 应成功");
    assert_eq!(latest_quest.quest_id, quest.quest_id);
}
