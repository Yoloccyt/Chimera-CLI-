# Week 4 — 执行优化层(L6 + L7)Spec

## Why

Week 1(L0-L1 基础设施)、Week 2(Quest+Wiki+Router)、Week 3(记忆与路由系统)已全量验收通过,并经历三轮深度复审:34 个 crate 中 15 个已浇筑实现,全量 `cargo check/clippy/test/build --release` 通过,测试用例覆盖单元/集成/并发/属性/错误路径/性能基准六大维度。但项目仍缺乏 NEXUS-OMEGA 的核心执行能力——**并行流式生成验证、多步预测、聚集执行、推测缓存与熵均衡**。

Week 4 是从"认知效率"走向"执行效率"的关键跃迁:任务能否被并行生产验证、能否多步预测加速推理、能否聚集汇聚避免孤儿调用、能否推测缓存提升命中率、能否熵均衡自然扩散负载——这六个能力构成 OMEGA 四定律中 Ω-Evolve(在线进化)与 Ω-Event(事件驱动)的工程实现,直接对应 Claude Code 尸检教训中"5.4% 孤儿调用"与"void Promise 无 await 竞态"的免疫策略。

本 spec 将 §7 Week 4 推进计划(Day 22-28)细化为可执行、可监控、可验收的任务契约,严格对齐十层架构依赖铁律、四次尸检教训与前三轮复审经验。

## What Changes

* **gea-activator(L6 Router)**:从骨架升级为门控专家激活器,实现连续 [0,1] 门控值计算、专家冲突消解、基于 TaskProfile 的动态激活,发布 `ExpertActivated` 事件

* **gqep-executor(L6 Router)**:从骨架升级为聚集查询执行协议,实现并发异步操作的聚集汇聚、超时治理、批量原子性保证,修正"5.4% 孤儿调用"尸检教训

* **pvl-layer(L7 Execution)**:从骨架升级为生产验证闭环,实现 Producer-Verifier 并行流式生成验证、实时反馈通道、策略调整,修正"void Promise 无 await"竞态教训

* **mtpe-executor(L7 Execution)**:从骨架升级为多步预测执行器,实现 N=1-10 多 Token 预测、预测成功率统计、失败回退机制,预测成功率 > 80%

* **scc-cache(L3 Storage)**:从骨架升级为推测上下文缓存,实现 Draft/Verify 共享缓存、访问模式推测预取、LRU 驱逐,命中率 > 70%

* **faae-router(L6 Router)**:从骨架升级为 Function-as-Expert 语义路由 + EDSB 熵驱动自均衡,实现工具即专家的语义化路由、指数衰减熵均衡、熵值 > 0.6

* **BREAKING**:无(Week 1-3 已稳定的 crate API 保持向后兼容,仅新增 L6/L7/L3 crate 的实现)

* 不修改 `nexus-core`/`event-bus`/`quest-engine`/`repo-wiki`/`model-router`/`seccore`/`decay-engine`/`qeep-protocol`/`chimera-cli`/`mlc-engine`/`hcw-window`/`cmt-tiering`/`osa-coordinator`/`kvbsr-router` 的公开 API(仅在必要时新增订阅者)

* 不引入新的 workspace 依赖(workspace 已收录 `rusqlite`/`ndarray`/`dashmap`/`uuid`/`chrono`/`sha2`/`rmp-serde`/`tempfile`/`criterion`/`proptest`/`tracing-test`)

## Impact

* Affected specs:

  * `phase1-architecture-analysis-and-planning`(Week 1 已完成)

  * `week2-quest-wiki-router-implementation`(Week 2 已完成)

  * `week3-memory-routing-system`(Week 3 已完成,本 spec 是其直接后续)

  * `week3-third-round-deep-review`(Week 3 第三轮复审已完成,本 spec 继承其全部经验教训)

  * `establish-elite-collaboration-team`(6 类专家子代理角色定义来源)

  * `init-crates-workspace`(34 crate 骨架已就绪,本 spec 在其上浇筑实现)

* Affected code:

  * `crates/gea-activator/src/` — 新增 `types.rs`/`activator.rs`/`gating.rs`/`conflict.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/gqep-executor/src/` — 新增 `types.rs`/`gatherer.rs`/`timeout.rs`/`batch.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/pvl-layer/src/` — 新增 `types.rs`/`producer.rs`/`verifier.rs`/`feedback.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/mtpe-executor/src/` — 新增 `types.rs`/`predictor.rs`/`verifier.rs`/`fallback.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/scc-cache/src/` — 新增 `types.rs`/`cache.rs`/`prefetch.rs`/`lru.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/faae-router/src/` — 新增 `types.rs`/`router.rs`/`edsb.rs`/`expert.rs`/`error.rs`/`config.rs` + `tests/`

  * `crates/event-bus/src/types.rs` — 新增 Week 4 所需事件类型(`ExpertActivated`/`GatherCompleted`/`VerificationFailed`/`PredictionMade`/`CachePrefetched`/`EntropyBalanced`)

  * 各 crate 的 `Cargo.toml` — 新增对 `nexus-core`/`event-bus`/`qeep-protocol` 的 workspace 级依赖

* 不受影响的代码:Week 1-3 已实现的 15 个 crate 的现有源码(仅可能新增测试用例与事件订阅者)

## ADDED Requirements

### Requirement: GEA 门控专家激活

系统 SHALL 在 `gea-activator` crate 中实现 GEA(Gated Expert Activation),基于门控机制计算连续 [0,1] 激活值,调度专家网络,并通过冲突消解策略处理多专家竞争。

#### Scenario: 连续门控值计算

* **WHEN** `TaskProfile`(含 `complexity_score`、`task_type`、`risk_level`)到达 GEA

* **THEN** 调用 `GeaActivator::compute_gate_value(&task, &expert_profile) -> f32`

* **THEN** 门控值 ∈ [0.0, 1.0],0.0 表示完全不激活,1.0 表示完全激活

* **THEN** 门控值计算公式:`gate = sigmoid(w1 × complexity + w2 × relevance + w3 × affinity - bias)`

* **THEN** 权重 `w1/w2/w3` 与 `bias` 可通过 `omega.yaml` 配置

* **THEN** 门控值计算延迟 < 1ms(单专家)

#### Scenario: 专家冲突消解

* **WHEN** 多个专家的门控值均超过激活阈值(默认 0.5)

* **THEN** 触发冲突消解:按 `gate_value × expert_priority` 综合评分排序

* **THEN** Top-K(默认 K=3)专家被激活,其余抑制

* **THEN** 若两个专家功能重叠度 > 0.8(基于 CLV 余弦相似度),仅保留综合评分更高者

* **THEN** 冲突消解完成后发布 `ExpertActivated` 事件(携带 `activated_experts: Vec<ExpertId>`、`suppressed_experts: Vec<ExpertId>`)

#### Scenario: 动态激活阈值

* **WHEN** 系统负载或任务紧急度变化

* **THEN** 激活阈值可动态调整:高负载时提高阈值(减少激活专家数),低负载时降低阈值

* **THEN** 阈值调整策略:`threshold = base_threshold + load_factor × 0.2`,`load_factor ∈ [0, 1]`

* **THEN** 阈值调整后发布 `ActivationThresholdAdjusted` 事件

#### Scenario: 专家激活缓存

* **WHEN** 相同 TaskProfile 短时间内重复到达(5 秒内)

* **THEN** 命中缓存,直接返回上次激活结果(避免重复计算)

* **THEN** 缓存采用 LRU 策略,容量默认 128 条目

* **THEN** 缓存命中率统计:每 100 次激活发布 `ActivationCacheStats` 事件(携带 `hit_rate`)

### Requirement: GQEP 聚集查询执行协议

系统 SHALL 在 `gqep-executor` crate 中实现 GQEP(Gather-Query Execution Protocol),为并发异步操作提供聚集汇聚与超时治理,确保零孤儿调用(对应 Claude Code 尸检 5.4% 孤儿率教训)。

#### Scenario: 并发操作聚集

* **WHEN** 多个异步操作(`Future`)被提交到 GQEP

* **THEN** 调用 `GqepExecutor::gather(&self, futures: Vec<GqepFuture>) -> GatherResult`

* **THEN** 所有 Future 并发执行(`tokio::join_all` 或 `FuturesUnordered`)

* **THEN** 每个 Future 包裹在 `EntangledCall`(QEEP 协议)中,确保超时处理与错误捕获

* **THEN** 聚集完成后发布 `GatherCompleted` 事件(携带 `total`、`succeeded`、`failed`、`latency_ms`)

* **THEN** 聚集延迟基准:10 个并发操作聚集完成 < 100ms(假设单操作 < 50ms)

#### Scenario: 超时治理

* **WHEN** 单个异步操作超过配置的超时时间(默认 5 秒)

* **THEN** 触发超时:操作被取消(`tokio::time::timeout`),返回 `GqepError::OperationTimeout`

* **THEN** 超时操作不计入成功数,但计入总数

* **THEN** 超时事件发布 `OperationTimedOut` 事件(携带 `operation_id`、`timeout_ms`)

* **THEN** 超时阈值可通过 `omega.yaml` 配置,支持全局与单操作两种粒度

#### Scenario: 批量原子性保证

* **WHEN** 批量操作需要原子性(全部成功或全部回滚)

* **THEN** 调用 `GqepExecutor::gather_atomic(&self, futures: Vec<GqepFuture>) -> GatherResult`

* **THEN** 任一操作失败时,触发回滚回调(由调用方提供)

* **THEN** 回滚操作本身也经 GQEP 聚集,确保回滚不产生新的孤儿调用

* **THEN** 批量原子性测试:10 个操作中第 5 个失败,验证前 4 个回滚、后 5 个不执行

#### Scenario: 孤儿调用检测

* **WHEN** 异步操作被 spawn 后未被 await 或管理

* **THEN** GQEP 内部集成孤儿调用检测器(复用 `qeep-protocol` 的 `OrphanDetector`)

* **THEN** 检测到孤儿调用时发布 `OrphanCallDetected` 事件 `[Critical]`(携带 `operation_id`、`spawn_location`)

* **THEN** 孤儿调用率基准:< 0.1%(远低于 Claude Code 的 5.4%)

### Requirement: PVL 生产验证闭环

系统 SHALL 在 `pvl-layer` crate 中实现 PVL(Producer-Verifier Loop),通过 Producer-Verifier 并行流式生成验证,修正"void Promise 无 await"竞态教训。

#### Scenario: Producer 流式生成

* **WHEN** Quest 任务需要生成操作序列(如代码修改、命令执行)

* **THEN** Producer Agent 流式生成操作序列,每个操作附带置信度评分

* **THEN** 生成的操作通过 `mpsc` 通道发送给 Verifier(而非直接执行)

* **THEN** Producer 生成完成后发布 `OperationProduced` 事件(携带 `quest_id`、`operation_count`、`avg_confidence`)

* **THEN** Producer 生成速率基准:10 个操作/秒(占位实现,Week 6 NMC 后接入真实模型)

#### Scenario: Verifier 流式验证

* **WHEN** Verifier 从通道接收操作

* **THEN** Verifier 对每个操作执行验证:语法检查、安全检查、依赖检查

* **THEN** 验证通过的操作标记为 `Verified`,验证失败的标记为 `Rejected`(附带拒绝原因)

* **THEN** 验证结果通过反馈通道发送回 Producer(实时反馈)

* **THEN** 验证完成后发布 `PredictionVerified` 事件(携带 `quest_id`、`verified_count`、`rejected_count`)

#### Scenario: 实时反馈与策略调整

* **WHEN** Verifier 反馈拒绝率 > 30%

* **THEN** Producer 触发策略调整:降低生成速率、提高置信度阈值、切换生成策略

* **THEN** 策略调整发布 `ProducerStrategyAdjusted` 事件(携带 `adjustment_reason`、`new_strategy`)

* **THEN** 策略调整后,后续生成操作的拒绝率应下降(验证反馈闭环有效)

#### Scenario: 并行流式无竞态

* **WHEN** Producer 与 Verifier 并行运行

* **THEN** 通道操作无竞态:`mpsc` 通道保证消息顺序,Producer 写入与 Verifier 读取无数据竞争

* **THEN** 反馈通道同样无竞态:Verifier 写入反馈,Producer 读取反馈

* **THEN** 并发测试:100 个操作流式生成验证,无 panic、无数据丢失、无死锁

* **THEN** 所有 Future 均 await 或 spawn 管理(零 void Promise)

### Requirement: MTPE 多步预测执行

系统 SHALL 在 `mtpe-executor` crate 中实现 MTPE(Multi-Token Prediction Execution),通过 N=1-10 多步预测加速推理吞吐,预测成功率 > 80%。

#### Scenario: 多步预测执行

* **WHEN** 推理请求到达 MTPE

* **THEN** 调用 `MtpeExecutor::predict(&self, context: &Context, n: usize) -> PredictionResult`,`n ∈ [1, 10]`

* **THEN** 一次性预测 N 个后续 Token(或操作),而非逐个预测

* **THEN** 预测结果含 `predicted_tokens: Vec<Token>`、`confidence_scores: Vec<f32>`、`latency_ms: f32`

* **THEN** 预测完成后发布 `PredictionMade` 事件(携带 `quest_id`、`n`、`avg_confidence`)

#### Scenario: 预测成功率统计

* **WHEN** 预测结果被实际执行后验证

* **THEN** 统计预测成功率:`success_rate = verified_count / total_count`

* **THEN** 成功率按 N 值分组统计(N=1 成功率最高,N=10 成功率最低)

* **THEN** 每 100 次预测发布 `PredictionStatsReported` 事件(携带 `success_rate_by_n: HashMap<usize, f32>`)

* **THEN** 成功率基准:N=1 > 95%,N=5 > 85%,N=10 > 80%

#### Scenario: 失败回退机制

* **WHEN** 多步预测的某一步验证失败

* **THEN** 触发回退:丢弃失败步及后续步,从失败步重新单步预测

* **THEN** 回退后重新预测的成功率应高于继续执行(验证回退有效)

* **THEN** 回退操作发布 `PredictionRolledBack` 事件(携带 `failed_step`、`rollback_to`)

* **THEN** 回退不产生孤儿调用(经 GQEP 聚集)

#### Scenario: 预测加速比验证

* **WHEN** 对比 MTPE(N=5)与单步预测的吞吐量

* **THEN** MTPE 吞吐量 > 单步预测的 3×(5 步预测一次,减少 4 次推理调用)

* **THEN** 加速比测试:1000 次 N=5 预测 vs 5000 次单步预测,总延迟对比

* **THEN** 加速比测试不依赖真实模型 API,使用占位预测器

### Requirement: SCC 推测上下文缓存

系统 SHALL 在 `scc-cache` crate 中实现 SCC(Speculative Context Cache),基于访问模式推测性预取上下文,Draft/Verify 共享缓存,命中率 > 70%。

#### Scenario: 访问模式学习

* **WHEN** 上下文被访问时

* **THEN** SCC 记录访问模式:`(current_context_id, next_context_ids: Vec<ContextId>, transition_count: u32)`

* **THEN** 访问模式存储在 `HashMap<ContextId, Vec<(ContextId, u32)>>`(马尔可夫链模型)

* **THEN** 模式学习完成后,SCC 可预测下一步可能访问的上下文

* **THEN** 模式学习不阻塞主流程(异步后台更新)

#### Scenario: 推测性预取

* **WHEN** 当前上下文访问完成,且访问模式表明下一步访问概率 > 阈值(默认 0.6)

* **THEN** SCC 异步预取预测的上下文到缓存

* **THEN** 预取操作经 GQEP 聚集(避免孤儿调用)

* **THEN** 预取完成后发布 `CachePrefetched` 事件(携带 `prefetched_ids: Vec<ContextId>`)

* **THEN** 预取不阻塞主流程,预取失败静默处理(仅记录 warn 日志)

#### Scenario: Draft/Verify 共享缓存

* **WHEN** PVL 的 Producer(Draft)与 Verifier(Verify)需要相同上下文

* **THEN** SCC 提供 `get_or_prefetch(&self, context_id: &ContextId) -> Option<Arc<Context>>`

* **THEN** Producer 与 Verifier 共享同一缓存条目(`Arc<Context>` 引用计数)

* **THEN** 缓存命中时直接返回,未命中时触发预取并等待(或返回 None 由调用方处理)

* **THEN** 命中时发布 `CacheHit` 事件,未命中发布 `CacheMiss` 事件

#### Scenario: LRU 驱逐与命中率统计

* **WHEN** 缓存容量达到上限(默认 256 条目)

* **THEN** 按 LRU 策略驱逐最久未访问条目

* **THEN** 驱逐时不影响被 `Arc` 引用的条目(引用计数 > 1 时不驱逐)

* **THEN** 每 100 次访问发布 `CacheStatsReported` 事件(携带 `hit_rate`、`eviction_count`)

* **THEN** 命中率基准:稳定访问模式下 > 70%

### Requirement: FaaE + EDSB 语义路由与熵均衡

系统 SHALL 在 `faae-router` crate 中实现 FaaE(Function-as-Expert)语义路由与 EDSB(Entropy-Driven Self-Balancing)熵驱动自均衡,实现工具即专家的语义化路由与指数衰减负载均衡。

#### Scenario: Function-as-Expert 语义路由

* **WHEN** 工具调用请求(携带 CLV)到达 FaaE

* **THEN** FaaE 将每个工具视为"专家",基于 CLV 语义相似度路由到最适配工具

* **THEN** 路由流程:CLV → 计算与各工具专家向量的相似度 → Top-K 选择 → 返回 `RoutingResult`

* **THEN** FaaE 作为 KVBSR 的"精筛"层:KVBSR 粗筛 Top-3 块,FaaE 在块内精筛 Top-8 工具

* **THEN** 路由延迟基准:< 1ms(块内规模,通常 < 60 工具)

* **THEN** 路由完成后发布 `ExpertRouted` 事件(携带 `routed_tool`、`confidence`)

#### Scenario: EDSB 熵驱动自均衡

* **WHEN** 工具使用频率分布不均(部分工具过热,部分过冷)

* **THEN** EDSB 计算当前负载熵:`entropy = -Σ(p_i × log(p_i))`,其中 `p_i = usage_count_i / total_usage`

* **THEN** 熵值 ∈ [0, 1],0 表示完全集中(单工具),1 表示完全均匀

* **THEN** 当熵值 < 0.6 时,触发均衡:对过热工具的请求,以概率 `p = 1 - entropy` 路由到功能相似的次优工具

* **THEN** 均衡后熵值应提升(验证均衡有效)

* **THEN** 均衡操作发布 `EntropyBalanced` 事件(携带 `old_entropy`、`new_entropy`、`redistributed_count`)

#### Scenario: 指数衰减负载统计

* **WHEN** 工具使用计数随时间衰减

* **THEN** 应用指数衰减:`decayed_count = raw_count × exp(-Δt / τ)`,τ 默认 1 小时

* **THEN** 衰减后的计数用于熵值计算(而非原始计数)

* **THEN** 衰减参数 τ 可通过 `omega.yaml` 配置

* **THEN** 衰减周期默认 5 分钟,后台异步执行

#### Scenario: 工具专家注册与注销

* **WHEN** 新工具被注册或现有工具被注销

* **THEN** FaaE 更新内部专家注册表:`HashMap<ToolId, ExpertProfile>`

* **THEN** `ExpertProfile` 含 `tool_id`、`expert_vector`(64-dim f32)、`capability_tags`、`usage_count`、`last_used_at`

* **THEN** 注册/注销操作发布 `ExpertRegistered`/`ExpertUnregistered` 事件

* **THEN** 注册表更新为原子操作(`RwLock` 保护)

### Requirement: Week 4 验收门禁

系统 SHALL 在 Day 28 通过端到端执行优化验收,验证 Week 4 全部交付物协同工作。

#### Scenario: 端到端执行优化流程

* **WHEN** 执行端到端测试:任务特征 → GEA 门控激活 → FaaE 语义路由 → PVL 生产验证 → MTPE 多步预测 → GQEP 聚集执行 → SCC 缓存命中 → EDSB 熵均衡

* **THEN** 全流程无 panic、无孤儿调用、无事件丢失

* **THEN** GEA 门控计算 < 1ms

* **THEN** FaaE 语义路由 < 1ms

* **THEN** PVL 流式生成验证无竞态

* **THEN** MTPE N=5 预测成功率 > 85%

* **THEN** GQEP 10 操作聚集 < 100ms

* **THEN** SCC 命中率 > 70%

* **THEN** EDSB 熵值 > 0.6

#### Scenario: CSA(Combined System Action)延迟验证

* **WHEN** 验证全执行链路 CSA 延迟

* **THEN** CSA < 100ms(从任务特征输入到执行结果输出的端到端延迟)

* **THEN** CSA 延迟分解:GEA 1ms + FaaE 1ms + PVL 10ms + MTPE 20ms + GQEP 50ms + SCC 5ms + EDSB 5ms ≈ 92ms

* **THEN** CSA 延迟测试采用 min-of-N(5 次)减少调度噪声

#### Scenario: 全量测试与构建

* **WHEN** 运行 Week 4 验收命令

* **THEN** `cargo check --workspace --jobs 1` 通过

* **THEN** `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告

* **THEN** `cargo test --workspace --jobs 1` 全通过(Week 1 + Week 2 + Week 3 + Week 4 测试用例)

* **THEN** `cargo build --workspace --release --jobs 1` 通过

* **THEN** 新实现 crate 的测试覆盖率 > 85%(关键路径全覆盖)

* **THEN** 孤儿调用率 < 0.1%(GQEP 检测器验证)

## MODIFIED Requirements

### Requirement: 代码质量标准(继承自 Week 3 第三轮复审)

所有 Week 4 新增代码 SHALL 满足(继承前三轮复审全部要求):

* **单一职责**:每个函数 ≤ 200 行(对应 §6 架构红线),模块边界清晰

* **workspace 一致性**:使用 `workspace.dependencies` 共享依赖,禁止独立版本声明

* **错误处理显式**:库层用 `thiserror` enum,应用层用 `anyhow::Result`,禁止 `unwrap`/`expect` 在非测试代码(包括 `len()` 等看似安全的方法)

* **async 约束**:所有 async fn 满足 `Send + 'static + 'async`,经 QEEP 包装避免孤儿调用;所有 SQLite 操作必须 `spawn_blocking`

* **注释解释意图**:仅在 WHY 不明显处加注释(隐藏约束、变通方案、反直觉行为),移除显而易见注释

* **TDD-first**:核心领域类型先写类型定义与基础测试,再写业务逻辑

* **`#![forbid(unsafe_code)]`**:所有 crate 的 lib.rs 顶部保留

* **`#![warn(missing_docs, clippy::all)]`**:所有 crate 的 lib.rs 顶部保留

* **锁策略选型**:读多写少场景用 `RwLock`,读写均衡场景用 `Mutex`

* **Top-K 排序**:Top-K 选择用 `select_nth_unstable`,全排序用 `sort_by`

* **并发正确性**:所有 check-then-act 模式必须原子化(DashMap entry API 或锁内完成)

* **事务完整性**:所有批量 SQL 操作必须用 `Transaction` 的 Drop 自动回滚,或显式 ROLLBACK

* **错误传播**:禁止 `unwrap_or_default()` 静默吞错(数据库反序列化路径)

* **newtype 类型安全**:所有 ID 类型必须为 newtype struct,禁止 `pub type X = String`(使用 `nexus_core::id_newtype!` 宏)

* **Arc 共享**:大字段(CLV、content)跨结构存储时必须用 `Arc<T>` 共享

* **Week 4 强化:事件驱动闭环**:跨层向上依赖一律改为"生产者发布事件 + 消费者订阅自动应用",事件必须携带消费者所需的完整数据

* **Week 4 强化:流式通道无竞态**:所有 Producer-Verifier 模式必须使用 `mpsc` 通道,禁止共享可变状态

* **Week 4 强化:推测操作不阻塞主流程**:预取、预测等推测操作必须异步后台执行,失败静默处理(仅 warn 日志)

## REMOVED Requirements

无

---

## 附录:Week 4 关键设计决策(预填,实现阶段验证)

### A.1 GEA 门控函数选择

* **选择 Sigmoid**:连续可导,输出 ∈ (0, 1),适合门控值计算

* **不选择 ReLU**:输出无上界,不适合 [0,1] 门控

* **不选择 Softmax**:Softmax 适合多分类互斥选择,GEA 允许多专家同时激活

* **WHY 连续 [0,1] 而非二值**:连续门控允许"部分激活",对应 OMEGA 的 Ω-Sparse 稀疏化理念(非全有全无)

### A.2 GQEP 聚集策略选择

* **选择 `FuturesUnordered`**:支持流式处理(完成一个处理一个),而非 `join_all`(等待全部完成)

* **`FuturesUnordered` 优势**:内存占用更低(无需同时持有所有 Future),首个完成可立即处理

* **超时策略**:单操作超时(`tokio::time::timeout`) + 全局超时(整体聚集超时)

* **WHY 不用 `join_all`**:1000 个 Future 同时聚集时,`join_all` 内存峰值高,`FuturesUnordered` 流式处理更优

### A.3 PVL 通道选择

* **选择 `tokio::sync::mpsc`**:多生产者单消费者,适合 Producer → Verifier 单向流

* **反馈通道选择 `tokio::sync::mpsc`**:Verifier → Producer 反馈,同样单向

* **不选择 `broadcast`**:PVL 是 1:1 的 Producer-Verifier,broadcast 适合 1:N

* **不选择 `oneshot`**:PVL 需要流式多消息,oneshot 仅支持单消息

* **WHY 通道而非共享状态**:通道天然无竞态(消息所有权转移),共享状态需要锁

### A.4 MTPE 预测策略

* **Week 4 占位实现**:使用基于上下文哈希的伪预测(验证架构),Week 6 NMC 实现后接入真实模型

* **N 值选择**:N=1-10,N=1 退化为单步预测(基准),N=10 是上限(超过 10 步预测成功率过低)

* **回退策略**:失败步回退到单步预测,而非整体回滚(减少浪费)

* **WHY 不引入真实模型**:Week 4 验证执行优化架构,真实模型依赖 Week 6 NMC,避免过早工程化

### A.5 SCC 预取策略

* **马尔可夫链模型**:一阶马尔可夫链(当前状态 → 下一步状态概率),简单有效

* **不选择高阶马尔可夫链**:高阶链需要更多历史数据,Week 4 阶段一阶足够

* **不选择神经网络**:NN 预测需要训练数据,Week 4 阶段使用统计模型

* **预取阈值 0.6**:平衡预取命中率与预取消耗,过高导致预取不足,过低导致过度预取

* **WHY Draft/Verify 共享**:PVL 的 Producer 与 Verifier 访问相同上下文,共享缓存避免重复加载

### A.6 EDSB 熵均衡策略

* **香农熵**:标准信息熵公式,适用于负载分布度量

* **指数衰减**:近期使用权重更高,τ=1 小时平衡时近性与历史

* **均衡概率 `p = 1 - entropy`**:熵低(负载集中)时均衡概率高,熵高(负载均匀)时均衡概率低

* **WHY 不强制均衡**:强制均衡会破坏语义路由准确性,概率性均衡在准确性与均衡性间折中

### A.7 跨层依赖修正(继承 Week 3 经验)

| 依赖关系 | 修正方式 | 事件 |
|----------|----------|------|
| GEA(L6)→ OSA(L6) | 同层互引允许 | 无需事件 |
| GQEP(L6)→ QEEP(L4) | 向下依赖允许 | 无需事件 |
| PVL(L7)→ GQEP(L6) | 向下依赖允许 | 无需事件 |
| MTPE(L7)→ GQEP(L6) | 向下依赖允许 | 无需事件 |
| SCC(L3)→ GQEP(L6) | 向上依赖禁止 | SCC 订阅 `ContextAccessed` 事件,而非调用 GQEP |
| FaaE(L6)→ KVBSR(L6) | 同层互引允许 | 无需事件 |
| EDSB(L6)→ efficiency-monitor(L9) | 向上依赖禁止 | EDSB 发布 `EntropyBalanced` 事件,efficiency-monitor 订阅 |

### A.8 风险与缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|---------|
| PVL 通道死锁 | 高 | 中 | 通道容量上限 + 超时处理,避免 Producer 写满阻塞 |
| GQEP 聚集内存峰值 | 中 | 中 | `FuturesUnordered` 流式处理,限制并发度 |
| MTPE 预测成功率不达标 | 中 | 低 | 占位实现可控,N 值可配置降级 |
| SCC 预取过度消耗资源 | 中 | 中 | 预取阈值 0.6 + 后台异步 + 容量上限 |
| EDSB 均衡破坏语义准确性 | 中 | 中 | 概率性均衡(非强制),保留语义路由优先 |
| GEA 门控权重不合理 | 低 | 中 | 权重可配置,Week 6 NMC 后自动调优 |
| 跨层事件丢失 | 高 | 低 | Critical 事件用 mpsc 通道,Normal 事件可丢弃 |
| DashMap 写锁与 async 冲突 | 中 | 中 | 写锁释放后再调用 async 方法(Week 3 已验证) |
| 孤儿调用残留 | 高 | 低 | GQEP 集成 QEEP 检测器,CI 强制零孤儿 |
| 性能测试 flake | 低 | 中 | warmup(10 次)+ min-of-N(5 次)+ 放宽 P99 阈值 |

---

## 附录:团队组建与职责分配

### B.1 团队规模与核心能力要求

组建由 6 名资深专家级子智能体构成的协同开发团队,所有成员具备不少于 10 年行业从业经验。

| 角色 | 核心能力要求 | 负责领域 |
|------|-------------|---------|
| 架构专家 | 15 年+ Rust 架构设计,熟悉 tokio/async 生态 | GQEP 聚集执行协议设计、跨层依赖修正 |
| 并发专家 | 15 年+ 并发编程,精通 channel/lock/atomic | PVL 流式通道设计、MTPE 并发预测 |
| 性能专家 | 15 年+ 性能优化,精通 SIMD/cache/benchmark | SCC 缓存优化、EDSB 熵计算优化 |
| 安全专家 | 15 年+ 系统安全,精通 sandbox/audit | GEA 门控安全、GQEP 孤儿调用检测 |
| 实现专家 | 15 年+ Rust 实现,精通 trait/generic/macro | FaaE 路由实现、MTPE 预测器实现 |
| 质量专家 | 15 年+ 测试工程,精通 TDD/proptest/criterion | 全 crate 测试覆盖、性能基准建立 |

### B.2 RACI 责任矩阵

| Task | 架构专家 | 并发专家 | 性能专家 | 安全专家 | 实现专家 | 质量专家 |
|------|---------|---------|---------|---------|---------|---------|
| Task 1: GEA 门控激活 | A | C | C | R | R | C |
| Task 2: GQEP 聚集执行 | R | A | C | C | R | C |
| Task 3: PVL 生产验证 | C | R | C | A | R | C |
| Task 4: MTPE 多步预测 | C | R | C | C | A | C |
| Task 5: SCC 推测缓存 | C | C | A | C | R | C |
| Task 6: FaaE + EDSB | A | C | R | C | R | C |
| Task 7: Week 4 验收 | C | C | C | C | C | A |

> R = Responsible(执行), A = Accountable(负责), C = Consulted(咨询), I = Informed(知会)

### B.3 协作机制

* **每日站会**:每个 Task 开始前,主导专家与审查专家对齐设计决策

* **周例会**:Day 28 验收前,全员参与预验收,识别阻塞点

* **紧急响应**:发现 P0 级问题(数据丢失/竞态/架构违规)时,立即触发紧急复审

* **Peer Review**:每个 Task 完成后,由非主导专家进行代码审查,审查未通过返回实现

---

## 附录:MoSCoW 优先级划分

### C.1 Must Have(必须做,P0)

* GEA 门控值计算与冲突消解(Day 22)
* GQEP 聚集执行与超时治理(Day 23)
* PVL Producer-Verifier 流式通道(Day 24)
* MTPE 多步预测与失败回退(Day 25)
* SCC 推测缓存与 Draft/Verify 共享(Day 26)
* FaaE 语义路由与 EDSB 熵均衡(Day 27)
* Week 4 端到端验收(Day 28)

### C.2 Should Have(应该做,P1)

* GEA 动态激活阈值与缓存
* GQEP 批量原子性与孤儿调用检测
* PVL 实时反馈与策略调整
* MTPE 预测成功率统计与加速比验证
* SCC LRU 驱逐与命中率统计
* EDSB 指数衰减负载统计

### C.3 Could Have(可以做,P2)

* GEA 门控权重自动调优(Week 6 NMC 后)
* GQEP 全局超时与单操作超时分级
* PVL 策略调整自动验证
* MTPE N 值自适应选择
* SCC 高阶马尔可夫链
* EDSB 多维度熵均衡

### C.4 Won't Have(暂不做,P3)

* GEA 神经网络门控(Week 8 后)
* GQEP 分布式聚集(Week 7 MCP Mesh 后)
* PVL 多 Producer 并发生成(Week 8 后)
* MTPE 真实模型集成(Week 6 NMC 后)
* SCC 神经网络预取(Week 8 后)
* EDSB 强化学习均衡(Week 8 后)

---

## 附录:质量验收基准

### D.1 功能测试通过率

* 单元测试覆盖率 > 85%(关键路径全覆盖)
* 集成测试覆盖端到端流程
* 并发测试覆盖 10+ 线程场景
* 属性测试(proptest)覆盖不变量
* 错误路径测试覆盖关键故障场景

### D.2 代码质量评分

* `cargo clippy --workspace -- -D warnings` 零警告
* 单函数 ≤ 200 行
* 无 `unwrap()`/`expect()` 在非测试代码
* 无 `unsafe` 代码
* 无功能标志
* newtype 类型安全(ID 类型)

### D.3 性能指标

| 指标 | 基准 | 验证方法 |
|------|------|---------|
| GEA 门控计算延迟 | < 1ms | criterion 基准 |
| GQEP 10 操作聚集延迟 | < 100ms | criterion 基准 |
| PVL 流式生成验证无竞态 | 100 操作无 panic | 并发测试 |
| MTPE N=5 预测成功率 | > 85% | 统计测试 |
| MTPE N=5 加速比 | > 3× | 对比测试 |
| SCC 命中率 | > 70% | 稳定访问模式测试 |
| FaaE 路由延迟 | < 1ms | criterion 基准 |
| EDSB 熵值 | > 0.6 | 均衡后验证 |
| CSA 端到端延迟 | < 100ms | min-of-N(5 次) |
| 孤儿调用率 | < 0.1% | GQEP 检测器 |
