# ADR-027: CHIMERA-MAS 孙代理四象限稳定分工 + 优先级调度 + 精英专家团队

## 状态

已接受 (2026-07-20)

## 背景

- 项目当前处于 v2.0.0-omega(ADR-026 Stage A 已落地),`chimera-mas` crate(L9 Quest 层,workspace 第 35 个 crate)已实现核心骨架:`agent/`(meta/factory/lifecycle)、`context/`(manager/isolation/budget)、`delegation.rs`、`orchestrator.rs`、`error.rs`,并含 ~5000 行测试。event-bus 已含 7 个 Agent 变体 + `TaskPriority`/`ConsultUrgency`/`AgentStatus`。
- 设计文档 `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md`(代号 CHIMERA-MAS-Q,v1.0)在 Stage A 之上系统化补齐三项核心结构能力,但该文档声明为"纯设计蓝图,不改代码",落地须另立 spec + ADR(其 §13.4 落地路径)。
- 经 8 专家视角(发布分析/架构优化/Rust 架构/代码审查/TDD 守护/安全审计/性能优化/DevOps 可观测)分布式深度分析与多轮交叉验证,确认现有实现的三处系统化缺口:
  1. `orchestrator.rs` 的 `pick_agent_type()` 对 `depth >= 3` 一律返回 `GrandAgent` 且扇出无上限(`sub_agent_count(VeryComplex)=5`),存在"无界委托爆炸"与"职责漂移"风险(对应三重悖论文档的推理悖论)。
  2. `delegate()` 未把 `TaskPriority` 作为调度一等公民,无动态优先级/抢占/饥饿保护。
  3. 精英专家团队(E01-E08)仅在设计文档中草拟,无代码级显式编制与权限分级。
- 本 ADR 记录将上述能力以**非破坏性 minor 增量**落地的 6 项关键决策,作为 spec `add-chimera-mas-quadrant` 实现的权威依据。

> **ADR 编号确认**: `docs/architecture/adr_index.md` 现有最大编号为 ADR-026 + ADR-SIMD-001。`add-chimera-mas-subsystem` spec/checklist 中残留的 "ADR-027" 引用系 Stage A 早期草稿(实际落地为 ADR-026,见 ADR-026 编号说明),从未被真正占用。本 ADR 确认为 **ADR-027**。

## 决策

### 决策 1: 孙代理四象限收敛 — 固定角色 + 扇出 ≤ 4

孙代理(GrandAgent,depth 3)由"无界扇出的同质执行单元"收敛为**固定四象限稳定分工**:

- **Q1 实现象限 Implementation**(Produce×Core): 主业务逻辑与核心算法。
- **Q2 集成象限 Integration**(Produce×Cross): 接口对接、依赖管理、兼容性适配。
- **Q3 验证象限 Verification**(Assure×Core): 单元/集成测试、正确性验证。
- **Q4 加固象限 Hardening**(Assure×Cross): 安全、性能基准、文档、可观测性。

引入两条运行不变量:

- **INV-3(孙层扇出界)**: 任意子代理专家的直接孙代理数 ≤ 4。
- **INV-4(象限唯一)**: 同一子代理下,四象限中每个象限至多一个活跃孙代理实例。

### 决策 2: 象限经 `task_scope` 尾缀编码 — 不修改 AgentType 五变体

象限归属通过在 `AgentType::GrandAgent { task_scope }` 的 `task_scope` 字段追加尾缀 `#Q1..#Q4` 编码(如 `"refactor-parser#Q3"`),而非新增枚举变体:

- **不修改** `AgentType` 五变体(Root/Main/Sub/Grand/Expert)与 `AgentType::depth()` 语义。
- **不修改** `nexus_core::Task`(遵守 ADR-026 决策 3 领域类型稳定性)。
- 新增独立的 `Quadrant` 枚举与编解码函数(`encode_scope` / `from_task_scope`),象限元信息与 AgentType 解耦,保证向后兼容。

### 决策 3: 象限激活矩阵 — 按 TaskComplexity 激活,控制协调成本

四象限是"固定角色",但**是否激活**由 `TaskComplexity` 决定(回应推理悖论的"按需多智能体"):

| TaskComplexity | 激活象限 | 象限数 |
|----------------|---------|:------:|
| Simple | Q1 | 1 |
| Medium | Q1, Q3 | 2 |
| Complex | Q1, Q2, Q3 | 3 |
| VeryComplex | Q1, Q2, Q3, Q4 | 4 |

与现有 `sub_agent_count`(Simple→1/Medium→2/Complex→3/VeryComplex→5)语义对齐(复杂度越高激活越多);差异在于孙层扇出上界恒为 4(INV-3),`VeryComplex` 的第 5 个并行度由**象限内子任务**承载,而非新增第 5 种象限。

### 决策 4: WSJF 式动态优先级模型 — 复用 event-bus TaskPriority,不新建类型

调度以 `event_bus::TaskPriority`(Low/Medium/High/Critical)为一等公民,叠加加权最短作业优先(WSJF)评分:

```
Priority Score = (W1·业务价值 + W2·时间敏感度 + W3·风险消减 + W4·依赖解锁度) / 任务规模
```

- 各输入项 1~10 归一;`score_to_priority` 按阈值 T1/T2/T3 映射回 `TaskPriority`。
- 动态重排触发时机: 新任务入队 / 风险登记册更新 / 里程碑推进 / `AgentTaskFailed`(Critical) 到达。
- 抢占规则: 仅 Critical 可抢占正在执行的 Low(抢占前须落 checkpoint,以钩子建模,不引入破坏性依赖)。
- 饥饿保护: Low 任务等待超阈值后线性提权。
- `AgentTask` 新增 `priority: TaskPriority` 字段,`new()` 内默认 `Medium`(签名不变),新增 `with_priority()` builder(对齐现有 `with_parent`/`with_depth`)。已核验全代码库无 `AgentTask` 结构体字面量构造,故为非破坏性增量。

### 决策 5: 精英专家团队 E01-E08 显式编制 + 三级权限

将设计文档 §6 的 8 位精英专家(SubAgent 层)编码为 `experts` 注册表:

| 编号 | 角色 | 主责象限 |
|------|------|:--------:|
| E01 | 发布分析专家 | Q4 |
| E02 | 架构优化分析师 | Q2 |
| E03 | Rust 架构专家 | Q1 |
| E04 | 代码审查专家 | Q3 |
| E05 | TDD 守护者 | Q3 |
| E06 | 安全审计专家 | Q4 |
| E07 | 性能优化专家 | Q4 |
| E08 | DevOps/可观测专家 | Q4 |

每位专家含: 角色名、对应子代理类型、归口 MainAgent 域、主责象限、工具白名单(带三级权限)、准入条件、退出条件。三级权限模型(§11.2): **L0 只读 / L1 受限写 / L2 高危需审批**,遵循最小权限原则。

### 决策 6: 非破坏性 minor 交付 — v2.0.0-omega → v2.1.0-omega

本增量为完整 Stage B 的**结构子集**(四象限 + 优先级 + 专家团队),以 SemVer **minor** 交付:

- 仅新增类型/方法/字段(`Quadrant`/`PriorityScheduler`/`ExpertRegistry`/`AgentTask.priority`/2 个 MasError 变体),不改动任何现有公共签名与测试。
- `workspace.package.version`: `2.0.0-omega` → `2.1.0-omega`。
- **调和 ADR-026 决策 8**: ADR-026 原将完整 Stage B 标注为 `3.0.0-omega`。本 ADR 明确: 完整 Stage B(叠加记忆三级归档 / 知识协同 / 复杂度分块 / 稳定闭环 / PDCA 自动化)保留为未来里程碑;当且仅当出现破坏性变更时才升 major(3.0.0-omega)。本结构增量因严格向后兼容,归为 minor(2.1.0-omega),不违反 SemVer 渐进演进。

## 理由

### 决策 1 理由(四象限收敛)

- **推理悖论**(`AI_Agent_三重悖论_x_Chimera_深度映射分析.md`): 协调成本超过推理增益时多智能体不如单智能体。无界扇出 → 委托爆炸。四象限固定分工把孙层并行度锁定在 ≤4,协调成本可预测。
- **PVL 生产-验证闭环**(L7 `pvl-layer`): 四象限的 X 轴(Produce↔Assure)直接对应 PVL 的 Producer/Verifier 对立,理论根基来自代码库自有能力,非凭空构造。
- **职责可判定**: 象限 ↔ 六维质量 ↔ 三步验证存在稳定映射,杜绝"无人负责测试/安全"的职责漂移。

### 决策 2 理由(task_scope 编码)

- **§3.3.1 第 4 条(领域类型稳定性)**: `AgentType` 与 `nexus_core::Task` 为核心类型,变更需重 ADR 流程且有破坏性风险。尾缀编码零侵入,`Quadrant` 独立解耦。
- **向后兼容**: 现有 `pick_agent_type()` 语义与 `orchestrator_test.rs` 断言不受影响。

### 决策 3 理由(激活矩阵)

- **按需激活**: 回应推理悖论"禁止默认多智能体"红线,低复杂度任务不激活验证/加固象限,节省协调成本。
- **语义对齐**: 与现有 `sub_agent_count` 复杂度梯度一致,降低认知负担。

### 决策 4 理由(WSJF + 复用 TaskPriority)

- **§2.2 依赖铁律**: `TaskPriority` 已定义在 event-bus(L1),chimera-mas(L9)向下复用,不新建类型(避免重复实现)。
- **ADR-006 quest-priority-field**: 优先级作为调度一等公民已有先例,WSJF 是其量化增强。
- **非破坏**: `AgentTask::new()` 5 参数签名不变,priority 走默认值 + builder。

### 决策 5 理由(专家团队编制)

- **用户要求**: "组建 8 名 10+ 年经验精英专家级子代理专业化协作团队"。代码级注册表使专家编制可查询、可校验、可审计。
- **§11 最小权限 + 审计**: 三级权限模型落地"分级权限管理 + 最小权限原则"。

### 决策 6 理由(minor 交付)

- **§3.3.1 第 5 条(向后兼容)**: 新增类型/方法/字段属非破坏性变更,SemVer 规范归为 minor。
- **降低单次发布风险**: 结构增量独立可验收,不与 Part II 大能力耦合,契合渐进演进。

## 影响

### 新增内容

- **新增模块**: `crates/chimera-mas/src/quadrant.rs`(Quadrant + 激活矩阵 + QuadrantPlan)、`src/scheduler.rs`(WSJF + PriorityScheduler)、`src/experts.rs`(ExpertRegistry E01-E08 + PermissionTier)。
- **新增测试**: `tests/quadrant_test.rs`、`tests/scheduler_test.rs`、`tests/experts_test.rs`;扩展 `tests/proptest.rs`(INV-3/INV-4 + Critical>Low 排序不变量)。
- **新增 MasError 变体**: `QuadrantFanoutExceeded { requested, max }`、`QuadrantConflict { quadrant }`(变体数 25 → 27)。
- **新增 ADR**: 本文档(ADR-027)。
- **新增 spec**: `.trae/specs/add-chimera-mas-quadrant/`(spec + tasks + checklist)。

### 修改内容

- **`crates/chimera-mas/src/delegation.rs`**: `AgentTask` 新增 `priority` 字段 + `with_priority()` builder(非破坏)。
- **`crates/chimera-mas/src/orchestrator.rs`**: 新增象限感知孙层编排路径(保留 `delegate()` 现有语义)。
- **`crates/chimera-mas/src/lib.rs`**: 声明 3 个新模块 + 重导出 + prelude。
- **`Cargo.toml`**: `workspace.package.version` `2.0.0-omega` → `2.1.0-omega`。
- **`CHANGELOG.md`**: 新增 v2.1.0-omega 章节。
- **`docs/architecture/CODE_WIKI.md`**: chimera-mas 条目补 quadrant/scheduler/experts。
- **`docs/architecture/adr_index.md`**: 新增 ADR-027 条目(总数 27 → 28)。

### 资源影响评估

| 维度 | 评估 |
|------|------|
| crate 数量 | 35(不变,增量在 chimera-mas 内) |
| 依赖变更 | 无新增外部依赖(不引入 unsafe 依赖,ADR-026 决策 4 保持) |
| Docker/binary 体积 | 不受影响(纯 Rust 新增代码) |
| NexusEvent 变体数 | 不变(复用现有 7 Agent 变体) |
| MasError 变体数 | 25 → 27 |
| 测试覆盖 | 新增 3 个测试文件,目标覆盖率 ≥ 85% |
| 版本号 | 2.0.0-omega → 2.1.0-omega(minor) |

## 考虑的方案

### 方案 1: 新增 `AgentType::GrandAgent { quadrant }` 字段(否决)

- **内容**: 直接在 AgentType 枚举中加入象限字段。
- **否决理由**: 修改核心类型(§3.3.1 第 4 条),破坏 `orchestrator_test.rs`/`proptest.rs`/`agent_meta_test.rs` 对 GrandAgent 的现有构造与断言,属破坏性变更。task_scope 尾缀编码零侵入且等效。

### 方案 2: 象限扇出上界设为 5(对齐 sub_agent_count)(否决)

- **内容**: 象限数上界跟随 `sub_agent_count(VeryComplex)=5`。
- **否决理由**: 四象限是 2×2 矩阵,第 5 个象限无语义归属。设计文档 §3.4 明确 VeryComplex 第 5 并行度由象限内子任务承载。上界锁 4 保证 INV-3/INV-4 可判定。

### 方案 3: 直接升 major 到 3.0.0-omega(否决)

- **内容**: 按 ADR-026 决策 8 原标注,本增量归 Stage B 即升 3.0.0-omega。
- **否决理由**: 本增量严格向后兼容(仅新增),SemVer 规范应归 minor。过早升 major 丢失 2.x 演进历史,且完整 Stage B 尚未交付,3.0.0 语义应留给真正的破坏性/完整里程碑。

### 方案 4: 改写 `delegate()` 使其在 depth 3 象限感知(否决)

- **内容**: 直接在现有 `delegate()` 中注入象限逻辑。
- **否决理由**: `delegate()` 现有 depth 0/1/2 行为被 `orchestrator_test.rs` 断言覆盖,改写有回归风险。以新路径(`delegate_quadrants`)隔离象限编排,风险可控。

## 合规性

- **§2.1 分层映射**: 符合。增量在 chimera-mas(L9)内,不改分层。
- **§2.2 依赖铁律 + 唯一通道**: 符合。复用 event-bus 现有 7 变体,不新建总线;复用 L1 `TaskPriority`,不向上依赖。
- **§3.3.1 第 1 条(OMEGA 四定律守恒)**: 符合。不自实现压缩/进化/总线。
- **§3.3.1 第 4 条(领域类型稳定性)**: 符合。不改 `AgentType`/`nexus_core::Task`/`ThinkingMode`。
- **§3.3.1 第 5 条(向后兼容)**: 符合。仅新增,minor 版本。
- **§4.1 编码规范**: 符合。`#![forbid(unsafe_code)]` 保持;库层 thiserror;无生产路径 unwrap/expect;单函数 ≤200 行。
- **ADR-026 八项决策**: 全部保持(本 ADR 为其 Stage B 结构子集的落地补充,不推翻任何决策,仅调和决策 8 的版本标注语义)。

## 相关文档

- **spec**: `.trae/specs/add-chimera-mas-quadrant/spec.md`(本 ADR 配套)
- **设计文档**: `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md`(CHIMERA-MAS-Q v1.0,设计源)
- **分析源**: `AI_Agent_三重悖论_x_Chimera_深度映射分析.md`(推理/进化/记忆三悖论)
- **规则**: `.trae/rules/nuxus规则.md` §2.2/§3.3.1/§4.1/§4.4/§6.1/§6.2
- **关联 ADR**: ADR-026(MAS 子系统 Stage A,本 ADR 承接)/ADR-006(quest-priority-field)/ADR-019(TTG 思考模式)/ADR-012(forbid unsafe)

---

> **维护者**: NEXUS-OMEGA 团队
> **创建日期**: 2026-07-20
> **基线版本**: v2.0.0-omega(创建时)→ v2.1.0-omega(本增量落地后)
> **分析团队**: 8 专家视角分布式深度分析(发布分析 + 架构优化 + Rust 架构 + 代码审查 + TDD 守护 + 安全审计 + 性能优化 + DevOps 可观测)
