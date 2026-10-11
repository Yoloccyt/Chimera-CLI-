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

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
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

/// 生产路径不变量:`create_quest` 产出的 `quest.tasks` 必满足依赖先行
///
/// WHY 断在集成层而非单测层:本批接线的目的就是把"Vec 序恰好等于依赖序"
/// 从 `decompose` 的巧合升为**生产不变量**,只有走真实入口才能证明接线
/// 真的生效。下游多个消费者直接按 Vec 序取用(commands/quest.rs 编号
/// 任务列表、ttg.rs::compute_dependency_depth、trajectory_exporter.rs)。
#[tokio::test]
async fn test_create_quest_tasks_are_topologically_ordered() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);
    let quest = engine
        .create_quest(make_intent(
            "i-topo-1",
            "采集需求。设计架构。实现代码。集成联调。回归验证。",
        ))
        .await
        .unwrap();

    let ids: Vec<String> = quest.tasks.iter().map(|t| t.task_id.clone()).collect();
    // 1. 实体序逐项等于 topological_order 的 id 序(同根,不得养出第二套定序语义)
    assert_eq!(
        ids,
        quest_engine::dag::topological_order(&quest.tasks).unwrap(),
        "生产路径的 tasks 序必须等于拓扑序"
    );

    // 2. 依赖先行:每个 task 的全部依赖都排在其之前
    let pos = |id: &str| ids.iter().position(|x| x == id).unwrap();
    for task in &quest.tasks {
        for dep in &task.dependencies {
            assert!(
                pos(dep) < pos(&task.task_id),
                "依赖 {dep} 必须先于 {}",
                task.task_id
            );
        }
    }
    assert_eq!(quest.tasks.len(), 5, "5 句应分解为 5 个 Task");
}

/// I1 逐位等价回归闸:接线不得改变现有唯一生产者的输出
///
/// WHY 这是本批最关键的一条回归护栏:`decompose` 产出按句子序的线性链,
/// 其拓扑序恒等于句子序,故定序接入后输出应**逐位不变**(描述与 id 的
/// 配对也不得被打乱)。若本测试转红,说明定序把生产路径改坏了。
#[tokio::test]
async fn test_create_quest_preserves_decompose_sentence_order() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);
    let quest = engine
        .create_quest(make_intent("i-topo-2", "甲步。乙步。丙步。"))
        .await
        .unwrap();

    let ids: Vec<&str> = quest.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["task-0", "task-1", "task-2"],
        "线性链定序后必须仍按句子序(逐位不变)"
    );
    // 描述与句子的配对关系不得随定序飘移
    let descs: Vec<&str> = quest.tasks.iter().map(|t| t.description.as_str()).collect();
    assert_eq!(
        descs,
        vec!["甲步", "乙步", "丙步"],
        "任务描述必须与句子序一致"
    );
}

/// 公开 API 层:乱序输入必被纠正 —— 证明定序不是恒等映射
///
/// WHY 单独立一条:上面两条走生产入口,输入序恰好等于依赖序,定序在此
/// 是恒等映射(只能证"不改坏")。本条用故意打乱的菱形依赖证"能改对",
/// 堵住 HashMap drain 错序/丢任务这类真 bug。
#[tokio::test]
async fn test_sort_tasks_topologically_fixes_shuffled_input() {
    use nexus_core::Task;

    let task = |id: &str, deps: Vec<&str>| Task {
        task_id: id.into(),
        description: id.into(),
        status: TaskStatus::Pending,
        dependencies: deps.into_iter().map(String::from).collect(),
    };
    // 输入序与依赖序故意不一致:d 排在最前,a 排在中间
    let shuffled = vec![
        task("d", vec!["b", "c"]),
        task("b", vec!["a"]),
        task("a", vec![]),
        task("c", vec!["a"]),
    ];

    let sorted = quest_engine::dag::sort_tasks_topologically(shuffled).unwrap();
    let ids: Vec<&str> = sorted.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a", "b", "c", "d"],
        "乱序输入必须被重排为依赖先行(同层 b/c 按字典序)"
    );
}

/// create_quest 的双重守卫不得被绕过:成环与悬空依赖均拒
///
/// 取证边界(为何不直接走 `create_quest`):本会话实测 `decompose()` 是
/// quest-engine 内**唯一**任务生产者(`engine.rs:412`,无接受外部 `Vec<Task>`
/// 的公开 API),且它恒产 `task-{idx}` 线性链——经公开入口无法注入环。
/// 故本测试在 `create_quest` 所使用的**同一双闸公开 API 层**断言两重守卫
/// 语义一致且都不被绕过;若未来引入非线序生产者(checkpoint 恢复、
/// agent 提议任务),本测试即升级为生产入口的直接守卫。
///
/// 守的是两个具体退化:(a) 把定序改成"静默忽略环"(则依赖环到执行期才爆);
/// (b) 把 `validate_dag` 从 `create_quest` 删掉而定序又不判环。
#[tokio::test]
async fn test_create_quest_dag_guards_reject_cyclic_and_dangling() {
    use nexus_core::Task;
    use quest_engine::QuestError;

    let task = |id: &str, deps: Vec<&str>| Task {
        task_id: id.into(),
        description: id.into(),
        status: TaskStatus::Pending,
        dependencies: deps.into_iter().map(String::from).collect(),
    };

    // 1. 成环输入:两重守卫必须给出同一个 `CyclicDependency`(与 create_quest 中的调用顺序同序)
    let cyclic = vec![
        task("a", vec!["c"]),
        task("b", vec!["a"]),
        task("c", vec!["b"]),
    ];
    let v = quest_engine::dag::validate_dag(&cyclic);
    let s = quest_engine::dag::sort_tasks_topologically(cyclic.clone());
    assert!(
        matches!(v, Err(QuestError::CyclicDependency)),
        "validate_dag 应判环,实际: {v:?}"
    );
    assert!(
        matches!(s, Err(QuestError::CyclicDependency)),
        "定序应判环而非静默返回乱序表(绕过校验),实际: {s:?}"
    );

    // 2. 悬空依赖:两重守卫均拒(防止执行期找不到前置任务)
    let dangling = vec![task("a", vec![]), task("b", vec!["ghost"])];
    assert!(matches!(
        quest_engine::dag::validate_dag(&dangling),
        Err(QuestError::DecompositionFailed(_))
    ));
    assert!(matches!(
        quest_engine::dag::sort_tasks_topologically(dangling),
        Err(QuestError::DecompositionFailed(_))
    ));

    // 3. 生产入口自身仍可用且输出合法(证明未把守卫改坏到拒掉正常输入)
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);
    let quest = engine
        .create_quest(make_intent("i-guard-1", "甲。乙。丙。"))
        .await
        .unwrap();
    assert_eq!(quest.tasks.len(), 3, "正常输入应仍由 create_quest 接受");
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
