# Tasks — Week 2 Quest + Wiki + 模型路由实现

本任务清单对齐 §7 Week 2 推进计划(Day 8-14),按优先级 P1(关键路径功能)推进。
所有任务依赖 Week 1 已稳定的 `event-bus`/`seccore`/`decay-engine`/`qeep-protocol`/`chimera-cli`。

> 状态说明:`[x]` 已完成 · `[ ]` 待执行 · `[~]` 代码完成待验证
> 优先级:全部 P1(Week 2 验收未通过则不进入 Week 3)
> 责任人映射:参照 `establish-elite-collaboration-team` spec 的 6 类专家子代理

---

## Task 1 (Day 8): Nexus Core 核心领域类型 — 全局类型基础

> 责任人:架构专家(主导)+ 实现专家(协助 CLV 实现)
> 优先级:P1(被 Task 2/3/4 依赖,必须最先完成)
> 代码目标:定义 `NexusState`/`UserIntent`/`CLV`/`Quest`/`Task`/`Checkpoint` 核心类型
> 测试目标:类型可序列化/反序列化、CLV 余弦相似度正确、NexusState 线程安全
> 验收标准:`cargo test -p nexus-core` 通过、`cargo clippy -p nexus-core -- -D warnings` 零警告

- [x] SubTask 1.1: 创建 `crates/nexus-core/src/types.rs` — 定义 `UserIntent`(含 `intent_id`/`raw_text`/`multimodal_inputs`/`risk_level`)、`Quest`(含 `quest_id`/`title`/`tasks`/`thinking_mode`/`checkpoint_id`)、`Task`(含 `task_id`/`description`/`status`/`dependencies`)、`Checkpoint`(含 `quest_id`/`checkpoint_id`/`memory_snapshot_hash`/`serialized_state`)
  - 所有类型派生 `Debug`/`Clone`/`Serialize`/`Deserialize`/`PartialEq`
  - `TaskStatus` 枚举:`Pending`/`Running`/`Completed`/`Failed`
  - `ThinkingMode` 枚举:`Fast`/`Standard`/`Deep`
- [x] SubTask 1.2: 创建 `crates/nexus-core/src/clv.rs` — 实现 `CLV` 类型(512-dim `ndarray::Array1<f32>`)
  - `CLV::zero() -> Self`:全零向量
  - `CLV::from_vec(Vec<f32>) -> Result<Self, NexusError>`:维度校验(必须 512)
  - `CLV::cosine_similarity(&self, other: &Self) -> f32`:余弦相似度,处理零向量边界
  - `CLV::dimension() -> usize`:返回 512
- [x] SubTask 1.3: 创建 `crates/nexus-core/src/state.rs` — 实现 `NexusState`(含 `active_quests`/`global_budget`/`model_registry_snapshot`)
  - 使用 `Arc<RwLock<NexusStateInner>>` 实现线程安全
  - `NexusState::new() -> Self`
  - `NexusState::register_quest(&self, quest: Quest) -> Result<()>`
  - `NexusState::update_quest_progress(&self, quest_id: &str, completed: u32, total: u32) -> Result<()>`
  - `NexusState::snapshot_hash(&self) -> String`:SHA-256 哈希,用于 `NexusStateChanged` 事件
- [x] SubTask 1.4: 创建 `crates/nexus-core/src/error.rs` — 定义 `NexusError`(thiserror enum)
  - 变体:`InvalidClvDimension`/`QuestNotFound`/`QuestAlreadyExists`/`SerializationError`/`IoError`
- [x] SubTask 1.5: 更新 `crates/nexus-core/src/lib.rs` — 公开 `pub mod types; pub mod clv; pub mod state; pub mod error;` 并 re-export 核心类型
- [x] SubTask 1.6: 更新 `crates/nexus-core/Cargo.toml` — 添加 workspace 级依赖(`ndarray`/`serde`/`serde_json`/`thiserror`/`sha2`/`hex`/`uuid`/`chrono`/`tokio`/`tracing`)
- [x] SubTask 1.7: 编写单元测试 — `crates/nexus-core/tests/types.rs`(CLV 余弦相似度、NexusState 线程安全、UserIntent 序列化往返)

---

## Task 2 (Day 8-9): Quest Engine 任务分解与生命周期

> 责任人:架构专家(主导)+ 质量专家(审查任务图校验)
> 优先级:P1(依赖 Task 1 的 `Quest`/`Task` 类型)
> 代码目标:实现 Quest 创建、任务图分解、进度追踪、完成通知
> 测试目标:4 步任务图分解 < 100ms、`QuestCreated`/`QuestProgressUpdated`/`ExecutionCompleted` 事件正确发布
> 验收标准:任务图 DAG 校验(无环)、事件发布顺序正确

- [x] SubTask 2.1: 创建 `crates/quest-engine/src/types.rs` — 定义 `QuestEngine` 内部状态(含 `quests: DashMap<QuestId, Quest>`、`event_bus: EventBus`)
- [x] SubTask 2.2: 创建 `crates/quest-engine/src/engine.rs` — 实现 `QuestEngine` 核心 API
  - `QuestEngine::new(event_bus: EventBus) -> Self`
  - `QuestEngine::create_quest(&self, intent: UserIntent) -> Result<Quest>`:从意图分解任务图
  - `QuestEngine::decompose(&self, intent: &UserIntent) -> Result<Vec<Task>>`:任务分解核心逻辑(Week 2 阶段使用规则分解:按句子切分 + 依赖关系推导)
  - `QuestEngine::update_task_status(&self, quest_id: &str, task_id: &str, status: TaskStatus) -> Result<()>`
  - `QuestEngine::complete_quest(&self, quest_id: &str) -> Result<()>`
- [x] SubTask 2.3: 创建 `crates/quest-engine/src/dag.rs` — 实现任务图 DAG 校验
  - `validate_dag(tasks: &[Task]) -> Result<(), QuestError>`:拓扑排序检测环
  - `topological_order(tasks: &[Task]) -> Result<Vec<TaskId>, QuestError>`:返回执行顺序
- [x] SubTask 2.4: 创建 `crates/quest-engine/src/error.rs` — 定义 `QuestError`(thiserror enum)
  - 变体:`CyclicDependency`/`TaskNotFound`/`QuestNotFound`/`InvalidStatus`/`DecompositionFailed`/`EventBusError`
- [x] SubTask 2.5: 创建 `crates/quest-engine/src/config.rs` — 定义 `QuestConfig`(含 `max_tasks_per_quest: u32`(默认 16)、`checkpoint_interval: u32`(默认 3))
- [x] SubTask 2.6: 更新 `crates/quest-engine/src/lib.rs` — 公开模块并 re-export
- [x] SubTask 2.7: 更新 `crates/quest-engine/Cargo.toml` — 添加依赖(`nexus-core`/`event-bus`/`qeep-protocol`/`dashmap`/`tokio`/`tracing`/`thiserror`/`serde`)
- [x] SubTask 2.8: 编写集成测试 — `crates/quest-engine/tests/engine.rs`
  - 测试 4 步任务图分解(简单意图 < 100ms)
  - 测试 `QuestCreated`/`QuestProgressUpdated`/`ExecutionCompleted` 事件发布
  - 测试 DAG 循环依赖检测
  - 测试并发 Quest 创建(10 个并发,无冲突)

---

## Task 3 (Day 9): LHQP 检查点持久化

> 责任人:实现专家(主导)+ 安全专家(审查完整性校验)
> 优先级:P1(依赖 Task 2 的 QuestEngine)
> 代码目标:实现 Checkpoint 保存/加载/恢复,崩溃恢复能力
> 测试目标:检查点可保存可恢复、SHA-256 完整性校验、保留最近 5 个检查点
> 验收标准:模拟崩溃后可从检查点恢复,不丢失已完成 Task 结果

- [x] SubTask 3.1: 创建 `crates/quest-engine/src/checkpoint.rs` — 实现 `CheckpointManager`
  - `CheckpointManager::new(checkpoint_dir: PathBuf) -> Self`
  - `CheckpointManager::save(&self, quest: &Quest) -> Result<Checkpoint>`:序列化为 MessagePack,写入 `<checkpoint_dir>/<quest_id>/<checkpoint_id>.bin`
  - `CheckpointManager::load(&self, quest_id: &str, checkpoint_id: &str) -> Result<Checkpoint>`:读取并反序列化
  - `CheckpointManager::load_latest(&self, quest_id: &str) -> Result<Option<Checkpoint>>`:加载最新检查点
  - `CheckpointManager::verify_integrity(&self, checkpoint: &Checkpoint) -> Result<()>`:SHA-256 校验
  - `CheckpointManager::prune_old(&self, quest_id: &str, keep: usize) -> Result<()>`:保留最近 N 个,删除其余
- [x] SubTask 3.2: 在 `QuestEngine` 中集成检查点触发
  - `QuestEngine::save_checkpoint(&self, quest_id: &str) -> Result<Checkpoint>`:调用 `CheckpointManager::save` 并发布 `CheckpointSaved` 事件
  - `QuestEngine::restore_from_checkpoint(&self, quest_id: &str) -> Result<Quest>`:调用 `CheckpointManager::load_latest` 并重建内存状态,发布 `CheckpointLoaded` 事件
  - 在 `update_task_status` 中每 N 个 Task 完成自动触发检查点(N 由 `QuestConfig::checkpoint_interval` 控制)
- [x] SubTask 3.3: 使用 `tokio::task::spawn_blocking` 包装同步文件 I/O,避免阻塞异步运行时
- [x] SubTask 3.4: 编写集成测试 — `crates/quest-engine/tests/checkpoint.rs`
  - 测试检查点保存与加载(往返一致性)
  - 测试 SHA-256 完整性校验(篡改文件后 `load` 返回 `QuestError::CheckpointCorrupted`)
  - 测试保留最近 5 个检查点(创建 7 个后仅保留最新 5 个)
  - 测试崩溃恢复:创建 Quest → 完成 2 个 Task → 保存检查点 → 丢弃内存状态 → 从检查点恢复 → 验证已完成 Task 状态正确

---

## Task 4 (Day 10): Repo Wiki SQLite + 向量索引

> 责任人:实现专家(主导)+ 性能专家(审查检索延迟)
> 优先级:P1(依赖 Task 1 的 CLV 类型)
> 代码目标:实现 Wiki 条目 CRUD + 向量相似度检索
> 测试目标:10 条 Wiki 生成 < 2s、向量检索 < 50ms、`WikiUpdated` 事件发布
> 验收标准:SQLite WAL 模式、sqlite-vec 集成、向量维度 512

- [x] SubTask 4.1: 创建 `crates/repo-wiki/src/types.rs` — 定义 `WikiEntry`(含 `entry_id`/`title`/`content`/`tags`/`embedding`/`created_at`/`updated_at`)、`WikiConfig`(含 `db_path`/`vector_dim`(默认 512)/`wal_enabled`(默认 true))
- [x] SubTask 4.2: 创建 `crates/repo-wiki/src/store.rs` — 实现 `WikiStore`(SQLite 持久化层)
  - `WikiStore::open(path: &Path) -> Result<Self>`:打开/创建数据库,启用 WAL,创建 `entries` 表与 `vec_entries` 虚拟表(sqlite-vec)
  - `WikiStore::insert(&self, entry: &WikiEntry) -> Result<()>`
  - `WikiStore::get(&self, entry_id: &str) -> Result<Option<WikiEntry>>`
  - `WikiStore::delete(&self, entry_id: &str) -> Result<()>`:同步删除向量索引
  - `WikiStore::list_by_tag(&self, tag: &str) -> Result<Vec<WikiEntry>>`
  - `WikiStore::search_fulltext(&self, query: &str) -> Result<Vec<WikiEntry>>`:LIKE 模糊匹配
- [x] SubTask 4.3: 创建 `crates/repo-wiki/src/vector.rs` — 实现 `VectorIndex`(sqlite-vec 检索层)
  - `VectorIndex::upsert(&self, entry_id: &str, embedding: &[f32]) -> Result<()>`
  - `VectorIndex::search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>>`:返回 `(entry_id, similarity_score)`
  - `VectorIndex::delete(&self, entry_id: &str) -> Result<()>`
- [x] SubTask 4.4: 创建 `crates/repo-wiki/src/generator.rs` — 实现 `WikiGenerator`(从 Quest 结果提取条目)
  - `WikiGenerator::from_quest_result(quest: &Quest, results: &[TaskResult]) -> Vec<WikiEntry>`
  - Week 2 阶段:嵌入向量使用内容 SHA-256 哈希扩展为 512-dim 占位向量(文档注释说明 Week 6 替换为真实 CLV)
- [x] SubTask 4.5: 创建 `crates/repo-wiki/src/error.rs` — 定义 `WikiError`(thiserror enum)
  - 变体:`DatabaseError`/`VectorIndexError`/`EntryNotFound`/`AnchorDangling`/`SerializationError`/`IoError`
- [x] SubTask 4.6: 创建 `crates/repo-wiki/src/config.rs` — 定义 `WikiConfig`
- [x] SubTask 4.7: 更新 `crates/repo-wiki/src/lib.rs` — 公开模块并 re-export `WikiStore`/`VectorIndex`/`WikiGenerator`/`WikiEntry`
- [x] SubTask 4.8: 更新 `crates/repo-wiki/Cargo.toml` — 添加依赖(`nexus-core`/`event-bus`/`rusqlite`/`sqlite-vec`/`serde`/`serde_json`/`thiserror`/`sha2`/`hex`/`uuid`/`chrono`/`tokio`/`tracing`)
- [x] SubTask 4.9: 编写集成测试 — `crates/repo-wiki/tests/store.rs`
  - 测试 10 条 Wiki 条目 CRUD(增删改查)
  - 测试向量相似度检索(Top-K 返回正确条目,延迟 < 50ms)
  - 测试 WAL 模式并发读不阻塞
  - 测试 `WikiUpdated` 事件发布(通过 mock EventBus 订阅验证)

---

## Task 5 (Day 11): ISCM 跨层共享索引

> 责任人:架构专家(主导)+ 质量专家(审查跨层一致性)
> 优先级:P1(依赖 Task 4 的 WikiStore)
> 代码目标:实现跨层共享锚点机制,确保同一知识实体在不同层间一致引用
> 测试目标:跨层一致性校验、悬空锚点检测
> 验收标准:锚点 UUIDv7 全局唯一、L9→L5→L2 跨层引用返回同一版本

- [x] SubTask 5.1: 创建 `crates/repo-wiki/src/iscm.rs` — 实现 `IscmAnchor`(跨层共享锚点)
  - `IscmAnchor` 结构体:含 `anchor_id: Uuid`(UUIDv7)、`layer: Layer`、`crate_name: String`、`entity_id: String`、`created_at: DateTime<Utc>`、`updated_at: DateTime<Utc>`
  - `Layer` 枚举:`L1_Core`/`L2_Memory`/`L3_Storage`/`L4_Security`/`L5_Knowledge`/`L6_Router`/`L7_Execution`/`L8_Parliament`/`L9_Quest`/`L10_Interface`
- [x] SubTask 5.2: 在 `WikiStore` 中扩展锚点表
  - 新增 `anchors` 表:`anchor_id`/`layer`/`crate_name`/`entity_id`/`created_at`/`updated_at`/`is_dangling`
  - `WikiStore::create_anchor(&self, layer: Layer, crate_name: &str, entity_id: &str) -> Result<IscmAnchor>`
  - `WikiStore::resolve_anchor(&self, anchor_id: Uuid) -> Result<WikiEntry>`:返回锚点指向的最新 Wiki 条目
  - `WikiStore::mark_dangling(&self, anchor_id: Uuid) -> Result<()>`:标记悬空(实体被删除时调用)
- [x] SubTask 5.3: 在 `WikiStore::delete` 中联动标记悬空锚点(物理删除条目,逻辑标记锚点)
- [x] SubTask 5.4: 编写集成测试 — `crates/repo-wiki/tests/iscm.rs`
  - 测试锚点创建与解析(L9 创建 → L5 解析返回同一版本)
  - 测试悬空锚点检测(删除条目后 `resolve_anchor` 返回 `WikiError::AnchorDangling`)
  - 测试跨层一致性(L9 创建锚点 → L5 更新条目 → L2 读取,三者返回同一 `updated_at`)
  - 测试锚点 UUIDv7 全局唯一性(创建 1000 个锚点无冲突)

---

## Task 6 (Day 12): Model Router 多策略路由

> 责任人:架构专家(主导)+ 性能专家(审查路由延迟)
> 优先级:P1(依赖 Task 1 的 UserIntent 类型)
> 代码目标:实现 Lite/Efficient/Auto 三策略路由 + 模型注册表
> 测试目标:路由准确率 > 90%(10 条标注用例)、`ModelRouteSelected` 事件发布
> 验收标准:三策略可切换、Auto 策略基于特征匹配(不引入 ML)

- [x] SubTask 6.1: 创建 `crates/model-router/src/types.rs` — 定义 `ModelInfo`(含 `model_id`/`provider`/`cost_per_1k_tokens`/`avg_latency_ms`/`max_context`)、`RoutingStrategy` 枚举(`Lite`/`Efficient`/`Auto`)、`RoutingRequest`(含 `quest_id`/`intent`/`estimated_tokens`/`strategy`)、`RoutingDecision`(含 `model_id`/`route_reason`/`estimated_cost`)
- [x] SubTask 6.2: 创建 `crates/model-router/src/registry.rs` — 实现 `ModelRegistry`(模型注册表)
  - `ModelRegistry::new() -> Self`
  - `ModelRegistry::register(&self, model: ModelInfo) -> Result<()>`
  - `ModelRegistry::unregister(&self, model_id: &str) -> Result<()>`
  - `ModelRegistry::get(&self, model_id: &str) -> Option<ModelInfo>`
  - `ModelRegistry::list(&self) -> Vec<ModelInfo>`
  - `ModelRegistry::from_config(config: &RouterConfig) -> Self`:从 `omega.yaml` 的 `[models]` 章节加载
- [x] SubTask 6.3: 创建 `crates/model-router/src/strategies.rs` — 实现三种路由策略
  - `route_lite(registry: &ModelRegistry, req: &RoutingRequest) -> Result<RoutingDecision>`:选择 `cost_per_1k_tokens` 最低的模型
  - `route_efficient(registry: &ModelRegistry, req: &RoutingRequest) -> Result<RoutingDecision>`:选择 `avg_latency_ms` 最低的模型
  - `route_auto(registry: &ModelRegistry, req: &RoutingRequest) -> Result<RoutingDecision>`:基于任务特征(任务类型 + 预估 Token 数 + 风险等级)加权评分,选择综合最优
  - Auto 策略评分公式:`score = 0.4 * (1/cost) + 0.4 * (1/latency) + 0.2 * quality_score`(Week 2 阶段 `quality_score` 使用静态值,Week 6 接入 GSOE 后动态化)
- [x] SubTask 6.4: 创建 `crates/model-router/src/router.rs` — 实现 `ModelRouter` 主入口
  - `ModelRouter::new(registry: ModelRegistry, event_bus: EventBus) -> Self`
  - `ModelRouter::route(&self, request: RoutingRequest) -> Result<RoutingDecision>`:按 `strategy` 分发到对应策略函数,发布 `ModelRouteSelected` 事件
- [x] SubTask 6.5: 创建 `crates/model-router/src/error.rs` — 定义 `RouterError`(thiserror enum)
  - 变体:`ModelNotFound`/`NoModelsRegistered`/`BudgetExceeded`/`EventBusError`/`ConfigError`
- [x] SubTask 6.6: 创建 `crates/model-router/src/config.rs` — 定义 `RouterConfig`(含 `models: Vec<ModelInfo>`/`default_strategy: RoutingStrategy`)
- [x] SubTask 6.7: 更新 `crates/model-router/src/lib.rs` — 公开模块并 re-export
- [x] SubTask 6.8: 更新 `crates/model-router/Cargo.toml` — 添加依赖(`nexus-core`/`event-bus`/`serde`/`serde_json`/`thiserror`/`tokio`/`tracing`/`dashmap`)
- [x] SubTask 6.9: 编写集成测试 — `crates/model-router/tests/router.rs`
  - 测试三策略路由(各策略选择正确模型)
  - 测试 10 条标注用例路由准确率 > 90%
  - 测试 `ModelRouteSelected` 事件发布
  - 测试模型动态注册/注销

---

## Task 7 (Day 13): CACR 成本感知路由模块

> 责任人:安全专家(主导)+ 性能专家(审查降级逻辑)
> 优先级:P1(依赖 Task 6 的 ModelRouter)
> 代码目标:实现 CACR 成本保护层,作为 Model Router 的拦截模块
> 测试目标:预算 80% 降级、预算 100% 拒绝、`BudgetExceeded` 事件发布
> 验收标准:降级/拒绝决策可被测试用例验证(不依赖真实模型 API)

- [x] SubTask 7.1: 创建 `crates/model-router/src/cacr.rs` — 实现 `CacrGuard`(成本感知守卫)
  - `CacrGuard::new(budget_limit: u64, warn_threshold: f32(默认 0.8), block_threshold: f32(默认 1.0)) -> Self`
  - `CacrGuard::check(&self, estimated_cost: u64, remaining_budget: u64) -> CacrDecision`:返回 `Allow`/`Downgrade(reason)`/`Block(reason)`
  - `CacrGuard::from_config(config: &CacrConfig) -> Self`
- [x] SubTask 7.2: 定义 `CacrDecision` 枚举(`Allow`/`Downgrade(String)`/`Block(String)`)与 `CacrConfig`(含 `budget_limit`/`warn_threshold`/`block_threshold`,从 `omega.yaml` 的 `[cacr]` 章节读取)
- [x] SubTask 7.3: 在 `ModelRouter::route` 中集成 CACR 拦截
  - 路由前调用 `CacrGuard::check`,根据决策:
    - `Allow`:正常路由
    - `Downgrade`:降级到次优模型(从 `RoutingDecision` 候选列表中移除首选,选择下一个),记录降级原因
    - `Block`:返回 `RouterError::BudgetExceeded`,发布 `BudgetExceeded` 事件
- [x] SubTask 7.4: 在 `crates/model-router/src/config.rs` 中扩展 `RouterConfig`,新增 `cacr: CacrConfig` 字段
- [x] SubTask 7.5: 编写集成测试 — `crates/model-router/tests/cacr.rs`
  - 测试预算充足时 `Allow`(正常路由)
  - 测试预算 80% 时 `Downgrade`(降级到次优模型)
  - 测试预算 100% 时 `Block`(返回 `RouterError::BudgetExceeded`)
  - 测试 `BudgetExceeded` 事件发布
  - 测试阈值可通过配置调整

---

## Task 8 (Day 14): Week 2 验收门禁

> 责任人:质量专家(主导)+ 全员参与验收
> 优先级:P1(Week 2 验收未通过则不进入 Week 3)
> 代码目标:全量测试通过、端到端 Quest 流程验证
> 测试目标:覆盖率 > 85%、端到端 Quest 任务分解 < 1s
> 验收标准:`cargo check && cargo clippy -D warnings && cargo test && cargo build --release` 全通过

- [x] SubTask 8.1: 编写端到端集成测试 — `crates/quest-engine/tests/e2e.rs`
  - 测试完整流程:用户输入 → Quest 创建 → 任务分解 → 模型路由 → 检查点保存 → Wiki 沉淀
  - 验证全流程无 panic、无孤儿调用、无事件丢失
  - 验证任务分解耗时 < 1s
  - 验证检查点可保存可恢复
  - 验证 Wiki 条目可生成可检索
- [x] SubTask 8.2: 运行 `cargo check --workspace --jobs 1` 验证全 workspace 编译通过
- [x] SubTask 8.3: 运行 `cargo clippy --workspace --jobs 1 -- -D warnings` 验证零警告
- [x] SubTask 8.4: 运行 `cargo test --workspace --jobs 1` 验证全部测试通过(Week 1 + Week 2)
- [x] SubTask 8.5: 运行 `cargo build --workspace --release --jobs 1` 验证 release 构建通过
- [x] SubTask 8.6: 验证性能基准(任务分解 < 1s、Wiki 生成 < 2s、向量检索 < 50ms)
- [x] SubTask 8.7: 更新 `CHANGELOG.md`,记录 Week 2 交付物
- [x] SubTask 8.8: 更新 `README.md` 的开发阶段表格,标记 Week 2 完成

---

# Task Dependencies

## 依赖关系图

```
Task 1 (Nexus Core)
  ├── Task 2 (Quest Engine) ──── Task 3 (LHQP Checkpoint)
  ├── Task 4 (Repo Wiki) ─────── Task 5 (ISCM 跨层索引)
  └── Task 6 (Model Router) ──── Task 7 (CACR 成本感知)
                                       │
                                       └── Task 8 (Week 2 验收)
```

## 依赖说明

- **Task 1**(Nexus Core)是所有后续任务的基础,必须最先完成
  - Task 2 依赖 Task 1 的 `Quest`/`Task`/`UserIntent` 类型
  - Task 4 依赖 Task 1 的 `CLV` 类型(用于 Wiki 嵌入向量)
  - Task 6 依赖 Task 1 的 `UserIntent` 类型(用于路由请求)
- **Task 2**(Quest Engine)是 Task 3 的基础
  - Task 3 在 QuestEngine 上扩展检查点能力
- **Task 4**(Repo Wiki)是 Task 5 的基础
  - Task 5 在 WikiStore 上扩展锚点表
- **Task 6**(Model Router)是 Task 7 的基础
  - Task 7 在 ModelRouter 上扩展 CACR 拦截层
- **Task 8**(验收)依赖 Task 1-7 全部完成

## 并行化机会

- Task 1 完成后,Task 2 / Task 4 / Task 6 可并行(均仅依赖 Nexus Core 类型)✓ 推荐并行
- Task 2 完成后,Task 3 可独立推进
- Task 4 完成后,Task 5 可独立推进
- Task 6 完成后,Task 7 可独立推进
- Task 3 / Task 5 / Task 7 可在 Task 2 / 4 / 6 完成后三路并行 ✓ 推荐并行

## 关键路径

Task 1 → Task 2 → Task 3 → Task 8(关键路径,决定 Week 2 整体进度)

---

# 执行原则

## 长期主义约束

- 优先选择最小可行方案,拒绝为假设性未来需求过度设计
- 三行相似代码优于一个过早抽象
- 不引入未被任务要求的特性、抽象、向后兼容垫片
- 不为不可能发生的场景写防御性代码

## 代码质量标准

- 单函数 ≤ 200 行(对应 §6 架构红线)
- 所有 async fn 满足 `Send + 'static + 'async`,经 QEEP 包装避免孤儿调用
- 库层用 `thiserror` enum,应用层用 `anyhow::Result`,禁止 `unwrap`/`expect` 在非测试代码
- 使用 `workspace.dependencies` 共享依赖,禁止独立声明版本
- 注释仅在 WHY 不明显处加(隐藏约束、变通方案、反直觉行为)

## 测试要求

- 每个 crate 至少包含核心路径测试(单元 + 集成)
- 关键路径测试覆盖率 > 85%
- 性能基准测试需在测试中显式断言(如 `assert!(decompose_time < Duration::from_millis(100))`)
- 端到端测试验证全流程无 panic、无孤儿调用

## 环境约束(继承自 Week 1)

- 使用 `--jobs 1` 避免内存不足
- 临时目录重定向到 `D:\Chimera CLI\tmp`(C 盘空间不足)
- PowerShell 环境变量设置:
  ```powershell
  $env:CARGO_HOME = 'D:\Chimera CLI\.toolchain\cargo'
  $env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
  $env:TMP = 'D:\Chimera CLI\tmp'
  $env:TEMP = 'D:\Chimera CLI\tmp'
  $env:PATH = "D:\Chimera CLI\.toolchain\cargo\bin;D:\msys64\mingw64\bin;$env:PATH"
  ```
