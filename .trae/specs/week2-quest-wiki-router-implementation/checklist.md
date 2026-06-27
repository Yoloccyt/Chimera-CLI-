# Checklist — Week 2 Quest + Wiki + 模型路由实现

本清单用于 Week 2 验收门禁,逐项验证 spec.md 中定义的全部 ADDED Requirements。
验收流程:每完成一个 Task 后立即勾选对应检查点,Day 14 全量复核。

> 标记说明:`[x]` 已通过 · `[ ]` 未通过/未验证 · `[!]` 验证失败需修复

---

## Task 1:Nexus Core 核心领域类型

- [x] `crates/nexus-core/src/types.rs` 定义 `UserIntent`/`Quest`/`Task`/`Checkpoint`/`TaskStatus`/`ThinkingMode` 类型,全部派生 `Debug`/`Clone`/`Serialize`/`Deserialize`/`PartialEq`
- [x] `crates/nexus-core/src/clv.rs` 实现 `CLV`(512-dim),`CLV::zero()`/`CLV::from_vec()`/`CLV::cosine_similarity()`/`CLV::dimension()` 方法可用
- [x] `CLV::from_vec` 对非 512 维输入返回 `NexusError::InvalidClvDimension`
- [x] `CLV::cosine_similarity` 对零向量返回 0.0(不 panic、不除零)
- [x] `crates/nexus-core/src/state.rs` 实现 `NexusState`,使用 `Arc<RwLock<...>>` 线程安全
- [x] `NexusState::register_quest`/`update_quest_progress`/`snapshot_hash` 方法可用
- [x] `NexusState::snapshot_hash` 返回 SHA-256 hex 字符串
- [x] `crates/nexus-core/src/error.rs` 定义 `NexusError`(thiserror enum),含 5 个变体
- [x] `crates/nexus-core/src/lib.rs` 公开 `pub mod types/clv/state/error` 并 re-export 核心类型
- [x] `crates/nexus-core/Cargo.toml` 使用 workspace 级依赖,无独立版本声明
- [x] `cargo test -p nexus-core` 全部通过
- [x] `cargo clippy -p nexus-core -- -D warnings` 零警告
- [x] 单元测试覆盖:CLV 余弦相似度、NexusState 线程安全(并发读写)、UserIntent 序列化往返

---

## Task 2:Quest Engine 任务分解与生命周期

- [x] `crates/quest-engine/src/types.rs` 定义 `QuestEngine` 内部状态(含 `DashMap`/`EventBus`)
- [x] `crates/quest-engine/src/engine.rs` 实现 `QuestEngine::new`/`create_quest`/`decompose`/`update_task_status`/`complete_quest`
- [x] `QuestEngine::create_quest` 从 `UserIntent` 分解任务图,发布 `QuestCreated` 事件
- [x] `QuestEngine::update_task_status` 状态变迁时发布 `QuestProgressUpdated` 事件
- [x] `QuestEngine::complete_quest` 所有 Task 达终态时发布 `ExecutionCompleted` 事件
- [x] `crates/quest-engine/src/dag.rs` 实现 `validate_dag`(拓扑排序检测环)与 `topological_order`
- [x] `validate_dag` 对循环依赖返回 `QuestError::CyclicDependency`
- [x] `crates/quest-engine/src/error.rs` 定义 `QuestError`(thiserror enum),含 6 个变体
- [x] `crates/quest-engine/src/config.rs` 定义 `QuestConfig`(`max_tasks_per_quest` 默认 16、`checkpoint_interval` 默认 3)
- [x] `crates/quest-engine/src/lib.rs` 公开模块并 re-export
- [x] `crates/quest-engine/Cargo.toml` 添加 `nexus-core`/`event-bus`/`qeep-protocol` 依赖(均 workspace 级)
- [x] 4 步任务图分解基准:简单意图 < 100ms(测试中断言)
- [x] 集成测试验证 `QuestCreated`/`QuestProgressUpdated`/`ExecutionCompleted` 事件正确发布
- [x] 集成测试验证 DAG 循环依赖检测
- [x] 集成测试验证 10 个并发 Quest 创建无冲突
- [x] `cargo test -p quest-engine` 全部通过
- [x] `cargo clippy -p quest-engine -- -D warnings` 零警告

---

## Task 3:LHQP 检查点持久化

- [x] `crates/quest-engine/src/checkpoint.rs` 实现 `CheckpointManager`
- [x] `CheckpointManager::save` 序列化为 MessagePack,写入 `<checkpoint_dir>/<quest_id>/<checkpoint_id>.bin`
- [x] `CheckpointManager::load`/`load_latest` 读取并反序列化检查点
- [x] `CheckpointManager::verify_integrity` 使用 SHA-256 校验检查点完整性
- [x] `CheckpointManager::prune_old` 保留最近 N 个检查点,删除其余
- [x] `QuestEngine::save_checkpoint` 调用 `CheckpointManager::save` 并发布 `CheckpointSaved` 事件 `[Critical]`
- [x] `QuestEngine::restore_from_checkpoint` 调用 `load_latest` 重建内存状态,发布 `CheckpointLoaded` 事件
- [x] `update_task_status` 每 N 个 Task 完成自动触发检查点(N 由 `QuestConfig::checkpoint_interval` 控制)
- [x] 同步文件 I/O 使用 `tokio::task::spawn_blocking` 包装,不阻塞异步运行时
- [x] 集成测试:检查点保存与加载往返一致性
- [x] 集成测试:篡改检查点文件后 `load` 返回 `QuestError::CheckpointCorrupted`
- [x] 集成测试:创建 7 个检查点后仅保留最新 5 个
- [x] 集成测试:崩溃恢复场景(创建 Quest → 完成 2 Task → 保存检查点 → 丢弃内存 → 恢复 → 验证 Task 状态)
- [x] `CheckpointSaved` 事件被正确标注为 `EventSeverity::Critical`
- [x] `cargo test -p quest-engine` 含检查点测试全部通过

---

## Task 4:Repo Wiki SQLite + 向量索引

- [x] `crates/repo-wiki/src/types.rs` 定义 `WikiEntry`/`WikiConfig`
- [x] `crates/repo-wiki/src/store.rs` 实现 `WikiStore`(SQLite 持久化层)
- [x] `WikiStore::open` 启用 WAL 模式,创建 `entries` 表与 `vec_entries` 虚拟表(sqlite-vec)
- [x] `WikiStore::insert`/`get`/`delete`/`list_by_tag`/`search_fulltext` 方法可用
- [x] `WikiStore::delete` 同步删除向量索引
- [x] `crates/repo-wiki/src/vector.rs` 实现 `VectorIndex`
- [x] `VectorIndex::upsert`/`search`/`delete` 方法可用
- [x] `VectorIndex::search` 返回 `(entry_id, similarity_score)` 列表,相似度 ∈ [0.0, 1.0]
- [x] `crates/repo-wiki/src/generator.rs` 实现 `WikiGenerator::from_quest_result`
- [x] Week 2 阶段嵌入向量使用内容 SHA-256 哈希扩展为 512-dim 占位向量(文档注释说明 Week 6 替换)
- [x] `crates/repo-wiki/src/error.rs` 定义 `WikiError`(thiserror enum),含 6 个变体
- [x] `crates/repo-wiki/src/lib.rs` 公开模块并 re-export `WikiStore`/`VectorIndex`/`WikiGenerator`/`WikiEntry`
- [x] `crates/repo-wiki/Cargo.toml` 添加 `rusqlite`/`sqlite-vec`/`nexus-core`/`event-bus` 依赖(均 workspace 级)
- [x] 集成测试:10 条 Wiki 条目 CRUD 全部正确
- [x] 集成测试:向量相似度检索 Top-K 返回正确条目,延迟 < 50ms
- [x] 集成测试:WAL 模式并发读不阻塞
- [x] 集成测试:`WikiUpdated` 事件正确发布(通过 mock EventBus 订阅验证)
- [x] 10 条 Wiki 生成 + 持久化基准 < 2s(测试中断言)
- [x] `cargo test -p repo-wiki` 全部通过
- [x] `cargo clippy -p repo-wiki -- -D warnings` 零警告

---

## Task 5:ISCM 跨层共享索引

- [x] `crates/repo-wiki/src/iscm.rs` 定义 `IscmAnchor` 与 `Layer` 枚举(L1-L10 全覆盖)
- [x] `IscmAnchor::anchor_id` 使用 UUIDv7
- [x] `WikiStore` 新增 `anchors` 表(`anchor_id`/`layer`/`crate_name`/`entity_id`/`created_at`/`updated_at`/`is_dangling`)
- [x] `WikiStore::create_anchor` 创建新锚点
- [x] `WikiStore::resolve_anchor` 返回锚点指向的最新 Wiki 条目
- [x] `WikiStore::mark_dangling` 标记悬空锚点
- [x] `WikiStore::delete` 联动标记悬空锚点(物理删除条目,逻辑标记锚点)
- [x] 集成测试:锚点创建与解析(L9 创建 → L5 解析返回同一版本)
- [x] 集成测试:悬空锚点检测(删除条目后 `resolve_anchor` 返回 `WikiError::AnchorDangling`)
- [x] 集成测试:跨层一致性(L9 创建 → L5 更新 → L2 读取,三者返回同一 `updated_at`)
- [x] 集成测试:锚点 UUIDv7 全局唯一性(1000 个锚点无冲突)
- [x] `cargo test -p repo-wiki` 含 ISCM 测试全部通过

---

## Task 6:Model Router 多策略路由

- [x] `crates/model-router/src/types.rs` 定义 `ModelInfo`/`RoutingStrategy`/`RoutingRequest`/`RoutingDecision`
- [x] `RoutingStrategy` 枚举含 `Lite`/`Efficient`/`Auto` 三个变体
- [x] `crates/model-router/src/registry.rs` 实现 `ModelRegistry`
- [x] `ModelRegistry::new`/`register`/`unregister`/`get`/`list`/`from_config` 方法可用
- [x] `crates/model-router/src/strategies.rs` 实现三种路由策略函数
- [x] `route_lite` 选择 `cost_per_1k_tokens` 最低的模型
- [x] `route_efficient` 选择 `avg_latency_ms` 最低的模型
- [x] `route_auto` 基于任务特征加权评分(0.4*cost + 0.4*latency + 0.2*quality)
- [x] `crates/model-router/src/router.rs` 实现 `ModelRouter::new`/`route`
- [x] `ModelRouter::route` 按 `strategy` 分发,发布 `ModelRouteSelected` 事件
- [x] `crates/model-router/src/error.rs` 定义 `RouterError`(thiserror enum),含 5 个变体
- [x] `crates/model-router/src/config.rs` 定义 `RouterConfig`(`models`/`default_strategy`)
- [x] `crates/model-router/src/lib.rs` 公开模块并 re-export
- [x] `crates/model-router/Cargo.toml` 添加 `nexus-core`/`event-bus` 依赖(均 workspace 级)
- [x] 集成测试:三策略路由各选择正确模型
- [x] 集成测试:10 条标注用例路由准确率 > 90%
- [x] 集成测试:`ModelRouteSelected` 事件正确发布
- [x] 集成测试:模型动态注册/注销
- [x] `cargo test -p model-router` 全部通过
- [x] `cargo clippy -p model-router -- -D warnings` 零警告

---

## Task 7:CACR 成本感知路由模块

- [x] `crates/model-router/src/cacr.rs` 实现 `CacrGuard`
- [x] `CacrGuard::new(budget_limit, warn_threshold, block_threshold)` 构造函数可用
- [x] `CacrGuard::check(estimated_cost, remaining_budget)` 返回 `CacrDecision`
- [x] `CacrDecision` 枚举含 `Allow`/`Downgrade(String)`/`Block(String)` 三个变体
- [x] `CacrConfig` 含 `budget_limit`/`warn_threshold`(默认 0.8)/`block_threshold`(默认 1.0)
- [x] `ModelRouter::route` 集成 CACR 拦截:Allow 正常路由、Downgrade 降级、Block 拒绝
- [x] Block 时返回 `RouterError::BudgetExceeded` 并发布 `BudgetExceeded` 事件
- [x] `RouterConfig` 新增 `cacr: CacrConfig` 字段
- [x] 集成测试:预算充足时 `Allow`(正常路由)
- [x] 集成测试:预算 80% 时 `Downgrade`(降级到次优模型)
- [x] 集成测试:预算 100% 时 `Block`(返回 `RouterError::BudgetExceeded`)
- [x] 集成测试:`BudgetExceeded` 事件正确发布
- [x] 集成测试:阈值可通过配置调整
- [x] `cargo test -p model-router` 含 CACR 测试全部通过
- [x] `cargo clippy -p model-router -- -D warnings` 零警告

---

## Task 8:Week 2 验收门禁

- [x] 端到端集成测试 `crates/quest-engine/tests/e2e.rs` 编写完成
- [x] 端到端测试覆盖:用户输入 → Quest 创建 → 任务分解 → 模型路由 → 检查点保存 → Wiki 沉淀
- [x] 端到端测试验证:全流程无 panic、无孤儿调用、无事件丢失
- [x] 端到端测试验证:任务分解耗时 < 1s
- [x] 端到端测试验证:检查点可保存可恢复
- [x] 端到端测试验证:Wiki 条目可生成可检索
- [x] `cargo check --workspace --jobs 1` 通过
- [x] `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告
- [x] `cargo test --workspace --jobs 1` 全部通过(Week 1 + Week 2 测试用例)
- [x] `cargo build --workspace --release --jobs 1` 通过
- [x] 新实现 crate 测试覆盖率 > 85%(关键路径全覆盖)
- [x] 性能基准达标:任务分解 < 1s、Wiki 生成 < 2s、向量检索 < 50ms
- [x] `CHANGELOG.md` 更新,记录 Week 2 交付物
- [x] `README.md` 开发阶段表格更新,Week 2 标记完成

---

## 架构红线复核(对应 §6 尸检教训)

- [x] **单函数 ≤ 200 行**:Week 2 新增代码无超长函数(对应 Claude Code `print.ts` 3167 行教训)
- [x] **所有异步操作有 GQEP 聚集/超时处理**:Quest Engine 的 async 操作经 QEEP 包装(对应 5.4% 孤儿调用教训)
- [x] **所有外部调用经 SecCore 沙箱**:Week 2 阶段无外部调用(文件 I/O 使用 `spawn_blocking`,不触发沙箱)
- [x] **所有 async 必须 await 或 spawn 管理**:无 void Promise(对应竞态教训)
- [x] **禁止功能标志**:CACR 阈值通过配置调整,不引入 feature flag(对应 44 个未发布标志教训)
- [x] **必须经 HCW 分层 + OSA 稀疏化后再加载**:Week 2 阶段无 1M Token 加载场景(Week 3 实现 HCW/OSA)
- [x] **依赖方向铁律**:L9(quest-engine)→ L1(nexus-core)向下依赖 ✓;L5(repo-wiki)→ L1(nexus-core)向下依赖 ✓;L1(model-router)→ L1(nexus-core)同层互引 ✓
- [x] **跨层通信走 Event Bus**:Quest Engine 发布 `QuestCreated`/`CheckpointSaved` 等;Repo Wiki 发布 `WikiUpdated`;Model Router 发布 `ModelRouteSelected`/`BudgetExceeded`

---

## 代码质量复核(对应 §4 Rust 编码规范)

- [x] 所有 crate 使用 `version.workspace = true` 与 `edition.workspace = true`
- [x] 所有依赖使用 `{ workspace = true }`,无独立版本声明
- [x] 所有 async fn 满足 `Send + 'static + 'async` 约束
- [x] 库层错误用 `thiserror` enum,应用层用 `anyhow::Result`
- [x] 无 `unwrap()`/`expect()` 在非测试代码
- [x] 无 `Box<dyn Trait>`(优先 `impl Trait` 或 `enum dispatch`)
- [x] `#![forbid(unsafe_code)]` 在所有 crate 的 lib.rs 顶部保留
- [x] `#![warn(missing_docs, clippy::all)]` 在所有 crate 的 lib.rs 顶部保留
- [x] 注释仅在 WHY 不明显处加(隐藏约束、变通方案、反直觉行为)
- [x] `cargo fmt --all` 格式化通过

---

## 环境约束复核(继承自 Week 1)

- [x] 使用 `--jobs 1` 避免内存不足
- [x] 临时目录重定向到 `D:\Chimera CLI\tmp`
- [x] PowerShell 环境变量正确设置(`CARGO_HOME`/`RUSTUP_HOME`/`TMP`/`TEMP`/`PATH`)
- [x] 检查点文件路径使用 `~/.aether/checkpoints/`(Windows 下为 `%USERPROFILE%\.aether\checkpoints\`)
- [x] Wiki 数据库路径使用 `~/.aether/wiki/wiki.db`(Windows 下为 `%USERPROFILE%\.aether\wiki\wiki.db`)

---

## 验收结论

- [x] 全部检查点通过
- [x] Week 2 验收门禁通过,可进入 Week 3(记忆与路由系统)
- [x] 更新 `project_memory.md`,记录 Week 2 完成状态与经验教训
