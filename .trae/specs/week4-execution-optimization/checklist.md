# Checklist — Week 4 执行优化层(L6 + L7)

> 本 checklist 对应 `tasks.md` 中 Task 23-29 的每个 SubTask,提供具体可验证的检查点。
> 每个 SubTask 完成后,勾选对应检查项。

---

## Task 23:GEA 门控专家激活

### SubTask 23.1:定义 GEA 核心类型与错误类型
- [x] `crates/gea-activator/src/types.rs` 含 `ExpertId`(newtype)、`ExpertProfile`、`GateValue`、`ActivationResult`、`TaskProfile`
- [x] `crates/gea-activator/src/error.rs` 含 `GeaError` enum(thiserror),含 `InvalidGateValue`/`ExpertNotFound`/`ConflictResolutionFailed`/`ConfigError`
- [x] `crates/gea-activator/src/config.rs` 含 `GeaConfig`,含权重 `w1/w2/w3`、`bias`、`activation_threshold`、`cache_capacity`、`overlap_threshold`
- [x] `ExpertId` 使用 `nexus_core::id_newtype!` 宏(继承 Week 3 经验)
- [x] `cargo check -p gea-activator` 通过

### SubTask 23.2:实现门控值计算(Sigmoid)
- [x] `crates/gea-activator/src/gating.rs` 含 `compute_gate_value` 函数
- [x] 公式:`gate = sigmoid(w1 × complexity + w2 × relevance + w3 × affinity - bias)`
- [x] 门控值 ∈ [0.0, 1.0],clamp 防止浮点误差
- [x] 使用标准库 `exp`(不引入新依赖)
- [x] 单元测试:门控值范围、边界值(complexity=0/1)、权重影响
- [x] 门控值计算延迟 < 1ms(criterion 基准)

### SubTask 23.3:实现专家冲突消解
- [x] `crates/gea-activator/src/conflict.rs` 含 `resolve_conflicts` 函数
- [x] 按 `gate_value × expert_priority` 综合评分排序,选 Top-K(默认 K=3)
- [x] 功能重叠度检查:基于 CLV 余弦相似度(复用 `nexus_core::cosine_similarity_slices`)
- [x] 重叠度 > 0.8 时仅保留评分更高者
- [x] Top-K 排序用 `select_nth_unstable`(继承 Week 3 经验)
- [x] 单元测试:无冲突、有冲突(重叠度 > 0.8)、Top-K 边界

### SubTask 23.4:实现 GEA 激活器主逻辑与事件发布
- [x] `crates/gea-activator/src/activator.rs` 含 `GeaActivator` 结构体
- [x] `activate` 方法:计算门控值 → 冲突消解 → 发布事件
- [x] 持有 `EventBus` 发布端,激活后发布 `ExpertActivated` 事件
- [x] 持有 `RwLock<HashMap<ExpertId, ExpertProfile>>` 专家注册表(读多写少)
- [x] 集成测试:激活流程、事件发布验证
- [x] `cargo test -p gea-activator` 通过

### SubTask 23.5:实现动态激活阈值与激活缓存
- [x] `GeaActivator` 含 `dynamic_threshold(load_factor: f32) -> f32` 方法
- [x] 阈值调整:`threshold = base_threshold + load_factor × 0.2`
- [x] `GeaActivator` 含 `activation_cache: DashMap<TaskProfileHash, ActivationResult>`(LRU,容量 128)
- [x] 相同 TaskProfile(基于 hash)5 秒内命中缓存
- [x] 缓存命中率统计:每 100 次激活发布 `ActivationCacheStats` 事件
- [x] 单元测试:阈值动态调整、缓存命中/未命中、LRU 驱逐
- [x] 缓存命中时延迟 < 0.1ms

### SubTask 23.6:GEA 并发测试与性能基准
- [x] `crates/gea-activator/tests/concurrent.rs` 含 10 线程并发 activate 测试
- [x] `crates/gea-activator/benches/gate_compute.rs` 含 criterion 基准(warmup 10 次 + P50/P99 100 次测量)
- [x] 性能断言测试标记 `#[ignore = "perf: run with --ignored"]`
- [x] 门控计算 < 1ms
- [x] 并发无竞态(无 panic、无数据竞争)

---

## Task 24:GQEP 聚集查询执行协议

### SubTask 24.1:定义 GQEP 核心类型与错误类型
- [x] `crates/gqep-executor/src/types.rs` 含 `GqepFuture`、`GatherResult`、`OperationId`(newtype)
- [x] `crates/gqep-executor/src/error.rs` 含 `GqepError` enum,含 `OperationTimeout`/`OperationFailed`/`BatchAtomicFailure`/`OrphanCallDetected`
- [x] `crates/gqep-executor/src/config.rs` 含 `GqepConfig`,含 `default_timeout_ms`、`max_concurrency`、`batch_atomic_enabled`
- [x] `OperationId` 使用 `nexus_core::id_newtype!` 宏
- [x] `cargo check -p gqep-executor` 通过

### SubTask 24.2:实现并发操作聚集(FuturesUnordered)
- [x] `crates/gqep-executor/src/gatherer.rs` 含 `GqepExecutor::gather` 方法
- [x] 使用 `tokio::stream::FuturesUnordered` 流式处理(继承 A.2 设计决策)
- [x] 每个 Future 包裹在 QEEP `EntangledCall` 中(复用 `qeep-protocol`)
- [x] 聚集完成后发布 `GatherCompleted` 事件(携带 `total`/`succeeded`/`failed`/`latency_ms`)
- [x] 单元测试:全成功、部分失败、全失败
- [x] 10 操作聚集 < 100ms

### SubTask 24.3:实现超时治理
- [x] `crates/gqep-executor/src/timeout.rs` 含 `with_timeout` 函数
- [x] 使用 `tokio::time::timeout` 包装,超时返回 `GqepError::OperationTimeout`
- [x] 支持全局超时与单操作超时(覆盖全局)
- [x] 超时事件发布 `OperationTimedOut` 事件(携带 `operation_id`/`timeout_ms`)
- [x] 单元测试:超时触发、未超时正常返回、单操作超时覆盖全局
- [x] 超时操作不计入成功数,但计入总数

### SubTask 24.4:实现批量原子性保证
- [x] `crates/gqep-executor/src/batch.rs` 含 `GqepExecutor::gather_atomic` 方法
- [x] 任一操作失败时,触发回滚回调
- [x] 回滚操作本身也经 GQEP 聚集(避免孤儿调用)
- [x] 单元测试:10 操作中第 5 个失败,验证前 4 个回滚、后 5 个不执行
- [x] 批量原子性正确

### SubTask 24.5:集成孤儿调用检测器
- [x] GQEP 内部集成 `qeep-protocol::OrphanDetector`
- [x] 检测到孤儿调用时发布 `OrphanCallDetected` 事件 `[Critical]`(携带 `operation_id`/`spawn_location`)
- [x] 单元测试:模拟孤儿调用,验证事件发布
- [x] 孤儿调用率 < 0.1%

### SubTask 24.6:GQEP 并发测试与性能基准
- [x] `crates/gqep-executor/tests/concurrent.rs` 含 100 操作并发聚集测试
- [x] `crates/gqep-executor/benches/gather.rs` 含 criterion 基准(10/50/100 操作聚集延迟)
- [x] 性能断言测试标记 `#[ignore]`
- [x] 10 操作聚集 < 100ms
- [x] 100 操作聚集 < 500ms
- [x] 并发无孤儿调用

---

## Task 25:PVL 生产验证闭环

### SubTask 25.1:定义 PVL 核心类型与错误类型
- [x] `crates/pvl-layer/src/types.rs` 含 `Operation`、`VerificationResult`、`FeedbackMessage`、`ProducerStrategy`
- [x] `crates/pvl-layer/src/error.rs` 含 `PvlError` enum,含 `ChannelClosed`/`VerificationFailed`/`StrategyAdjustmentFailed`
- [x] `crates/pvl-layer/src/config.rs` 含 `PvlConfig`,含 `channel_capacity`、`rejection_rate_threshold`、`producer_rate_limit`
- [x] `cargo check -p pvl-layer` 通过

### SubTask 25.2:实现 Producer 流式生成
- [x] `crates/pvl-layer/src/producer.rs` 含 `Producer` 结构体
- [x] `produce` 方法:流式生成操作,通过 `mpsc` 通道发送给 Verifier
- [x] 每个操作附带置信度评分(占位实现:基于内容哈希)
- [x] 生成完成后发布 `OperationProduced` 事件(携带 `quest_id`/`operation_count`/`avg_confidence`)
- [x] 通道容量上限(默认 128),避免 Producer 写满阻塞
- [x] 单元测试:生成 N 个操作、通道满时阻塞
- [x] 生成速率 10 操作/秒(占位)

### SubTask 25.3:实现 Verifier 流式验证
- [x] `crates/pvl-layer/src/verifier.rs` 含 `Verifier` 结构体
- [x] `verify` 方法:语法检查、安全检查、依赖检查(占位实现)
- [x] 验证结果通过反馈通道发送回 Producer
- [x] 验证完成后发布 `PredictionVerified` 事件(携带 `quest_id`/`verified_count`/`rejected_count`)
- [x] 单元测试:验证通过、验证拒绝、反馈发送
- [x] 验证结果正确标记

### SubTask 25.4:实现实时反馈与策略调整
- [x] `crates/pvl-layer/src/feedback.rs` 含 `FeedbackChannel`
- [x] Verifier 反馈拒绝率 > 30% 时,Producer 触发策略调整
- [x] 策略调整:降低生成速率、提高置信度阈值、切换生成策略
- [x] 策略调整发布 `ProducerStrategyAdjusted` 事件(携带 `adjustment_reason`/`new_strategy`)
- [x] 单元测试:拒绝率 > 30% 触发调整、调整后拒绝率下降
- [x] 反馈闭环有效

### SubTask 25.5:PVL 并发测试与无竞态验证
- [x] `crates/pvl-layer/tests/concurrent.rs` 含 100 个操作流式生成验证测试
- [x] 无 panic、无数据丢失、无死锁
- [x] 所有 Future 均 await 或 spawn 管理(零 void Promise)
- [x] 通道操作无竞态

### SubTask 25.6:PVL 性能基准
- [x] `crates/pvl-layer/benches/produce_verify.rs` 含 criterion 基准(10/50/100 操作流式生成验证延迟)
- [x] 性能断言测试标记 `#[ignore]`
- [x] 100 操作流式生成验证 < 1s

---

## Task 26:MTPE 多步预测执行

### SubTask 26.1:定义 MTPE 核心类型与错误类型
- [x] `crates/mtpe-executor/src/types.rs` 含 `Token`、`PredictionResult`、`PredictionStats`
- [x] `crates/mtpe-executor/src/error.rs` 含 `MtpeError` enum,含 `InvalidN`/`PredictionFailed`/`RollbackFailed`
- [x] `crates/mtpe-executor/src/config.rs` 含 `MtpeConfig`,含 `max_n`(默认 10)、`success_rate_threshold`、`rollback_enabled`
- [x] `cargo check -p mtpe-executor` 通过

### SubTask 26.2:实现多步预测执行
- [x] `crates/mtpe-executor/src/predictor.rs` 含 `MtpeExecutor::predict` 方法
- [x] N ∈ [1, 10],超出范围返回 `MtpeError::InvalidN`
- [x] 占位实现:基于上下文哈希的伪预测(验证架构)
- [x] 预测完成后发布 `PredictionMade` 事件(携带 `quest_id`/`n`/`avg_confidence`)
- [x] 单元测试:N=1/N=5/N=10 预测、N=0/N=11 错误
- [x] 预测结果含 N 个 Token

### SubTask 26.3:实现预测成功率统计
- [x] `MtpeExecutor` 含 `stats: RwLock<PredictionStats>` 字段
- [x] `record_verification` 方法:记录预测验证结果
- [x] 成功率按 N 值分组统计
- [x] 每 100 次预测发布 `PredictionStatsReported` 事件(携带 `success_rate_by_n`)
- [x] 单元测试:成功率统计、分组统计、事件发布
- [x] N=1 > 95%,N=5 > 85%,N=10 > 80%

### SubTask 26.4:实现失败回退机制
- [x] `crates/mtpe-executor/src/fallback.rs` 含 `rollback_to_single_step` 方法
- [x] 失败步回退到单步预测(N=1)
- [x] 回退操作发布 `PredictionRolledBack` 事件(携带 `failed_step`/`rollback_to`)
- [x] 回退经 GQEP 聚集(避免孤儿调用)
- [x] 单元测试:第 3 步失败回退、回退后成功率提升
- [x] 回退不产生孤儿调用

### SubTask 26.5:MTPE 加速比验证与性能基准
- [x] `crates/mtpe-executor/tests/speedup.rs` 含加速比测试(1000 次 N=5 vs 5000 次单步)
- [x] 加速比 > 3×(5 步预测一次,减少 4 次推理调用)
- [x] `crates/mtpe-executor/benches/predict.rs` 含 criterion 基准(N=1/5/10 预测延迟)
- [x] 性能断言测试标记 `#[ignore]`
- [x] 加速比 > 3×

---

## Task 27:SCC 推测上下文缓存

### SubTask 27.1:定义 SCC 核心类型与错误类型
- [x] `crates/scc-cache/src/types.rs` 含 `ContextId`(newtype)、`ContextEntry`(含 `Arc<str>` content)、`AccessPattern`、`CacheStats`
- [x] `crates/scc-cache/src/error.rs` 含 `SccError` enum,含 `CacheMiss`/`PrefetchFailed`/`PatternNotFound`
- [x] `crates/scc-cache/src/config.rs` 含 `SccConfig`,含 `capacity`(默认 256)、`prefetch_threshold`(默认 0.6)、`prefetch_enabled`
- [x] `ContextId` 使用 `nexus_core::id_newtype!` 宏
- [x] `ContextEntry.content` 使用 `Arc<str>`(继承 Week 3 Arc 共享经验)
- [x] `cargo check -p scc-cache` 通过

### SubTask 27.2:实现访问模式学习(马尔可夫链)
- [x] `crates/scc-cache/src/prefetch.rs` 含 `AccessPatternLearner`
- [x] 记录访问模式:`HashMap<ContextId, Vec<(ContextId, u32)>>`(一阶马尔可夫链)
- [x] `record_access` 方法:更新转移计数
- [x] `predict_next` 方法:返回下一步可能访问的上下文及概率
- [x] 模式学习异步后台更新(不阻塞主流程)
- [x] 单元测试:模式学习、概率预测、未知上下文

### SubTask 27.3:实现推测性预取
- [x] `AccessPatternLearner` 含 `prefetch` 方法
- [x] 访问概率 > 阈值(0.6)的上下文异步预取到缓存
- [x] SCC(L3)→GQEP(L6) 向上依赖禁止,改为 SCC 内部 spawn 后台任务
- [x] 预取完成后发布 `CachePrefetched` 事件(携带 `prefetched_ids`)
- [x] 预取不阻塞主流程,预取失败静默处理(仅 warn 日志)
- [x] 单元测试:预取触发、预取失败静默处理

### SubTask 27.4:实现 Draft/Verify 共享缓存
- [x] `crates/scc-cache/src/cache.rs` 含 `SccCache` 结构体
- [x] `get_or_prefetch` 方法:命中返回,未命中触发预取
- [x] Producer 与 Verifier 共享同一缓存条目(`Arc<ContextEntry>` 引用计数)
- [x] 命中时发布 `CacheHit` 事件,未命中发布 `CacheMiss` 事件
- [x] 单元测试:命中/未命中、Arc 共享

### SubTask 27.5:实现 LRU 驱逐与命中率统计
- [x] `crates/scc-cache/src/lru.rs` 含 LRU 驱逐策略
- [x] 缓存容量达到上限时,按 LRU 策略驱逐最久未访问条目
- [x] 驱逐时不影响被 `Arc` 引用的条目(引用计数 > 1 时不驱逐)
- [x] 每 100 次访问发布 `CacheStatsReported` 事件(携带 `hit_rate`/`eviction_count`)
- [x] 单元测试:LRU 驱逐、Arc 引用保护、命中率统计
- [x] 命中率 > 70%(稳定访问模式,性能测试标记 `#[ignore]`)

### SubTask 27.6:SCC 并发测试与性能基准
- [x] `crates/scc-cache/tests/concurrent.rs` 含 10 线程并发访问测试
- [x] `crates/scc-cache/benches/cache_hit.rs` 含 criterion 基准(命中/未命中延迟)
- [x] 性能断言测试标记 `#[ignore]`
- [x] 命中率 > 70%(性能测试,标记 `#[ignore]`)
- [x] 并发无竞态(4 个并发测试通过)

---

## Task 28:FaaE + EDSB 语义路由与熵均衡

### SubTask 28.1:定义 FaaE + EDSB 核心类型与错误类型
- [x] `crates/faae-router/src/types.rs` 含 `ExpertProfile`、`RoutingResult`、`EntropyStats`
- [x] `crates/faae-router/src/error.rs` 含 `FaaeError` enum,含 `ExpertNotFound`/`RoutingFailed`/`EntropyCalculationFailed`
- [x] `crates/faae-router/src/config.rs` 含 `FaaeConfig`,含 `top_k`、`entropy_threshold`(默认 0.6)、`decay_tau`(默认 3600 秒)、`balance_probability`
- [x] `cargo check -p faae-router` 通过

### SubTask 28.2:实现 Function-as-Expert 语义路由
- [x] `crates/faae-router/src/router.rs` 含 `FaaeRouter` 结构体
- [x] `route` 方法:基于 CLV 语义相似度路由
- [x] FaaE 作为 KVBSR 的"精筛"层:接收 KVBSR 粗筛的 Top-3 块工具集,精筛 Top-8 工具
- [x] 相似度计算复用 `nexus_core::cosine_similarity_slices`
- [x] Top-K 排序用 `select_nth_unstable`
- [x] 路由完成后发布 `ExpertRouted` 事件(携带 `routed_tool`/`confidence`)
- [x] 单元测试:路由准确率、Top-K 选择、空候选集
- [x] 路由延迟 < 1ms(块内规模)

### SubTask 28.3:实现 EDSB 熵驱动自均衡
- [x] `crates/faae-router/src/edsb.rs` 含 `EdsbBalancer` 结构体
- [x] `compute_entropy` 方法:计算当前负载熵 `entropy = -Σ(p_i × log(p_i))`
- [x] 熵值 ∈ [0, 1],归一化(除以 `log(n)`)
- [x] 当熵值 < 0.6 时,触发均衡:以概率 `p = 1 - entropy` 路由到次优工具
- [x] 均衡操作发布 `EntropyBalanced` 事件(携带 `old_entropy`/`new_entropy`/`redistributed_count`)
- [x] 单元测试:熵值计算、均衡触发、均衡后熵值提升
- [x] 熵值 > 0.6(均衡后)

### SubTask 28.4:实现指数衰减负载统计
- [x] `EdsbBalancer` 含 `decay_usage_counts` 方法
- [x] `decayed_count = raw_count × exp(-Δt / τ)`,τ 默认 1 小时
- [x] 衰减后的计数用于熵值计算
- [x] 衰减周期默认 5 分钟,后台异步执行(`tokio::spawn`)
- [x] 单元测试:衰减计算、衰减周期触发

### SubTask 28.5:实现工具专家注册与注销
- [x] `FaaeRouter` 含 `register_expert`/`unregister_expert` 方法
- [x] 专家注册表 `RwLock<HashMap<ToolId, ExpertProfile>>`(读多写少)
- [x] 注册/注销操作发布 `ExpertRegistered`/`ExpertUnregistered` 事件
- [x] 注册表更新为原子操作
- [x] 单元测试:注册/注销、并发注册

### SubTask 28.6:FaaE + EDSB 并发测试与性能基准
- [x] `crates/faae-router/tests/concurrent.rs` 含 10 线程并发路由测试
- [x] `crates/faae-router/benches/route.rs` 含 criterion 基准(路由延迟、熵计算延迟)
- [x] 性能断言测试标记 `#[ignore]`
- [x] 路由 < 1ms
- [x] 熵值 > 0.6

---

## Task 29:Week 4 端到端验收

### SubTask 29.1:新增 Week 4 所需事件类型
- [x] `crates/event-bus/src/types.rs` 含 16 个新事件类型(`ExpertActivated`/`GatherCompleted`/`OperationTimedOut`/`OrphanCallDetected`/`ProducerStrategyAdjusted`/`PredictionMade`/`PredictionStatsReported`/`PredictionRolledBack`/`CachePrefetched`/`CacheStatsReported`/`ExpertRouted`/`EntropyBalanced`/`ExpertRegistered`/`ExpertUnregistered`/`ActivationThresholdAdjusted`/`ActivationCacheStats`)
- [x] `metadata()`/`severity()`/`type_name()` match 分支已更新
- [x] 单元测试:新事件序列化/反序列化、severity 正确
- [x] `cargo test -p event-bus` 通过(23 单元 + 24 集成 + 2 文档测试)

### SubTask 29.2:端到端执行优化流程测试
- [x] 集成测试:任务特征 → GEA 门控激活 → FaaE 语义路由 → PVL 生产验证 → MTPE 多步预测 → GQEP 聚集执行 → SCC 缓存命中 → EDSB 熵均衡
- [x] 全流程无 panic、无孤儿调用、无事件丢失
- [x] GEA 门控计算 < 1ms
- [x] FaaE 语义路由 < 1ms
- [x] PVL 流式生成验证无竞态
- [x] MTPE N=5 预测成功率 > 85%
- [x] GQEP 10 操作聚集 < 100ms
- [x] SCC 命中率 > 70%
- [x] EDSB 熵值 > 0.6

### SubTask 29.3:CSA 延迟验证
- [x] CSA 延迟测试:端到端延迟 < 100ms
- [x] CSA 延迟分解:GEA 1ms + FaaE 1ms + PVL 10ms + MTPE 20ms + GQEP 50ms + SCC 5ms + EDSB 5ms ≈ 92ms
- [x] CSA 延迟测试采用 min-of-N(5 次)减少调度噪声
- [x] 性能断言测试标记 `#[ignore]`
- [x] CSA < 100ms

### SubTask 29.4:全量测试与构建验收
- [x] `cargo check --workspace --jobs 1` 通过
- [x] `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告
- [x] `cargo test --workspace --jobs 1` 全通过(Week 1-4 测试用例)
- [x] `cargo build --workspace --release --jobs 1` 通过
- [x] 新实现 crate 的测试覆盖率 > 85%
- [x] 孤儿调用率 < 0.1%(GQEP 检测器验证)

### SubTask 29.5:更新文档
- [x] `CHANGELOG.md` 含 "## Week 4 执行优化层" 章节
- [x] `CODE_WIKI.md` 含 6 个 crate 的模块职责说明
- [x] `project_memory.md` 含 Week 4 经验教训(通道无竞态、推测操作不阻塞、熵均衡策略)
- [x] 文档更新完整

### SubTask 29.6:补充 proptest 与错误路径测试
- [x] GEA proptest:门控值 ∈ [0,1]、Top-K 数 ≤ K
- [x] GQEP proptest:聚集 succeeded + failed = total、超时操作计入 total
- [x] PVL proptest:通道消息无丢失、反馈闭环有效
- [x] MTPE proptest:预测 Token 数 = N、回退后 N=1
- [x] SCC proptest:LRU 驱逐后容量恒定、命中率 ∈ [0,1]
- [x] FaaE proptest:路由结果数 ≤ top_k、熵值 ∈ [0,1]
- [x] 每个 crate 补充 5 个错误路径测试(共 30 个)
- [x] proptest 与错误路径测试通过

---

## Week 4 验收标准

- [x] P0 核心功能:6 个 crate(GEA/GQEP/PVL/MTPE/SCC/FaaE+EDSB)全部实现
- [x] P1 增强功能:动态阈值/批量原子性/反馈策略/成功率统计/LRU 驱逐/指数衰减全部实现
- [x] 端到端集成:全执行链路无 panic、无孤儿调用、无事件丢失
- [x] 性能达标:CSA < 100ms、GEA < 1ms、FaaE < 1ms、GQEP < 100ms、SCC 命中率 > 70%、EDSB 熵值 > 0.6
- [x] 代码质量:`cargo clippy --workspace -- -D warnings` 零警告、单函数 ≤ 200 行、无 `unwrap`/`expect`/`unsafe`
- [x] 测试覆盖:单元/集成/并发/属性/错误路径/性能基准六大维度全覆盖,覆盖率 > 85%
- [x] 全量验收:`cargo check/clippy/test/build --workspace --jobs 1` 全绿
- [x] Week 4 验收门禁通过,可进入 Week 5
