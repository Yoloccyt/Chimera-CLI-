# Week 4 深度复审 Spec

## Why

Week 4 执行优化层已交付 6 个核心 crate（gea-activator / gqep-executor / pvl-layer / mtpe-executor / scc-cache / faae-router），共 39 个 SubTask 全部勾选完成，全量 `cargo check/clippy/test/build --release` 通过。但"测试通过"不等于"代码无隐患"——本项目继承 Claude Code 四次尸检教训（5.4% 孤儿调用、void Promise 无 await、1M Token 暴力加载、44 个未发布标志），需在进入 Week 5 前对 Week 4 代码进行系统性深度复审，识别潜在架构违规、并发隐患、性能瓶颈与代码质量问题，确保 Week 5 建立在稳固的执行优化层之上。

本 spec 定义复审的范围、维度、方法与验收标准，由 6 类资深专家子代理（架构/并发/性能/安全/实现/质量）协同执行。

## What Changes

* **不修改任何源代码** — 本复审为只读分析，产出物为复审报告与改进建议清单
* 若复审发现 Critical 级问题，在 `tasks.md` 中新增修复任务，经用户批准后另行执行
* 若复审发现 Non-critical 改进建议，记录到复审报告中，供后续迭代参考

## Impact

* Affected specs:
  * `week4-execution-optimization`（被复审对象，本 spec 对其全部交付物进行验证）
  * `week3-third-round-deep-review`（继承其复审方法论与经验教训）
  * `establish-elite-collaboration-team`（6 类专家子代理角色定义来源）

* Affected code（只读审查，不修改）:
  * `crates/gea-activator/src/` — 7 个源文件
  * `crates/gqep-executor/src/` — 7 个源文件
  * `crates/pvl-layer/src/` — 7 个源文件
  * `crates/mtpe-executor/src/` — 6 个源文件
  * `crates/scc-cache/src/` — 7 个源文件
  * `crates/faae-router/src/` — 7 个源文件
  * `crates/event-bus/src/types.rs` — Week 4 新增事件类型
  * 各 crate 的 `tests/` 与 `benches/` 目录

* 不受影响的代码：Week 1-3 已实现的 15 个 crate（仅作为依赖参考）

## ADDED Requirements

### Requirement: 架构完整性复审

系统 SHALL 对 Week 4 全部 6 个 crate 进行十层架构依赖方向验证，确保无向上依赖违规、无跨层直接调用、所有跨层通信经 EventBus。

#### Scenario: 依赖方向验证
- **WHEN** 审查各 crate 的 `Cargo.toml` 与 `use` 语句
- **THEN** 验证 L6（gea-activator/gqep-executor/faae-router）仅依赖 L1-L5
- **THEN** 验证 L7（pvl-layer/mtpe-executor）仅依赖 L1-L6
- **THEN** 验证 L3（scc-cache）仅依赖 L1-L2
- **THEN** 任何向上依赖必须通过 EventBus 事件解耦（发布者/订阅者模式）

#### Scenario: 单函数行数验证
- **WHEN** 扫描全部源文件的函数体
- **THEN** 单函数 ≤ 200 行（架构红线 §6）
- **THEN** 超过 200 行的函数必须标记并建议拆分

#### Scenario: unsafe 代码验证
- **WHEN** 扫描全部源文件
- **THEN** 非测试代码无 `unsafe` 块（`#![forbid(unsafe_code)]` 约束）
- **THEN** 测试代码中的 `unsafe`（如有）必须有充分注释说明原因

### Requirement: 并发正确性复审

系统 SHALL 对 Week 4 全部 6 个 crate 的并发代码进行深度审查，识别死锁风险、竞态条件、锁粒度问题与 async 安全隐患。

#### Scenario: 锁持有与 await 安全
- **WHEN** 审查所有含 `RwLock`/`Mutex` 的代码路径
- **THEN** 验证锁守卫不跨越 `.await` 点（避免死锁与 Send 约束违反）
- **THEN** 验证 `DashMap` 写锁在 async 调用前释放（继承 project_memory 教训）
- **THEN** 验证 `std::sync::RwLock` 与 `tokio::sync::RwLock` 选型正确（同步 vs async 上下文）

#### Scenario: 通道死锁验证
- **WHEN** 审查 `mpsc` 通道使用（PVL Producer-Verifier、GQEP 聚集）
- **THEN** 验证所有 `Sender` 在适当时机 `drop`（避免 `recv()` 永久阻塞）
- **THEN** 验证通道容量上限不会导致 Producer 饥饿
- **THEN** 验证反馈通道闭环无死锁

#### Scenario: 原子操作与内存序
- **WHEN** 审查 `AtomicU64`/`AtomicUsize` 使用（计数器、统计）
- **THEN** 验证 `Ordering` 选型合理（Relaxed 用于统计，AcqRel 用于状态变更）
- **THEN** 验证无虚假的 `Ordering::SeqCst`（过度同步）

### Requirement: 性能优化复审

系统 SHALL 对 Week 4 全部 6 个 crate 的性能关键路径进行审查，验证算法复杂度、内存使用与延迟指标。

#### Scenario: 算法复杂度验证
- **WHEN** 审查 Top-K 选择、相似度计算、LRU 驱逐等关键路径
- **THEN** Top-K 使用 `select_nth_unstable`（O(n)），非 `sort_by`（O(n log n)）
- **THEN** 余弦相似度复用 `nexus_core::cosine_similarity_slices`（无重复实现）
- **THEN** LRU 驱逐在 256 条目规模下 O(n) 扫描可接受

#### Scenario: 内存与 Arc 共享
- **WHEN** 审查 `Arc` 使用（SCC 缓存条目、FaaE 专家画像）
- **THEN** 验证 `Arc::strong_count` 检查的 TOCTOU 竞态可接受性（已有注释说明）
- **THEN** 验证 `Arc<str>` 用于内容共享（SCC `ContextEntry.content`）
- **THEN** 验证无不必要的 `clone()`（尤其是大向量）

#### Scenario: 异步并发模式
- **WHEN** 审查 `FuturesUnordered` vs `join_all` 选型
- **THEN** GQEP 聚集使用 `FuturesUnordered`（流式处理，低内存）
- **THEN** 验证 `tokio::spawn` 后台任务无泄漏（衰减循环、预取任务）

### Requirement: 代码质量复审

系统 SHALL 对 Week 4 全部 6 个 crate 的代码可读性、注释完整性、错误处理与命名规范进行审查。

#### Scenario: 错误处理验证
- **WHEN** 审查全部 `Result` 返回路径
- **THEN** 非测试代码无 `unwrap()`/`expect()`/`panic!()`
- **THEN** 库层错误使用 `thiserror` enum，应用层使用 `anyhow::Result`
- **THEN** 错误传播使用 `?` 运算符，错误上下文用 `.context()` 或 `map_err()` 补充

#### Scenario: 注释完整性
- **WHEN** 审查全部 `pub` 项（结构体、函数、方法）
- **THEN** 每个 `pub` 项有文档注释 `///`
- **THEN** WHY 注释解释非显而易见的设计决策（隐藏约束、变通方案、反直觉行为）
- **THEN** 复杂算法（Sigmoid 门控、香农熵、马尔可夫链）有公式注释

#### Scenario: 命名规范
- **WHEN** 审查全部类型、函数、变量命名
- **THEN** 遵循项目命名模式（`*Coordinator`/`*Engine`/`*Router`/`*Protocol`/`*Governor`/`*Mask`/`*Block`）
- **THEN** newtype 使用 `nexus_core::id_newtype!` 宏
- **THEN** 配置结构体命名 `*Config`，错误枚举命名 `*Error`

### Requirement: 事件完整性复审

系统 SHALL 对 Week 4 全部 6 个 crate 的 EventBus 使用进行审查，验证事件覆盖、事件载荷完整性与发布时机正确性。

#### Scenario: 事件覆盖验证
- **WHEN** 对照 spec.md 中定义的全部 Scenario
- **THEN** 每个 Scenario 对应的事件类型已在 `event-bus/src/types.rs` 中定义
- **THEN** 每个事件发布点实际发布对应事件
- **THEN** 事件发布失败不阻塞主流程（`if let Err(e) = ... { warn!(...) }` 模式）

#### Scenario: 事件载荷完整性
- **WHEN** 审查每个事件发布点
- **THEN** 事件载荷字段与 spec 定义一致
- **THEN** `EventMetadata::new(source)` 的 source 字段正确标识发布者 crate
- **THEN** 数值字段（latency_ms/confidence/entropy 等）精度合理

### Requirement: 测试质量复审

系统 SHALL 对 Week 4 全部 6 个 crate 的测试用例进行审查，验证覆盖率、边界用例、并发测试与属性测试。

#### Scenario: 测试覆盖验证
- **WHEN** 审查全部测试文件
- **THEN** 每个 `pub` 函数至少有一个正向测试和一个错误路径测试
- **THEN** 边界用例覆盖（空输入、单元素、最大值、零值）
- **THEN** 并发测试存在（`#[tokio::test]` + 多线程）

#### Scenario: 属性测试验证
- **WHEN** 审查 proptest 用例
- **THEN** 不变量正确（如熵值 ∈ [0,1]、门控值 ∈ [0,1]、概率和 = 1.0）
- **THEN** proptest 策略合理（输入范围、生成器）

#### Scenario: 性能测试验证
- **WHEN** 审查 `#[ignore = "perf: ..."]` 测试
- **THEN** 性能断言阈值合理（参考 project_memory 中的经验值）
- **THEN** 性能测试可用 `--ignored` 手动执行

## MODIFIED Requirements

### Requirement: 复审报告产出

复审完成后 SHALL 产出结构化复审报告，包含以下维度：

1. **架构完整性**：依赖方向验证结果、函数行数统计、unsafe 代码扫描结果
2. **并发正确性**：锁安全分析、通道死锁分析、内存序分析
3. **性能优化**：算法复杂度分析、内存使用分析、异步并发模式分析
4. **代码质量**：错误处理统计、注释覆盖率、命名规范符合度
5. **事件完整性**：事件覆盖矩阵、载荷完整性、发布时机
6. **测试质量**：测试覆盖率、边界用例覆盖、属性测试质量

每个维度按严重程度分级：
- **Critical**：必须修复才能进入 Week 5（架构违规、死锁风险、数据丢失风险）
- **Major**：建议在 Week 5 期间修复（性能瓶颈、错误处理缺失、测试覆盖不足）
- **Minor**：可在后续迭代改进（命名优化、注释补充、代码风格）

## REMOVED Requirements

无（本 spec 为纯复审，不移除任何现有需求）
