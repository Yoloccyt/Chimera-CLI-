# Tasks — Week 4 执行优化层(L6 + L7)

> 本任务列表基于 6 位资深专家(架构/并发/性能/安全/实现/质量)的分布式深度分析结果制定。
> 按 Day 22-28 时间线排序,共 7 个 Task(Task 23-29),39 个 SubTask。
> 每个 SubTask 完成后立即勾选对应 checklist 项。
> 严格遵循 MoSCoW 优先级:P0(Must Have)→ P1(Should Have)→ P2(Could Have)。

---

## Task 23:GEA 门控专家激活(Day 22,P0)

实现 `gea-activator` crate,连续 [0,1] 门控值计算、专家冲突消解、动态激活阈值、激活缓存。

- [x] SubTask 23.1:定义 GEA 核心类型与错误类型
  - 在 `crates/gea-activator/src/types.rs` 定义 `ExpertId`(newtype)、`ExpertProfile`、`GateValue`(f32 包装)、`ActivationResult`、`TaskProfile`(复用 OSA 的或重新定义)
  - 在 `crates/gea-activator/src/error.rs` 定义 `GeaError` enum(thiserror),含 `InvalidGateValue`/`ExpertNotFound`/`ConflictResolutionFailed`/`ConfigError`
  - 在 `crates/gea-activator/src/config.rs` 定义 `GeaConfig`,含权重 `w1/w2/w3`、`bias`、`activation_threshold`、`cache_capacity`、`overlap_threshold`
  - 文件:`crates/gea-activator/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p gea-activator` 通过

- [x] SubTask 23.2:实现门控值计算(Sigmoid)
  - 在 `crates/gea-activator/src/gating.rs` 实现 `compute_gate_value(task: &TaskProfile, expert: &ExpertProfile, config: &GeaConfig) -> f32`
  - 公式:`gate = sigmoid(w1 × complexity + w2 × relevance + w3 × affinity - bias)`
  - `sigmoid(x) = 1.0 / (1.0 + exp(-x))`,使用标准库 `exp`(不引入新依赖)
  - 门控值 ∈ [0.0, 1.0],clamp 防止浮点误差
  - 文件:`crates/gea-activator/src/gating.rs`
  - 新增单元测试:门控值范围、边界值(complexity=0/1)、权重影响
  - 验证:门控值计算延迟 < 1ms(criterion 基准)

- [x] SubTask 23.3:实现专家冲突消解
  - 在 `crates/gea-activator/src/conflict.rs` 实现 `resolve_conflicts(candidates: Vec<(ExpertId, f32)>, expert_profiles: &HashMap<ExpertId, ExpertProfile>, config: &GeaConfig) -> ActivationResult`
  - 按 `gate_value × expert_priority` 综合评分排序,选 Top-K(默认 K=3)
  - 功能重叠度检查:基于 CLV 余弦相似度(复用 `nexus_core::cosine_similarity_slices`),重叠度 > 0.8 时仅保留评分更高者
  - Top-K 排序用 `select_nth_unstable`(继承 Week 3 经验)
  - 文件:`crates/gea-activator/src/conflict.rs`
  - 新增单元测试:无冲突、有冲突(重叠度 > 0.8)、Top-K 边界
  - 验证:冲突消解正确激活 Top-K、抑制其余

- [x] SubTask 23.4:实现 GEA 激活器主逻辑与事件发布
  - 在 `crates/gea-activator/src/activator.rs` 实现 `GeaActivator` 结构体
  - `activate(&self, task: &TaskProfile) -> Result<ActivationResult, GeaError>`:计算门控值 → 冲突消解 → 发布事件
  - 持有 `EventBus` 发布端,激活后发布 `ExpertActivated` 事件(携带 `activated_experts`/`suppressed_experts`)
  - 持有 `RwLock<HashMap<ExpertId, ExpertProfile>>` 专家注册表(读多写少)
  - 文件:`crates/gea-activator/src/activator.rs`、`crates/gea-activator/src/lib.rs`
  - 新增集成测试:激活流程、事件发布验证
  - 验证:`cargo test -p gea-activator` 通过

- [x] SubTask 23.5:实现动态激活阈值与激活缓存(P1)
  - 在 `GeaActivator` 新增 `dynamic_threshold(load_factor: f32) -> f32` 方法
  - 阈值调整:`threshold = base_threshold + load_factor × 0.2`
  - 在 `GeaActivator` 新增 `activation_cache: DashMap<TaskProfileHash, ActivationResult>`(LRU,容量 128)
  - 相同 TaskProfile(基于 hash)5 秒内命中缓存
  - 缓存命中率统计:每 100 次激活发布 `ActivationCacheStats` 事件
  - 文件:`crates/gea-activator/src/activator.rs`
  - 新增单元测试:阈值动态调整、缓存命中/未命中、LRU 驱逐
  - 验证:缓存命中时延迟 < 0.1ms

- [x] SubTask 23.6:GEA 并发测试与性能基准
  - 新增并发测试:10 线程同时 activate,无 panic、无数据竞争
  - 新增 `benches/gate_compute.rs` criterion 基准:warmup(10 次)+ P50/P99(100 次测量)
  - 性能断言测试标记 `#[ignore = "perf: run with --ignored"]`(继承 Week 3 经验)
  - 文件:`crates/gea-activator/tests/concurrent.rs`、`crates/gea-activator/benches/gate_compute.rs`
  - 验证:门控计算 < 1ms,并发无竞态

---

## Task 24:GQEP 聚集查询执行协议(Day 23,P0)

实现 `gqep-executor` crate,并发异步操作聚集汇聚、超时治理、批量原子性、孤儿调用检测。

- [x] SubTask 24.1:定义 GQEP 核心类型与错误类型
  - 在 `crates/gqep-executor/src/types.rs` 定义 `GqepFuture`(包装 `Pin<Box<dyn Future<Output = Result<T, GqepError>> + Send>>`)、`GatherResult`、`OperationId`(newtype)
  - 在 `crates/gqep-executor/src/error.rs` 定义 `GqepError` enum,含 `OperationTimeout`/`OperationFailed`/`BatchAtomicFailure`/`OrphanCallDetected`
  - 在 `crates/gqep-executor/src/config.rs` 定义 `GqepConfig`,含 `default_timeout_ms`、`max_concurrency`、`batch_atomic_enabled`
  - 文件:`crates/gqep-executor/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p gqep-executor` 通过

- [x] SubTask 24.2:实现并发操作聚集(FuturesUnordered)
  - 在 `crates/gqep-executor/src/gatherer.rs` 实现 `GqepExecutor::gather(&self, futures: Vec<GqepFuture>) -> GatherResult`
  - 使用 `tokio::stream::FuturesUnordered` 流式处理(继承 A.2 设计决策)
  - 每个 Future 包裹在 QEEP `EntangledCall` 中(复用 `qeep-protocol`)
  - 聚集完成后发布 `GatherCompleted` 事件(携带 `total`/`succeeded`/`failed`/`latency_ms`)
  - 文件:`crates/gqep-executor/src/gatherer.rs`、`crates/gqep-executor/src/lib.rs`
  - 新增单元测试:全成功、部分失败、全失败
  - 验证:10 操作聚集 < 100ms

- [x] SubTask 24.3:实现超时治理
  - 在 `crates/gqep-executor/src/timeout.rs` 实现 `with_timeout(future: GqepFuture, timeout_ms: u64) -> GqepFuture`
  - 使用 `tokio::time::timeout` 包装,超时返回 `GqepError::OperationTimeout`
  - 支持全局超时(`GqepConfig.default_timeout_ms`)与单操作超时(覆盖全局)
  - 超时事件发布 `OperationTimedOut` 事件(携带 `operation_id`/`timeout_ms`)
  - 文件:`crates/gqep-executor/src/timeout.rs`
  - 新增单元测试:超时触发、未超时正常返回、单操作超时覆盖全局
  - 验证:超时操作不计入成功数,但计入总数

- [x] SubTask 24.4:实现批量原子性保证(P1)
  - 在 `crates/gqep-executor/src/batch.rs` 实现 `GqepExecutor::gather_atomic(&self, futures: Vec<GqepFuture>, rollback: impl Fn() -> Future<Output = ()>) -> GatherResult`
  - 任一操作失败时,触发回滚回调
  - 回滚操作本身也经 GQEP 聚集(避免孤儿调用)
  - 文件:`crates/gqep-executor/src/batch.rs`
  - 新增单元测试:10 操作中第 5 个失败,验证前 4 个回滚、后 5 个不执行
  - 验证:批量原子性正确

- [x] SubTask 24.5:集成孤儿调用检测器(P1)
  - GQEP 内部集成 `qeep-protocol::OrphanDetector`
  - 检测到孤儿调用时发布 `OrphanCallDetected` 事件 `[Critical]`(携带 `operation_id`/`spawn_location`)
  - 文件:`crates/gqep-executor/src/gatherer.rs`
  - 新增单元测试:模拟孤儿调用,验证事件发布
  - 验证:孤儿调用率 < 0.1%

- [x] SubTask 24.6:GQEP 并发测试与性能基准
  - 新增并发测试:100 操作并发聚集,无 panic、无孤儿调用
  - 新增 `benches/gather.rs` criterion 基准:10/50/100 操作聚集延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/gqep-executor/tests/concurrent.rs`、`crates/gqep-executor/benches/gather.rs`
  - 验证:10 操作聚集 < 100ms,100 操作聚集 < 500ms

---

## Task 25:PVL 生产验证闭环(Day 24,P0)

实现 `pvl-layer` crate,Producer-Verifier 并行流式生成验证、实时反馈通道、策略调整。

- [x] SubTask 25.1:定义 PVL 核心类型与错误类型
  - 在 `crates/pvl-layer/src/types.rs` 定义 `Operation`(含 `operation_id`/`content`/`confidence`/`status`)、`VerificationResult`、`FeedbackMessage`、`ProducerStrategy`
  - 在 `crates/pvl-layer/src/error.rs` 定义 `PvlError` enum,含 `ChannelClosed`/`VerificationFailed`/`StrategyAdjustmentFailed`
  - 在 `crates/pvl-layer/src/config.rs` 定义 `PvlConfig`,含 `channel_capacity`、`rejection_rate_threshold`、`producer_rate_limit`
  - 文件:`crates/pvl-layer/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p pvl-layer` 通过

- [x] SubTask 25.2:实现 Producer 流式生成
  - 在 `crates/pvl-layer/src/producer.rs` 实现 `Producer` 结构体
  - `produce(&self, quest_id: &str, count: usize) -> Result<(), PvlError>`:流式生成操作,通过 `mpsc` 通道发送给 Verifier
  - 每个操作附带置信度评分(占位实现:基于内容哈希)
  - 生成完成后发布 `OperationProduced` 事件(携带 `quest_id`/`operation_count`/`avg_confidence`)
  - 通道容量上限(默认 128),避免 Producer 写满阻塞
  - 文件:`crates/pvl-layer/src/producer.rs`
  - 新增单元测试:生成 N 个操作、通道满时阻塞
  - 验证:生成速率 10 操作/秒(占位)

- [x] SubTask 25.3:实现 Verifier 流式验证
  - 在 `crates/pvl-layer/src/verifier.rs` 实现 `Verifier` 结构体
  - `verify(&self, operation: Operation) -> VerificationResult`:语法检查、安全检查、依赖检查(占位实现)
  - 验证结果通过反馈通道发送回 Producer
  - 验证完成后发布 `PredictionVerified` 事件(携带 `quest_id`/`verified_count`/`rejected_count`)
  - 文件:`crates/pvl-layer/src/verifier.rs`
  - 新增单元测试:验证通过、验证拒绝、反馈发送
  - 验证:验证结果正确标记

- [x] SubTask 25.4:实现实时反馈与策略调整(P1)
  - 在 `crates/pvl-layer/src/feedback.rs` 实现 `FeedbackChannel`
  - Verifier 反馈拒绝率 > 30% 时,Producer 触发策略调整
  - 策略调整:降低生成速率、提高置信度阈值、切换生成策略
  - 策略调整发布 `ProducerStrategyAdjusted` 事件(携带 `adjustment_reason`/`new_strategy`)
  - 文件:`crates/pvl-layer/src/feedback.rs`、`crates/pvl-layer/src/producer.rs`
  - 新增单元测试:拒绝率 > 30% 触发调整、调整后拒绝率下降
  - 验证:反馈闭环有效

- [x] SubTask 25.5:PVL 并发测试与无竞态验证
  - 新增并发测试:100 个操作流式生成验证,无 panic、无数据丢失、无死锁
  - 验证所有 Future 均 await 或 spawn 管理(零 void Promise)
  - 文件:`crates/pvl-layer/tests/concurrent.rs`
  - 验证:通道操作无竞态

- [x] SubTask 25.6:PVL 性能基准
  - 新增 `benches/produce_verify.rs` criterion 基准:10/50/100 操作流式生成验证延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/pvl-layer/benches/produce_verify.rs`
  - 验证:100 操作流式生成验证 < 1s

---

## Task 26:MTPE 多步预测执行(Day 25,P0)

实现 `mtpe-executor` crate,N=1-10 多步预测、预测成功率统计、失败回退机制。

- [x] SubTask 26.1:定义 MTPE 核心类型与错误类型
  - 在 `crates/mtpe-executor/src/types.rs` 定义 `Token`、`PredictionResult`(含 `predicted_tokens`/`confidence_scores`/`latency_ms`)、`PredictionStats`
  - 在 `crates/mtpe-executor/src/error.rs` 定义 `MtpeError` enum,含 `InvalidN`/`PredictionFailed`/`RollbackFailed`
  - 在 `crates/mtpe-executor/src/config.rs` 定义 `MtpeConfig`,含 `max_n`(默认 10)、`success_rate_threshold`、`rollback_enabled`
  - 文件:`crates/mtpe-executor/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p mtpe-executor` 通过

- [x] SubTask 26.2:实现多步预测执行
  - 在 `crates/mtpe-executor/src/predictor.rs` 实现 `MtpeExecutor::predict(&self, context: &Context, n: usize) -> Result<PredictionResult, MtpeError>`
  - N ∈ [1, 10],超出范围返回 `MtpeError::InvalidN`
  - 占位实现:基于上下文哈希的伪预测(验证架构)
  - 预测完成后发布 `PredictionMade` 事件(携带 `quest_id`/`n`/`avg_confidence`)
  - 文件:`crates/mtpe-executor/src/predictor.rs`、`crates/mtpe-executor/src/lib.rs`
  - 新增单元测试:N=1/N=5/N=10 预测、N=0/N=11 错误
  - 验证:预测结果含 N 个 Token

- [x] SubTask 26.3:实现预测成功率统计(P1)
  - 在 `MtpeExecutor` 新增 `stats: RwLock<PredictionStats>` 字段
  - `record_verification(&self, n: usize, success: bool)`:记录预测验证结果
  - 成功率按 N 值分组统计
  - 每 100 次预测发布 `PredictionStatsReported` 事件(携带 `success_rate_by_n`)
  - 文件:`crates/mtpe-executor/src/predictor.rs`
  - 新增单元测试:成功率统计、分组统计、事件发布
  - 验证:N=1 > 95%,N=5 > 85%,N=10 > 80%

- [x] SubTask 26.4:实现失败回退机制
  - 在 `crates/mtpe-executor/src/fallback.rs` 实现 `rollback_to_single_step(&self, failed_step: usize) -> Result<PredictionResult, MtpeError>`
  - 失败步回退到单步预测(N=1)
  - 回退操作发布 `PredictionRolledBack` 事件(携带 `failed_step`/`rollback_to`)
  - 回退经 GQEP 聚集(避免孤儿调用)
  - 文件:`crates/mtpe-executor/src/fallback.rs`
  - 新增单元测试:第 3 步失败回退、回退后成功率提升
  - 验证:回退不产生孤儿调用

- [x] SubTask 26.5:MTPE 加速比验证与性能基准
  - 新增加速比测试:1000 次 N=5 预测 vs 5000 次单步预测,总延迟对比
  - 加速比 > 3×(5 步预测一次,减少 4 次推理调用)
  - 新增 `benches/predict.rs` criterion 基准:N=1/5/10 预测延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/mtpe-executor/tests/speedup.rs`、`crates/mtpe-executor/benches/predict.rs`
  - 验证:加速比 > 3×

---

## Task 27:SCC 推测上下文缓存(Day 26,P0)

实现 `scc-cache` crate,访问模式学习、推测性预取、Draft/Verify 共享缓存、LRU 驱逐。

- [x] SubTask 27.1:定义 SCC 核心类型与错误类型
  - 在 `crates/scc-cache/src/types.rs` 定义 `ContextId`(newtype)、`ContextEntry`(含 `Arc<str>` content)、`AccessPattern`、`CacheStats`
  - 在 `crates/scc-cache/src/error.rs` 定义 `SccError` enum,含 `CacheMiss`/`PrefetchFailed`/`PatternNotFound`
  - 在 `crates/scc-cache/src/config.rs` 定义 `SccConfig`,含 `capacity`(默认 256)、`prefetch_threshold`(默认 0.6)、`prefetch_enabled`
  - 文件:`crates/scc-cache/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p scc-cache` 通过

- [x] SubTask 27.2:实现访问模式学习(马尔可夫链)
  - 在 `crates/scc-cache/src/prefetch.rs` 实现 `AccessPatternLearner`
  - 记录访问模式:`HashMap<ContextId, Vec<(ContextId, u32)>>`(一阶马尔可夫链)
  - `record_access(&self, current: &ContextId, next: &ContextId)`:更新转移计数
  - `predict_next(&self, current: &ContextId) -> Vec<(ContextId, f32)>`:返回下一步可能访问的上下文及概率
  - 模式学习异步后台更新(不阻塞主流程)
  - 文件:`crates/scc-cache/src/prefetch.rs`
  - 新增单元测试:模式学习、概率预测、未知上下文
  - 验证:模式学习正确

- [x] SubTask 27.3:实现推测性预取
  - 在 `AccessPatternLearner` 新增 `prefetch(&self, current: &ContextId, cache: &SccCache) -> Vec<ContextId>`
  - 访问概率 > 阈值(0.6)的上下文异步预取到缓存
  - 预取操作经 GQEP 聚集(避免孤儿调用)— 注意:SCC(L3)→GQEP(L6) 向上依赖禁止,改为 SCC 内部 spawn 后台任务
  - 预取完成后发布 `CachePrefetched` 事件(携带 `prefetched_ids`)
  - 预取不阻塞主流程,预取失败静默处理(仅 warn 日志)
  - 文件:`crates/scc-cache/src/prefetch.rs`
  - 新增单元测试:预取触发、预取失败静默处理
  - 验证:预取不阻塞主流程

- [x] SubTask 27.4:实现 Draft/Verify 共享缓存
  - 在 `crates/scc-cache/src/cache.rs` 实现 `SccCache` 结构体
  - `get_or_prefetch(&self, context_id: &ContextId) -> Option<Arc<ContextEntry>>`:命中返回,未命中触发预取
  - Producer 与 Verifier 共享同一缓存条目(`Arc<ContextEntry>` 引用计数)
  - 命中时发布 `CacheHit` 事件,未命中发布 `CacheMiss` 事件
  - 文件:`crates/scc-cache/src/cache.rs`、`crates/scc-cache/src/lib.rs`
  - 新增单元测试:命中/未命中、Arc 共享
  - 验证:Draft/Verify 共享缓存正确

- [x] SubTask 27.5:实现 LRU 驱逐与命中率统计(P1)
  - 在 `crates/scc-cache/src/lru.rs` 实现 LRU 驱逐策略
  - 缓存容量达到上限时,按 LRU 策略驱逐最久未访问条目
  - 驱逐时不影响被 `Arc` 引用的条目(引用计数 > 1 时不驱逐)
  - 每 100 次访问发布 `CacheStatsReported` 事件(携带 `hit_rate`/`eviction_count`)
  - 文件:`crates/scc-cache/src/lru.rs`、`crates/scc-cache/src/cache.rs`
  - 新增单元测试:LRU 驱逐、Arc 引用保护、命中率统计
  - 验证:命中率 > 70%(稳定访问模式)

- [x] SubTask 27.6:SCC 并发测试与性能基准
  - 新增并发测试:10 线程并发访问,无 panic、无数据竞争
  - 新增 `benches/cache_hit.rs` criterion 基准:命中/未命中延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/scc-cache/tests/concurrent.rs`、`crates/scc-cache/benches/cache_hit.rs`
  - 验证:命中率 > 70%,并发无竞态

---

## Task 28:FaaE + EDSB 语义路由与熵均衡(Day 27,P0)

实现 `faae-router` crate,Function-as-Expert 语义路由、EDSB 熵驱动自均衡、指数衰减负载统计。

- [x] SubTask 28.1:定义 FaaE + EDSB 核心类型与错误类型
  - 在 `crates/faae-router/src/types.rs` 定义 `ExpertProfile`(含 `tool_id`/`expert_vector`/`capability_tags`/`usage_count`/`last_used_at`)、`RoutingResult`、`EntropyStats`
  - 在 `crates/faae-router/src/error.rs` 定义 `FaaeError` enum,含 `ExpertNotFound`/`RoutingFailed`/`EntropyCalculationFailed`
  - 在 `crates/faae-router/src/config.rs` 定义 `FaaeConfig`,含 `top_k`、`entropy_threshold`(默认 0.6)、`decay_tau`(默认 3600 秒)、`balance_probability`
  - 文件:`crates/faae-router/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p faae-router` 通过

- [x] SubTask 28.2:实现 Function-as-Expert 语义路由
  - 在 `crates/faae-router/src/router.rs` 实现 `FaaeRouter` 结构体
  - `route(&self, clv: &CLV, candidate_tools: &[ToolId]) -> Result<RoutingResult, FaaeError>`:基于 CLV 语义相似度路由
  - FaaE 作为 KVBSR 的"精筛"层:接收 KVBSR 粗筛的 Top-3 块工具集,精筛 Top-8 工具
  - 相似度计算复用 `nexus_core::cosine_similarity_slices`
  - Top-K 排序用 `select_nth_unstable`
  - 路由完成后发布 `ExpertRouted` 事件(携带 `routed_tool`/`confidence`)
  - 文件:`crates/faae-router/src/router.rs`、`crates/faae-router/src/lib.rs`
  - 新增单元测试:路由准确率、Top-K 选择、空候选集
  - 验证:路由延迟 < 1ms(块内规模)

- [x] SubTask 28.3:实现 EDSB 熵驱动自均衡
  - 在 `crates/faae-router/src/edsb.rs` 实现 `EdsbBalancer` 结构体
  - `compute_entropy(&self) -> f32`:计算当前负载熵 `entropy = -Σ(p_i × log(p_i))`
  - 熵值 ∈ [0, 1],归一化(除以 `log(n)`)
  - 当熵值 < 0.6 时,触发均衡:对过热工具的请求,以概率 `p = 1 - entropy` 路由到功能相似的次优工具
  - 均衡操作发布 `EntropyBalanced` 事件(携带 `old_entropy`/`new_entropy`/`redistributed_count`)
  - 文件:`crates/faae-router/src/edsb.rs`
  - 新增单元测试:熵值计算、均衡触发、均衡后熵值提升
  - 验证:熵值 > 0.6(均衡后)

- [x] SubTask 28.4:实现指数衰减负载统计(P1)
  - 在 `EdsbBalancer` 新增 `decay_usage_counts(&self)`:应用指数衰减
  - `decayed_count = raw_count × exp(-Δt / τ)`,τ 默认 1 小时
  - 衰减后的计数用于熵值计算
  - 衰减周期默认 5 分钟,后台异步执行(`tokio::spawn`)
  - 文件:`crates/faae-router/src/edsb.rs`
  - 新增单元测试:衰减计算、衰减周期触发
  - 验证:衰减后计数正确

- [x] SubTask 28.5:实现工具专家注册与注销
  - 在 `FaaeRouter` 新增 `register_expert(&self, profile: ExpertProfile)`/`unregister_expert(&self, tool_id: &ToolId)`
  - 专家注册表 `RwLock<HashMap<ToolId, ExpertProfile>>`(读多写少)
  - 注册/注销操作发布 `ExpertRegistered`/`ExpertUnregistered` 事件
  - 注册表更新为原子操作
  - 文件:`crates/faae-router/src/router.rs`、`crates/faae-router/src/expert.rs`
  - 新增单元测试:注册/注销、并发注册
  - 验证:注册表更新原子性

- [x] SubTask 28.6:FaaE + EDSB 并发测试与性能基准
  - 新增并发测试:10 线程并发路由,无 panic、无数据竞争
  - 新增 `benches/route.rs` criterion 基准:路由延迟、熵计算延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/faae-router/tests/concurrent.rs`、`crates/faae-router/benches/route.rs`
  - 验证:路由 < 1ms,熵值 > 0.6

---

## Task 29:Week 4 端到端验收(Day 28,P0)

全执行链路集成测试、CSA 延迟验证、全量构建验收。

- [x] SubTask 29.1:新增 Week 4 所需事件类型
  - 在 `crates/event-bus/src/types.rs` 新增事件:`ExpertActivated`/`GatherCompleted`/`OperationTimedOut`/`OrphanCallDetected`/`ProducerStrategyAdjusted`/`PredictionMade`/`PredictionStatsReported`/`PredictionRolledBack`/`CachePrefetched`/`CacheStatsReported`/`ExpertRouted`/`EntropyBalanced`/`ExpertRegistered`/`ExpertUnregistered`/`ActivationThresholdAdjusted`/`ActivationCacheStats`
  - 更新 `metadata()`/`severity()`/`type_name()` match 分支
  - 文件:`crates/event-bus/src/types.rs`
  - 新增单元测试:新事件序列化/反序列化、severity 正确
  - 验证:`cargo test -p event-bus` 通过(23 单元 + 24 集成 + 2 文档测试全绿)

- [x] SubTask 29.2:端到端执行优化流程测试
  - 新增集成测试:任务特征 → GEA 门控激活 → FaaE 语义路由 → PVL 生产验证 → MTPE 多步预测 → GQEP 聚集执行 → SCC 缓存命中 → EDSB 熵均衡
  - 验证全流程无 panic、无孤儿调用、无事件丢失
  - 文件:`crates/gea-activator/tests/e2e.rs`(或独立集成测试目录)
  - 验证:全流程通过

- [x] SubTask 29.3:CSA 延迟验证
  - 新增 CSA 延迟测试:端到端延迟 < 100ms
  - CSA 延迟分解:GEA 1ms + FaaE 1ms + PVL 10ms + MTPE 20ms + GQEP 50ms + SCC 5ms + EDSB 5ms ≈ 92ms
  - CSA 延迟测试采用 min-of-N(5 次)减少调度噪声
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/gea-activator/tests/csa.rs`
  - 验证:CSA < 100ms

- [x] SubTask 29.4:全量测试与构建验收
  - 运行 `cargo check --workspace --jobs 1` 通过
  - 运行 `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告
  - 运行 `cargo test --workspace --jobs 1` 全通过(Week 1-4 测试用例)
  - 运行 `cargo build --workspace --release --jobs 1` 通过
  - 验证新实现 crate 的测试覆盖率 > 85%
  - 验证孤儿调用率 < 0.1%(GQEP 检测器)
  - 文件:无(验证步骤)
  - 验证:全量验收通过

- [x] SubTask 29.5:更新文档(CHANGELOG/CODE_WIKI/project_memory)
  - 更新 `CHANGELOG.md`:新增 "## Week 4 执行优化层" 章节
  - 更新 `CODE_WIKI.md`:新增 6 个 crate 的模块职责说明
  - 更新 `project_memory.md`:记录 Week 4 经验教训(通道无竞态、推测操作不阻塞、熵均衡策略)
  - 文件:`CHANGELOG.md`、`CODE_WIKI.md`、`c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`
  - 验证:文档更新完整

- [x] SubTask 29.6:补充 proptest 与错误路径测试
  - 每个 crate 补充 proptest(不变量验证):
    - GEA:门控值 ∈ [0,1]、Top-K 数 ≤ K
    - GQEP:聚集 succeeded + failed = total、超时操作计入 total
    - PVL:通道消息无丢失、反馈闭环有效
    - MTPE:预测 Token 数 = N、回退后 N=1
    - SCC:LRU 驱逐后容量恒定、命中率 ∈ [0,1]
    - FaaE:路由结果数 ≤ top_k、熵值 ∈ [0,1]
  - 每个 crate 补充错误路径测试(5 个/crate,共 30 个)
  - 文件:6 个 crate 的 `tests/proptest.rs`、`tests/error_paths.rs`
  - 验证:proptest 与错误路径测试通过

---

## Task Dependencies

- Task 23(GEA)→ 无依赖,优先执行
- Task 24(GQEP)→ 无依赖,可与 Task 23 并行
- Task 25(PVL)→ 依赖 Task 24(GQEP 聚集 PVL 的操作)
- Task 26(MTPE)→ 依赖 Task 24(GQEP 聚集 MTPE 的回退)
- Task 27(SCC)→ 无依赖,可与 Task 23/24 并行(注意:SCC 不依赖 GQEP,内部 spawn)
- Task 28(FaaE + EDSB)→ 无依赖,可与 Task 23/24/27 并行
- Task 29(验收)→ 依赖 Task 23-28(全部完成后验收)

## 优先级执行顺序

1. **第一批(并行)**:Task 23(GEA)+ Task 24(GQEP)+ Task 27(SCC)+ Task 28(FaaE + EDSB)
2. **第二批(并行)**:Task 25(PVL,依赖 GQEP)+ Task 26(MTPE,依赖 GQEP)
3. **第三批**:Task 29(验收,依赖全部完成)

## 关键路径

Task 24(GQEP)→ Task 25(PVL)/ Task 26(MTPE)→ Task 29(验收)

GQEP 是关键路径上的核心组件,PVL 与 MTPE 均依赖 GQEP 聚集操作,需优先完成 GQEP。
