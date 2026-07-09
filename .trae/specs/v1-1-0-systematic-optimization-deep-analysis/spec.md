# v1.1.0-omega 系统性深度优化 — 算法与架构整体升级 Spec

> **Spec ID**: `v1-1-0-systematic-optimization-deep-analysis`
> **阶段**: v1.1.0-omega 第二阶段开发(GA 后演进)
> **依据文档**: `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` (2026-07-08,34 crate 源码级全量审计) + `DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md` (16 项优化算法) + `DEEP_RESEARCH_LLM_ARCHITECTURE_MAPPING.md` (10 项创新映射)
> **参考主文档**: `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` v3.0.0-omega(创新演进主参考)+ `AETHER_NEXUS_OMEGA_从零搭建完全指南.md` v2.0.0-omega(工程实施主参考)
> **创建时间**: 2026-07-09
> **基线状态**: 34/34 crate 全实现,3002+ 测试,代码质量基线极高(生产代码零 unwrap / 零 unsafe / 依赖铁律零违规)

---

## Why

项目已通过 v1.0.0-omega GA 发布,但 2026-07-08 完成的 6 路子代理源码级分布式深度审计(`DEEP_RESEARCH_34CRATE_OPTIMIZATION.md` ~109K LOC 全量审计)发现:项目架构纪律虽然优秀,但仍存在 **1 项 Critical 安全漏洞、3 项 P0 正确性 bug、5 项 P0 性能反模式,以及 26 项已有优化方案中 21 项尚未实施(81% 未实施率)**。这些短板若不在 v1.1.0-omega 周期内闭环,将成为后续演进的长期技术债。

本 Spec 的目标是:在严格遵守第二阶段开发规则(nuxus规则 §3.3:OMEGA 四定律守恒 / 依赖方向不可逆 / TDD 守恒 / 领域类型稳定性 / 向后兼容 / 新 crate 准入)的前提下,以"安全修复 → 正确性修复 → P0 性能优化 → P1 架构补债 → P2 渐进优化"五阶段顺序推进,系统性闭环 v1.1.0-omega 周期内所有可执行优化点,实现端到端链路延迟降低 60%+、锁竞争降低 80%+、内存效率提升 2-3× 的可量化收益。

## What Changes

### 阶段 I:安全紧急修复(2 天)

- **[N1 Critical]** `seccore/policy.rs` 移除 `cmd` 命令白名单条目,或对 `cmd /c` 参数链做二次校验,堵住 `cmd /c "危险命令"` 绕过四层防御的 Critical 漏洞
- **[N4 High]** `seccore/asa.rs` 当风险关键字列表为空时返回 `RiskLevel::Unknown` 而非 `Low`,触发额外审计检查
- **[N5 High]** `seccore/audit.rs` 将 AuditChain 追加改为 pre-execution append(执行前记录意图),命令完成后更新执行状态;append 失败必须阻止命令执行

### 阶段 II:正确性修复(3 天)

- **[N2 P0]** `ssra-fusion` 修复 `select_top_k_desc` 主导策略选取 bug — `select_nth_unstable_by` 后取 `selected[0]` 不保证是最大值,改为 `selected.iter().max_by()` 确保选取真正最大权重
- **[N3 P0]** `qeep-protocol` 实现 Request→Ack→Receipt 完整三元组 — 当前 Ack 从未被创建,仅实现二元组
- **[A1 P0]** `quest-engine/checkpoint.rs` 将 `save()`/`load()`/`load_latest()`/`prune_old()` 的同步 `fs::write`/`fs::read` 包装到 `tokio::task::spawn_blocking`,消除 async runtime 阻塞

### 阶段 III:P0 性能优化(1 周)

- **[B1]** `repo-wiki/vector.rs` `Mutex<HashMap>` → `RwLock<HashMap>`,允许并发 KNN 搜索
- **[B3]** `model-router/registry.rs` `DashMap<String, ModelInfo>` → `RwLock<HashMap<String, ModelInfo>>`(≤10 模型场景分片锁开销大于 RwLock)
- **[N10]** `scc-cache/prefetch.rs` 为马尔可夫链转移矩阵增加 LRU 淘汰(容量上限 10000),消除长时间运行内存泄漏
- **[A3]** `repo-wiki/store.rs` 写操作通过 mpsc channel 序列化到专用写入线程,读操作通过 `spawn_blocking`(利用 WAL 并发读)
- **[N11]** `model-router/cacr.rs` `f32` 预算计算改为 `u64` 整数运算,消除大预算值精度丢失

### 阶段 IV:P1 架构补债(2 周)

- **[C1]** `event-bus` 增加 `EventTopic` 枚举(7 类)+ `FilteredSubscriber`,每个订阅者仅接收相关事件,消除全量广播反序列化开销
- **[N6/N7]** `acb-governor` 增加时间维度滞后机制(参照 DECB `tier_switch_lag_ms`),并在 TTG 中增加 ACB/DECB 仲裁层,消除双治理器矛盾指令与振荡风险
- **[N8]** `parliament` 增加 Skeptic 否决覆议机制 — 其他 4 角色以 2/3 超级多数可推翻否决,防止决策僵局
- **[N9]** `sesa-router`(链路末端)增加前置事件校验 — 验证是否收到 OSA + KVBSR + FaaE 事件,代码强制五层路由顺序
- **[F1]** 将 `chimera-cli/config.rs` 的 14 个 section 类型迁移到 `nexus-core/config.rs`,各 crate 通过共享引用消除重复定义
- **[D1]** `repo-wiki` 引入 r2d2 连接池(单写入连接 + 只读连接池),充分利用 WAL 并发读优势

### 阶段 V:P2 渐进优化(4 周)

- **[I4]** `event-bus` Critical 事件走 mpsc 保证投递 + Normal 事件走 broadcast + 注意力过滤
- **[I1]** `model-router` MoE 稀疏门控路由 — 50+ 模型时 O(n)→O(k)
- **[N14]** `gqep-executor` 增加 `gather_deadline_ms` 全局超时,防止大规模 gather 超时累积
- **[N17]** `gea-activator` 为 `TaskProfile` 实现 `Hash` trait 替代 serde_json 序列化
- **[N18]** `quest-engine/ttg.rs` 模式切换事件从 `tracing::info` 迁移到 EventBus
- **[N15]** `repo-wiki` 启用 SQLite FTS5 扩展替代 `LIKE '%query%'` 全表扫描
- **[E1]** `chimera-cli` `OnceCell` 懒初始化配置各 section,消除全量 eager 加载
- **[G2]** `event-bus/BusLogger` 注册 Prometheus 指标导出
- **[Top-K 全量优化]** 全项目 grep `sort_by` / `sort_unstable_by` 用于 Top-K 选取的位置,统一替换为 `select_nth_unstable`(§4.1 Engineering Convention)

### **BREAKING** 变更声明

- `model-router`: `ModelRegistry` 内部数据结构从 `DashMap` 改为 `RwLock<HashMap>`(仅内部实现变更,公开 API 保持不变,符合 §3.3.1.5 向后兼容)
- `repo-wiki`: `VectorIndex` 内部锁从 `Mutex` 改为 `RwLock`(仅内部实现变更)
- `acb-governor`: 新增滞后参数 `tier_switch_lag_ms`(配置默认值向后兼容,缺省时为 1000ms)
- `parliament`: 新增覆议机制 API `reopen_veto()`(新增方法,不修改既有 API)
- `event-bus`: 新增 `FilteredSubscriber` 与 `EventTopic` 类型(新增 API,既有 `subscribe()` 全量广播保持兼容)
- `chimera-cli`/`nexus-core`: 14 个配置 section 类型从 `chimera-cli` 迁移到 `nexus-core`(类型路径变更,可能需要下游 crate 调整 `use` 路径 — 严格遵循 §3.3.1.5 向后兼容,通过 re-export 保持旧路径可用)

## Impact

### 受影响的 Spec(已有)

- `v1-1-0-f2-rusqlite-descoping`(F2 已完成)— 本 Spec 不与之冲突,共享 `nexus-core` 重构原则
- `v1-0-0-omega-ga-release-sprint`(GA 已发布)— 本 Spec 不影响已发布 GA 产物,所有变更将在 v1.1.0-omega 体现

### 受影响的代码

**L1 Core**:
- `crates/nexus-core/src/config.rs`(新增,F1 配置类型迁移目标)
- `crates/event-bus/src/lib.rs` + `types.rs`(C1 主题过滤层 + I4 双通道)
- `crates/model-router/src/registry.rs` + `cacr.rs`(B3 + N11)

**L2 Memory**:
- `crates/mlc-engine/src/engine.rs`(Top-K 优化)
- `crates/hcw-window/src/window.rs`(Top-K 优化)

**L3 Storage**:
- `crates/scc-cache/src/prefetch.rs`(N10 LRU 淘汰)
- `crates/cmt-tiering/src/coordinator.rs`(await 持锁修复,Top-K 优化)

**L4 Security**:
- `crates/seccore/src/policy.rs` + `asa.rs` + `audit.rs`(N1 + N4 + N5 三层安全修复)
- `crates/qeep-protocol/src/protocol.rs`(N3 Ack 三元组补全)

**L5 Knowledge**:
- `crates/repo-wiki/src/vector.rs` + `store.rs`(B1 + A3 + D1 + N15)

**L6 Router**:
- `crates/osa-coordinator/src/masks.rs`(Top-K 优化)
- `crates/kvbsr-router/src/router.rs`(Top-K 优化)
- `crates/sesa-router/src/sparsity.rs`(Top-K 优化 + N9 前置校验)
- `crates/gea-activator/src/activator.rs`(N17 Hash 优化)

**L7 Execution**:
- `crates/ssra-fusion/src/strategy.rs`(N2 主导策略 bug 修复)
- `crates/gqep-executor/src/gather.rs`(N14 全局超时)

**L8 Parliament**:
- `crates/parliament/src/voting.rs` + `debate.rs`(N8 覆议机制)
- `crates/acb-governor/src/governor.rs`(N6/N7 滞后机制)

**L9 Quest**:
- `crates/quest-engine/src/checkpoint.rs`(A1 spawn_blocking)
- `crates/quest-engine/src/ttg.rs`(N18 EventBus 集成 + ACB/DECB 仲裁)

**L10 Interface**:
- `crates/chimera-cli/src/config.rs`(F1 类型迁移源)

### 受影响的依赖

- 新增 `r2d2` + `r2d2_sqlite`(D1 连接池)— 通过 workspace.dependencies 声明
- 无新增 crate(严格遵守 §3.3.1.6 新 crate 准入:本 Spec 不新建 crate,所有优化在现有 34 crate 内完成)
- 不变更核心领域类型(`UserIntent`/`Quest`/`Checkpoint`/`OmniSparseMasks`/`CLV`/`NexusState` — §3.3.1.4 领域类型稳定性)

## ADDED Requirements

### Requirement: 五阶段顺序推进优化

系统 SHALL 按"安全紧急修复 → 正确性修复 → P0 性能优化 → P1 架构补债 → P2 渐进优化"五阶段顺序推进,严禁跨阶段并行执行(Critical 安全漏洞必须在正确性修复前闭环)。

#### Scenario: 阶段顺序强制

- **WHEN** 开发者尝试在 Phase I 安全修复未完成时启动 Phase II 正确性修复
- **THEN** tasks.md 检查清单阻止后续阶段启动,提示"Phase I 必须先完成 N1/N4/N5 三项安全修复"

#### Scenario: 每阶段独立验收

- **WHEN** 一个阶段所有 task 完成
- **THEN** 该阶段所有测试通过(`cargo test --workspace` 退出码 0)、clippy 零警告、fmt 一致、checklist.md 全部勾选,才能进入下一阶段

### Requirement: TDD 守恒(RED-GREEN-REFACTOR)

每个 bugfix / 性能优化 / 架构调整 MUST 先写失败测试(RED)再实现(GREEN),最后重构(REFACTOR)。不允许删除已有测试,只允许增强。

#### Scenario: bugfix TDD 流程

- **WHEN** 修复 N2 SSRA 主导策略 bug
- **THEN** 先添加测试验证当前 `selected[0]` 在特定数据下不是最大值(RED)→ 修复实现使测试通过(GREEN)→ 重构提取辅助函数(REFACTOR)

### Requirement: 依赖方向不可逆(§2.2 铁律)

任何变更 MUST 遵守 L(N)→L(N-1) 允许 / L(N)→L(N+1) 禁止的依赖铁律。跨层通信只能走 EventBus / MCP Mesh。

#### Scenario: F1 配置类型迁移依赖合规

- **WHEN** 将 `chimera-cli/config.rs` 的 14 个 section 类型迁移到 `nexus-core/config.rs`
- **THEN** `nexus-core` 不依赖 `chimera-cli`(向下:L1 → L10 禁止),所有下游 crate 通过 `use nexus_core::config::*` 引用

### Requirement: 第二阶段 OMEGA 四定律守恒(§3.3.1.1)

任何优化 MUST 不变更 OMEGA 四定律(Ω-Sparse / Ω-Compress / Ω-Evolve / Ω-Event),任何演进必须与之对齐。

#### Scenario: C1 事件主题过滤不破坏 Ω-Event

- **WHEN** 在 event-bus 增加 `EventTopic` + `FilteredSubscriber`
- **THEN** Ω-Event 定律(Tokio broadcast 跨层解耦)保持不变,既有 `subscribe()` 全量广播 API 保持向后兼容,FilteredSubscriber 是新增 API

### Requirement: 向后兼容(§3.3.1.5)

GA 后 API 变更须遵循 SemVer。破坏性变更需 major 版本升级(v2.0.0-omega)。

#### Scenario: ModelRegistry 数据结构变更向后兼容

- **WHEN** `ModelRegistry` 内部从 `DashMap` 改为 `RwLock<HashMap>`
- **THEN** 公开 API(`register`/`get`/`list`/`route`)签名不变,既有调用方零修改即可编译通过

### Requirement: 每阶段验证报告与质量评估文档

每个阶段完成后 MUST 产出验证报告(记录 cargo test / clippy / fmt / audit 结果)与质量评估文档(记录 LOC 变化、测试增量、性能基线对比)。

#### Scenario: 阶段验收文档归档

- **WHEN** Phase I 安全修复完成
- **THEN** `docs/optimization/v1.1.0/phase1_security_verification_report.md` 创建,记录 N1/N4/N5 三项修复的 before/after 代码片段、测试用例、cargo test 输出

## MODIFIED Requirements

### Requirement: seccore 零信任沙箱(N1/N4/N5 修复后)

`seccore` 在修复后 SHALL 实现:
- 命令白名单不再包含 `cmd`,或对 `cmd /c` 参数链做二次校验(N1)
- ASA 审计评分在风险关键字列表为空时返回 `RiskLevel::Unknown` 触发额外审计(N4)
- AuditChain 在命令执行**前**追加意图记录,执行**后**更新状态;append 失败阻止命令执行(N5)

### Requirement: ssra-fusion 主导策略选取(N2 修复后)

`ssra-fusion::select_top_k_desc` 在修复后 SHALL 选取真正最大权重的策略作为主导,而非 `selected[0]`。

### Requirement: qeep-protocol 三元组协议(N3 修复后)

`qeep-protocol` 在修复后 SHALL 实现 Request→Ack→Receipt 完整三元组,Ack 在 Receipt 之前创建并校验。

### Requirement: quest-engine Checkpoint 异步 I/O(A1 修复后)

`quest-engine::CheckpointManager` 在修复后 SHALL 将所有 `fs::write`/`fs::read` 包装到 `tokio::task::spawn_blocking`,不阻塞 async runtime。

## REMOVED Requirements

### Requirement: seccore `cmd` 命令白名单条目

**Reason**: N1 Critical 安全漏洞 — `cmd /c "危险命令"` 可绕过四层防御
**Migration**: 移除 `cmd` 白名单条目,或对 `cmd /c` 参数链做二次校验;更新 `tests/security/owasp_top10.rs` 新增渗透用例验证修复

### Requirement: ssra-fusion `selected[0]` 作为主导策略

**Reason**: N2 P0 正确性 bug — `select_nth_unstable_by` 不保证 `[0]` 是最大值
**Migration**: 改为 `selected.iter().max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Equal))`;添加 proptest 验证不变量

### Requirement: qeep-protocol 二元组协议(无 Ack)

**Reason**: N3 P0 正确性 bug — Ack 从未被创建,三元组协议缺失
**Migration**: 在 Request→Receipt 链路增加 Ack 创建点,实现完整三元组;更新 qeep-protocol 集成测试验证 Ack 时序

---

## 附:执行策略说明

### 子代理协作团队配置(§9 要求)

按用户要求组建由 10 年以上行业经验的精英专家级子代理构成的协作团队,任务优先级为核心指导原则:

| 子代理 | 职责 | 阶段映射 |
|--------|------|---------|
| **安全专家 agent** | N1/N4/N5 三层安全修复 + OWASP 渗透用例 | Phase I |
| **正确性专家 agent** | N2/N3/A1 P0 bug 修复 + TDD | Phase II |
| **性能专家 agent** | B1/B3/N10/A3/N11 P0 性能优化 + bench | Phase III |
| **架构专家 agent** | C1/N6/N7/N8/N9/F1/D1 P1 架构补债 | Phase IV |
| **演进专家 agent** | I1/I4/N14/N15/N17/N18/E1/G2 P2 渐进优化 | Phase V |
| **质量验证 agent** | cargo test / clippy / fmt / audit / bench 全量回归 | 每阶段末尾 |
| **文档同步 agent** | CODE_WIKI / CHANGELOG / project_memory 同步 | 每阶段末尾 |

### 多轮结构化思考与验证流程

每个阶段执行前 MUST 经过以下三轮结构化思考:

1. **Round 1(现状核验)**:Read 目标代码当前状态,验证 DEEP_RESEARCH 报告发现的准确性
2. **Round 2(方案设计)**:基于核验结果设计具体修改方案,明确文件路径与代码片段
3. **Round 3(影响评估)**:评估变更对其他 crate 的影响,识别潜在 break 点

每个阶段执行后 MUST 经过严谨验证流程:

1. **V1(编译验证)**:`cargo check --workspace` 退出码 0
2. **V2(测试验证)**:`cargo test --workspace` 退出码 0,测试数量 ≥ 基线(3002+ Phase I 起点)
3. **V3(lint 验证)**:`cargo clippy --workspace --all-targets --jobs 2 -- -D warnings` 退出码 0
4. **V4(格式验证)**:`cargo fmt --all -- --check` 退出码 0
5. **V5(安全验证)**:`cargo audit --deny warnings` 退出码 0(网络可用时)
6. **V6(文档验证)**:相关 CODE_WIKI / CHANGELOG / project_memory 章节已同步

### 长期主义工作理念

- 不为短期性能牺牲架构纪律(如不引入 unsafe / 不违反依赖铁律)
- 每个优化点必须有测试覆盖,不允许"裸奔"优化
- 每个阶段必须有验证报告归档,作为后续演进的历史参考
- 不竭泽而渔:Phase V 渐进优化可按需延后到 v1.2.0-omega,不阻塞 v1.1.0-omega 发布

### 资源授权

团队可调用所有符合任务要求且系统允许的工具资源,包括但不限于:
- **MCP**:Sequential Thinking(多轮思考)、Memory(经验记录)、DesktopCommander(文件操作)
- **Skills**:test-driven-development、systematic-debugging、requesting-code-review、verification-before-completion
- **Sub-agents**:Explore(代码搜索)、general-purpose(多步骤任务)、rust-architecture-expert(Rust 架构专家)、algorithm-optimization-team(算法优化团队)、backend-architecture-diagnostician(后端架构诊断)

### 交付成果

每个阶段交付:
1. **优化方案文档**:具体修改的文件路径、代码片段、设计理由
2. **验证报告**:`cargo test` / `clippy` / `fmt` / `audit` 输出截图
3. **质量评估文档**:LOC 变化、测试增量、性能基线对比(min-of-N 5 次采样)
4. **CODE_WIKI / CHANGELOG / project_memory 同步**:章节更新,记录决策依据与经验教训

最终交付:
- `docs/optimization/v1.1.0/full_optimization_report.md` — 全量优化报告
- `docs/optimization/v1.1.0/performance_baseline_comparison.md` — 性能基线对比(before vs after)
- `CHANGELOG.md` v1.1.0-omega 章节 — 完整变更记录
- `project_memory.md` 新增 5-10 条 Lessons Learned(每阶段 1-2 条核心教训)
