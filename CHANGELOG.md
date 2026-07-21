# Changelog

## v2.3.1-omega (2026-07-21)

**版本代号**: NEXUS-OMEGA (Release Pipeline Remediation)
**架构基线**: v2.3.0-omega → v2.3.1-omega(patch,发布流程补救)
**关联 Spec**: [v2-3-0-omega-release-finalization](.trae/specs/v2-3-0-omega-release-finalization/spec.md)

### 发布流程补救

- **Commit message 编码修复**: v2.3.0-omega tag 因 GitHub 仓库规则被锁定且包含编码损坏的 commit message(PowerShell 沙箱 GBK 编码问题),无法更新或强制推送
- **新 tag 触发 release workflow**: 创建 v2.3.1-omega tag 触发 release.yml 工作流,确保 5 平台构建 + Docker 镜像 + GitHub Release 正常生成
- **版本号 SemVer patch 递增**: 代码内容与 v2.3.0-omega 完全一致,仅包含版本号递增与 CHANGELOG 同步
- **完整性验证**: 2877 tests passed, clippy 零警告, fmt 一致, cargo audit 零漏洞, release binary 3.44MB

## v2.3.0-omega (2026-07-20)

**版本代号**: NEXUS-OMEGA (Comprehensive Audit · TUI Finalization · Governance)
**架构基线**: v2.2.0-omega → v2.3.0-omega(minor,发布就绪验证)
**关联文档**:
- 架构审计: [architecture-audit-2026-07-20](docs/architecture/audit/architecture-audit-2026-07-20.md)
- 专家团队: [expert-team-framework](docs/architecture/governance/expert-team-framework.md)
- 任务优先级: [task-priority-system](docs/architecture/governance/task-priority-system.md)

### Phase A: 架构审计

- **10 层架构系统性审计**: 35 crate 全覆盖,L1-L10 各层实现状态评估,零 Stub
- **依赖铁律合规验证**: 零向上依赖违规,`nexus-core` 最小依赖,`event-bus` 唯一跨层通道
- **`#![forbid(unsafe_code)]` 核验**: 35/35 crate = 100% 合规
- **测试覆盖率统计**: ~1039 单元测试 + 88 proptest + 42 criterion bench + 11 E2E + 6 fuzz target
- **已知风险清单**: 4 项(3 低风险 + 1 中风险),含缓解建议

### Phase B: TUI 收尾

- **动态 tick 模式**: `DataSourceConfig` 新增 `eco_tick_interval_ms`/`event_backlog_threshold`,`TickMode` 枚举(Normal/Eco),`DataPipeline` 根据事件积压量自动切换
- **TaskManagerPanel 综合测试**: 新增 12 项单元测试覆盖多选(Space/Ctrl+A/Esc)、批量操作(B/R/T)、过滤搜索(/?/Backspace/Enter)、状态统计(Pending/Running/Paused/Completed)

### Phase C: 治理规范化

- **专家团队协作框架**: E01-E05 五角色定义(首席架构师/全栈后端/全栈前端/测试专家/DevOps),7 阶段协作流程
- **任务优先级量化体系**: P0-P4 五级优先级定义,资源分配策略(P0:100%→P4:10%),技术债务管理(每迭代 ≥ 20%)

## v2.2.0-omega (2026-07-20)

**版本代号**: NEXUS-OMEGA (CHIMERA-MAS · Part II Closure)
**Spec**: [complete-chimera-mas-part2](.trae/specs/complete-chimera-mas-part2/spec.md)
**架构基线**: v2.1.0-omega → v2.2.0-omega(Stage B 主体能力补齐,append-only minor,无 BREAKING)
**ADR**: [ADR-028-chimera-mas-part2-closure](docs/architecture/ADR-028-chimera-mas-part2-closure.md)

### 重大变更

- **Part II 七项闭环能力补齐**(§15-§21):新增 7 个子模块覆盖上下文预算 / 分块调度 / 三级归档 / 知识协同 / 稳定闭环 / PDCA 基准 / INV-7/INV-8 不变量,Stage B 主体能力闭环
- **append-only 策略**(ADR-028 决策 1):Part I 公共签名(`Quadrant` / `PriorityScheduler` / `ExpertRegistry` / `AgentTask.priority`)零变更,全部为新增类型 / 方法 / 字段
- **80% 能力复用**(ADR-028 决策 2):复用 hcw-window / osa-coordinator / event-bus / quest-engine / gqep-executor / qeep-protocol / repo-wiki / mlc-engine / cmt-tiering / efficiency-monitor / parliament 等 11 个现有 crate
- **INV-7/INV-8 不变量**(ADR-028 决策 3):上下文预算界 + 归档单调性,proptest 1000 次无违反
- **三类压力源降级链**(ADR-028 决策 4):MemoryNearBudget / ExpertOverload / ArchiveIoContention → 降级步骤序列
- **5 项 criterion 基准**(ADR-028 决策 5):window_select < 1ms / mlc_l2_knn < 5ms / wiki_knn@1000 < 10ms / wiki_knn@10 < 1ms / decay_compute < 1μs / 50agent_mem_peak ≤ 130MB
- **SemVer minor 升级**(v2.1.0-omega → v2.2.0-omega):严格向后兼容,无 BREAKING 变更

### Added

#### chimera-mas Part II 子模块(L9 Quest 层)

- **§15 独立上下文管理 + HCW 稀疏化 + INV-7 准入闸**
  - 新增 `src/context/budget_model.rs`:`MemoryBudgetModel`(50 Agent + 1M 上下文 = 130MB 预算模型)+ `AdmissionGate::check()`(派生准入闸,违反 INV-7 时返回 `AdmissionGateDenied` 并发布 Critical 事件)
  - 复用 `hcw-window::HierarchicalWindow::select()` 四级窗口分层 + `osa-coordinator::compute_masks()` 五维稀疏掩码
  - INV-7 不变量:任意时刻 `m_total ≤ MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`(130MB × 0.9 = 117MB)

- **§16 任务复杂度分块与分批调度**
  - 新增 `src/chunker.rs`:`TaskChunker::chunk()`(按 TaskComplexity 切块)+ `BatchExecutor`(分批调度)+ `BatchConfig` / `ChunkOutput` / `BatchResult`
  - 复用 `quest-engine::Quest` DAG 与 `TaskComplexity` 映射
  - 切块深度上限保护:`delegation_depth >= MAX_AGENT_DEPTH`(5)时返回 `ChunkingFailed`

- **§17 Agent 记忆三级归档(1mo/3mo/6mo)+ INV-8**
  - 新增 `src/archive/` 目录:`compressor.rs`(归档压缩)+ `scheduler.rs`(归档调度)+ `tier.rs`(归档层级)+ `mod.rs`
  - 三级归档:Hot(1mo)→ Warm(3mo)→ Cold(6mo)→ Ice(永久),复用 `mlc-engine` L0-L3 + `cmt-tiering` Hot/Warm/Cold/Ice + `scc-cache` 推测缓存
  - INV-8 不变量:归档沿 Hot→Warm→Cold→Ice 单向降级,`InvariantChecker::check_inv8_archive_monotonicity()` 检查每次操作

- **§18 知识协同:专家咨询 + 互询 + Wiki 检索**
  - 新增 `src/knowledge/` 目录:`expert_consult.rs`(专家咨询)+ `mutual_inquiry.rs`(同僚互询)+ `wiki_retrieval.rs`(Wiki 检索)+ `mod.rs`
  - `ExpertConsultant`:复用 `model-router` 选择专家旗舰模型 + `faae-router` Function-as-Expert 语义路由
  - `MutualInquirer`:同象限专家互询,正则脱敏(§5.5 ContextIsolation)
  - `WikiRetriever`:复用 `repo-wiki` FTS5 全文检索 + 内存 KNN,三级检索链短路

- **§19 系统稳定运行与功能完整闭环(零孤儿)**
  - 新增 `src/stability.rs`:`StabilityGuard`(故障隔离,复用 `parliament::RoleRegistry`)+ `CircuitBreaker`(Closed/Open/HalfOpen 三态机)+ `DegradationChain`(三类压力源降级链)+ `DegradationStep` / `PressureSource` / `TerminalState`
  - 三类压力源:`MemoryNearBudget`(INV-7 阈值)/ `ExpertOverload`(咨询超时率 > 5%)/ `ArchiveIoContention`(IO 等待)
  - 降级链协同 `acb-governor` + `decb-governor` + `efficiency-monitor`,Critical 事件走 mpsc 通道

- **§20 PDCA 端到端闭环强化 + criterion 基准**
  - 新增 `src/pdca.rs`(930 行):`PdcaLoop`(check/act/plan_reflux 三方法)+ `PdcaMetrics`(6 项度量)+ `PdcaAdjustments`(tier 分布 / tau / pool_size / wsjf 权重)+ `PlanReflux` + `PdcaAlert` + `AlertThresholds`(4 条告警规则)
  - 新增 5 项 criterion 基准(从 4 项扩展到 9 项):`window_select` / `mlc_l2_knn_top10@4096` / `wiki_knn@1000` / `wiki_knn@10` / `decay_compute` / `50agent_mem_peak`

- **§21 合规增补与 INV-7/INV-8 不变量编码**
  - 新增 `src/invariants.rs`:`InvariantChecker`(INV-7/INV-8 检查器)+ `ArchiveTier` 枚举 + `MEMORY_BUDGET_MB`(130)/ `MEMORY_BUDGET_UTILIZATION`(0.9)常量
  - proptest 1000 次属性测试覆盖 INV-7(随机派生序列不超预算)+ INV-8(随机归档序列满足单调性)

#### 新增 MasError 变体(5 个,共 33 个)

- `ChunkingFailed`(§16)— 任务切块失败(深度超限等)
- `ArchiveTierInvalid`(§17)— 归档层级字符串未识别
- `ArchiveMonotonicityViolated`(§17 / §21 INV-8)— 归档违反 Hot→Warm→Cold→Ice 单调性
- `KnowledgeRetrievalFailed`(§18,Stage A 已有,核验保留)— 知识检索全部失败
- `CircuitBreakerOpen`(§19)— 熔断器 Open 态拒绝新请求
- `AdmissionGateDenied`(§15 / §21 INV-7)— 派生准入闸拒绝(全局内存预算超限)

#### 测试覆盖

- 新增 ~80 个测试用例(单元 + 集成 + proptest)
- proptest 1000 次属性测试:INV-7 上下文预算界 + INV-8 归档单调性
- 所有测试通过 `cargo test --workspace`

### Changed

- `workspace.package.version`:`2.1.0-omega` → `2.2.0-omega`(SemVer minor,append-only 非破坏性)
- `crates/chimera-mas/Cargo.toml` description 字段补 §15-§21 闭环能力描述
- `crates/chimera-mas/src/lib.rs` 新增 7 个模块声明 + 重导出 + prelude 更新
- `crates/chimera-mas/benches/mas_benchmark.rs` 从 4 项基准扩展到 9 项(新增 5 项 Part II 基准)
- `MasError` 变体数:27 → 33(新增 5 个 Part II 变体 + 1 个 AdmissionGateDenied,静态断言更新)

### ADR

- **ADR-028**: CHIMERA-MAS Part II 闭环能力补齐(8 项决策:append-only / 80% 复用映射 / INV-7/INV-8 / 降级链 / criterion 基准 / 版本号策略 / 否决方案规避 / Stage B 主体完成)

### Compatibility

- **无 BREAKING 变更**:Part I 公共签名(`Quadrant` / `PriorityScheduler` / `ExpertRegistry` / `AgentTask.priority` / `RootOrchestrator::delegate()` / `delegate_quadrants()`)不变
- **append-only minor 版本升级**:2.1.0-omega → 2.2.0-omega
- **承接 ADR-027 决策 6**:3.0.0-omega 留给未来真正的破坏性 API 变更里程碑
- **不新建 crate / 不新建消息总线 / 不自实现压缩 / 不引入 unsafe 依赖 / 不修改核心领域类型**(ADR-028 决策 7)

### 合规性

- 保持 `#![forbid(unsafe_code)]`;不新建消息总线;不引入外部依赖;不修改 `AgentType` / `nexus_core::Task` / `ThinkingMode` / `Quadrant` / `AgentTask`
- 八道质量门全绿:`cargo fmt --check` / `cargo check --workspace` / `clippy -D warnings` / `cargo test --workspace` / `cargo test --ignored`(release) / `cargo audit --deny warnings` / `cargo bench --no-run` / `cargo check --manifest-path fuzz/Cargo.toml`
- 承接 ADR-026 八项决策 + ADR-027 六项决策,全部保持

## v2.1.0-omega (2026-07-20)

**版本代号**: NEXUS-OMEGA (CHIMERA-MAS-Q · 四象限稳定分工)
**Spec**: [add-chimera-mas-quadrant](.trae/specs/add-chimera-mas-quadrant/spec.md)
**架构基线**: v2.0.0-omega → v2.1.0-omega(Stage A → Stage B 结构增量,非破坏性 minor)
**ADR**: [ADR-027-chimera-mas-quadrant](docs/architecture/ADR-027-chimera-mas-quadrant.md)

### 重大变更

- **孙代理四象限稳定分工**(§3):新增 `Quadrant` 四象限模型(Q1 实现 / Q2 集成 / Q3 验证 / Q4 加固),把无界孙层扇出收敛为固定 ≤4 分工,引入 INV-3(扇出界)/ INV-4(象限唯一)不变量
- **WSJF 优先级驱动调度**(§8):新增 `PriorityScheduler` + WSJF 评分模型,以 `TaskPriority` 为调度一等公民,支持动态重排、Critical 抢占 Low、饥饿线性提权
- **精英专家团队 E01-E08 显式编制**(§6):新增 `ExpertRegistry`,含 8 位 10+ 年经验专家的角色/象限/工具白名单/三级权限(L0/L1/L2)
- **SemVer minor 升级**(v2.0.0-omega → v2.1.0-omega):严格向后兼容,仅新增类型/方法/字段,现有公共签名与测试零回归

### Added

#### chimera-mas 结构增量(L9 Quest 层)

- 新增 `src/quadrant.rs`:`Quadrant` 枚举 + `axis()`/`tag()`/`encode_scope()`/`from_task_scope()`/`quality_dimensions()`/`validation_step()`;`activated_quadrants()` 复杂度激活矩阵;`QuadrantPlan`(强制 INV-3/INV-4)
- 新增 `src/scheduler.rs`:`WsjfWeights`/`WsjfInput`/`wsjf_score()`/`score_to_priority()`/`PriorityScheduler`(优先级队列 + 动态重排 + 抢占 + 饥饿老化)
- 新增 `src/experts.rs`:`PermissionTier`(L0/L1/L2)+ `ExpertProfile` + `ExpertRegistry`(内置 E01-E08)
- 扩展 `src/orchestrator.rs`:新增 `delegate_quadrants()` 象限感知孙层编排路径(保留 `delegate()` 现有语义)
- 扩展 `src/delegation.rs`:`AgentTask` 新增 `priority: TaskPriority` 字段(`new()` 默认 Medium + `with_priority()` builder,非破坏性)
- 扩展 `src/error.rs`:新增 `QuadrantFanoutExceeded` / `QuadrantConflict` 变体(25 → 27)
- 新增测试 **62+ 个**:24 quadrant + 19 scheduler + 13 experts + 6 orchestrator(delegate_quadrants)+ 5 proptest(INV-3/INV-4/Critical>Low)
- 新增 ADR-027 + spec `add-chimera-mas-quadrant`

### Changed

- `workspace.package.version`:`2.0.0-omega` → `2.1.0-omega`(SemVer minor,非破坏性)
- `MasError` 变体数:25 → 27(新增两个象限约束变体,附静态计数断言更新)

### 合规性

- 保持 `#![forbid(unsafe_code)]`;不新建消息总线;不引入外部依赖;不修改 `AgentType`/`nexus_core::Task`/`ThinkingMode`
- 六道质量门全绿:`cargo check --workspace` / `clippy -D warnings` / `fmt --check` / `test` / 零 unsafe / 关键路径 O(1)
- 承接 ADR-026 八项决策;调和其决策 8 的版本标注(完整 Stage B 3.0.0-omega 留待未来破坏性里程碑)

## v2.0.0-omega (2026-07-18)

**版本代号**: NEXUS-OMEGA (CHIMERA-MAS · Multi-Agent Orchestration)
**Spec**: [add-chimera-mas-subsystem](.trae/specs/add-chimera-mas-subsystem/spec.md)
**架构基线**: v1.8.0-omega → v2.0.0-omega(GA 后演进 Stage A,第三阶段深度演进里程碑)
**ADR**: [ADR-026-chimera-mas-subsystem](docs/architecture/adr/ADR-026-chimera-mas-subsystem.md)

### 重大变更

- **新增 `crates/chimera-mas` crate**(L9 Quest 层,workspace 第 35 个 crate),引入多 Agent 协同工作能力,支持根协调器委托、并行执行、上下文隔离、Token 预算管理与心跳监控
- **event-bus 扩展 7 个 Agent 相关 NexusEvent 变体**(67 → 74),新增 `EventTopic::Agent` 主题与 3 个辅助类型(`TaskPriority`/`ConsultUrgency`/`AgentStatus`)
- **SemVer major 版本升级**(v1.8.0-omega → v2.0.0-omega),按 §3.3.1 第 5 条向后兼容规则标记 GA 后演进里程碑

### Added

#### chimera-mas crate(L9 Quest 层)

- 新增 `crates/chimera-mas` crate,3 子模块结构:
  - `orchestrator/` — RootOrchestrator 根协调器(任务分发 + 心跳订阅 + `max_agent_depth=5` 深度限制)
  - `agent/` — AgentMeta/AgentType/AgentStatus/AgentFactory/AgentLifecycle(`Idle→Running→Paused→Completed/Failed/Terminated` 状态机)
  - `context/` — AgentContext/ContextBlock/ContextPriority/ContextIsolationGuard/TokenBudget
- 新增 `delegation.rs` 实现 `AgentTask` wrapper(包装 `nexus_core::Task` 不修改核心类型,§3.3.1 领域类型稳定性) + `DelegationExecutor`(`FuturesUnordered` 并行委托,§4.1 规范替代 `join_all`)
- 新增 `MasError` thiserror enum(25 变体 + `test_error_variant_count_at_least_25` 静态断言)
- 新增测试 **219 个**(47 unit + 15 context + 37 factory + 20 meta + 18 task + 16 delegation + 11 integration + 22 orchestrator + 6 proptest + 18 budget + 9 doctest)
- 新增 benchmark `crates/chimera-mas/benches/mas_benchmark.rs`(4 个 criterion benchmark:Agent 创建/消息路由/任务拆分/上下文构建)
- 新增 proptest `crates/chimera-mas/tests/proptest.rs`(6 个属性测试:JSON/MessagePack 序列化往返、ThinkingMode 映射、ContextIsolationGuard 跨 Agent 拒绝、委托深度不变量)

#### event-bus 扩展(L1 Core)

- 在 `NexusEvent` enum 新增 7 个 Agent 相关变体(均含 `metadata: EventMetadata` 字段维持 API 兼容):
  - `AgentTaskDelegated { metadata, from, to, task_id, deadline: DateTime<Utc>, priority: TaskPriority }`
  - `AgentTaskCompleted { metadata, from, to, task_id, result_summary }`
  - `AgentTaskFailed { metadata, from, to, task_id, error, retry_count: u32 }` — **severity = Critical**(§6.2 红线,走 mpsc 旁路通道)
  - `AgentConsultRequested { metadata, from, to, question, context, urgency: ConsultUrgency }`
  - `AgentConsultResponded { metadata, from, to, answer, references: Vec<String> }`
  - `AgentHeartbeat { metadata, from, status: AgentStatus, current_task: Option<String>, token_usage: u64, memory_usage_mb: u64 }`
  - `AgentContextOverflow { metadata, agent_id, current_tokens: usize, max_tokens: usize }`
- 在 `crates/event-bus/src/topic.rs` 新增 `EventTopic::Agent` 变体 + 7 个 match 分支(返回 `EventTopic::Agent`)
- 在 `crates/event-bus/src/logging.rs` 新增 `TopicLabel::Agent` + `From<EventTopic> for TopicLabel` impl(Prometheus 标签同步)
- 在 `crates/event-bus/src/lib.rs` 导出 3 个新类型(`TaskPriority`/`ConsultUrgency`/`AgentStatus`)+ prelude
- 新增测试 **39 个**(34 个 agent_events_test + 5 个 filtered_subscriber_test 新增函数),event-bus 全量 **177 测试通过**

#### ADR 与文档

- 新增 ADR-026 记录 MAS 子系统决策(`docs/architecture/adr/ADR-026-chimera-mas-subsystem.md`,254 行):
  - 决策 1: chimera-mas 归属 L9 Quest 层(与 quest-engine/gea-activator/efficiency-monitor 同层)
  - 决策 2: AgentMessageBus 合并到 event-bus(§2.2 唯一通道铁律)
  - 决策 3: AgentTask wrapper 包装 `nexus_core::Task`,不修改核心类型
  - 决策 4: 不引入 Kuzu/LanceDB/Cognee,用 petgraph + 内存 KNN + 自实现 KG(保持 `#![forbid(unsafe_code)]` 全覆盖)
  - 决策 5: 复用 `nexus_core::ThinkingMode`(不新建 ThinkingMode::Max)
  - 决策 6: Duration 类型用 `tokio::time::Duration`(非 `chrono::Duration`)
- 新增 `docs/architecture/adr_index.md` ADR-026 条目

### Changed

- `Cargo.toml` workspace.package.version `1.8.0-omega` → `2.0.0-omega`;workspace.members 新增 `"crates/chimera-mas"`;workspace.dependencies 新增 `chimera-mas = { path = "crates/chimera-mas" }`
- `crates/event-bus/src/types.rs` NexusEvent 变体数 67 → 74(新增 7 个 Agent 变体),`severity()`/`metadata()`/`type_name()` match 分支同步更新
- `crates/event-bus/src/topic.rs` 新增 `EventTopic::Agent` 变体 + `all()`/`topic()` match 同步更新,测试函数 rename `nine` → `ten`
- `crates/event-bus/src/logging.rs` 新增 `TopicLabel::Agent` + `From` impl
- `crates/event-bus/src/lib.rs` 导出 3 个新类型 + prelude 同步
- `crates/event-bus/tests/filtered_subscriber_test.rs` 变体总数断言 67 → 74 + 5 个新测试函数(`test_subscribe_agent_topic_receives_all_7_variants` / `test_agent_task_failed_has_critical_severity` / `test_agent_events_filtered_by_topic` / `test_recv_matching_agent_task_delegated` / `test_agent_heartbeat_topic_is_agent`)
- `crates/event-bus/tests/metrics_test.rs` EventTopic 计数断言 9 → 10
- `docs/architecture/CODE_WIKI.md` §1.1/§2.1/§3.1/§3.9/§11 + TOC + 目录树同步(34 → 35 crate,版本号 1.7.0-omega → 2.0.0-omega)
- `docs/architecture/adr_index.md` 新增 ADR-026 条目

### 设计文档微调(spec.md §"设计文档微调"13 项差异落地)

- **不新建 AgentMessageBus** — 合并到 event-bus(§2.2 唯一通道铁律;Ω-Event 单一实现)
- **AgentContext 不自实现压缩** — 委托 `hcw-window::HcwWindow`(Ω-Compress 单一实现;1M = 128K 实际 + 8× 稀疏压缩,不暴力加载)
- **AgentTask wrapper 包装 `nexus_core::Task`** — 不修改核心类型(§3.3.1 领域类型稳定性;AgentTask 含 inner + complexity + estimated_tokens + acceptable_latency + quality_requirement)
- **不引入 Kuzu/LanceDB/Cognee** — 用 petgraph + 内存 KNN + 自实现 KG(保持 `#![forbid(unsafe_code)]` 35/35 crate 全覆盖;ADR-005 sqlite-vec 禁用教训延续)
- **复用 `nexus_core::ThinkingMode`** — 不新建 ThinkingMode::Max(`TaskComplexity::From<ThinkingMode>` 映射:Simple→Fast, Medium→Standard, Complex/VeryComplex→Deep)
- **Duration 类型用 `tokio::time::Duration`** — 非 `chrono::Duration`(委托超时 `tokio::time::timeout` 包装,§6.1 零孤儿调用红线)
- **设计文档 11 子模块精简为 3 子模块** — 8/11 子模块与现有 crate 重复(orchestrator + agent + context 三模块覆盖 Stage A 全部需求)
- **6 Phase 42 天拆分为 Stage A(2-3 周)+ Stage B(待评估)** — Stage A 完成 Task 1-17 核心框架,Stage B 待 Stage A 验收后启动(深度集成 quest-engine/gqep-executor/qeep-protocol 三方协同)

### Compliance

- ✅ `#![forbid(unsafe_code)]` 全覆盖(35/35 crate,chimera-mas 顶层声明 + event-bus 维持)
- ✅ 依赖铁律零违规(chimera-mas L9 → 仅依赖 L1-L8 现有 crate,无向上依赖 L10)
- ✅ TDD RED-GREEN 强制(每个 Task 先写失败测试再实现,Task 3/7/8/9/10/11/12/13/14/15 全部遵循)
- ✅ 0 clippy warning,0 fmt 差异(`cargo clippy -p chimera-mas --all-targets --jobs 2 -- -D warnings` + `cargo fmt --all -- --check` 通过)
- ✅ Critical 事件走 mpsc 旁路(`AgentTaskFailed` 用 `publish_critical_blocking`,§6.2 红线)
- ✅ broadcast subscribe 在 spawn 之前同步调用(§4.4 反模式 3)
- ✅ `FuturesUnordered` 替代 `join_all`(§4.1 规范,DelegationExecutor)
- ✅ `tokio::time::Duration` 而非 `chrono::Duration`(§4.4 反模式规避)
- ✅ f32 不隐式转 f64(Task 10 WARNING_THRESHOLD 全程 f64)
- ✅ proptest block-named 语法(§4.1 `fn name(arg in strategy) { body }`)

### 测试矩阵

| 测试套件 | 通过率 | 备注 |
|---------|--------|------|
| chimera-mas 全量 | ✅ 219/219 | 47 unit + 15 context + 37 factory + 20 meta + 18 task + 16 delegation + 11 integration + 22 orchestrator + 6 proptest + 18 budget + 9 doctest |
| event-bus 全量 | ✅ 177/177 | 75 unit + 34 agent_events + 11 control + 4 critical + 27 bus + 10 filtered + 6 integration + 6 metrics + 4 doctest |
| clippy(chimera-mas) | ✅ 0 warning | `--all-targets --jobs 2 -- -D warnings` |
| clippy(event-bus) | ✅ 0 warning | `--all-targets --jobs 2 -- -D warnings` |
| fmt check | ✅ 通过 | `cargo fmt --all -- --check` |
| benchmark 编译 | ✅ 通过 | `cargo check -p chimera-mas --benches`(4 benchmark 函数) |
| **合计新增** | **+258 测试** | chimera-mas 219 + event-bus 39 |

### 升级路径(从 v1.8.0-omega)

1. **自动兼容**: chimera-mas 为新增 crate,不影响既有 34 crate 的 API 与运行时行为
2. **event-bus 变体扩展**: NexusEvent 新增 7 个变体,`match` 父分支需补充 `_ => ...` 或显式处理(本项目内已全量同步)
3. **新依赖**: workspace.dependencies 新增 `chimera-mas = { path = "crates/chimera-mas" }`,成员 crate 通过 `workspace = true` 引用
4. **配置**: chimera-mas 当前无外部配置文件需求,所有运行时参数通过构造器注入
5. **Stage B 衔接**: Stage A 完成核心框架,Stage B 将集成 quest-engine/gqep-executor/qeep-protocol 三方协同 + 专家咨询路由 + 记忆归档 + Wiki 知识共享 + CircuitBreaker + GSOE 进化

---

## v1.8.0-omega (2026-07-16)

**版本代号**: NEXUS-OMEGA (Enterprise TUI Monitoring · Task · Visualization)
**Spec**: [enterprise-tui-monitoring-task-viz](.trae/specs/enterprise-tui-monitoring-task-viz/spec.md)
**架构基线**: v1.7.0-omega → v1.8.0-omega(第三阶段深度演进)

### 主要变化

#### 🖥️ TUI 企业级监控套件(v1.8 P0-P3)

- **P0 设计手册** — `docs/architecture/NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` 8 章编写完成
  - §1 设计哲学(5 原则) · §2 主题系统 · §3 交互范式 · §4 布局系统
  - §5 面板 API · §6 可视化组件 · §7 性能预算 · §8 扩展指南
  - 9 个核心类型导出(TuiBibleVersion/LayoutTemplate/ColorRole/KeyBinding/VizChartKind/PerfBudget/PanelApi/ExtensionHook/PanelRegistry)
- **P0 趋势图 + 阈值告警** — `ResourceMonitorPanel` 增强
  - CPU/内存/网络/磁盘 IO 四维度 5 分钟滑动窗口(300 样本)
  - 中位数滤波(5 样本窗口)去抖动
  - 阈值告警(70%/90%) + RGB 平滑颜色渐变(P4.1 优化)
  - 网络/磁盘 IO 子区域新增
- **P0 任务管理面板** — `TaskManagerPanel` 新增
  - Quest CRUD:P 暂停 / R 恢复 / T 终止 / +/- 优先级(0-10 强校验) / Enter 详情
  - 双向控制通过 `TuiCommand::QuestControl { id, action: QuestAction }`
  - 优先级桥接 0-10 → 0-255(`priority_255 = level * 25`)
- **P1 可视化组件库** — `viz/` 5 组件新增
  - `line_chart` / `heatmap` / `bar_chart` / `gauge` / `histogram`
  - 全部基于 ratatui Canvas/Chart,纯文本渲染,无外部图像依赖
  - 统一 `VizWidget` trait + `VizChartKind` 枚举
- **P1 指标仪表盘** — `MetricsDashboardPanel` 5×2 网格
  - 10 个 cell 可绑定任意 `TuiDataSource::snapshot()` 数据源
  - `bind(source, kind, position)` + `unbind(position)` 动态管理
  - `PanelId::MetricsDashboard` 18 变体全循环自洽
- **P1 历史持久化** — `MetricsHistory` SQLite 落地
  - 路径:`~/.chimera/metrics_history.sqlite`,表 `(unix_ts, metric, value)` 复合主键
  - `INSERT ... ON CONFLICT REPLACE` 幂等,启动时 `cleanup(retention_days)` 自动清理
  - 所有 rusqlite 调用 `tokio::task::spawn_blocking` 包裹(Week 7 教训)
  - WAL 模式 + `synchronous=NORMAL` 平衡一致性/性能
- **P2 系统信息** — `SysinfoPanel` 主机与进程信息
  - 主机信息:OS 内核 · CPU 型号/核心数 · 总内存 · 启动时间
  - 进程信息:Chimera PID · RSS · 线程数
  - 5s 周期刷新(可由 `sysinfo_refresh_interval_ms` 配置覆盖)
  - 跨平台 Windows/Linux/macOS(sysinfo 0.32 统一 API)
- **P2 配置加载器** — `TuiBible` Figment 4 源合并
  - 路径:`~/.chimera/tui_bible.yaml`(与既有 tui.yaml 区分)
  - 环境变量前缀 `CHIMERA_BIBLE_*`,嵌套字段用 `__` 分隔
  - 配置文件不存在 → 静默回退默认;YAML 损坏 → `TuiError::ConfigError`
  - `examples/config/tui_bible.sample.yaml` 含主题/颜色/键位/阈值/布局 5 段示例
- **P3 颜色渐变** — `gradient_color` RGB 三段线性插值
  - Green(#2ECC40) → Yellow(#FFDC00) → OrangeRed(#FF851B) → Red(#FF4136)
  - `is_finite()` 守卫 NaN 输入,`clamp(0, 100)` 边界外钳制
- **P3 三模式排序** — `TaskManagerPanel` 排序模式
  - `SortMode::Priority` 默认 · `Status` · `CreatedAt`
  - 'S' 键循环切换,面板标题动态显示当前模式
  - `created_at_index` 侧表自治追踪首次观察时间(不修改 L1 域类型)

#### 🔧 TuiConfig 5 字段扩展(向后兼容)

- `enable_trend_charts: bool`(默认 false,需显式开启)
- `metrics_sample_interval_ms: u64`(默认 1000,范围 [100, 60000])
- `metrics_history_retention_days: u32`(默认 7,范围 ≥ 1)
- `task_manager_default_sort: SortMode`(默认 Priority)
- `sysinfo_refresh_interval_ms: u64`(默认 5000,范围 ≥ 100)
- 全部 `#[serde(default)]`,旧配置零迁移加载

### 测试矩阵

| 测试套件 | 通过率 | 备注 |
|---------|--------|------|
| `color_gradient_test` | ✅ 11/11 | P4.1 新增 |
| `task_manager_test` | ✅ 10/10 | 5 原有 + 4 排序 + 1 公共 |
| `sysinfo_panel_test` | ✅ 4/4 | P3.1 |
| `tui_bible_config_test` | ✅ 3/3 | P3.2 |
| `metrics_history_persistence_test` | ✅ 3/3 | P1.3 |
| `viz_components_test` | ✅ 5/5 | P1.1 |
| `metrics_dashboard_test` | ✅ 3/3 | P1.2 |
| `trend_charts_test` | ⚠️ 8/9 | 1 视觉测试预存在失败,与本任务无关 |
| `resource_monitor_panel_test` | ✅ 4/4 | 零破坏 |
| `config_persistence_integration` | ✅ 3/3 | 零破坏 |
| lib 单元测试 | ✅ 426/426 | 零破坏 |
| **合计新增** | **+30+ 测试** | P0/P1/P2/P3 全覆盖 |

### 架构约束保持

- ✅ 全部新文件保持 `#![forbid(unsafe_code)]`
- ✅ L10 → L1 event-bus 依赖方向保持,无向上依赖
- ✅ Panel trait 7 方法签名零修改(`id`/`title`/`render`/`handle_key`/`handle_mouse`/`focus`/`scroll_to_*`)
- ✅ 核心领域类型未变更(UserIntent/Quest/Checkpoint/OmniSparseMasks/CLV/NexusState)
- ✅ nexus-core 仍保持最小依赖
- ✅ 所有 rusqlite 调用 `spawn_blocking`(无同步调用,§4.4 #2 红线)
- ✅ broadcast subscribe 模式正确(无 Week 6 事件丢失)
- ✅ Top-K 选择遵循 `select_nth_unstable` 约定
- ✅ sysinfo 0.32 已存在于既有依赖,无新增 crate-level 依赖

### 文档同步

- ✅ `docs/architecture/NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` 新增(8 章)
- ✅ `docs/architecture/tui-suite-architecture.md` 新增(架构图)
- ✅ `docs/architecture/tui-api-impact-matrix.md` 新增(API 影响矩阵)
- ✅ `docs/architecture/tui-suite-tech-stack.md` 新增(技术栈评估)
- ✅ `examples/config/tui_bible.sample.yaml` 新增(配置样例)
- ✅ `Cargo.toml` workspace.package.version = 1.8.0-omega
- ✅ spec docs/architecture/INDEX.md 同步登记手册

### 升级路径(从 v1.7.0-omega)

1. **自动兼容**: `TuiConfig` 新增字段全部 `#[serde(default)]`,旧 `tui.yaml` 无需修改
2. **可选启用**: 趋势图默认关闭,`enable_trend_charts: true` 显式开启
3. **新配置文件**: `~/.chimera/tui_bible.yaml` 不存在时静默 fallback,与 `tui.yaml` 独立
4. **新面板**: `TaskManagerPanel` / `SysinfoPanel` / `MetricsDashboardPanel` 通过 `PanelId` 自动注册,无破坏既有焦点循环
5. **数据库**: `~/.chimera/metrics_history.sqlite` 自动创建,无需手工初始化

---

## v1.7.0-omega (2026-07-14)

**版本代号**: NEXUS-OMEGA (Evolved Interface)

### 主要变化

#### 🖥️ TUI 完整重构 (v1.7-omega Milestone M0-M6)

- **M0** — 接入 EventBus 实时数据流 (443a49c)
- **M1** — 重构为 Panel 架构，分离面板责任 (9b9c97f)
- **M2** — 5 系统监控面板交付 (70ed23d)
- **M3** — 增强交互：命令执行、搜索过滤、弹窗、鼠标与可调整布局 (e04e602)
- **M4** — 双向控制：TUI 通过 EventBus 发布控制请求 (1dfaf95)
- **M5** — TUI P1 验证与打磨 (9346d4a)
- **P2** — 5 监控面板完整交付 (49c22ef)
- **P3** — 交互能力升级 (2ca37ec)
- **P4** — 性能优化 (0b4a356)
- **P5** — 跨面板联动 (a267a6d)
- **P6** — 主题运行时切换与布局模板 (93bc535)

#### 🔒 P0 安全修复

- 清理 main/master 分叉 (af00fda)
- 移除 sqlite-vec 违规 unsafe 依赖 (af00fda)
- 升级 Dockerfile 基础镜像 rust:1.82 → 1.85 (af00fda)
- 添加 .gitignore .worktrees/ 排除 (af00fda)
- 添加 P0 验证脚本 `scripts/verify-p0-cleanup.ps1` (72fc20b)

#### 📦 安装与分发

- 新增 README.md 项目首页 (9cd6be8)
- 新增 Scoop 包管理器 manifest `bucket/chimela.json` (9cd6be8)
- 新增 Homebrew 包管理器 formula `Formula/chimela.rb` (9cd6be8)
- 新增 GPG 签名配置脚本 `scripts/setup-gpg-signing.ps1` (9cd6be8)
- 修复 PS 7.x 兼容性：改用 `[scriptblock]::Create()` (9cd6be8, 1da194d, 317e7ab)
- 统一品牌名为 chimela，消除 Release 下载 404 (9e3301b)

#### 📚 文档

- 更新发布指南至 v1.7.0-omega 精简版 (b6957a9)
- 强化 §9 代码修改前置思考与冗余代码杜绝规则 (eceafc6)
- 远程仓库文档清理 + 版本统一 (189c87b)
- TUI P3 验证归档 + v1.7-omega TUI 深化演进 spec 立项 (0f1d1a0)

#### ✅ 测试

| 类型 | 通过率 |
|------|--------|
| 单元测试 | ✅ 100% |
| 集成测试 | ✅ 100% |
| OWASP Top 10 | ✅ 20/20 |
| 压力测试 (1000 次) | ✅ 零失败 |

---

## v1.5.8-omega (2026-07-13)

- Cargo.lock 版本同步 + workspace 稳定性增强
- 发布物包含 Windows/Linux/macOS × x86_64/aarch64 五平台二进制

## v1.5.7-omega (2026-07-12)

- 首个含 GitHub Release artifacts 的版本
- 初始 5 平台 matrix 构建流水线

## v1.5.6-omega (2026-07-11)

- 持续集成与依赖更新

## v1.5.5-omega — v1.5.0-omega (2026-07-09 ~ 2026-07-11)

- MCP Mesh 量子网格迭代
- CSN 降级链完善
- 监控系统深化
- 集成测试体系建立

## v1.4.0-omega (2026-07-09)

- **架构跳跃版本**: L1-L10 全部 34 crate 功能完整
- SSRA Fusion、LSCT Tiering、GSOE Evolution、NMC Encoder、CHTC Bridge 五大 L2+L10 crate 接入
- MCP 量子网格原型
- E2E 测试体系建立

## v1.0.2-omega — v1.0.0-omega (2026-06-27 ~ 2026-06-28)

- **首周启动**: L0-L1 基础设施、Event Bus、SecCore、Decay、QEEP、CLI 入口
- L9+L5+L1: Quest Engine、Repo Wiki、Model Router
- L6: MLC、HCW、CMT、OSA、KVBSR
- L7: GEA、GQEP、PVL、MTPE、SCC
- L8+L4+L3: Parliament、ASA、AHIRT、TTG、DECB
- **v1.0.0-omega 初始发布** (2026-06-28): 34 crate 全覆盖，3000+ 测试全绿
