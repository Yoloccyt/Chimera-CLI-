# Checklist — Week 4 深度复审

> 本 checklist 对应 `tasks.md` 中 Task 30-36 的每个 SubTask，提供具体可验证的检查点。
> 每个 SubTask 完成后勾选对应检查项。

---

## Task 30:GEA 门控专家激活 — 深度复审

### SubTask 30.1:架构完整性审查
- [x] `crates/gea-activator/Cargo.toml` 依赖方向合规（L6 仅依赖 L1-L5）
- [x] 无 `use` 语句引用 L7+ crate
- [x] `activator.rs`/`gating.rs`/`conflict.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块
- [x] `#![forbid(unsafe_code)]` 属性存在（或等效约束）

### SubTask 30.2:并发正确性审查
- [x] `activate` 方法中 `RwLock` 读守卫在 `.await` 前释放
- [x] `DashMap` 激活缓存写锁在 async 调用前释放
- [x] `AtomicU64` 缓存统计使用 `Ordering::Relaxed`
- [x] 无锁守卫跨越 `.await` 点

### SubTask 30.3:性能优化审查
- [x] `conflict.rs` Top-K 使用 `select_nth_unstable`（O(n)）
- [x] `gating.rs` Sigmoid 使用标准库 `exp`
- [x] `hash_task_profile` 哈希效率合理
- [x] `dynamic_threshold` 计算复杂度 O(1)

### SubTask 30.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`/`panic!()`
- [x] `GeaError` 使用 `thiserror` 派生
- [x] `pub` 项有文档注释 `///`
- [x] WHY 注释解释 Sigmoid 选型、LRU TTL、动态阈值公式

### SubTask 30.5:事件完整性审查
- [x] `ExpertActivated` 事件发布点存在且载荷完整
- [x] `ActivationCacheStats` 事件每 100 次激活发布
- [x] 事件发布失败不阻塞主流程（`if let Err(e) = ... { warn!(...) }`）

### SubTask 30.6:测试质量审查
- [x] 门控值范围测试（∈ [0,1]）
- [x] 边界值测试（complexity=0/1）
- [x] 冲突消解测试（重叠度 > 0.8）
- [x] 并发测试（10 线程）
- [x] 性能测试标记 `#[ignore]`

---

## Task 31:GQEP 聚集查询执行协议 — 深度复审

### SubTask 31.1:架构完整性审查
- [x] `crates/gqep-executor/Cargo.toml` 依赖方向合规（L6 仅依赖 L1-L5 + L4 qeep）
- [x] `qeep-protocol`(L4) 依赖方向合规（L6→L4 向下允许）
- [x] `gatherer.rs`/`batch.rs`/`timeout.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块

### SubTask 31.2:并发正确性审查
- [x] `FuturesUnordered` 流式处理无竞态
- [x] QEEP `entangle` 正确包裹 Future
- [x] `batch.rs` 回滚回调并发安全
- [x] `OrphanDetector` 正确初始化与查询

### SubTask 31.3:性能优化审查
- [x] `FuturesUnordered` 选型（vs `join_all`）有注释说明
- [x] `tokio::time::timeout` 包装效率合理
- [x] 10 操作聚集 < 100ms（性能测试验证）

### SubTask 31.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`
- [x] `GqepError` enum 完整（4 个变体）
- [x] `GqepFuture` 类型别名定义清晰
- [x] WHY 注释解释 FuturesUnordered 与 QEEP 集成理由

### SubTask 31.5:事件完整性审查
- [x] `GatherCompleted` 事件载荷完整（total/succeeded/failed/latency_ms）
- [x] `OperationTimedOut` 事件载荷完整（operation_id/timeout_ms）
- [x] `OrphanCallDetected` 事件存在（Critical 级别）
- [x] 事件发布失败不阻塞聚集结果

### SubTask 31.6:测试质量审查
- [x] 聚集测试覆盖全成功/部分失败/全失败
- [x] 超时测试覆盖触发/未触发
- [x] 批量原子性测试覆盖回滚
- [x] 孤儿调用测试覆盖事件发布

---

## Task 32:PVL 生产验证闭环 — 深度复审

### SubTask 32.1:架构完整性审查
- [x] `crates/pvl-layer/Cargo.toml` 依赖方向合规（L7 仅依赖 L1-L6）
- [x] 无 `use` 语句引用 L8+ crate
- [x] `producer.rs`/`verifier.rs`/`feedback.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块

### SubTask 32.2:并发正确性审查
- [x] `verifier.rs` 中 `feedback_tx` 在 `run()` 返回后正确 `drop`
- [x] `drop(feedback_tx)` 有 WHY 注释解释死锁规避
- [x] `producer.rs` 通道满时阻塞行为正确
- [x] 反馈通道闭环无死锁
- [x] 所有 Future 均 `await` 或 `spawn` 管理（零 void Promise）

### SubTask 32.3:性能优化审查
- [x] 通道容量上限（128）合理性有注释
- [x] Producer 生成速率控制实现
- [x] `Operation` 结构体内存布局合理

### SubTask 32.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`
- [x] `PvlError` enum 完整（3 个变体）
- [x] WHY 注释解释通道死锁规避与反馈闭环
- [x] `ProducerStrategy` 枚举定义完整

### SubTask 32.5:事件完整性审查
- [x] `OperationProduced` 事件载荷完整
- [x] `PredictionVerified` 事件载荷完整
- [x] `ProducerStrategyAdjusted` 事件载荷完整
- [x] 事件发布失败不阻塞主流程

### SubTask 32.6:测试质量审查
- [x] Producer 测试覆盖生成 N 个操作
- [x] Verifier 测试覆盖通过/拒绝/反馈
- [x] 并发测试覆盖 100 操作流式（无 panic/死锁）
- [x] 死锁修复测试覆盖 `drop(feedback_tx)` 后 `recv()` 正常

---

## Task 33:MTPE 多步预测执行 — 深度复审

### SubTask 33.1:架构完整性审查
- [x] `crates/mtpe-executor/Cargo.toml` 依赖方向合规（L7 仅依赖 L1-L6）
- [x] 无向上依赖
- [x] `predictor.rs`/`fallback.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块

### SubTask 33.2:并发正确性审查
- [x] `RwLock<PredictionStats>` 守卫不跨越 `.await` 点
- [x] `AtomicU64` 预测计数器 `Ordering` 选型合理
- [x] `fallback.rs` 回退操作并发安全

### SubTask 33.3:性能优化审查
- [x] `SIMULATED_INFERENCE_DELAY` 值合理
- [x] `compute_context_hash` 哈希效率
- [x] 加速比 > 3×（性能测试验证）

### SubTask 33.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`
- [x] `MtpeError` enum 完整（3 个变体）
- [x] `MtpeConfig::is_valid_n` 验证逻辑正确
- [x] WHY 注释解释伪预测实现与回退机制

### SubTask 33.5:事件完整性审查
- [x] `PredictionMade` 事件载荷完整（quest_id/n/avg_confidence）
- [x] `PredictionStatsReported` 事件载荷完整（success_rate_by_n）
- [x] `PredictionRolledBack` 事件载荷完整（failed_step/rollback_to）
- [x] 事件发布频率正确（每 100 次预测）

### SubTask 33.6:测试质量审查
- [x] 预测测试覆盖 N=1/5/10 与 N=0/11 错误
- [x] 成功率统计测试覆盖分组统计
- [x] 回退测试覆盖第 3 步失败
- [x] 加速比测试覆盖 1000 次 N=5 vs 5000 次单步

---

## Task 34:SCC 推测上下文缓存 — 深度复审

### SubTask 34.1:架构完整性审查
- [x] `crates/scc-cache/Cargo.toml` 依赖方向合规（L3 仅依赖 L1-L2）
- [x] **重点**：SCC(L3) 不依赖 L6/L7（向上禁止）
- [x] SCC 内部 `spawn` 后台预取（不通过 GQEP）
- [x] `cache.rs`/`prefetch.rs`/`lru.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块

### SubTask 34.2:并发正确性审查
- [x] `lru.rs` `Arc::strong_count` TOCTOU 竞态有注释说明可接受性
- [x] `cache.rs` `DashMap` `insert_lock` 临界区原子性
- [x] `prefetch.rs` `std::sync::RwLock` 选型正确（同步方法）
- [x] `record_access_background` 的 `Arc<Self>` 模式正确（守卫不进 Future 状态机）

### SubTask 34.3:性能优化审查
- [x] LRU O(n) 扫描在 256 条目下可接受（有注释）
- [x] `Arc<str>` 用于内容共享
- [x] `get_or_prefetch` 命中路径延迟 < 0.1ms

### SubTask 34.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`
- [x] `SccError` enum 完整（3 个变体）
- [x] WHY 注释解释逻辑时钟、Arc 引用保护、std::sync 选型
- [x] `ContextId` 使用 `nexus_core::id_newtype!` 宏

### SubTask 34.5:事件完整性审查
- [x] `CacheHit`/`CacheMiss` 事件发布
- [x] `CachePrefetched` 事件载荷完整（prefetched_ids）
- [x] `CacheStatsReported` 事件载荷完整（hit_rate/eviction_count）
- [x] `publish_blocking` 用于同步上下文

### SubTask 34.6:测试质量审查
- [x] LRU 测试覆盖驱逐/Arc 保护/全引用
- [x] 马尔可夫链测试覆盖学习/预测/未知上下文
- [x] proptest 覆盖概率和 = 1.0 不变量
- [x] 并发测试覆盖 10 线程

---

## Task 35:FaaE + EDSB 语义路由与熵均衡 — 深度复审

### SubTask 35.1:架构完整性审查
- [x] `crates/faae-router/Cargo.toml` 依赖方向合规（L6 仅依赖 L1-L5）
- [x] 无向上依赖（L7/L8）
- [x] `router.rs`/`edsb.rs`/`expert.rs` 单函数 ≤ 200 行
- [x] 非测试代码无 `unsafe` 块

### SubTask 35.2:并发正确性审查
- [x] `router.rs` 双层 `RwLock` 嵌套无死锁（读锁收集 Arc → 释放 → 锁外计算）
- [x] `edsb.rs` `decay_usage_counts` 读锁 + 原子 store 模式正确
- [x] `spawn_decay_loop` 的 `Arc<Self>` 模式正确
- [x] `balance` 方法 `compute_entropy` 读锁不跨越均衡决策

### SubTask 35.3:性能优化审查
- [x] `router.rs` Top-K 使用 `select_nth_unstable_by`（O(n)）
- [x] 余弦相似度复用 `nexus_core::cosine_similarity_slices`
- [x] `edsb.rs` 香农熵计算复杂度 O(n)
- [x] `pseudo_random_probability` 纳秒随机数效率合理

### SubTask 35.4:代码质量审查
- [x] 非测试代码无 `unwrap()`/`expect()`
- [x] `FaaeError` enum 完整（3 个变体）
- [x] WHY 注释解释香农熵、概率均衡、指数衰减 τ=1h、伪随机选型
- [x] `ExpertProfile` 原子操作封装正确

### SubTask 35.5:事件完整性审查
- [x] `ExpertRouted` 事件载荷完整（routed_tool/confidence）
- [x] `ExpertRegistered`/`ExpertUnregistered` 事件存在
- [x] `EntropyBalanced` 事件载荷完整（old_entropy/new_entropy/redistributed_count）
- [x] 事件发布失败不阻塞路由结果

### SubTask 35.6:测试质量审查
- [x] 路由测试覆盖单候选/Top-K/空候选/未注册
- [x] 熵计算测试覆盖均匀/集中/零总量/单工具
- [x] 均衡测试覆盖触发/不触发
- [x] proptest 覆盖路由结果数与熵值不变量

---

## Task 36:综合复审报告与验收

### SubTask 36.1:汇总架构完整性结果
- [x] 依赖方向违规数统计（Critical）
- [x] 函数超 200 行数统计（Major）
- [x] unsafe 代码块数统计（Critical）

### SubTask 36.2:汇总并发正确性结果
- [x] 锁跨越 await 点数统计（Critical）
- [x] 通道死锁风险数统计（Critical）
- [x] 内存序选型问题数统计（Major）

### SubTask 36.3:汇总性能优化结果
- [x] 未用 `select_nth_unstable` 的 Top-K 统计（Major）
- [x] 不必要 `clone()` 统计（Minor）
- [x] `tokio::spawn` 泄漏风险统计（Major）

### SubTask 36.4:汇总代码质量结果
- [x] 非测试 `unwrap()`/`expect()` 数统计（Major）
- [x] `pub` 项文档注释覆盖率统计（Minor）
- [x] 命名规范违反数统计（Minor）

### SubTask 36.5:汇总事件完整性结果
- [x] 事件覆盖缺口统计（Major）
- [x] 事件载荷不一致数统计（Major）
- [x] 事件发布阻塞主流程数统计（Critical）

### SubTask 36.6:汇总测试质量结果
- [x] 错误路径测试缺失数统计（Major）
- [x] 边界用例缺失数统计（Minor）
- [x] proptest 不变量错误数统计（Critical）

### SubTask 36.7:生成复审报告与改进建议
- [x] 报告按严重程度分级（Critical/Major/Minor）
- [x] 报告按 crate 分组
- [x] 提供具体修复建议与文件位置
- [x] 结论明确：是否可进入 Week 5
