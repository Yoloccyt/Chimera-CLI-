# ADR-026: CHIMERA-MAS 多 Agent 协同子系统架构决策

## 状态

已接受 (2026-07-18)

## 背景

- 项目当前处于 v1.8.0-omega(GA 后演进,第三阶段模块级系统性优化),34/34 crate 全实现,OMEGA 十层架构完整。
- 设计文档 `CHIMERA_MULTI_AGENT_协同工作系统_终极设计文档.md`(v5.0.0-omega)设计了完整的 MAS 子系统(11 子模块、6 Phase 42 天),用于填补 L9 Quest 层多 Agent 编排能力空白。
- 经 3 位 10+ 年经验专家(chimera-release-analyst + architecture-optimization-analyst + rust-architecture-expert)分布式深度分析,识别 **17 项 P0 阻断级差异** 与项目规则 `.trae/rules/nuxus规则.md` 冲突,涉及层级归属、依赖方向、唯一通道、unsafe 禁令、领域类型稳定性、内存防爆、SemVer 渐进演进等核心红线。
- 需在落地前对设计文档进行必要微调,并将 8 项关键架构决策以 ADR 形式记录,作为 `crates/chimera-mas/` 实现的权威依据。
- 已批准 spec: `.trae/specs/add-chimera-mas-subsystem/spec.md`(2026-07-18)。

> **ADR 编号说明**: spec.md 中提到的 "ADR-027" 是基于专家早期分析草稿,实际 `docs/architecture/adr_index.md` 现有最大编号为 ADR-025 + ADR-SIMD-001,故本 ADR 编号确认为 **ADR-026**。

## 决策

经专家团队多轮结构化思考与双路径交叉验证,对 CHIMERA-MAS 子系统作出以下 8 项架构决策:

### 决策 1: 层级归属 — chimera-mas 归属 L9 Quest 层

`chimera-mas` crate 归属 **L9 Quest 层**,与 `quest-engine` / `gea-activator` / `efficiency-monitor` 同层,作为多 Agent 协同工作的编排入口。

- 不归 L8 Parliament 层(治理层): MAS 不是治理组件,而是任务编排组件
- 不归 L7 Execution 层: MAS 不直接执行,而是协调 Agent 间委托关系
- L9 层级定位使 chimera-mas 可向下依赖 L1-L8 全部 crate,复用现有 34 crate 80% 能力

### 决策 2: AgentMessageBus 合并到 event-bus — 不独立建设

设计文档 §2.3 定义的独立 `AgentMessageBus` **不独立建设**,Agent 间通信合并到现有 `event-bus`,在 `NexusEvent` 新增 7 个 Agent 相关变体:

- `AgentTaskDelegated { from, to, task_id, deadline, priority }` — Normal 级,走 broadcast
- `AgentTaskCompleted { from, to, task_id, summary }` — Normal 级,走 broadcast
- `AgentTaskFailed { from, to, task_id, error, retry_count }` — **Critical 级,走 mpsc 通道**
- `AgentConsultRequested { from, to, task_id, question }` — Normal 级
- `AgentConsultResponded { from, to, task_id, answer }` — Normal 级
- `AgentHeartbeat { agent_id, status, load }` — Normal 级
- `AgentContextOverflow { agent_id, current_tokens, max_tokens }` — **Critical 级,走 mpsc 通道**

NexusEvent 变体总数从 85 → 92。点对点通信通过 `EventBus::recv_matching(|e| matches!(e, NexusEvent::AgentTaskDelegated { to, .. } if to == target_id))` 实现。

### 决策 3: AgentTask wrapper — 包装 nexus_core::Task,不修改核心领域类型

设计文档 §4.1 的 `TaskDefinition` / `TaskComplexity` 改为在 `chimera-mas` crate 内定义 `AgentTask` wrapper 类型,包装 `nexus_core::Task` 并扩展 MAS 特有字段(complexity / estimated_tokens / parent_agent_id / delegation_depth 等)。

- 不修改 `nexus_core::Task` 的任何字段(遵守 §3.3.1 第 4 条领域类型稳定性)
- `TaskComplexity` 映射到 `nexus_core::ThinkingMode`:Simple→Fast, Medium→Standard, Complex/VeryComplex→Deep
- 不新建 `ThinkingMode::Max`(规则文档中不存在该变体)

### 决策 4: 不引入 unsafe 依赖 — 用 petgraph + 内存 KNN 替代 Kuzu/LanceDB/Cognee

设计文档建议引入的 Kuzu(图数据库)、LanceDB(向量数据库)、Cognee(知识图谱)依赖**全部不引入**,改为:

- Kuzu 图数据库 → 用 `petgraph`(纯 Rust,workspace 已隐含可用)
- LanceDB 向量数据库 → 复用 `repo-wiki` 现有 VectorSearcher(内存 KNN,10-1000 entry scale)
- Cognee 知识图谱 → 自实现基于 `petgraph` + `rusqlite` 的轻量 KG(若 Stage B 需要)
- sqlite-vec → 复用 `repo-wiki` 内存 KNN(ADR-005 已禁用 sqlite-vec)

### 决策 5: 精简 3 子模块 — 删除 8 个重叠子模块

设计文档 §10.1 定义的 11 子模块精简为 **3 个 MAS 特有子模块**:

- `orchestrator/` — 根协调器,层级递归委托编排
- `agent/` — AgentMeta/AgentType/AgentFactory/AgentLifecycle
- `context/` — AgentContext 独立上下文隔离(包装 hcw-window,不自实现压缩)

删除的 8 个重叠子模块及其复用映射:

| 删除子模块 | 复用 crate |
|-----------|-----------|
| memory/ | mlc-engine + cmt-tiering + scc-cache + hcw-window |
| wiki/ | repo-wiki(ISCM 索引 + FTS5 + 内存 KNN) |
| evolution/ | gsoe-evolution + auto-dpo |
| stability/ | parliament + acb-governor + decb-governor + qeep-protocol |
| monitoring/ | efficiency-monitor |
| message_bus/ | event-bus(本 ADR 决策 2) |
| storage/ | scc-cache + cmt-tiering |
| knowledge_graph/ | 自实现 petgraph + sqlite(若需要) |

### 决策 6: 复用 ThinkingMode — 不新建 ThinkingMode::Max

设计文档提到 `ThinkingMode::Max` **不存在于 `nexus_core::ThinkingMode` 枚举**,复用现有三级思考模式:

- `ThinkingMode::Fast` — 对应 Simple 任务(快速响应)
- `ThinkingMode::Standard` — 对应 Medium 任务(标准深度)
- `ThinkingMode::Deep` — 对应 Complex/VeryComplex 任务(深度推理)

`AgentTask` 的 `TaskComplexity` 通过映射函数转为 `ThinkingMode`,委托给 `quest-engine::TTG`(Thinking Toggle Governance)处理。

### 决策 7: 1M 上下文经 HCW 稀疏化 — 不暴力加载

设计文档 §3.1 的 1M Token 暴力加载违反内存防爆红线,改为通过 `hcw-window` 四级窗口分层 + `osa-coordinator` 稀疏掩码实现:

- `AgentContext` 包装 `hcw-window::HierarchicalWindow`,不自实现压缩逻辑
- 1M 上下文 = 128K 实际加载 + 8× 稀疏压缩(Ω-Compress 单一实现原则)
- `OmniSparseMasks` 由 `osa-coordinator::OmniSparseCoordinator::compute_masks()` 计算
- 50 Agent + 1M 上下文 = **130MB**(HCW 模式) vs 305MB(暴力加载,被否决)
- Critical 块(system_prompt)永不被压缩,Optional 块(wiki 知识)可完全丢弃

### 决策 8: Stage A/B 拆分 — 渐进演进,不一次性交付

设计文档 §11 的 6 Phase 42 天一次性交付拆分为两个阶段:

- **Stage A (v2.0.0-omega)** — 核心框架:3 子模块 + 7 NexusEvent 变体 + AgentTask wrapper + MasError 错误体系(2-3 周)
- **Stage B (v3.0.0-omega,待 Stage A 验收后启动)** — 完整子系统:委托执行深度集成 + 专家咨询路由 + 记忆归档策略 + Wiki 知识共享 + 稳定性保障

不采用设计文档 v5.0.0-omega 版本号,遵循 SemVer 渐进演进。

## 理由

### 决策 1 理由(L9 归属)

- **§2.1 分层映射**:L9 Quest 层包含 `quest-engine · gea-activator · efficiency-monitor`,职责是"任务分解与调度"。MAS 多 Agent 编排本质是任务调度的扩展,天然属于 L9。
- **§2.2 依赖铁律**:L9 可向下依赖 L1-L8 全部 crate,使 MAS 复用 quest-engine(L9 同层)、parliament(L8)、pvl/gqep/mtpe/ssra(L7)、osa/kvbsr/faae/sesa(L6)、repo-wiki/gsoe/auto-dpo(L5)、seccore/qeep/decay(L4)、scc/lsct/cmt(L3)、nmc/hcw/mlc(L2)、nexus-core/event-bus/model-router(L1)成为可能,80% 复用率有赖于此。

### 决策 2 理由(合并 event-bus)

- **§2.2 唯一通道铁律**:"event-bus 是唯一的模块间通信通道,所有状态变更必须通过事件类型广播"。新建 AgentMessageBus 形成第二个通信通道,直接违反铁律。
- **§5.3 事件总线**:Critical 级事件(SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded)必须用 mpsc channel 确保送达。`AgentTaskFailed` 与 `AgentContextOverflow` 同等关键,需走 mpsc 通道。
- **§4.4 async 反模式第 3 条**:`tokio::broadcast` 不缓存历史消息,`bus.subscribe()` 必须在 `tokio::spawn()` 之前同步调用。复用现有 event-bus 已有成熟模式,避免新通道引入新风险。

### 决策 3 理由(AgentTask wrapper)

- **§3.3.1 第 4 条(领域类型稳定性)**:"核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState`)变更需 ADR 记录"。`Task` 同为核心类型,修改需 ADR,且 wrapper 模式可避免修改。
- **§6.1 架构红线**:"禁止功能标志,用能力场自然进化替代"。AgentTask 通过 wrapper 扩展字段,不引入功能标志。

### 决策 4 理由(不引入 unsafe 依赖)

- **§4.1 编码规范**:"所有 crate 必须 `#![forbid(unsafe_code)]` — crate 级属性,只约束当前 crate 源码,不传播到依赖"。Kuzu/LanceDB/Cognee 含 unsafe binding,引入后虽不破坏当前 crate 的 forbid,但破坏项目"零 unsafe 依赖"哲学(ADR-012)。
- **§10.2 Docker 镜像约束**:镜像体积 < 100MB。引入 Kuzu/LanceDB/Cognee 将导致镜像膨胀至 200-400MB(违反约束)。
- **§7.2 发布前检查清单**:binary 体积 < 50MB(strip + panic=abort + opt-level=z + lto + codegen-units=1)。引入这些依赖将导致 binary 膨胀至 100-200MB(违反约束)。
- **ADR-005 持久化存储选型降级**:sqlite-vec 0.1.9 binding 需 unsafe,已改内存 KNN。MAS 沿用同一决策。

### 决策 5 理由(精简 3 子模块)

- **§6.1 架构红线(功能乱)**:Claude Code 尸检教训"44 个未发布标志",本项目红线"禁止功能标志,用能力场自然进化替代"。8 个重叠子模块是功能重复,违反 DRY。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**:Ω-Compress 由 hcw-window 单一实现, MAS 不应自实现压缩(memory/ 子模块删除);Ω-Evolve 由 gsoe-evolution 单一实现,MAS 不应自实现进化(evolution/ 子模块删除);Ω-Event 由 event-bus 单一实现,MAS 不应自实现消息总线(message_bus/ 子模块删除)。

### 决策 6 理由(复用 ThinkingMode)

- **ADR-019 TTG 思考模式**:已定义三级(Fast/Standard/Deep),预算联动。新建 ThinkingMode::Max 会破坏 ADR-019 决策,引入未发布的第 4 级(违反 §6.1 功能标志红线)。
- **§3.3.1 第 4 条(领域类型稳定性)**:`ThinkingMode` 是核心枚举,修改需 ADR。复用避免触发 ADR 流程。

### 决策 7 理由(1M 上下文经 HCW)

- **§6.1 架构红线(内存爆炸)**:Claude Code 尸检教训"1M Token 暴力加载",本项目红线"必须经 HCW 分层 + OSA 稀疏化后再加载(1M = 128K 实际 + 8× 稀疏压缩)"。MAS 必须遵守同一红线。
- **§3.3.1 第 1 条(Ω-Compress 守恒)**:四级窗口 + 神经形态记忆由 hcw-window + mlc-engine 单一实现。MAS 不应绕过 HCW 自实现压缩。
- **资源评估**:50 Agent + 1M 上下文 = 130MB(HCW 模式) vs 305MB(暴力加载),节省 175MB(57% 降低)。

### 决策 8 理由(Stage A/B 拆分)

- **§3.4 第三阶段开发**:已定义 v2.0.0-omega 及之后为"深度优化与模块级系统化阶段"。设计文档 v5.0.0-omega 跨度太大,违反 SemVer 渐进演进。
- **§3.3.1 第 5 条(向后兼容)**:"GA 后 API 变更须遵循 SemVer,破坏性变更需 major 版本升级"。Stage A(v2.0.0-omega,核心框架)与 Stage B(v3.0.0-omega,完整子系统)分开,降低单次发布风险。
- **§3.4.1 第 6 条(性能可证伪)**:"任何性能优化必须有 criterion benchmark 证据"。Stage A 完成后可对核心框架做基准测试,为 Stage B 提供数据支撑。

## 影响

### 新增内容

- **新增 crate**: `crates/chimera-mas/`(L9 Quest 层,workspace 第 35 个 crate),含 3 子模块(orchestrator/agent/context)+ delegation.rs + error.rs + lib.rs + tests/
- **新增 ADR**: 本文档(ADR-026)
- **新增 MasError thiserror enum**: 25+ 变体,覆盖 MAS 特有错误场景(ContextIsolationViolation / MaxDepthExceeded / AgentNotFound / DelegationTimeout 等)

### 修改内容

- **修改 `crates/event-bus/src/types.rs`**: `NexusEvent` 新增 7 个变体(85 → 92)
- **修改 `crates/event-bus/src/topic.rs`**: `topic()` 新增 `EventTopic::Agent` 映射(7 个新变体)
- **修改 `crates/event-bus/tests/filtered_subscriber_test.rs`**: 守护测试变体数断言更新(85 → 92)
- **修改 `Cargo.toml`**: `workspace.members` 新增 `crates/chimera-mas`,`workspace.dependencies` 新增 `chimera-mas = { path = "crates/chimera-mas" }`
- **修改 `docs/architecture/CODE_WIKI.md`**: §3.1 新增 chimera-mas 索引条目(34 → 35 crate);§11 新增 ADR-026 行
- **修改 `docs/architecture/adr_index.md`**: 新增 ADR-026 条目,总数 26 → 27
- **修改 `CHANGELOG.md`**: 新增 v2.0.0-omega 章节

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 34 → 35(最小变更) |
| 代码复用率 | MAS 80% 能力复用现有 crate,仅 20% 为 MAS 特有新代码 |
| Docker 镜像体积 | < 100MB(不引入 Kuzu/LanceDB/Cognee) |
| binary 体积 | < 50MB(不引入 unsafe 依赖) |
| 测试覆盖 | 新增 200+ 测试用例,目标 85% 覆盖率 |
| 内存评估 | 50 Agent + 1M 上下文 = 130MB(HCW 模式) |
| NexusEvent 变体数 | 85 → 92(+7 Agent 变体) |

### 版本号变更

- `workspace.package.version`: `1.8.0-omega` → `2.0.0-omega`(Stage A)
- Stage A 验收后:`2.0.0-omega` → `3.0.0-omega`(Stage B)

## 考虑的方案

### 方案 1: 按设计文档原样实现 11 子模块(否决)

- **方案内容**: 完全按 `CHIMERA_MULTI_AGENT_协同工作系统_终极设计文档.md` §10.1 实现 11 子模块
- **否决理由**: 8/11 子模块与现有 crate 重复(memory/wiki/evolution/stability/monitoring/message_bus/storage/knowledge_graph),违反 DRY 原则与 §3.3.1 第 1 条 OMEGA 四定律守恒(Ω-Compress/Ω-Evolve/Ω-Event 单一实现)。重复实现还会引入维护负担与一致性风险。

### 方案 2: 引入 Kuzu/LanceDB/Cognee(否决)

- **方案内容**: 按设计文档引入 Kuzu(图数据库)+ LanceDB(向量数据库)+ Cognee(知识图谱)
- **否决理由**:
  1. 违反 ADR-012 `#![forbid(unsafe_code)]` 全覆盖哲学(虽不破坏 crate 级 forbid,但破坏项目零 unsafe 依赖)
  2. Docker 镜像膨胀至 200-400MB,违反 §10.2 镜像 < 100MB 约束
  3. binary 膨胀至 100-200MB,违反 §7.2 binary < 50MB 约束
  4. ADR-005 已确立"sqlite-vec 禁用,改内存 KNN"先例,MAS 应沿用

### 方案 3: 新建 AgentMessageBus(否决)

- **方案内容**: 按设计文档 §2.3 独立建设 AgentMessageBus + 9 个 AgentMessage 变体
- **否决理由**:
  1. 违反 §2.2 "event-bus 是唯一的模块间通信通道"铁律
  2. 形成第二个通信通道,引入跨通道一致性难题
  3. 重复实现 broadcast/mpsc 双通道机制(event-bus 已有)
  4. Critical 级 Agent 事件无法复用 event-bus 的 mpsc 送达保证

### 方案 4: v5.0.0-omega 版本号(否决)

- **方案内容**: 按设计文档采用 v5.0.0-omega 作为 MAS 子系统首次发布版本
- **否决理由**:
  1. 违反 SemVer 渐进演进(从 v1.8.0-omega 直接跳到 v5.0.0-omega,跨 3 个 major 版本)
  2. 与 §3.4 第三阶段定义冲突(第三阶段定义为 v2.0.0-omega 及之后的深度优化)
  3. 跳号发布会丢失 v2/v3/v4 的演进历史,影响 CHANGELOG 可追溯性
  4. Stage A/B 一次性交付风险高,违反渐进演进原则

## 合规性

- **§2.1 分层映射**: 符合。chimera-mas 归属 L9 Quest 层,与 quest-engine/gea-activator/efficiency-monitor 同层。
- **§2.2 依赖铁律**: 符合。L9 → L1-L8 向下依赖,不向上依赖;跨层通信走 event-bus(决策 2 合并)。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: 符合。Ω-Sparse 委托 osa-coordinator,Ω-Compress 委托 hcw-window,Ω-Evolve 委托 gsoe-evolution,Ω-Event 委托 event-bus。
- **§3.3.1 第 4 条(领域类型稳定性)**: 符合。AgentTask wrapper 不修改 nexus_core::Task;ThinkingMode 复用不新建 Max 变体。
- **§3.3.1 第 5 条(向后兼容)**: 符合。Stage A/B 拆分遵循 SemVer,NexusEvent 新增变体属非破坏性 minor 变更(向后兼容)。
- **§3.4 第三阶段开发**: 符合。Stage A (v2.0.0-omega) + Stage B (v3.0.0-omega) 与第三阶段定义对齐。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 全覆盖,无 unwrap()/expect(),库层 thiserror + 应用层 anyhow。
- **§4.4 async 反模式**: 符合。决策 2 复用 event-bus 已有 broadcast 模式;委托执行用 FuturesUnordered;不持锁跨 .await。
- **§6.1 架构红线**: 符合。1M 上下文经 HCW 稀疏化(决策 7);不引入功能标志(决策 5 精简子模块);单函数 ≤200 行;async 必须 await 或 spawn 管理。
- **§6.2 Week 1-8 实战新红线**: 符合。不持锁 .await(CircuitBreaker 改 AtomicU8);rusqlite 必须 spawn_blocking;broadcast 先 subscribe 再 spawn;BudgetExceeded severity = Critical(决策 2 AgentTaskFailed/AgentContextOverflow 同级);Critical 安全事件用 mpsc;不引入 unsafe 依赖(决策 4);Top-K 用 select_nth_unstable。
- **§10.2 Docker 镜像约束**: 符合。镜像 < 100MB,binary < 50MB(决策 4 不引入 unsafe 依赖)。

## 相关文档

- **spec**: `d:/Chimera CLI/.trae/specs/add-chimera-mas-subsystem/spec.md`(2026-07-18 已批准)
- **设计文档**: `CHIMERA_MULTI_AGENT_协同工作系统_终极设计文档.md`(v5.0.0-omega,设计源)
- **规则**: `.trae/rules/nuxus规则.md` §2.1(分层映射)/§2.2(依赖铁律)/§3.3.1(第二阶段开发原则)/§3.4(第三阶段开发)/§4.1(编码规范)/§4.4(async 反模式)/§5.3(事件总线)/§6.1(架构红线)/§6.2(Week 1-8 新红线)/§10.2(Docker 镜像约束)
- **CODE_WIKI.md**: `docs/architecture/CODE_WIKI.md` §3.1(crate 索引)/§11(ADR 表)
- **ADR 索引**: `docs/architecture/adr_index.md`(本 ADR 同步更新)
- **关联 ADR**: ADR-005(持久化存储选型,sqlite-vec 禁用先例)/ADR-012(内存安全,forbid unsafe 全覆盖)/ADR-019(TTG 思考模式,复用 ThinkingMode)/ADR-003(Event Bus 实现选型,Tokio broadcast)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-18
> **基线版本**: v1.8.0-omega(创建时)→ v2.0.0-omega(Stage A 落地后)
> **分析团队**: chimera-release-analyst + architecture-optimization-analyst + rust-architecture-expert(3 位 10+ 年经验专家分布式深度分析)
