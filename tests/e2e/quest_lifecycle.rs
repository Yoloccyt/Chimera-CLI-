//! Week 8 Task 6 SubTask 6.1 — Quest 生命周期 E2E 测试
//!
//! 对应任务:Week 8 Task 6.1(Quest 生命周期端到端验收)
//! 架构层:L9 Quest(Quest Engine + Checkpoint Manager)
//!
//! # 测试覆盖
//! 1. Quest 创建 + 任务分解:UserIntent → QuestEngine::create_quest → 3 Task 线性链
//! 2. 崩溃恢复:保存检查点 → 丢弃原 Engine → 新 Engine 从检查点恢复
//! 3. 完整生命周期:创建 → 分解 → 推进 → 崩溃 → 恢复 → 完成 → ExecutionCompleted
//!
//! # 架构红线对齐
//! - `#![forbid(unsafe_code)]` 红线:测试代码也遵守,不引入 unsafe
//! - 单运行时:用 `tokio::runtime::Runtime::new()` 而非 `#[tokio::test]`,
//!   避免 runtime 冲突(子任务原则 #2)
//! - Windows 文件锁:用 `tempfile::TempDir` 模拟崩溃,避免手动路径(子任务原则 #3)

#![forbid(unsafe_code)]

use std::time::Duration;

use event_bus::{EventBus, NexusEvent};
use nexus_core::{MultimodalInput, TaskStatus, ThinkingMode, UserIntent};
use quest_engine::{QuestConfig, QuestEngine};
use tempfile::TempDir;

/// 构造测试用 UserIntent — 三句话对应三个 Task,模拟"分析→设计→实现"工作流
fn make_intent(intent_id: &str, text: &str) -> UserIntent {
    UserIntent {
        intent_id: intent_id.into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 30,
    }
}

/// 排空事件接收器,收集最多 `max_count` 个事件(每个事件 50ms 超时)
async fn drain_events(rx: &mut event_bus::EventReceiver, max_count: usize) -> Vec<NexusEvent> {
    let mut events = Vec::with_capacity(max_count);
    for _ in 0..max_count {
        match rx.recv_timeout(Duration::from_millis(50)).await {
            Ok(event) => events.push(event),
            Err(_) => break,
        }
    }
    events
}

/// 统计事件列表中指定类型的事件数量(按 `type_name()` 匹配)
fn count_event_by_type(events: &[NexusEvent], type_name: &str) -> usize {
    events.iter().filter(|e| e.type_name() == type_name).count()
}

// ============================================================
// 测试 1:Quest 创建 + 任务分解
// 验证:UserIntent 经 create_quest 后分解为 3 个 Task,线性依赖链,
// 并广播 QuestCreated 事件
// ============================================================

#[test]
fn test_quest_create_decompose() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = QuestEngine::new(bus);

        let intent = make_intent("i-create-1", "分析需求。设计方案。实现代码。");
        let quest = engine.create_quest(intent).await.expect("Quest 创建失败");

        // 验证:3 个 Task,线性依赖链
        assert_eq!(quest.tasks.len(), 3, "应分解为 3 个 Task");
        assert!(quest.tasks[0].dependencies.is_empty(), "首个 Task 无依赖");
        assert_eq!(
            quest.tasks[1].dependencies,
            vec!["task-0".to_string()],
            "第二个 Task 依赖 task-0"
        );
        assert_eq!(
            quest.tasks[2].dependencies,
            vec!["task-1".to_string()],
            "第三个 Task 依赖 task-1"
        );

        // 验证:默认思考模式为 Standard
        assert_eq!(
            quest.thinking_mode,
            ThinkingMode::Standard,
            "默认思考模式应为 Standard"
        );

        // 验证:QuestCreated 事件已发布
        let events = drain_events(&mut rx, 5).await;
        let created_count = count_event_by_type(&events, "QuestCreated");
        assert_eq!(
            created_count, 1,
            "应发布 1 个 QuestCreated 事件,实际 {created_count}"
        );
    });
}

// ============================================================
// 测试 2:崩溃恢复 — 保存检查点 → 丢弃 Engine → 新 Engine 恢复
// 验证:LHQP(Long-Horizon Quest Persistence)语义:
//   保存时 task-0=Completed → 恢复后状态一致
// ============================================================

#[test]
fn test_quest_crash_recovery() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let checkpoint_dir = tmp.path().join("checkpoints");
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 阶段 1:创建 Quest 并完成首个 Task
        let engine = QuestEngine::with_checkpoints(
            bus.clone(),
            QuestConfig::default(),
            checkpoint_dir.clone(),
        );
        let quest = engine
            .create_quest(make_intent("i-crash-1", "分析需求。设计方案。实现代码。"))
            .await
            .expect("Quest 创建失败");
        // 消费 QuestCreated 事件
        let _ = rx.recv().await.expect("应收到 QuestCreated 事件");

        // 推进 task-0 至 Completed
        engine
            .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
            .await
            .expect("task-0 Running 失败");
        engine
            .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
            .await
            .expect("task-0 Completed 失败");

        // 阶段 2:显式保存检查点
        let checkpoint = engine
            .save_checkpoint(&quest.quest_id)
            .await
            .expect("检查点保存失败");
        assert_eq!(checkpoint.quest_id, quest.quest_id, "quest_id 应一致");
        assert!(
            !checkpoint.serialized_state.is_empty(),
            "序列化状态不应为空"
        );
        assert!(
            !checkpoint.memory_snapshot_hash.is_empty(),
            "完整性哈希不应为空"
        );

        // 阶段 3:模拟崩溃 — 丢弃原 Engine,新建 Engine 指向同一检查点目录
        drop(engine);
        let new_bus = EventBus::new();
        let restored_engine = QuestEngine::with_checkpoints(
            new_bus.clone(),
            QuestConfig::default(),
            checkpoint_dir.clone(),
        );

        // 阶段 4:从检查点恢复 Quest
        let restored_quest = restored_engine
            .restore_from_checkpoint(&quest.quest_id)
            .await
            .expect("检查点恢复失败");

        // 阶段 5:验证恢复后的状态
        assert_eq!(restored_quest.quest_id, quest.quest_id, "quest_id 应一致");
        assert_eq!(restored_quest.tasks.len(), 3, "Task 数量应一致");
        // task-0 应为 Completed(保存检查点时已完成)
        assert_eq!(
            restored_quest.tasks[0].status,
            TaskStatus::Completed,
            "task-0 应为 Completed"
        );
        // task-1、task-2 应仍为 Pending(未推进)
        assert_eq!(
            restored_quest.tasks[1].status,
            TaskStatus::Pending,
            "task-1 应为 Pending"
        );
        assert_eq!(
            restored_quest.tasks[2].status,
            TaskStatus::Pending,
            "task-2 应为 Pending"
        );
    });
}

// ============================================================
// 测试 3:完整生命周期 — 创建 → 分解 → 推进 → 崩溃 → 恢复 → 完成
// 验证:全链路事件流 + 终态可达(ExecutionCompleted)
// ============================================================

#[test]
fn test_quest_full_lifecycle() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建失败");
    rt.block_on(async {
        let tmp = TempDir::new().expect("TempDir 创建失败");
        let checkpoint_dir = tmp.path().join("checkpoints");
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // === 阶段 1:创建 + 分解 ===
        let engine = QuestEngine::with_checkpoints(
            bus.clone(),
            QuestConfig::default(),
            checkpoint_dir.clone(),
        );
        let quest = engine
            .create_quest(make_intent("i-full-1", "分析需求。设计方案。实现代码。"))
            .await
            .expect("Quest 创建失败");
        assert_eq!(quest.tasks.len(), 3, "应分解为 3 个 Task");

        // === 阶段 2:推进 task-0 → Completed,触发自动检查点 ===
        engine
            .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
            .await
            .expect("task-0 Running 失败");
        engine
            .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
            .await
            .expect("task-0 Completed 失败");

        // 显式保存检查点(模拟崩溃前最后一次持久化)
        let _checkpoint = engine
            .save_checkpoint(&quest.quest_id)
            .await
            .expect("崩溃前检查点保存失败");

        // === 阶段 3:模拟崩溃 — 丢弃原 Engine ===
        drop(engine);
        // 排空旧 bus 上的事件,避免与恢复阶段混淆
        let _ = drain_events(&mut rx, 20).await;

        // === 阶段 4:新 Engine 从检查点恢复 ===
        let new_bus = EventBus::new();
        let mut new_rx = new_bus.subscribe();
        let restored_engine = QuestEngine::with_checkpoints(
            new_bus.clone(),
            QuestConfig::default(),
            checkpoint_dir.clone(),
        );
        let restored_quest = restored_engine
            .restore_from_checkpoint(&quest.quest_id)
            .await
            .expect("检查点恢复失败");

        // 验证:恢复后 task-0=Completed,task-1/task-2=Pending
        assert_eq!(
            restored_quest.tasks[0].status,
            TaskStatus::Completed,
            "恢复后 task-0 应为 Completed"
        );
        assert_eq!(
            restored_quest.tasks[1].status,
            TaskStatus::Pending,
            "恢复后 task-1 应为 Pending"
        );

        // 验证:CheckpointLoaded 事件已发布
        let restore_events = drain_events(&mut new_rx, 5).await;
        assert!(
            count_event_by_type(&restore_events, "CheckpointLoaded") >= 1,
            "应发布 CheckpointLoaded 事件"
        );

        // === 阶段 5:恢复后继续推进剩余 Task 至完成 ===
        // task-1:Pending → Running → Completed
        restored_engine
            .update_task_status(&quest.quest_id, "task-1", TaskStatus::Running)
            .await
            .expect("恢复后 task-1 Running 失败");
        restored_engine
            .update_task_status(&quest.quest_id, "task-1", TaskStatus::Completed)
            .await
            .expect("恢复后 task-1 Completed 失败");

        // task-2:Pending → Running → Completed
        // 当 task-2 完成时,所有 Task 达终态,自动触发 complete_quest → ExecutionCompleted
        restored_engine
            .update_task_status(&quest.quest_id, "task-2", TaskStatus::Running)
            .await
            .expect("恢复后 task-2 Running 失败");
        restored_engine
            .update_task_status(&quest.quest_id, "task-2", TaskStatus::Completed)
            .await
            .expect("恢复后 task-2 Completed 失败");

        // === 阶段 6:验证终态 ===
        // 排空事件,断言 ExecutionCompleted 已发布(所有 Task 达终态时自动触发)
        let final_events = drain_events(&mut new_rx, 20).await;
        let progress_count = count_event_by_type(&final_events, "QuestProgressUpdated");
        let completed_count = count_event_by_type(&final_events, "ExecutionCompleted");
        assert!(
            progress_count >= 2,
            "应至少发布 2 个 QuestProgressUpdated 事件(task-1 + task-2)"
        );
        assert_eq!(
            completed_count, 1,
            "应发布 1 个 ExecutionCompleted 事件,实际 {completed_count}"
        );

        // 验证:Quest 的所有 Task 已完成
        let final_quest = restored_engine
            .get_quest(&quest.quest_id)
            .expect("恢复后应能获取 Quest");
        assert!(
            final_quest
                .tasks
                .iter()
                .all(|t| t.status == TaskStatus::Completed),
            "所有 Task 应为 Completed"
        );
    });
}
