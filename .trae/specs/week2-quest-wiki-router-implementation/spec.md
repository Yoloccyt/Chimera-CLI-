# Week 2 — Quest + Wiki + 模型路由实现 Spec

## Why

Week 1(L0-L1 基础设施)已全量验收通过:`event-bus`(28 事件类型)、`seccore`(零信任沙箱)、`decay-engine`(能力衰减)、`qeep-protocol`(零孤儿调用)、`chimera-cli`(Clap 入口)均通过 `cargo test/clippy/build --release`。但 34 个 crate 中仍有 29 个处于 Stage 0 骨架状态(仅 `lib.rs` 注释,无 `pub mod` 声明)。

Week 2 是从"地基"走向"核心运行时"的关键跃迁:用户输入能否被分解为可执行任务图、能否在崩溃后恢复、能否路由到最适配模型、能否沉淀为可复用知识——这四个能力构成 NEXUS-OMEGA 的认知主循环。本 spec 将 §7 Week 2 推进计划(Day 8-14)细化为可执行、可监控、可验收的任务契约,严格对齐 OMEGA 四定律与十层架构依赖铁律。

## What Changes

- **quest-engine(L9)**:从骨架升级为可运行的任务分解引擎,实现 `Quest`/`Task`/`Checkpoint` 领域类型、4 步任务图分解、LHQP 检查点持久化、TTG 三级思考模式切换
- **repo-wiki(L5)**:从骨架升级为 SQLite + 向量索引的知识沉淀引擎,实现 Wiki 条目自动生成、ISCM 跨层共享锚点、`WikiUpdated` 事件发布
- **model-router(L1)**:从骨架升级为多策略路由器,实现 Lite/Efficient/Auto 三策略、CACR 成本感知模块、`ModelRouteSelected` 事件发布
- **nexus-core(L1)**:从骨架升级为核心领域类型库,定义 `NexusState`/`UserIntent`/`CLV`(512-dim)等全局类型,供 L9/L5/L1 复用
- **BREAKING**:无(Week 1 已稳定的 crate API 保持向后兼容,仅新增上层 crate 的实现)
- 不修改 `event-bus`/`seccore`/`decay-engine`/`qeep-protocol`/`chimera-cli` 的公开 API(仅在必要时新增订阅者)
- 不引入新的 workspace 依赖(workspace 已收录 `rusqlite`/`sqlite-vec`/`ndarray`/`uuid`/`chrono`/`sha2`)

## Impact

- Affected specs:
  - `phase1-architecture-analysis-and-planning`(Week 1 已完成,本 spec 是其直接后续)
  - `establish-elite-collaboration-team`(6 类专家子代理角色定义来源)
  - `init-crates-workspace`(34 crate 骨架已就绪,本 spec 在其上浇筑实现)
- Affected code:
  - `crates/quest-engine/src/` — 新增 `types.rs`/`engine.rs`/`checkpoint.rs`/`thinking.rs`/`error.rs`/`config.rs` + `tests/`
  - `crates/repo-wiki/src/` — 新增 `types.rs`/`store.rs`/`vector.rs`/`iscm.rs`/`error.rs`/`config.rs` + `tests/`
  - `crates/model-router/src/` — 新增 `types.rs`/`router.rs`/`strategies.rs`/`cacr.rs`/`error.rs`/`config.rs` + `tests/`
  - `crates/nexus-core/src/` — 新增 `types.rs`/`state.rs`/`clv.rs`/`error.rs` + `tests/`
  - 各 crate 的 `Cargo.toml` — 新增对 `event-bus`/`nexus-core`/`qeep-protocol` 的 workspace 级依赖
- 不受影响的代码:Week 1 已实现的 5 个 crate 的现有源码(仅可能新增测试用例)

## ADDED Requirements

### Requirement: Quest Engine 任务分解与生命周期管理

系统 SHALL 在 `quest-engine` crate 中实现长期任务的创建、分解、进度追踪与完成通知,所有状态变更通过 `event-bus` 广播。

#### Scenario: Quest 创建与任务图分解
- **WHEN** 用户意图(`UserIntent`)经由 `UserIntentEncoded` 事件到达 Quest Engine
- **THEN** 创建 `Quest` 实例(含 `quest_id`、`title`、`tasks`、`thinking_mode`、`checkpoint_id`)
- **THEN** 将意图分解为不少于 1 个、不超过 16 个 `Task` 节点,形成有向无环图(DAG)
- **THEN** 每个 `Task` 含 `task_id`、`description`、`status`(Pending/Running/Completed/Failed)、`dependencies`(前置 Task ID 列表)
- **THEN** 发布 `QuestCreated` 事件(携带 `quest_id`、`title`、`task_count`)
- **THEN** 4 步任务图分解基准:简单意图分解耗时 < 100ms,复杂意图 < 1s

#### Scenario: Quest 进度更新与完成
- **WHEN** Task 状态发生变迁(Pending → Running → Completed/Failed)
- **THEN** 更新 Quest 内部任务图状态
- **THEN** 发布 `QuestProgressUpdated` 事件(携带 `quest_id`、`completed`、`total`)
- **THEN** 当所有 Task 达到终态(Completed 或 Failed),Quest 标记为已完成并发布 `ExecutionCompleted` 事件

#### Scenario: TTG 三级思考模式切换
- **WHEN** 任务复杂度或预算触发思考模式切换
- **THEN** 支持三级模式:`Fast`(快速,低延迟)、`Standard`(标准,平衡)、`Deep`(深度,高质量)
- **THEN** 发布 `ThinkingModeSwitched` 事件(携带 `quest_id`、`from_mode`、`to_mode`)
- **THEN** 模式切换为原子操作,不可中断

### Requirement: LHQP 长周期任务检查点持久化

系统 SHALL 在 `quest-engine` crate 中实现 LHQP(Long-Horizon Quest Persistence)检查点机制,确保 Quest 在进程崩溃后可从最近检查点恢复。

#### Scenario: 检查点保存
- **WHEN** Quest 达到检查点触发条件(每 N 个 Task 完成或显式调用)
- **THEN** 序列化 Quest 当前状态(含任务图、已完成 Task 结果、记忆快照哈希)为 `Checkpoint`
- **THEN** 持久化到磁盘(`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`)
- **THEN** 发布 `CheckpointSaved` 事件 `[Critical]`(携带 `quest_id`、`checkpoint_id`、`memory_snapshot_hash`)
- **THEN** 检查点保存失败时返回 `QuestError::CheckpointSaveFailed`,不破坏 Quest 内存状态

#### Scenario: 检查点加载与恢复
- **WHEN** 进程重启且检测到未完成的 Quest
- **THEN** 从磁盘加载最新 `Checkpoint`
- **THEN** 校验 `memory_snapshot_hash` 完整性(SHA-256),校验失败返回 `QuestError::CheckpointCorrupted`
- **THEN** 重建 Quest 内存状态,将已完成 Task 标记为 Completed,未完成 Task 标记为 Pending
- **THEN** 发布 `CheckpointLoaded` 事件(携带 `quest_id`、`checkpoint_id`)

#### Scenario: 崩溃恢复测试
- **WHEN** 模拟进程在 Quest 执行中途崩溃(保存检查点后立即退出)
- **THEN** 重启后可从检查点恢复,不丢失已完成 Task 的结果
- **THEN** 恢复后 Quest 可继续执行未完成 Task

### Requirement: Repo Wiki 知识沉淀与向量索引

系统 SHALL 在 `repo-wiki` crate 中实现仓库知识的自动沉淀,使用 SQLite 存储结构化条目,使用 sqlite-vec 提供向量相似度检索。

#### Scenario: Wiki 条目自动生成
- **WHEN** Quest 完成或显式触发 Wiki 更新
- **THEN** 从 Quest 执行结果提取知识条目(含 `entry_id`、`title`、`content`、`tags`、`embedding`)
- **THEN** 将条目持久化到 SQLite(`~/.aether/wiki/wiki.db`)
- **THEN** 计算条目内容的 512-dim 嵌入向量(Week 2 阶段使用占位哈希向量,Week 6 NMC 实现后替换为真实 CLV)
- **THEN** 发布 `WikiUpdated` 事件(携带 `wiki_hash`、`delta`)
- **THEN** 基准:10 条 Wiki 条目生成 + 持久化 < 2s

#### Scenario: 向量相似度检索
- **WHEN** 查询方提供查询向量
- **THEN** 使用 sqlite-vec 执行 KNN 检索,返回 Top-K 最相似条目
- **THEN** 检索延迟 < 50ms(10 条条目规模)
- **THEN** 相似度分数 ∈ [0.0, 1.0]

#### Scenario: Wiki 条目 CRUD
- **WHEN** 对 Wiki 条目执行增删改查
- **THEN** 支持按 `entry_id` 精确查找、按 `tags` 过滤、按全文内容模糊匹配
- **THEN** 删除条目时同步删除其向量索引
- **THEN** 所有写操作使用 SQLite WAL 模式,保证并发读不阻塞

### Requirement: ISCM 跨层共享索引

系统 SHALL 在 `repo-wiki` crate 中实现 ISCM(Inter-Shared Cross Module)跨层共享锚点机制,确保同一知识实体在不同层间可被一致引用。

#### Scenario: 共享锚点创建
- **WHEN** Wiki 条目被创建或更新
- **THEN** 为条目生成跨层共享锚点(`AnchorId`,128-bit UUIDv7)
- **THEN** 锚点映射到 `(layer, crate, entity_id)` 三元组,记录知识实体的来源层与归属 crate
- **THEN** 同一知识实体在 L2 Memory、L5 Knowledge、L9 Quest 中引用同一锚点

#### Scenario: 跨层一致性校验
- **WHEN** 任意层通过锚点查询知识实体
- **THEN** 返回实体的最新版本(基于 `updated_at` 时间戳)
- **THEN** 若实体在源层已被删除,返回 `WikiError::AnchorDangling` 并标记为悬空锚点
- **THEN** 跨层一致性校验测试:在 L9 创建锚点 → L5 更新 → L2 读取,三者返回同一版本

### Requirement: Model Router 多策略路由

系统 SHALL 在 `model-router` crate 中实现多模型分层路由,按任务特征将请求路由至最适配的底层模型。

#### Scenario: 三策略路由
- **WHEN** 路由请求到达 Model Router
- **THEN** 支持三种路由策略:
  - `Lite`:轻量级任务(如简单问答、格式转换)→ 路由到低成本模型
  - `Efficient`:效率优先(如代码补全、快速检索)→ 路由到低延迟模型
  - `Auto`:自动选择(基于任务特征向量与历史路由数据)
- **THEN** `Auto` 策略使用简单的特征匹配(任务类型 + 预估 Token 数 + 风险等级),Week 2 阶段不引入 ML 模型
- **THEN** 路由决策完成后发布 `ModelRouteSelected` 事件(携带 `quest_id`、`model_id`、`route_reason`)
- **THEN** 路由准确率基准:在 10 条标注测试用例上准确率 > 90%

#### Scenario: 模型注册表
- **WHEN** Router 初始化
- **THEN** 从配置文件(`omega.yaml` 的 `[models]` 章节)加载已注册模型列表
- **THEN** 每个模型含 `model_id`、`provider`、`cost_per_1k_tokens`、`avg_latency_ms`、`max_context`
- **THEN** 支持运行时动态注册/注销模型

### Requirement: CACR 成本感知路由模块

系统 SHALL 在 `model-router` crate 中实现 CACR(Cost-Aware Cognitive Routing)模块,作为 Model Router 的成本保护层。

#### Scenario: 预算保护与成本告警
- **WHEN** 路由决策即将选择高成本模型
- **THEN** CACR 检查当前 Quest 的剩余预算(从 `decb-governor` 或配置读取,Week 2 阶段使用静态阈值)
- **THEN** 若预估成本超过剩余预算的 80%,降级到次优模型并记录降级原因
- **THEN** 若预估成本超过剩余预算的 100%,拒绝路由并返回 `RouterError::BudgetExceeded`
- **THEN** 发布 `BudgetExceeded` 事件(携带 `budget_type`、`current`、`limit`)

#### Scenario: 成本告警测试
- **WHEN** 模拟预算耗尽场景
- **THEN** CACR 正确触发降级或拒绝
- **THEN** 降级/拒绝决策可被测试用例验证(不依赖真实模型 API)

### Requirement: Nexus Core 核心领域类型

系统 SHALL 在 `nexus-core` crate 中定义全局共享的核心领域类型,供 L1-L10 所有上层 crate 复用。

#### Scenario: NexusState 全局状态
- **WHEN** 系统运行时
- **THEN** 提供 `NexusState` 类型,聚合当前活跃 Quest 列表、全局预算、模型注册表快照
- **THEN** `NexusState` 实现线程安全(`Arc<RwLock<NexusState>>` 或 actor 模型)
- **THEN** 状态变更时发布 `NexusStateChanged` 事件(携带 `state_hash`、`prev_hash`)

#### Scenario: UserIntent 用户意图
- **WHEN** 接收用户输入
- **THEN** 提供 `UserIntent` 类型,含 `intent_id`、`raw_text`、`multimodal_inputs`(Week 2 阶段仅文本)、`risk_level`(0-100)
- **THEN** `UserIntent` 实现 `Serialize`/`Deserialize`,可经 Event Bus 传输

#### Scenario: CLV 上下文潜在向量
- **WHEN** 任何层需要跨层语义表达
- **THEN** 提供 `CLV` 类型(512-dim `f32` 数组,基于 `ndarray::Array1<f32>`)
- **THEN** `CLV` 实现 `Serialize`/`Deserialize`/`Clone`/`PartialEq`
- **THEN** 提供 `CLV::zero()`、`CLV::from_vec(Vec<f32>)`、`CLV::cosine_similarity(&CLV) -> f32` 方法

### Requirement: Week 2 验收门禁

系统 SHALL 在 Day 14 通过端到端 Quest 验收,验证 Week 2 全部交付物协同工作。

#### Scenario: 端到端 Quest 流程
- **WHEN** 执行端到端测试:用户输入 → Quest 创建 → 任务分解 → 模型路由 → 检查点保存 → Wiki 沉淀
- **THEN** 全流程无 panic、无孤儿调用、无事件丢失
- **THEN** 任务分解耗时 < 1s
- **THEN** 检查点可保存可恢复
- **THEN** Wiki 条目可生成可检索

#### Scenario: 全量测试与构建
- **WHEN** 运行 Week 2 验收命令
- **THEN** `cargo check --workspace` 通过
- **THEN** `cargo clippy --workspace -- -D warnings` 零警告
- **THEN** `cargo test --workspace` 全通过(Week 1 + Week 2 测试用例)
- **THEN** `cargo build --workspace --release` 通过
- **THEN** 新实现 crate 的测试覆盖率 > 85%(关键路径全覆盖)

## MODIFIED Requirements

### Requirement: 代码质量标准(继承自 establish-elite-collaboration-team)
所有 Week 2 新增代码 SHALL 满足:
- **单一职责**:每个函数 ≤ 200 行(对应 §6 架构红线),模块边界清晰
- **workspace 一致性**:使用 `workspace.dependencies` 共享依赖,禁止独立声明版本
- **错误处理显式**:库层用 `thiserror` enum,应用层用 `anyhow::Result`,禁止 `unwrap`/`expect` 在非测试代码
- **async 约束**:所有 async fn 满足 `Send + 'static + 'async`,经 QEEP 包装避免孤儿调用
- **注释解释意图**:仅在 WHY 不明显处加注释(隐藏约束、变通方案、反直觉行为)
- **TDD-first**:核心领域类型先写类型定义与基础测试,再写业务逻辑

## REMOVED Requirements

无

---

## 附录:Week 2 关键设计决策(预填,实现阶段验证)

### A.1 Quest 任务图表示

- 使用 `Vec<Task>` + `dependencies: Vec<TaskId>` 表示 DAG,不引入 `petgraph` 依赖(避免过度工程化)
- 任务图校验:检测循环依赖,存在环则返回 `QuestError::CyclicDependency`
- 拓扑排序用于确定执行顺序,但 Week 2 阶段不实现并行执行(Week 4 PVL 实现)

### A.2 检查点序列化格式

- 使用 `rmp-serde`(MessagePack)序列化 `Checkpoint`,与 Event Bus 序列化协议一致(ADR-004)
- 检查点文件结构:`~/.aether/checkpoints/<quest_id>/<checkpoint_id>.bin`
- 保留最近 5 个检查点,超出则删除最旧(避免磁盘膨胀)

### A.3 Wiki 向量索引策略

- Week 2 阶段:使用 512-dim 占位向量(基于内容 SHA-256 哈希扩展),验证 sqlite-vec 集成
- Week 6 阶段:NMC 实现后替换为真实 CLV 嵌入
- 向量维度固定 512,与 CLV 对齐,避免后续迁移成本

### A.4 CACR 与 DECB 的关系

- CACR 是 Model Router(L1)内的成本保护模块,负责单次路由的成本决策
- DECB(`decb-governor`,L8)是全局双档预算治理器,Week 5 实现
- Week 2 阶段 CACR 使用静态阈值(从 `omega.yaml` 读取),Week 5 接入 DECB 后改为动态预算查询
- 此设计避免 L1 → L8 向上依赖违规(CACR 不 import decb-governor,而是通过 `BudgetExceeded` 事件反向通信)

### A.5 风险与缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|---------|
| sqlite-vec Windows 兼容性 | 高 | 中 | Week 1 已验证 workspace 依赖收录,若链接失败则降级为内存向量检索 |
| 检查点磁盘 I/O 阻塞 | 中 | 中 | 使用 `tokio::task::spawn_blocking` 包装同步文件操作 |
| Quest 任务图循环依赖 | 中 | 低 | 创建时拓扑排序校验,存在环则拒绝 |
| CACR 静态阈值不合理 | 中 | 中 | 阈值可通过 `omega.yaml` 配置,Week 5 接入 DECB 后动态化 |
| 跨层锚点一致性 | 高 | 低 | ISCM 锚点使用 UUIDv7 全局唯一,删除时标记悬空而非物理删除 |
