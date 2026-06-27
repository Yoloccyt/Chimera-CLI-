//! 端到端集成测试 — Week 2 验收门禁(Task 8 SubTask 8.1)
//!
//! 覆盖完整数据流:
//! 用户输入 → Quest 创建 → 任务分解 → 模型路由 → 检查点保存 → Wiki 沉淀
//!
//! # 验证目标
//! - 全流程无 panic、无孤儿调用、无事件丢失
//! - 任务分解耗时 < 1s
//! - 检查点可保存可恢复(LHQP 崩溃恢复语义)
//! - Wiki 条目可生成可检索(SQLite 持久化 + 向量 KNN)
//! - 性能基准:Wiki 生成 < 2s,向量检索 < 50ms
//!
//! # 架构红线对齐
//! - 跨层通信仅通过 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,测试代码允许 unwrap()
//! - 遵循 `#![forbid(unsafe_code)]`

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use event_bus::{EventBus, NexusEvent};
use model_router::{
    CacrConfig, ModelRegistry, ModelRouter, RouterConfig, RoutingRequest, RoutingStrategy,
};
use nexus_core::{MultimodalInput, TaskStatus, ThinkingMode, UserIntent};
use quest_engine::{QuestConfig, QuestEngine};
use repo_wiki::{Layer, VectorIndex, WikiGenerator, WikiStore};
use tempfile::TempDir;

/// 构造测试用 UserIntent — 三句话对应三个 Task,模拟"分析→设计→实现"工作流
fn make_intent() -> UserIntent {
    UserIntent {
        intent_id: "i-e2e-1".into(),
        raw_text: "分析需求。设计方案。实现代码。".into(),
        multimodal_inputs: vec![MultimodalInput::Text("用户输入示例".into())],
        risk_level: 30,
    }
}

/// 完整流程端到端测试 — 覆盖 Quest → 路由 → 检查点 → Wiki 全链路
///
/// 验证点:
/// 1. Quest 创建并分解为 3 个 Task(线性依赖链)
/// 2. ModelRouter 路由成功并发布 ModelRouteSelected 事件
/// 3. Task 状态推进触发 QuestProgressUpdated 事件
/// 4. 检查点保存触发 CheckpointSaved 事件
/// 5. WikiGenerator 从已完成 Task 生成 Wiki 条目
/// 6. WikiStore 持久化条目,VectorIndex 支持检索
/// 7. 任务分解耗时 < 1s
/// 8. Wiki 生成耗时 < 2s
/// 9. 向量检索耗时 < 50ms
#[tokio::test]
async fn test_e2e_full_pipeline_happy_path() {
    let tmp = TempDir::new().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    // ========== 阶段 1:Quest 创建与任务分解 ==========
    let engine = QuestEngine::with_checkpoints(
        bus.clone(),
        QuestConfig::default(),
        tmp.path().join("checkpoints"),
    );

    let decompose_start = Instant::now();
    let quest = engine.create_quest(make_intent()).await.unwrap();
    let decompose_elapsed = decompose_start.elapsed();

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
    // 性能基准:任务分解 < 1s
    assert!(
        decompose_elapsed < Duration::from_secs(1),
        "任务分解耗时 {decompose_elapsed:?} 超过 1s 限制"
    );

    // 验证:QuestCreated 事件已发布
    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::QuestCreated {
            quest_id,
            task_count,
            ..
        } => {
            assert_eq!(quest_id, quest.quest_id);
            assert_eq!(task_count, 3);
        }
        other => panic!("期望 QuestCreated 事件,实际收到 {other:?}"),
    }

    // ========== 阶段 2:模型路由 ==========
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = ModelRouter::new(registry, bus.clone());
    let routing_req = RoutingRequest {
        quest_id: quest.quest_id.clone(),
        intent: make_intent(),
        estimated_tokens: 1000,
        strategy: RoutingStrategy::Auto,
    };
    let decision = router.route(routing_req).await.unwrap();
    assert!(!decision.model_id.is_empty(), "路由应选中非空模型");

    // 验证:ModelRouteSelected 事件已发布
    let event = rx.recv().await.unwrap();
    assert!(
        matches!(event, NexusEvent::ModelRouteSelected { ref quest_id, .. } if quest_id == &quest.quest_id),
        "期望 ModelRouteSelected 事件,实际收到 {event:?}"
    );

    // ========== 阶段 3:Task 状态推进 ==========
    // 推进所有 Task 至 Completed,触发自动检查点与 ExecutionCompleted
    for i in 0..quest.tasks.len() {
        let task_id = format!("task-{i}");
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
            .await
            .unwrap();
    }

    // ========== 阶段 4:Wiki 沉淀 ==========
    // 重新从 engine 获取最新 Quest(Task 状态已更新为 Completed)
    let quest = engine.get_quest(&quest.quest_id).unwrap();
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    let vector_index = VectorIndex::new(512);

    let wiki_start = Instant::now();
    let entries = WikiGenerator::from_quest_result(&quest);
    let wiki_elapsed = wiki_start.elapsed();

    // 验证:3 个已完成 Task 生成 3 个 Wiki 条目
    assert_eq!(entries.len(), 3, "应生成 3 个 Wiki 条目");

    // 性能基准:Wiki 生成 < 2s
    assert!(
        wiki_elapsed < Duration::from_secs(2),
        "Wiki 生成耗时 {wiki_elapsed:?} 超过 2s 限制"
    );

    // 持久化条目并构建向量索引
    for entry in &entries {
        store.insert(entry).unwrap();
        vector_index
            .upsert(&entry.entry_id, &entry.embedding)
            .unwrap();
    }
    assert_eq!(store.count().unwrap(), 3, "WikiStore 应有 3 条记录");

    // ========== 阶段 5:向量检索性能基准 ==========
    let query = entries[0].embedding.clone();
    let search_start = Instant::now();
    let results = vector_index.search(&query, 3).unwrap();
    let search_elapsed = search_start.elapsed();

    // 性能基准:向量检索 < 50ms
    assert!(
        search_elapsed < Duration::from_millis(50),
        "向量检索耗时 {search_elapsed:?} 超过 50ms 限制"
    );

    // 验证:Top-1 应是查询向量自身(余弦相似度 = 1.0)
    assert_eq!(results.len(), 3, "应返回 3 个结果");
    assert_eq!(results[0].0, entries[0].entry_id, "Top-1 应是自身");
    assert!(
        (results[0].1 - 1.0).abs() < 1e-5,
        "自身余弦相似度应接近 1.0"
    );

    // ========== 阶段 6:全文检索验证 ==========
    let found = store.search_fulltext("分析").unwrap();
    assert!(!found.is_empty(), "全文检索'分析'应命中条目");

    // ========== 阶段 7:ISCM 跨层锚点 ==========
    let anchor = store
        .create_anchor(Layer::L9_Quest, "quest-engine", &entries[0].entry_id)
        .unwrap();
    let resolved = store.resolve_anchor(anchor.anchor_id).unwrap();
    assert_eq!(resolved.entry_id, entries[0].entry_id, "锚点应解析到原条目");
}

/// 检查点保存与恢复测试 — 验证 LHQP 崩溃恢复语义
///
/// 流程:
/// 1. 创建 Quest 并完成部分 Task
/// 2. 保存检查点(显式调用 save_checkpoint)
/// 3. 模拟崩溃:丢弃原 Engine,新建 Engine 指向同一检查点目录
/// 4. 从检查点恢复 Quest
/// 5. 验证恢复后的 Quest 状态与保存时一致
#[tokio::test]
async fn test_e2e_checkpoint_save_and_restore() {
    let tmp = TempDir::new().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let checkpoint_dir = tmp.path().join("checkpoints");

    // 阶段 1:创建 Quest 并完成首个 Task
    let engine =
        QuestEngine::with_checkpoints(bus.clone(), QuestConfig::default(), checkpoint_dir.clone());
    let quest = engine.create_quest(make_intent()).await.unwrap();
    // 消费 QuestCreated 事件
    let _ = rx.recv().await.unwrap();

    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Running)
        .await
        .unwrap();
    engine
        .update_task_status(&quest.quest_id, "task-0", TaskStatus::Completed)
        .await
        .unwrap();

    // 阶段 2:显式保存检查点
    let checkpoint = engine.save_checkpoint(&quest.quest_id).await.unwrap();
    assert_eq!(checkpoint.quest_id, quest.quest_id);
    assert!(!checkpoint.serialized_state.is_empty());
    assert!(!checkpoint.memory_snapshot_hash.is_empty());

    // 验证:CheckpointSaved 事件已发布(Critical 事件,不可丢失)
    let mut got_checkpoint_saved = false;
    // try_recv 返回 Result<Option<NexusEvent>, EventBusError>,Ok(None) 表示无事件
    while let Ok(Some(event)) = rx.try_recv() {
        if matches!(event, NexusEvent::CheckpointSaved { .. }) {
            got_checkpoint_saved = true;
            break;
        }
    }
    assert!(got_checkpoint_saved, "应收到 CheckpointSaved 事件");

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
        .unwrap();

    // 阶段 5:验证恢复后的状态
    assert_eq!(restored_quest.quest_id, quest.quest_id, "quest_id 应一致");
    assert_eq!(restored_quest.tasks.len(), 3, "Task 数量应一致");
    // task-0 应为 Completed(保存检查点时已完成)
    assert_eq!(
        restored_quest.tasks[0].status,
        TaskStatus::Completed,
        "task-0 应为 Completed"
    );
    // task-1、task-2 应仍为 Pending
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
}

/// Wiki 生成与检索测试 — 验证知识沉淀可生成可检索
///
/// 流程:
/// 1. 创建 Quest 并完成所有 Task
/// 2. 使用 WikiGenerator 生成 Wiki 条目
/// 3. 持久化到 WikiStore
/// 4. 验证可按 tag 检索
/// 5. 验证可按全文检索
/// 6. 验证向量检索返回正确排序
#[tokio::test]
async fn test_e2e_wiki_generation_and_retrieval() {
    let tmp = TempDir::new().unwrap();
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 阶段 1:创建 Quest 并完成所有 Task
    let quest = engine.create_quest(make_intent()).await.unwrap();
    for i in 0..quest.tasks.len() {
        let task_id = format!("task-{i}");
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
            .await
            .unwrap();
    }

    // 阶段 2:生成 Wiki 条目
    // 重新从 engine 获取最新 Quest(Task 状态已更新为 Completed)
    let quest = engine.get_quest(&quest.quest_id).unwrap();
    let entries = WikiGenerator::from_quest_result(&quest);
    assert_eq!(entries.len(), 3, "应生成 3 个 Wiki 条目");

    // 阶段 3:持久化
    let store = WikiStore::open(&tmp.path().join("wiki.db")).unwrap();
    let vector_index = VectorIndex::new(512);
    for entry in &entries {
        store.insert(entry).unwrap();
        vector_index
            .upsert(&entry.entry_id, &entry.embedding)
            .unwrap();
    }

    // 阶段 4:按 tag 检索(WikiGenerator 生成的条目含 "quest" 与 quest_id 两个 tag)
    let by_tag = store.list_by_tag("quest").unwrap();
    assert_eq!(by_tag.len(), 3, "按 'quest' tag 应检索到 3 条");

    let by_quest_tag = store.list_by_tag(&quest.quest_id).unwrap();
    assert_eq!(by_quest_tag.len(), 3, "按 quest_id tag 应检索到 3 条");

    // 阶段 5:全文检索
    let by_text = store.search_fulltext("分析").unwrap();
    assert!(!by_text.is_empty(), "全文检索 '分析' 应命中");

    // 阶段 6:向量检索排序验证
    let query = entries[1].embedding.clone();
    let results = vector_index.search(&query, 3).unwrap();
    assert_eq!(results.len(), 3, "应返回 3 个结果");
    // Top-1 应是 entries[1](查询向量自身)
    assert_eq!(results[0].0, entries[1].entry_id, "Top-1 应是查询自身");
    assert!((results[0].1 - 1.0).abs() < 1e-5, "自身相似度应接近 1.0");
}

/// 无孤儿事件测试 — 验证全流程发布的事件均被订阅者接收
///
/// 架构红线:所有异步操作必须有 GQEP 聚集/超时处理,避免孤儿调用。
/// 本测试通过订阅 EventBus,收集全流程事件,验证关键事件类型均被接收。
#[tokio::test]
async fn test_e2e_no_orphan_events() {
    let tmp = TempDir::new().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let engine = QuestEngine::with_checkpoints(
        bus.clone(),
        QuestConfig::default(),
        tmp.path().join("checkpoints"),
    );
    let quest = engine.create_quest(make_intent()).await.unwrap();

    // 路由
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = ModelRouter::new(registry, bus.clone());
    let routing_req = RoutingRequest {
        quest_id: quest.quest_id.clone(),
        intent: make_intent(),
        estimated_tokens: 1000,
        strategy: RoutingStrategy::Lite,
    };
    router.route(routing_req).await.unwrap();

    // 推进所有 Task 至 Completed
    for i in 0..quest.tasks.len() {
        let task_id = format!("task-{i}");
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
            .await
            .unwrap();
    }

    // 收集所有事件(给一点时间让最后的事件投递完成)
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut got_quest_created = false;
    let mut got_model_route_selected = false;
    let mut got_quest_progress = false;
    let mut got_execution_completed = false;

    // try_recv 返回 Result<Option<NexusEvent>, EventBusError>,Ok(None) 表示无事件
    while let Ok(Some(event)) = rx.try_recv() {
        match event {
            NexusEvent::QuestCreated { .. } => got_quest_created = true,
            NexusEvent::ModelRouteSelected { .. } => got_model_route_selected = true,
            NexusEvent::QuestProgressUpdated { .. } => got_quest_progress = true,
            NexusEvent::ExecutionCompleted { .. } => got_execution_completed = true,
            _ => {}
        }
    }

    assert!(got_quest_created, "应收到 QuestCreated 事件");
    assert!(got_model_route_selected, "应收到 ModelRouteSelected 事件");
    assert!(got_quest_progress, "应收到 QuestProgressUpdated 事件");
    assert!(got_execution_completed, "应收到 ExecutionCompleted 事件");
}

/// CACR 集成测试 — 验证启用 CACR 守卫后正常路由走 Allow 路径
///
/// 默认 CacrConfig 预算充足(1_000_000 美分),正常路由应被放行。
#[tokio::test]
async fn test_e2e_cacr_allow_path() {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = ModelRouter::with_cacr(registry, bus.clone(), CacrConfig::default());

    let req = RoutingRequest {
        quest_id: "q-cacr-test".into(),
        intent: make_intent(),
        estimated_tokens: 1000,
        strategy: RoutingStrategy::Lite,
    };
    let decision = router.route(req).await.unwrap();
    assert!(!decision.model_id.is_empty(), "CACR Allow 应放行路由");
    // route_reason 不应包含 CACR Downgrade 标识
    assert!(
        !decision.route_reason.contains("CACR Downgrade"),
        "正常路由不应触发降级"
    );
}

/// 思考模式切换测试 — 验证 TTG(Thinking Toggle Governance)事件广播
///
/// 切换思考模式应发布 ThinkingModeSwitched 事件,供 Parliament 调整预算。
#[tokio::test]
async fn test_e2e_thinking_mode_switch() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus.clone());

    let quest = engine.create_quest(make_intent()).await.unwrap();
    // 消费 QuestCreated 事件
    let _ = rx.recv().await.unwrap();

    // 切换到 Deep 模式
    engine
        .switch_thinking_mode(&quest.quest_id, ThinkingMode::Deep)
        .await
        .unwrap();

    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::ThinkingModeSwitched {
            from_mode, to_mode, ..
        } => {
            assert_eq!(from_mode, "Standard", "源模式应为 Standard");
            assert_eq!(to_mode, "Deep", "目标模式应为 Deep");
        }
        other => panic!("期望 ThinkingModeSwitched 事件,实际收到 {other:?}"),
    }

    // 验证 Quest 内部状态已更新
    let updated = engine.get_quest(&quest.quest_id).unwrap();
    assert_eq!(
        updated.thinking_mode,
        ThinkingMode::Deep,
        "思考模式应已切换为 Deep"
    );
}

/// 性能基准综合测试 — 集中验证三个性能指标
///
/// - 任务分解 < 1s
/// - Wiki 生成 < 2s
/// - 向量检索 < 50ms
#[tokio::test]
async fn test_e2e_performance_benchmarks() {
    let tmp = TempDir::new().unwrap();
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 基准 1:任务分解 < 1s
    let start = Instant::now();
    let quest = engine.create_quest(make_intent()).await.unwrap();
    let decompose_elapsed = start.elapsed();
    assert!(
        decompose_elapsed < Duration::from_secs(1),
        "任务分解耗时 {decompose_elapsed:?} 超过 1s"
    );

    // 完成所有 Task 以便生成 Wiki
    for i in 0..quest.tasks.len() {
        let task_id = format!("task-{i}");
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&quest.quest_id, &task_id, TaskStatus::Completed)
            .await
            .unwrap();
    }

    // 基准 2:Wiki 生成 < 2s
    // 重新从 engine 获取最新 Quest(Task 状态已更新为 Completed)
    let quest = engine.get_quest(&quest.quest_id).unwrap();
    let start = Instant::now();
    let entries = WikiGenerator::from_quest_result(&quest);
    let wiki_elapsed = start.elapsed();
    assert!(
        wiki_elapsed < Duration::from_secs(2),
        "Wiki 生成耗时 {wiki_elapsed:?} 超过 2s"
    );

    // 构建向量索引
    let vector_index = VectorIndex::new(512);
    for entry in &entries {
        vector_index
            .upsert(&entry.entry_id, &entry.embedding)
            .unwrap();
    }

    // 基准 3:向量检索 < 50ms
    let query = entries[0].embedding.clone();
    let start = Instant::now();
    let _ = vector_index.search(&query, 3).unwrap();
    let search_elapsed = start.elapsed();
    assert!(
        search_elapsed < Duration::from_millis(50),
        "向量检索耗时 {search_elapsed:?} 超过 50ms"
    );

    // 额外验证:WikiStore 持久化可用
    let store = WikiStore::open(&tmp.path().join("bench.db")).unwrap();
    for entry in &entries {
        store.insert(entry).unwrap();
    }
    assert_eq!(store.count().unwrap(), 3, "WikiStore 应持久化 3 条记录");
}
