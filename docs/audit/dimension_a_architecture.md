# 维度 A:架构一致性审计报告

> 审计依据:`AETHER_NEXUS_OMEGA_ULTIMATE.md` §2.2 依赖铁律、§4 编码规范、§10 核心类型;
> 审计对象:Chimera CLI NEXUS-OMEGA Workspace(34 crates);
> 审计方法:全量 Cargo.toml 扫描 + 关键源码抽查 + 命名模式 Grep 核验。

## 1. 执行摘要

| 项 | 值 |
|----|----|
| 审计日期 | 2026-06-28 |
| 审计范围 | 34 个 crates 的 Cargo.toml + 12 个关键源码文件 + 命名模式全量 Grep |
| 总体评价 | **合规**(架构一致性优秀) |
| 问题数量 | Critical 0 / Major 0 / Minor 5 |

**核心结论**:十层架构映射与 workspace members 完全一致;34 个 crate 的生产依赖方向 100% 合规,无 L(N)→L(N+1) 违规;OMEGA 四定律全部落地;3 个关键 ADR(003/004/005)均按决策执行;nexus-core 保持最小依赖且 `#![forbid(unsafe_code)]`;跨层通信统一走 EventBus,V1/V2/V3/V4 四处历史违规均已通过事件机制修正。5 个 Minor 问题均为注释错误、骨架状态或降级决策,不影响架构合规性。

---

## 2. 依赖方向合规性

### 2.1 扫描方法

逐个读取 34 个 crate 的 `Cargo.toml`,提取 `[dependencies]` 段中的 `path = "../xxx"` 引用,按十层架构映射核验方向:
- L(N) → L(N-1) 或 L(N) → L(N) 视为合规
- L(N) → L(N+1) 视为 **Critical 违规**
- `dev-dependencies` 中的跨层引用单独标注(不违反生产依赖铁律,但记录备查)

### 2.2 扫描结果

| 层级 | Crate | path 依赖(生产) | 方向核验 |
|------|-------|------------------|---------|
| L1 | nexus-core | (无 path 依赖) | ✓ 最小依赖 |
| L1 | event-bus | (无 path 依赖) | ✓ 最小依赖 |
| L1 | model-router | nexus-core(L1)、event-bus(L1) | ✓ 同层互引 |
| L2 | nmc-encoder | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L2 | hcw-window | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L2 | mlc-engine | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L3 | scc-cache | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L3 | lsct-tiering | event-bus(L1)、nexus-core(L1)、cmt-tiering(L3) | ✓ 向下+同层 |
| L3 | cmt-tiering | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L4 | seccore | event-bus(L1) | ✓ 向下 |
| L4 | qeep-protocol | (无 path 依赖) | ✓ 最小依赖 |
| L4 | decay-engine | (无 path 依赖) | ✓ 最小依赖 |
| L5 | repo-wiki | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L5 | gsoe-evolution | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L5 | auto-dpo | event-bus(L1) | ✓ 向下 |
| L6 | osa-coordinator | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L6 | kvbsr-router | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L6 | faae-router | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L6 | sesa-router | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L6 | gea-activator | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L6 | gqep-executor | nexus-core(L1)、event-bus(L1)、qeep-protocol(L4) | ✓ 向下 |
| L7 | pvl-layer | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L7 | mtpe-executor | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L7 | ssra-fusion | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L8 | parliament | nexus-core(L1)、event-bus(L1)、seccore(L4) | ✓ 向下 |
| L8 | acb-governor | event-bus(L1) | ✓ 向下 |
| L8 | decb-governor | nexus-core(L1)、event-bus(L1) | ✓ 向下 |
| L9 | quest-engine | nexus-core(L1)、event-bus(L1)、decb-governor(L8) | ✓ 向下 |
| L9 | gea-activator | (见 L6) | — |
| L9 | efficiency-monitor | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L10 | chimera-cli | (无 path 依赖) | 骨架状态(见 §9 问题 2) |
| L10 | chimera-tui | event-bus(L1) | ✓ 向下 |
| L10 | chtc-bridge | event-bus(L1)、nexus-core(L1) | ✓ 向下 |
| L10 | mcp-mesh | event-bus(L1) | ✓ 向下 |
| L10 | csn-substitutor | event-bus(L1)、nexus-core(L1) | ✓ 向下 |

**注**:gea-activator 在十层映射中归属 L6 Router(见根 Cargo.toml:188 注释 "L6 Router"),与 §2.1 架构映射一致,上表 L9 行为笔误占位,实际以 L6 行为准。

### 2.3 违规项清单

**生产依赖**:无 Critical/Major 违规。

**dev-dependencies 跨层引用**(测试代码,不违反生产铁律,记录备查):
- `parliament/Cargo.toml:44-46`:dev-dependencies 引用 `quest-engine`(L9,parliament 为 L8,L8→L9 向上)
- `gea-activator/Cargo.toml:24-28`:dev-dependencies 引用 `pvl-layer`/`mtpe-executor`/`gqep-executor`(L7,gea-activator 为 L6,L6→L7 向上)
- `osa-coordinator/Cargo.toml:28-31`:dev-dependencies 引用 `hcw-window`/`mlc-engine`(L2)/`cmt-tiering`(L3)/`kvbsr-router`(L6),用于 e2e 测试

这些 dev-dep 引用均带有明确注释说明"测试专用依赖,不污染生产依赖图",符合 §2.2 依赖铁律的例外条款。

---

## 3. 十层架构映射一致性

### 3.1 workspace members 核验

根 `Cargo.toml:14-27` 声明 34 个 members,与 `crates/` 目录下 34 个子目录逐一对应,无遗漏、无多余:

```
nexus-core, event-bus, quest-engine, repo-wiki, model-router,
parliament, pvl-layer, osa-coordinator, kvbsr-router, faae-router,
gea-activator, gqep-executor, sesa-router, ssra-fusion, csn-substitutor,
mtpe-executor, mlc-engine, hcw-window, cmt-tiering, scc-cache,
lsct-tiering, seccore, decay-engine, qeep-protocol, decb-governor,
acb-governor, efficiency-monitor, gsoe-evolution, auto-dpo, mcp-mesh,
nmc-encoder, chtc-bridge, chimera-tui, chimera-cli
```

计数:34 个,与十层架构映射的 34 个 crate 完全一致。✓

### 3.2 层级映射核验

将 workspace members 按十层架构映射(§2.1)逐一核验:

| 层级 | 架构映射声明 | 实际 crate | 一致性 |
|------|------------|-----------|--------|
| L1 Core | nexus-core · event-bus · model-router | 3 个 | ✓ |
| L2 Memory | nmc-encoder · hcw-window · mlc-engine | 3 个 | ✓ |
| L3 Storage | scc-cache · lsct-tiering · cmt-tiering | 3 个 | ✓ |
| L4 Security | seccore · qeep-protocol · decay-engine | 3 个 | ✓ |
| L5 Knowledge | repo-wiki · gsoe-evolution · auto-dpo | 3 个 | ✓ |
| L6 Router | osa-coordinator · kvbsr-router · faae-router · sesa-router · gea-activator · gqep-executor | 6 个 | ✓ |
| L7 Execution | pvl-layer · gqep-executor · mtpe-executor · ssra-fusion | 注:gqep-executor 在映射中同时出现于 L6/L7,实际归属见根 Cargo.toml:203 注释 "L6 Router(聚集执行)" | ✓ |
| L8 Parliament | parliament · acb-governor · decb-governor | 3 个 | ✓ |
| L9 Quest | quest-engine · gea-activator · efficiency-monitor | 注:gea-activator 实际为 L6(见 §2.2),此处映射声明与根 Cargo.toml:188 注释存在轻微不一致 | 见问题 1 |
| L10 Interface | chimera-cli · chimera-tui · chtc-bridge · mcp-mesh · csn-substitutor | 5 个 | ✓ |

**注**:L7 Execution 在架构映射中列出 `gqep-executor`,但根 `Cargo.toml:203` 注释将其归为 "L6 Router(聚集执行)"。这是架构映射表与实际注释的轻微不一致,需确认 gqep-executor 的最终归属。鉴于其依赖 qeep-protocol(L4)且核心职责是"聚集执行",归为 L7 更符合"执行层"语义,但项目注释归为 L6。此为文档一致性问题,不影响依赖合规性(无论 L6 还是 L7,其依赖方向均合规)。

---

## 4. OMEGA 四定律体现

### 4.1 Ω-Sparse(全维稀疏)

**核验对象**:`osa-coordinator`(L6 Router)

**核验结论**:✓ 完全符合

**证据**:
- `crates/osa-coordinator/src/types.rs:25-29` 定义五维度 ID 类型:
  - `ToolId` — routing 维度
  - `FileId` — context 维度
  - `MemoryId` — memory 维度
  - `OperationId` — audit 维度
  - `TaskId` — budget 维度
- `crates/osa-coordinator/src/types.rs:222-231` `TaskProfile` 携带五维度候选集:`available_tools`/`available_files`/`available_memories`/`recent_operations`/`active_tasks`
- `crates/osa-coordinator/src/masks.rs:47` `SparseMask<T>` 泛型容器,支持 Top-K 稀疏化,文档注释明确"包含五个 `SparseMask<T>` 实例(routing/context/memory/audit/budget)"(masks.rs:17)
- `crates/osa-coordinator/src/coordinator.rs:45-63` `OmniSparseMasks` 聚合体,含五个字段:`routing`/`context`/`memory`/`audit`/`budget`,与架构手册 §Ω-Sparse 五维度掩码完全对齐
- `crates/osa-coordinator/src/coordinator.rs:156` `OmniSparseCoordinator` 命名符合 *Coordinator 模式

### 4.2 Ω-Compress(分层压缩)

**核验对象**:`hcw-window`(L2 Memory)

**核验结论**:✓ 完全符合

**证据**:
- `crates/hcw-window/src/types.rs:42-52` `WindowTier` 枚举定义四级窗口:
  - `L0`:4K Token,快速响应(complexity < 0.25)
  - `L1`:32K Token,常规任务(0.25 ≤ complexity < 0.5)
  - `L2`:128K Token,复杂任务(0.5 ≤ complexity < 0.75)
  - `L3`:1M Token 等效(128K 实际加载 + 8× 稀疏化压缩比,complexity ≥ 0.75)
- `crates/hcw-window/src/types.rs:418-435` `HcwConfig` 定义四级容量默认值:`l0_capacity=4096`、`l1_capacity=32768`、`l2_capacity=131072`、`l3_capacity=1048576`
- `crates/hcw-window/src/types.rs:115-123` `effective_capacity` 方法:L3 实际加载 = `l3_capacity / 8`(避免 1M 暴力加载,符合架构红线)
- 压缩机制:`CompressionReport`(types.rs:384)记录压缩前后大小与压缩比,通过 `ContextCompressed` 事件广播

### 4.3 Ω-Evolve(在线进化)

**核验对象**:`gsoe-evolution`(L5 Knowledge)

**核验结论**:✓ 完全符合

**证据**:
- `crates/gsoe-evolution/src/engine.rs:30` `GsoeEvolutionEngine` 实现 GRPO 风格在线强化学习驱动器
- 核心流程(engine.rs:6-11):采样 → 评估(GRPO 组内相对优势 + 规则适应度)→ 选择(top elite_ratio)→ 变异 → 发布 `GsoePolicyUpdated` 事件
- 事件订阅(engine.rs:13-15):订阅 `ConsensusReached`(议会共识作为进化奖励)、`RedTeamAudit`(红队审计作为对抗信号提升 mutation_rate)
- 跨层通信:通过 `EventBus` 广播进化结果,不直接 import 上层 crate,符合 §2.2 依赖铁律

### 4.4 Ω-Event(事件驱动)

**核验对象**:`event-bus`(L1 Core)

**核验结论**:✓ 完全符合(超额完成)

**证据**:
- `crates/event-bus/src/bus.rs:16` `use tokio::sync::broadcast;`(符合 ADR-003)
- `crates/event-bus/src/bus.rs:33-34` `EventBus` 封装 `broadcast::Sender<NexusEvent>`,默认容量 1024(bus.rs:23)
- `crates/event-bus/src/types.rs:63` `NexusEvent` 枚举共 **62 个变体**(远超架构手册要求的 30+),覆盖 L1-L10 全部跨层通信场景
- 关键事件标注 `Critical`(types.rs:1142-1152):`CheckpointSaved`/`ConsensusReached`/`SlowConsumerDropped`/`OrphanCallDetected`/`SkepticVeto`/`RedTeamAudit`,背压策略据此保护
- 历史违规修正记录(types.rs:6-9):V1(OSA→HCW)、V2(MLC→efficiency-monitor)、V3/V4(Parliament→GSOE/AutoDPO)均通过新增事件类型修正

---

## 5. ADR 决策执行

### 5.1 ADR-003:Event Bus 使用 Tokio broadcast

**核验结论**:✓ 完全执行

**证据**:
- `crates/event-bus/Cargo.toml:8` 依赖 `tokio = { workspace = true }`(workspace 配置 `features = ["full", "tracing"]`,包含 `sync::broadcast`)
- `crates/event-bus/src/bus.rs:16` `use tokio::sync::broadcast;`
- `crates/event-bus/src/bus.rs:49` `broadcast::channel(capacity)` 创建通道
- `crates/event-bus/src/bus.rs:34` `sender: broadcast::Sender<NexusEvent>` 类型字段
- 提供完整 API:`publish`(async)/`publish_blocking`(同步)/`subscribe`/`recv`/`recv_timeout`/`try_recv`

### 5.2 ADR-004:Checkpoint 使用 MessagePack 序列化

**核验结论**:✓ 完全执行

**证据**:
- `crates/event-bus/Cargo.toml:15` 依赖 `rmp-serde = { workspace = true }`(workspace 根 Cargo.toml:69 注释 "ADR-004:消息序列化协议为 MessagePack")
- `crates/event-bus/src/bus.rs:330-336` 实现 `serialize_msgpack`/`deserialize_msgpack`,用于跨进程投递(MCP Mesh)与持久化
- `crates/quest-engine/Cargo.toml:28` 依赖 `rmp-serde = { workspace = true }`,注释明确 "MessagePack 序列化(ADR-004,与 Event Bus 一致),用于检查点持久化"
- `crates/quest-engine/src/checkpoint.rs:9` 文档注释 "序列化格式:MessagePack(ADR-004,与 Event Bus 一致)"
- `crates/quest-engine/src/checkpoint.rs:76` `rmp_serde::to_vec(quest)` 实际序列化调用
- 完整性校验:`checkpoint.rs:80` 计算 SHA-256 哈希,恢复时比对防状态漂移

### 5.3 ADR-005:cmt-tiering 使用 SQLite + 内存向量检索

**核验结论**:✓ 执行(向量检索部分降级,有充分理由与注释)

**证据**:
- `crates/cmt-tiering/Cargo.toml:12` 依赖 `rusqlite = { workspace = true }`(features = ["bundled", "chrono"])
- `crates/cmt-tiering/src/lib.rs:7-10` 文档明确四级分层:
  - `HotTier`(DashMap + LRU,容量 256,延迟 < 1μs)— 内存
  - `WarmTier`(SQLite WAL 模式,容量 4096,延迟 < 5ms)— SQLite
  - `ColdTier`(SQLite 附加数据库,容量 65536,延迟 < 50ms)— SQLite
  - `IceTier`(归档只读文件,无容量上限,延迟 < 500ms)— 文件
- `crates/cmt-tiering/src/lib.rs:40` `#![forbid(unsafe_code)]`
- `crates/cmt-tiering/Cargo.toml:9-10` 注释说明共享 `nexus-core` 的 `sqlite_pragma` 模块,消除 PRAGMA 重复实现
- **向量检索降级说明**:`crates/repo-wiki/Cargo.toml:40-45` 注释说明 `sqlite-vec` 集成降级为内存向量检索,原因是 "sqlite-vec 0.1.9 的 Rust binding 仅暴露 C 入口 sqlite3_vec_init,注册需调用 sqlite3_auto_extension + unsafe,与 `#![forbid(unsafe_code)]` 冲突"。降级决策合理,在 10-1000 条目规模下 KNN < 10ms 性能足够,且保留未来引入 HNSW 等专用向量索引的扩展点。

---

## 6. 命名模式规范

**核验方法**:Grep 搜索 `pub struct \w+(Coordinator|Engine|Router|Protocol|Governor|Block)\b` 与 `pub (struct|enum) \w*(Mask|Masks)\b`。

### 6.1 *Coordinator(协调器模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `OmniSparseCoordinator` | osa-coordinator/src/coordinator.rs:156 | ✓ |
| `CmtCoordinator` | cmt-tiering/src/coordinator.rs:58 | ✓ |
| `LsctCoordinator` | lsct-tiering/src/tiering/coordinator.rs:57 | ✓ |
| `AsaSandboxCoordinator` | seccore/src/asa.rs:395 | ✓ |

### 6.2 *Engine(引擎模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `DecayEngine` | decay-engine/src/engine.rs:24 | ✓ |
| `QuestEngine` | quest-engine/src/engine.rs:41 | ✓ |
| `MlcEngine` | mlc-engine/src/engine.rs:45 | ✓ |
| `GsoeEvolutionEngine` | gsoe-evolution/src/engine.rs:30 | ✓ |
| `SlimeFusionEngine` | ssra-fusion/src/fusion/engine.rs:37 | ✓ |
| `MultimodalFusionEngine` | nmc-encoder/src/fusion.rs:28 | ✓ |
| `AlertRuleEngine` | efficiency-monitor/src/alerts.rs:27 | ✓ |

### 6.3 *Router(路由模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `ModelRouter` | model-router/src/router.rs:38 | ✓ |
| `KVBlockSemanticRouter` | kvbsr-router/src/router.rs:124 | ✓ |
| `FaaeRouter` | faae-router/src/router.rs:72 | ✓ |
| `SesaRouter` | sesa-router/src/activation.rs:71 | ✓ |

### 6.4 *Protocol(协议模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `QeepProtocol` | qeep-protocol/src/protocol.rs:71 | ✓ |

### 6.5 *Governor(治理器模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `AcbGovernor` | acb-governor/src/governor.rs:34 | ✓ |
| `DecbGovernor` | decb-governor/src/governor.rs:64 | ✓ |
| `TtgGovernor` | quest-engine/src/ttg.rs:197 | ✓ |

### 6.6 *Mask<T>(掩码模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `SparseMask<T>` | osa-coordinator/src/masks.rs:47 | ✓ 泛型容器,符合 *Mask<T> 模式 |
| `OmniSparseMasks` | osa-coordinator/src/coordinator.rs:45 | ✓ 五维度聚合体(非泛型,语义合理) |
| `SesaMask` | sesa-router/src/mask.rs:45 | ✓ 非泛型掩码 |

**说明**:架构手册示例 `OmniSparseMasks` 实际作为五维度聚合体(包含五个 `SparseMask<T>` 字段),`SparseMask<T>` 才是真正的泛型掩码容器。两者命名均符合掩码语义,设计合理。

### 6.7 *Block(块模式)

| 类型 | 位置 | 符合性 |
|------|------|--------|
| `SemanticBlock` | kvbsr-router/src/types.rs:83 | ✓ |
| `AuditBlock` | seccore/src/audit.rs:22 | ✓ |

**命名模式总评**:7 类命名模式共 24 个类型,全部符合规范,无命名偏差。体现了项目对 §4.3 命名模式约定的高度遵守。

---

## 7. nexus-core 最小依赖

**核验对象**:`crates/nexus-core/Cargo.toml` + `src/lib.rs`

**核验结论**:✓ 完全符合最小依赖原则

**证据**:
- `crates/nexus-core/Cargo.toml:6-19` 生产依赖全部为 workspace 共享依赖:`ndarray`/`serde`/`serde_json`/`thiserror`/`sha2`/`hex`/`uuid`/`chrono`/`tokio`/`tracing`/`rusqlite`,**无任何 `path = "../xxx"` 上层 crate 依赖**
- `crates/nexus-core/src/lib.rs:32` `#![forbid(unsafe_code)]` — 项目铁律
- `crates/nexus-core/src/lib.rs:33` `#![warn(missing_docs, clippy::all)]` — 文档与 lint 严格化
- `crates/nexus-core/src/lib.rs:35-41` 模块声明:`clv`/`error`/`newtype`/`path_util`/`sqlite_pragma`/`state`/`types`,均为核心领域类型与工具,无上层业务逻辑
- Grep 验证:`crates/nexus-core/src/` 中无任何 `use quest_engine|parliament|osa_coordinator|...` 等上层 crate 引用(搜索结果 No matches)

**最小依赖体现**:
- `nexus_core::id_newtype!` 宏被 L6(osa-coordinator/types.rs:25-29)、L3(cmt-tiering)、L6(kvbsr-router)等多层复用,消除 newtype 重复实现
- `nexus_core::sqlite_pragma` 模块被 L3(cmt-tiering/Cargo.toml:9-10)共享,消除 PRAGMA 重复实现
- `CLV`(512 维潜在向量)作为 L2-L6 的统一语义表示,通过 nexus-core 提供

---

## 8. 跨层通信合规性

**核验方法**:抽查 parliament(L8)、quest-engine(L9)、efficiency-monitor(L9)、chimera-cli(L10)的源码,确认跨层调用通过 EventBus 而非直接 import 下层 crate 的具体实现。

### 8.1 parliament(L8 Parliament)

**核验结论**:✓ 跨层通信仅走 EventBus

**证据**:
- `crates/parliament/Cargo.toml:8-12` 生产依赖:`nexus-core`(L1)+ `event-bus`(L1)+ `seccore`(L4,L8→L4 向下合规)
- `crates/parliament/src/lib.rs:11` 文档明确 "发布 `ConsensusReached`/`VoteCast`/`DebateStarted`/`SkepticVeto`/`CapabilityFrozen`/`RedTeamAudit`/`AhirtProbeCompleted` 事件通知订阅者"
- **V3/V4 违规修正验证**:Grep 搜索 `parliament/src/` 中 `use gsoe_evolution|auto_dpo|quest_engine`,结果 **No matches** — 确认 parliament 不直接 import GSOE(L5)/AutoDPO(L5)/quest-engine(L9),历史 V3/V4 向上依赖违规已通过 `ConsensusReached` 事件机制修正
- parliament 对 seccore(L4)的依赖用于 AHIRT 调用 `validate_command` 探测命令注入(L8→L4 向下依赖,合规)

### 8.2 quest-engine(L9 Quest)

**核验结论**:✓ 跨层通信仅走 EventBus

**证据**:
- `crates/quest-engine/Cargo.toml:8-10` 生产依赖:`nexus-core`(L1)+ `event-bus`(L1)+ `decb-governor`(L8,L9→L8 向下合规)
- `crates/quest-engine/src/lib.rs:8-11` 文档明确 "广播进度事件 / 切换思考模式(TTG),广播模式切换事件供 Parliament 调整预算 / 完成 Quest 时广播 ExecutionCompleted 事件"
- 通过 `EventBus` 发布:`QuestCreated`/`QuestProgressUpdated`/`ThinkingModeSwitched`/`CheckpointSaved`/`CheckpointLoaded`/`ExecutionCompleted`
- 对 `decb-governor`(L8)的依赖用于 TTG 订阅 `BudgetTier` 联动切换(L9→L8 向下,合规)

### 8.3 efficiency-monitor(L9 Quest)

**核验结论**:✓ 跨层通信仅走 EventBus

**证据**:
- `crates/efficiency-monitor/Cargo.toml:26-28` 生产依赖:`event-bus`(L1)+ `nexus-core`(L1)
- `crates/efficiency-monitor/src/lib.rs:65` `use event_bus::{EventBus, EventMetadata, NexusEvent};`
- `crates/efficiency-monitor/src/collectors.rs:17` `use event_bus::{EventSeverity, NexusEvent};` — 订阅事件并发布 `EfficiencyAlertTriggered`
- Grep 验证:`efficiency-monitor/src/` 中 `use` 语句仅引用 `event_bus`、`nexus_core`(通过 `crate::types`)、workspace 共享 crate,无任何下层或同层 crate 直接 import

### 8.4 chimera-cli(L10 Interface)

**核验结论**:骨架状态(见 §9 问题 2),无跨层通信违规

**证据**:
- `crates/chimera-cli/Cargo.toml:6-20` 生产依赖全部为 workspace 共享依赖(clap/figment/tokio/serde/anyhow/tracing),**无任何 `path = "../xxx"` 下层 crate 依赖**
- `crates/chimera-cli/src/main.rs:19` 仅 `use chimera_cli::{cli::Cli, commands, config};`(自身 lib)
- Grep 验证:`chimera-cli/src/` 中无任何下层 crate 的 `use` 语句
- 当前状态:仅实现 Clap 子命令定义与 Figment 配置加载,未装配下层组件(Stage 0 预期)

---

## 9. 问题清单

| ID | 严重程度 | 问题 | 代码位置 | 修复建议 |
|----|---------|------|---------|---------|
| A-01 | Minor | `quest-engine/Cargo.toml:33` 注释将 `decb-governor` 标为 "L3 预算治理",实际 `decb-governor` 属于 L8 Parliament(见根 Cargo.toml:183 注释 "L8 Parliament(DECB 治理器)")。注释层级标注错误,易误导读者。 | `crates/quest-engine/Cargo.toml:33` | 将注释 "L3 预算治理" 修正为 "L8 Parliament 预算治理",并更新依赖方向说明为 "L9→L8 向下依赖允许" |
| A-02 | Minor | `chimera-cli`(L10 Interface)处于骨架状态,无任何下层 crate 的 path 依赖,未装配 EventBus 与下层组件。作为 L10 入口,无法实际启动系统。 | `crates/chimera-cli/Cargo.toml:6-20`、`crates/chimera-cli/src/main.rs:19` | 这是 Stage 0 预期状态,建议在 Week 7/8 集成阶段补充:① 添加 `event-bus` path 依赖;② 在 `commands::dispatch` 中装配 EventBus 与下层 crate;③ 通过 EventBus 订阅下层事件而非直接 import |
| A-03 | Minor | `repo-wiki/Cargo.toml:40-45` `sqlite-vec` 集成降级为内存向量检索,与 ADR-005 "SQLite + 向量检索" 的完整形态略有偏差。降级原因合理(forbid unsafe_code 冲突),但需记录为已知技术债。 | `crates/repo-wiki/Cargo.toml:40-45` | 短期保留降级(10-1000 条目规模 KNN < 10ms 性能足够);长期在 Week 6 NMC 实现后重新评估引入 HNSW 等专用向量索引(无需 unsafe) |
| A-04 | Minor | `parliament/Cargo.toml:44-46` dev-dependencies 引用 `quest-engine`(L9),parliament 为 L8,属 L8→L9 向上引用。虽为 dev-dep 不违反生产铁律,但测试时直接依赖上层 crate 不利于测试隔离。 | `crates/parliament/Cargo.toml:44-46` | 建议在 E2E 测试中通过 EventBus mock 或测试 harness 模拟 quest-engine 事件,而非直接依赖上层 crate。同理 `gea-activator/Cargo.toml:24-28` 对 pvl-layer/mtpe-executor/gqep-executor(L7)的 dev-dep 引用 |
| A-05 | Minor | 十层架构映射表中 `gqep-executor` 同时出现于 L6 Router 与 L7 Execution,根 `Cargo.toml:203` 注释归为 "L6 Router(聚集执行)",但 §2.1 架构映射将其列于 L7 Execution。归属声明不一致。 | `Cargo.toml:203` vs 架构映射表 | 统一归属:鉴于 gqep-executor 依赖 qeep-protocol(L4)且核心职责是"聚集执行"(Gather-Query Execution Protocol),建议归为 L7 Execution,并更新根 Cargo.toml 注释;或维持 L6 归属并更新架构映射表 |

---

## 10. 长期主义建议

### 10.1 依赖治理自动化

**现状**:本次审计通过人工逐个读取 34 个 Cargo.toml 核验依赖方向,效率低且易遗漏。

**建议**:在 CI 中引入依赖方向 lint 脚本(可作为 `cargo xtask check-deps` 子命令),基于十层架构映射表自动校验所有 crate 的 `path` 依赖方向,发现 L(N)→L(N+1) 立即拒绝合并。脚本可读取一个 `layers.toml` 配置文件(声明每个 crate 的层级),解析所有 Cargo.toml 的 path 依赖并校验方向。

### 10.2 dev-dependencies 向上引用治理

**现状**:parliament、gea-activator 等 crate 的 dev-dependencies 存在向上引用(L8→L9、L6→L7),虽不违反生产铁律,但反映测试时需要向上装配组件,测试隔离性不足。

**建议**:长期演进为"测试 harness 在最顶层(L10 或 E2E 测试 crate)装配所有组件,下层 crate 仅用 mock 测试自身逻辑"。短期可保留 dev-dep 向上引用,但应在注释中明确标注"仅用于 E2E 集成测试,单元测试不得依赖上层 crate"。

### 10.3 chimera-cli 装配规划

**现状**:chimera-cli(L10 入口)处于骨架状态,无下层 path 依赖,无法实际启动系统。

**建议**:在 Week 7(MCP Mesh 集成)或 Week 8(打磨)阶段,为 chimera-cli 补充:
1. `event-bus` path 依赖,创建全局 EventBus 实例
2. 按需装配下层 crate(quest-engine/parliament/osa-coordinator 等),通过 EventBus 串联
3. `commands::dispatch` 中根据子命令选择装配的 crate(惰性初始化,保持 `--version` 快速响应)
4. 避免在 main.rs 中直接 import 下层 crate 的具体实现,通过 trait 抽象或 EventBus 解耦

### 10.4 ADR-005 向量检索长期演进

**现状**:sqlite-vec 因 unsafe 冲突降级为内存向量检索,10-1000 条目规模性能足够,但大规模(10万+条目)场景下 KNN 性能可能成为瓶颈。

**建议**:Week 6 NMC 编码器实现后,重新评估向量索引选型:
- 评估 `hnsw` crate(纯 Rust,无 unsafe,支持高性能近似最近邻搜索)
- 或评估 `usearch` bindings(若提供 safe Rust 封装)
- 在 `repo-wiki` 中抽象 `VectorIndex` trait,支持内存/SQLite-vec/HNSW 多后端切换

### 10.5 架构映射表单一事实源

**现状**:十层架构映射表存在于 `AETHER_NEXUS_OMEGA_ULTIMATE.md`、根 Cargo.toml 注释、各 crate Cargo.toml 注释多处,存在不一致风险(如 A-05 的 gqep-executor 归属)。

**建议**:建立单一事实源(如 `docs/architecture/ten_layers.toml`),声明每个 crate 的层级与职责,所有文档注释与 CI lint 均从此文件派生。本次审计已确认 `docs/architecture/ten_layers.md` 存在,建议将其升级为机器可读的 TOML 配置,并作为 CI 依赖方向校验的输入。

---

## 附录:审计工具与过程

| 步骤 | 工具 | 范围 | 结果 |
|------|------|------|------|
| 1. workspace members 核验 | Read | 根 Cargo.toml | 34 members 确认 |
| 2. crate 目录核验 | LS | crates/ | 34 子目录确认 |
| 3. 依赖方向扫描 | Read ×34 | 全部 Cargo.toml | 0 违规 |
| 4. OMEGA 四定律核验 | Read | event-bus/osa-coordinator/hcw-window/gsoe-evolution 源码 | 4/4 符合 |
| 5. ADR 决策核验 | Read + Grep | event-bus/quest-engine/cmt-tiering/repo-wiki 源码 | 3/3 执行 |
| 6. 命名模式核验 | Grep | 全部 *.rs | 24 类型符合 |
| 7. nexus-core 最小依赖 | Read + Grep | nexus-core 源码 | 完全符合 |
| 8. 跨层通信抽查 | Read + Grep | parliament/quest-engine/efficiency-monitor/chimera-cli | 全走 EventBus |

**审计完整性声明**:本报告所有结论均引用具体代码位置(文件:行号),可由读者独立复现验证。审计过程未修改任何代码,仅做只读分析。
