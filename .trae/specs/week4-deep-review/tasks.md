# Tasks — Week 4 深度复审

> 本任务列表基于 6 类资深专家（架构/并发/性能/安全/实现/质量）的分布式深度分析。
> 按 crate 维度组织，6 个 crate × 6 个复审维度 = 36 个审查任务 + 1 个综合报告任务。
> 严格遵循只读审查原则，不修改任何源代码。

---

## Task 30:GEA 门控专家激活 — 深度复审（Day 1，P0）

对 `crates/gea-activator/` 全部 7 个源文件进行 6 维度复审。

- [x] SubTask 30.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：GEA(L6) 仅依赖 L1-L5，无向上依赖
  - 审查 `use` 语句：无跨层直接调用（如直接 import L7/L8 crate）
  - 扫描函数行数：`activator.rs`/`gating.rs`/`conflict.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码：非测试代码无 `unsafe` 块
  - 验证 `#![forbid(unsafe_code)]` 属性存在

- [x] SubTask 30.2:并发正确性审查
  - 审查 `activator.rs` 中 `RwLock<HashMap<ExpertId, ExpertProfile>>` 使用
  - 验证读锁守卫不跨越 `.await` 点（`activate` 方法中锁释放后再 `publish` 事件）
  - 审查 `DashMap` 激活缓存使用：写锁在 async 调用前释放
  - 验证 `AtomicU64` 缓存统计的 `Ordering` 选型（Relaxed 用于统计）

- [x] SubTask 30.3:性能优化审查
  - 验证 `conflict.rs` Top-K 使用 `select_nth_unstable`（O(n)）
  - 验证 `gating.rs` Sigmoid 计算使用标准库 `exp`（无外部依赖）
  - 审查 `hash_task_profile` 哈希效率（缓存键计算）
  - 验证 `dynamic_threshold` 计算复杂度 O(1)

- [x] SubTask 30.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`/`panic!()`
  - 审查 `pub` 项文档注释覆盖率
  - 验证 `GeaError` 使用 `thiserror` 派生
  - 审查 WHY 注释：Sigmoid 选型、LRU 缓存 TTL、动态阈值公式

- [x] SubTask 30.5:事件完整性审查
  - 验证 `ExpertActivated` 事件发布点与 spec Scenario 对应
  - 验证 `ActivationCacheStats` 事件发布频率（每 100 次激活）
  - 验证 `ActivationThresholdAdjusted` 事件（若实现）
  - 验证事件发布失败不阻塞主流程

- [x] SubTask 30.6:测试质量审查
  - 审查单元测试：门控值范围、边界值、权重影响
  - 审查冲突消解测试：无冲突、有冲突（重叠度 > 0.8）、Top-K 边界
  - 审查并发测试：10 线程同时 activate
  - 审查性能测试：`#[ignore]` 标记与阈值合理性

---

## Task 31:GQEP 聚集查询执行协议 — 深度复审（Day 1，P0）

对 `crates/gqep-executor/` 全部 7 个源文件进行 6 维度复审。

- [x] SubTask 31.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：GQEP(L6) 仅依赖 L1-L5 + `qeep-protocol`(L4)
  - 验证 `qeep-protocol` 依赖方向合规（L6→L4 向下依赖允许）
  - 扫描函数行数：`gatherer.rs`/`batch.rs`/`timeout.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码

- [x] SubTask 31.2:并发正确性审查
  - 审查 `gatherer.rs` 中 `FuturesUnordered` 使用：流式处理无竞态
  - 验证 QEEP `EntangledCall` 集成：`entangle` 方法正确包裹 Future
  - 审查 `batch.rs` 批量原子性：回滚回调的并发安全
  - 验证孤儿调用检测器集成：`OrphanDetector` 正确初始化与查询

- [x] SubTask 31.3:性能优化审查
  - 验证 `FuturesUnordered` 选型（vs `join_all`）：流式处理低内存
  - 审查 `timeout.rs` 超时包装效率：`tokio::time::timeout` 零分配
  - 验证聚集延迟基准：10 操作 < 100ms，100 操作 < 500ms

- [x] SubTask 31.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`
  - 审查 `GqepError` enum 完整性：`OperationTimeout`/`OperationFailed`/`BatchAtomicFailure`/`OrphanCallDetected`
  - 审查 `GqepFuture` 类型别名定义
  - 验证 WHY 注释：FuturesUnordered 选型理由、QEEP 集成理由

- [x] SubTask 31.5:事件完整性审查
  - 验证 `GatherCompleted` 事件载荷：`total`/`succeeded`/`failed`/`latency_ms`
  - 验证 `OperationTimedOut` 事件载荷：`operation_id`/`timeout_ms`
  - 验证 `OrphanCallDetected` 事件（Critical 级别）
  - 验证事件发布失败不阻塞聚集结果返回

- [x] SubTask 31.6:测试质量审查
  - 审查聚集测试：全成功、部分失败、全失败
  - 审查超时测试：超时触发、未超时正常返回
  - 审查批量原子性测试：10 操作中第 5 个失败，验证回滚
  - 审查孤儿调用测试：模拟孤儿调用，验证事件发布

---

## Task 32:PVL 生产验证闭环 — 深度复审（Day 2，P0）

对 `crates/pvl-layer/` 全部 7 个源文件进行 6 维度复审。

- [x] SubTask 32.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：PVL(L7) 仅依赖 L1-L6
  - 验证无向上依赖（L8 Parliament 等）
  - 扫描函数行数：`producer.rs`/`verifier.rs`/`feedback.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码

- [x] SubTask 32.2:并发正确性审查
  - **重点**：审查 `verifier.rs` 中 `mpsc` 通道使用
  - 验证 `feedback_tx` 在 `verifier.run()` 返回后正确 `drop`（继承 project_memory 死锁教训）
  - 审查 `producer.rs` 中 `mpsc::Sender` 使用：通道满时阻塞行为
  - 验证反馈通道闭环无死锁：Verifier 写入反馈 → Producer 读取反馈
  - 验证所有 Future 均 `await` 或 `spawn` 管理（零 void Promise）

- [x] SubTask 32.3:性能优化审查
  - 审查通道容量上限（默认 128）合理性
  - 验证 Producer 生成速率控制（10 操作/秒占位实现）
  - 审查 `Operation` 结构体内存布局（是否含不必要的大字段）

- [x] SubTask 32.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`
  - 审查 `PvlError` enum：`ChannelClosed`/`VerificationFailed`/`StrategyAdjustmentFailed`
  - 审查 WHY 注释：通道死锁规避方案、反馈闭环设计
  - 验证 `ProducerStrategy` 枚举定义完整性

- [x] SubTask 32.5:事件完整性审查
  - 验证 `OperationProduced` 事件载荷：`quest_id`/`operation_count`/`avg_confidence`
  - 验证 `PredictionVerified` 事件载荷：`op_id`/`score`
  - 验证 `ProducerStrategyAdjusted` 事件载荷：`adjustment_reason`/`new_strategy`
  - 验证 `VerificationFailed` 事件（若定义）

- [x] SubTask 32.6:测试质量审查
  - 审查 Producer 测试：生成 N 个操作、通道满时阻塞
  - 审查 Verifier 测试：验证通过、验证拒绝、反馈发送
  - 审查并发测试：100 个操作流式生成验证，无 panic/无死锁
  - 验证死锁修复测试：`drop(feedback_tx)` 后 `recv()` 正常返回

---

## Task 33:MTPE 多步预测执行 — 深度复审（Day 2，P0）

对 `crates/mtpe-executor/` 全部 6 个源文件进行 6 维度复审。

- [x] SubTask 33.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：MTPE(L7) 仅依赖 L1-L6
  - 验证无向上依赖
  - 扫描函数行数：`predictor.rs`/`fallback.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码

- [x] SubTask 33.2:并发正确性审查
  - 审查 `predictor.rs` 中 `RwLock<PredictionStats>` 使用
  - 验证统计读写锁不跨越 `.await` 点
  - 审查 `AtomicU64` 预测计数器的 `Ordering` 选型
  - 验证 `fallback.rs` 回退操作的并发安全

- [x] SubTask 33.3:性能优化审查
  - 审查 `predict` 方法延迟：`SIMULATED_INFERENCE_DELAY` 合理性
  - 验证 `compute_context_hash` 哈希效率
  - 审查 `generate_pseudo_predictions` 伪预测生成复杂度
  - 验证加速比基准：N=5 vs 单步 > 3×

- [x] SubTask 33.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`
  - 审查 `MtpeError` enum：`InvalidN`/`PredictionFailed`/`RollbackFailed`
  - 审查 `MtpeConfig::is_valid_n` 验证逻辑
  - 验证 WHY 注释：伪预测实现理由、回退机制设计

- [x] SubTask 33.5:事件完整性审查
  - 验证 `PredictionMade` 事件载荷：`quest_id`/`n`/`avg_confidence`
  - 验证 `PredictionStatsReported` 事件载荷：`success_rate_by_n`
  - 验证 `PredictionRolledBack` 事件载荷：`failed_step`/`rollback_to`
  - 验证事件发布频率（每 100 次预测）

- [x] SubTask 33.6:测试质量审查
  - 审查预测测试：N=1/N=5/N=10、N=0/N=11 错误
  - 审查成功率统计测试：分组统计、事件发布
  - 审查回退测试：第 3 步失败回退、回退后成功率提升
  - 审查加速比测试：1000 次 N=5 vs 5000 次单步

---

## Task 34:SCC 推测上下文缓存 — 深度复审（Day 3，P0）

对 `crates/scc-cache/` 全部 7 个源文件进行 6 维度复审。

- [x] SubTask 34.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：SCC(L3) 仅依赖 L1-L2
  - **重点**：验证 SCC(L3) 不依赖 L6/L7（向上依赖禁止）
  - 验证 SCC 内部 `spawn` 后台预取任务（不通过 GQEP 聚集，因 L3→L6 向上禁止）
  - 扫描函数行数：`cache.rs`/`prefetch.rs`/`lru.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码

- [x] SubTask 34.2:并发正确性审查
  - **重点**：审查 `lru.rs` 中 `Arc::strong_count` TOCTOU 竞态
  - 验证 TOCTOU 竞态有注释说明可接受性
  - 审查 `cache.rs` 中 `DashMap` 使用：`insert_lock` 临界区原子性
  - 审查 `prefetch.rs` 中 `std::sync::RwLock` 选型（同步方法，非 async）
  - 验证 `record_access_background` 的 `Arc<Self>` 模式：`RwLockWriteGuard` 不进入 Future 状态机

- [x] SubTask 34.3:性能优化审查
  - 验证 LRU 驱逐 O(n) 扫描在 256 条目规模下可接受
  - 审查 `Arc<str>` 内容共享（`ContextEntry.content`）
  - 验证马尔可夫链 `predict_next` 概率计算效率
  - 审查 `get_or_prefetch` 命中路径延迟（< 0.1ms 目标）

- [x] SubTask 34.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`
  - 审查 `SccError` enum：`CacheMiss`/`PrefetchFailed`/`PatternNotFound`
  - 审查 WHY 注释：逻辑时钟 vs 墙钟时间、Arc 引用保护、std::sync vs tokio::sync 选型
  - 验证 `ContextId` 使用 `nexus_core::id_newtype!` 宏

- [x] SubTask 34.5:事件完整性审查
  - 验证 `CacheHit`/`CacheMiss` 事件发布
  - 验证 `CachePrefetched` 事件载荷：`prefetched_ids`
  - 验证 `CacheStatsReported` 事件载荷：`hit_rate`/`eviction_count`
  - 验证 `publish_blocking` 用于同步上下文（`get_or_prefetch` 是同步方法）

- [x] SubTask 34.6:测试质量审查
  - 审查 LRU 测试：驱逐、Arc 引用保护、全引用返回 None
  - 审查马尔可夫链测试：模式学习、概率预测、未知上下文
  - 审查 proptest：概率和 = 1.0 不变量
  - 审查并发测试：10 线程并发访问

---

## Task 35:FaaE + EDSB 语义路由与熵均衡 — 深度复审（Day 3，P0）

对 `crates/faae-router/` 全部 7 个源文件进行 6 维度复审。

- [x] SubTask 35.1:架构完整性审查
  - 审查 `Cargo.toml` 依赖方向：FaaE(L6) 仅依赖 L1-L5
  - 验证无向上依赖（L7/L8）
  - 扫描函数行数：`router.rs`/`edsb.rs`/`expert.rs` 单函数 ≤ 200 行
  - 扫描 `unsafe` 代码

- [x] SubTask 35.2:并发正确性审查
  - **重点**：审查 `router.rs` 中双层 `RwLock` 嵌套（registry → profile）
  - 验证 `route` 方法：读锁收集 Arc → 释放锁 → 锁外计算相似度
  - 审查 `edsb.rs` 中 `decay_usage_counts`：读锁 + 原子 store 模式
  - 验证 `spawn_decay_loop` 的 `Arc<Self>` 模式
  - 审查 `balance` 方法：`compute_entropy` 读锁不跨越均衡决策

- [x] SubTask 35.3:性能优化审查
  - 验证 `router.rs` Top-K 使用 `select_nth_unstable_by`（O(n)）
  - 验证余弦相似度复用 `nexus_core::cosine_similarity_slices`
  - 审查 `edsb.rs` 香农熵计算复杂度 O(n)
  - 验证 `estimate_entropy_after_redistribution` 模拟计算效率
  - 审查 `pseudo_random_probability` 纳秒随机数效率

- [x] SubTask 35.4:代码质量审查
  - 扫描非测试代码的 `unwrap()`/`expect()`
  - 审查 `FaaeError` enum：`ExpertNotFound`/`RoutingFailed`/`EntropyCalculationFailed`
  - 审查 WHY 注释：香农熵选型、概率均衡 vs 强制均衡、指数衰减 τ=1h、伪随机选型
  - 验证 `ExpertProfile` 的 `usage_count` 原子操作封装

- [x] SubTask 35.5:事件完整性审查
  - 验证 `ExpertRouted` 事件载荷：`routed_tool`/`confidence`
  - 验证 `ExpertRegistered`/`ExpertUnregistered` 事件
  - 验证 `EntropyBalanced` 事件载荷：`old_entropy`/`new_entropy`/`redistributed_count`
  - 验证事件发布失败不阻塞路由结果

- [x] SubTask 35.6:测试质量审查
  - 审查路由测试：单候选、Top-K 选择、空候选集、未注册候选
  - 审查熵计算测试：均匀分布(1.0)、集中分布(≈0.0)、零总量(1.0)、单工具(1.0)
  - 审审查均衡测试：熵 < 阈值触发均衡、熵 ≥ 阈值不触发
  - 审查 proptest：路由结果数不变量、熵值 ∈ [0,1] 不变量

---

## Task 36:综合复审报告与验收（Day 4，P0）

汇总 6 个 crate 的复审结果，生成结构化复审报告。

- [x] SubTask 36.1:汇总架构完整性结果
  - 统计依赖方向违规数（Critical）
  - 统计函数超 200 行数（Major）
  - 统计 unsafe 代码块数（Critical）

- [x] SubTask 36.2:汇总并发正确性结果
  - 统计锁跨越 await 点数（Critical）
  - 统计通道死锁风险数（Critical）
  - 统计内存序选型问题数（Major）

- [x] SubTask 36.3:汇总性能优化结果
  - 统计未用 `select_nth_unstable` 的 Top-K 实现（Major）
  - 统计不必要的 `clone()`（Minor）
  - 统计 `tokio::spawn` 泄漏风险（Major）

- [x] SubTask 36.4:汇总代码质量结果
  - 统计非测试代码 `unwrap()`/`expect()` 数（Major）
  - 统计 `pub` 项文档注释覆盖率（Minor）
  - 统计命名规范违反数（Minor）

- [x] SubTask 36.5:汇总事件完整性结果
  - 统计事件覆盖缺口（Major）
  - 统计事件载荷不一致数（Major）
  - 统计事件发布阻塞主流程数（Critical）

- [x] SubTask 36.6:汇总测试质量结果
  - 统计错误路径测试缺失数（Major）
  - 统计边界用例缺失数（Minor）
  - 统计 proptest 不变量错误数（Critical）

- [x] SubTask 36.7:生成复审报告与改进建议
  - 按严重程度分级（Critical/Major/Minor）
  - 按 crate 分组
  - 提供具体修复建议与文件位置
  - 结论：是否可进入 Week 5

---

# Task Dependencies

- Task 30-35 相互独立，可并行执行（6 个 crate 审查无依赖）
- Task 36 依赖 Task 30-35 全部完成（汇总报告）
