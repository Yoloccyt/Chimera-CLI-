//! Quest Engine 集成测试 — 覆盖任务分解、生命周期事件、DAG 校验与并发
//!
//! 测试矩阵:
//! 1. 4 步任务图分解 + 性能(<100ms)
//! 2. QuestCreated 事件发布
//! 3. QuestProgressUpdated 事件发布
//! 4. ExecutionCompleted 事件发布(所有 Task 达终态自动触发)
//! 5. DAG 循环依赖检测
//! 6. 拓扑排序正确性(菱形依赖)
//! 7. 并发 Quest 创建(10 个并发,无冲突)
//! 8. ThinkingModeSwitched 事件发布
//! 9. 状态转换校验(合法/非法)

use std::time::{Duration, Instant};

use event_bus::{EventBus, NexusEvent};
use nexus_core::{MultimodalInput, TaskStatus, ThinkingMode, UserIntent};
use quest_engine::{QuestConfig, QuestEngine};
use tokio::task::JoinSet;

/// 构造测试用 UserIntent
fn make_intent(id: &str, text: &str) -> UserIntent {
    UserIntent {
        intent_id: id.into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

#[tokio::test]
async fn test_4_step_task_decomposition() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);
    let intent = make_intent(
        "i-1",
        "第一步分析需求。第二步设计方案。第三步实现代码。第四步测试验证。",
    );
    let start = Instant::now();
    let quest = engine.create_quest(intent).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(quest.tasks.len(), 4, "应分解为 4 个 Task");
    assert!(
        elapsed.as_millis() < 100,
        "分解耗时 {:?} 超过 100ms",
        elapsed
    );

    // 验证线性依赖链:task-0 无依赖,task-i 依赖 task-{i-1}
    assert!(quest.tasks[0].dependencies.is_empty());
    for i in 1..quest.tasks.len() {
        assert_eq!(
            quest.tasks[i].dependencies,
            vec![quest.tasks[i - 1].task_id.clone()],
            "task-{i} 应依赖 task-{}",
            i - 1
        );
    }

    // 验证所有 Task 初始状态为 Pending
    for task in &quest.tasks {
        assert_eq!(task.status, TaskStatus::Pending);
    }
}

#[tokio::test]
async fn test_quest_created_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 接收事件(带超时,防止死锁)
    let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("接收事件超时")
        .expect("接收事件失败");

    match event {
        NexusEvent::QuestCreated {
            quest_id,
            title,
            task_count,
            ..
        } => {
            assert_eq!(quest_id, quest.quest_id);
            assert_eq!(task_count, 2);
            assert!(!title.is_empty());
        }
        other => panic!("期望 QuestCreated 事件,实际收到 {other:?}"),
    }
}

#[tokio::test]
async fn test_quest_progress_updated_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。第二步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated 事件
    let _created = rx.recv().await.unwrap();

    // 更新第一个 Task:Pending → Running
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .unwrap();

    // 接收 Running 状态的 ProgressUpdated 事件
    let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("接收事件超时")
        .expect("接收事件失败");

    match event {
        NexusEvent::QuestProgressUpdated {
            quest_id,
            completed,
            total,
            ..
        } => {
            assert_eq!(quest_id, quest.quest_id);
            assert_eq!(total, 2);
            assert_eq!(completed, 0); // Running 不算 completed
        }
        other => panic!("期望 QuestProgressUpdated 事件,实际收到 {other:?}"),
    }
}

#[tokio::test]
async fn test_execution_completed_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。第二步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated 事件
    let _created = rx.recv().await.unwrap();

    // 完成 task-0:Pending → Running → Completed
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // ProgressUpdated (Running)

    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // ProgressUpdated (Completed=1)

    // 完成 task-1:Pending → Running → Completed
    engine
        .update_task_status(&quest.quest_id, "task-1", TaskStatus::Running)
        .await
        .unwrap();
    let _ = rx.recv().await.unwrap(); // ProgressUpdated (Running, completed=1)

    engine
        .update_task_status(&quest.quest_id, "task-1", TaskStatus::Completed)
        .await
        .unwrap();

    // 此时应收到两条事件:ProgressUpdated(completed=2) + ExecutionCompleted
    let progress_event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("接收 ProgressUpdated 超时")
        .expect("接收 ProgressUpdated 失败");
    match progress_event {
        NexusEvent::QuestProgressUpdated {
            completed, total, ..
        } => {
            assert_eq!(completed, 2);
            assert_eq!(total, 2);
        }
        other => panic!("期望 QuestProgressUpdated 事件,实际收到 {other:?}"),
    }

    let complete_event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("接收 ExecutionCompleted 超时")
        .expect("接收 ExecutionCompleted 失败");
    match complete_event {
        NexusEvent::ExecutionCompleted { quest_id, .. } => {
            assert_eq!(quest_id, quest.quest_id);
        }
        other => panic!("期望 ExecutionCompleted 事件,实际收到 {other:?}"),
    }
}

#[tokio::test]
async fn test_dag_cyclic_dependency_detected() {
    use nexus_core::Task;
    use quest_engine::QuestError;

    // 构造循环依赖:a → b → c → a
    let tasks = vec![
        Task {
            task_id: "a".into(),
            description: "a".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["c".into()],
        },
        Task {
            task_id: "b".into(),
            description: "b".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["a".into()],
        },
        Task {
            task_id: "c".into(),
            description: "c".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["b".into()],
        },
    ];

    let result = quest_engine::dag::validate_dag(&tasks);
    assert!(
        matches!(result, Err(QuestError::CyclicDependency)),
        "应检测到循环依赖,实际: {result:?}"
    );
}

#[tokio::test]
async fn test_topological_order_correctness() {
    use nexus_core::Task;

    // 菱形依赖:a → b, a → c, b → d, c → d
    let tasks = vec![
        Task {
            task_id: "a".into(),
            description: "a".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        },
        Task {
            task_id: "b".into(),
            description: "b".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["a".into()],
        },
        Task {
            task_id: "c".into(),
            description: "c".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["a".into()],
        },
        Task {
            task_id: "d".into(),
            description: "d".into(),
            status: TaskStatus::Pending,
            dependencies: vec!["b".into(), "c".into()],
        },
    ];

    let order = quest_engine::dag::topological_order(&tasks).unwrap();
    assert_eq!(order.len(), 4);

    // 验证拓扑序:a 必须在 b/c 之前,b/c 必须在 d 之前
    let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
    assert!(pos("a") < pos("b"));
    assert!(pos("a") < pos("c"));
    assert!(pos("b") < pos("d"));
    assert!(pos("c") < pos("d"));
}

#[tokio::test]
async fn test_concurrent_quest_creation() {
    let bus = EventBus::new();
    let engine = std::sync::Arc::new(QuestEngine::new(bus));

    // 10 个并发 create_quest,使用 JoinSet 管理任务
    let mut set: JoinSet<Result<nexus_core::Quest, quest_engine::QuestError>> = JoinSet::new();
    for i in 0..10 {
        let engine = engine.clone();
        set.spawn(async move {
            let intent = make_intent(&format!("i-{i}"), &format!("任务{i}。任务{i}b。"));
            engine.create_quest(intent).await
        });
    }

    // 收集所有结果
    let mut quest_ids = Vec::new();
    while let Some(result) = set.join_next().await {
        let quest = result.expect("task panicked").expect("create_quest failed");
        quest_ids.push(quest.quest_id);
    }

    // 验证所有 quest_id 唯一(无冲突)
    let unique_count = quest_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(unique_count, 10, "10 个并发 Quest 应有 10 个唯一 ID");

    // 验证引擎内注册了 10 个 Quest
    assert_eq!(engine.list_quests().len(), 10);
}

#[tokio::test]
async fn test_thinking_mode_switched_event_published() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "分析需求。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 跳过 QuestCreated 事件
    let _created = rx.recv().await.unwrap();

    // 切换到 Deep 模式
    engine
        .switch_thinking_mode(&quest.quest_id, ThinkingMode::Deep)
        .await
        .unwrap();

    let event = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("接收事件超时")
        .expect("接收事件失败");

    match event {
        NexusEvent::ThinkingModeSwitched {
            quest_id,
            from_mode,
            to_mode,
            ..
        } => {
            assert_eq!(quest_id, quest.quest_id);
            assert_eq!(from_mode, "Standard");
            assert_eq!(to_mode, "Deep");
        }
        other => panic!("期望 ThinkingModeSwitched 事件,实际收到 {other:?}"),
    }

    // 验证 Quest 内部状态已更新
    let updated = engine.get_quest(&quest.quest_id).unwrap();
    assert_eq!(updated.thinking_mode, ThinkingMode::Deep);
}

#[tokio::test]
async fn test_status_transition_legal() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // Pending → Running(合法)
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .expect("Pending→Running 应为合法转换");

    // Running → Completed(合法)
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
        .await
        .expect("Running→Completed 应为合法转换");
}

#[tokio::test]
async fn test_status_transition_illegal_returns_error() {
    use quest_engine::QuestError;

    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。第二步。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 先将 task-0 设为 Running
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .unwrap();

    // Running → Pending(非法)
    let result = engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Pending)
        .await;
    assert!(
        matches!(result, Err(QuestError::InvalidStatus(_))),
        "Running→Pending 应返回 InvalidStatus,实际: {result:?}"
    );

    // Pending → Completed(非法,跳过 Running)
    let result = engine
        .update_task_status(&quest.quest_id, "task-1", TaskStatus::Completed)
        .await;
    assert!(
        matches!(result, Err(QuestError::InvalidStatus(_))),
        "Pending→Completed 应返回 InvalidStatus,实际: {result:?}"
    );
}

#[tokio::test]
async fn test_quest_not_found_error() {
    use quest_engine::QuestError;

    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let result = engine
        .update_task_status("nonexistent", "task-0", TaskStatus::Running)
        .await;
    assert!(
        matches!(result, Err(QuestError::QuestNotFound(_))),
        "应返回 QuestNotFound,实际: {result:?}"
    );
}

#[tokio::test]
async fn test_task_not_found_error() {
    use quest_engine::QuestError;

    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "第一步。");
    let quest = engine.create_quest(intent).await.unwrap();

    let result = engine
        .update_task_status(&quest.quest_id, "nonexistent", TaskStatus::Running)
        .await;
    assert!(
        matches!(result, Err(QuestError::TaskNotFound(_))),
        "应返回 TaskNotFound,实际: {result:?}"
    );
}

#[tokio::test]
async fn test_get_and_list_quests() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent1 = make_intent("i-1", "第一步。");
    let quest1 = engine.create_quest(intent1).await.unwrap();

    let intent2 = make_intent("i-2", "第二步。");
    let _quest2 = engine.create_quest(intent2).await.unwrap();

    // get_quest
    let retrieved = engine.get_quest(&quest1.quest_id).unwrap();
    assert_eq!(retrieved.quest_id, quest1.quest_id);

    // 不存在的 Quest
    assert!(engine.get_quest("nonexistent").is_none());

    // list_quests
    let all = engine.list_quests();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_with_custom_config() {
    let bus = EventBus::new();
    let config = QuestConfig::new(2, 1);
    let engine = QuestEngine::with_config(bus, config);

    // 5 个句子,但 max_tasks_per_quest=2,应截断到 2
    let intent = make_intent("i-1", "一。二。三。四。五。");
    let quest = engine.create_quest(intent).await.unwrap();
    assert_eq!(quest.tasks.len(), 2);
}

#[tokio::test]
async fn test_empty_intent_creates_placeholder_task() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("i-1", "");
    let quest = engine.create_quest(intent).await.unwrap();
    assert_eq!(quest.tasks.len(), 1);
    assert_eq!(quest.tasks[0].task_id, "task-0");
}
