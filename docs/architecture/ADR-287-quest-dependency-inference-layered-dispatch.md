# ADR-287 -- Quest 依赖推断策略化 + 波次执行：`DependencyInference` 接口与 `dependency_mode` 开关

- 状态：Accepted（2026-11-12）
- 决策者：R7 批次（WP-E，方案 C 策略接口+开关，用户裁决）
- 关联：ADR-247 D3（`run --execute` 批量派发首个生产调用方，载荷桩边界）/ 工单集 §0.3 B 节（计划层并行度现状：分解硬编码线性链为调度层最大结构锁死点）/ ADR-281（计划层混层写屏障分段——波内并行的安全模型先例）
- 影响面：`crates/quest-engine/src/{decompose.rs(新),dag.rs,engine.rs,config.rs,lib.rs}`、`crates/chimera-cli/src/{composition.rs,commands/run.rs}`
- 行为影响：默认 `DependencyMode::Linear` + 单批派发 = **与历史行为逐字节等价**（零行为变化）；`HeuristicDag` 开启后分解器产出真实 DAG，执行面按拓扑波次（波内并行、波间串行）。

## 1. 决策

### D1 — 分解器策略接口

`decompose` 的硬编码线性链改为 `DependencyInference` trait（quest-engine 内部域概念，不上提 L0——依赖铁律）：

- `LinearChain`（默认）：task-i → task-{i-1}，与旧硬编码逐字节等价（`linear_matches_legacy` 测试 + proptest 双锁定）。
- `HeuristicDag`：保守启发式——句首显式序数（"先/首先/第一步"）与承接词（"然后/再/接着"）产边、**序数顺承**（序数句的直接后继自身未命中规则时依赖前一句）、独立句无依赖；指代推断暂不实现（误判率高，登记为未来扩展）。
- 方向性安全：依赖只指向更早句子（结构上不可成环）；**宁可漏边不可错边**——漏边只损失并行度，错边是并行了有依赖任务的正确性事故。`heuristic_always_dag` proptest 守护产出恒过 `validate_dag`。

### D2 — 执行面：同一开关驱动，单一真相源

`run --execute` 按同一 `DependencyMode` 分派：

- `Linear`：恒走旧路径单批派发（逐字节等价——避免按形状检测引入"N 波固定开销"回退）。
- `HeuristicDag`：`dag::topological_layers` 分层 → 单层退化为单批；多波时波间 await 串行（依赖序）、波内既有 8 路有界并行，结果按**输入序槽位回填**（`batch_outcome_to_cli_error` 的"首个失败按输入序归因"契约不破坏），整批共享 120s 进程级预算。
- 不开独立 bool 开关：chimera-cli 已依赖 quest-engine（L10→L9 合法向下），跨 crate 枚举直接复用，消除"分解产 DAG 边但执行不消费"的错配状态。

### D3 — 配置面与组合根

- `QuestConfig::dependency_mode`（`#[serde(default)]` = Linear，存量配置零兼容问题）。
- 组合根经 env 三态开关 `CHIMERA_QUEST_DEPENDENCY_MODE`（`heuristic-dag` 开启）接入——WHY 走 env 而非 ChimeraConfig：后者受 G-69 十五字段对拍门锁定，实验性开关不值得扩 config schema（与 `CHIMERA_SCHED_CLAIM` 同惯例）。

### D4 — `topological_layers` 与既有 Kahn 单实现

新增分层出口复用 `kahn_core` 提取的 `build_kahn_state`（悬空/判环语义与 `topological_order` 零漂移）；既有三个公开 dag 函数行为零变化（proptest 守护保持绿）。

### D5 — 默认方向裁决：维持 `Linear`（2026-11-12 用户裁决，翻转前置条件在此登记）

"默认是否应为 HeuristicDag"经专项评估后裁决**维持 Linear**。核心论据（防止未来重复讨论）：

1. **叙事文本性能回退是决定性的**：`topological_layers` 对线性链输出 N 层 × 1 任务，HeuristicDag 模式下叙事型 quest（"先A。然后B。C。"）从单批无差别并行退化为 N 波串行（每波 1 次 `execute_batch` + 波间 await），墙钟回退 ≈ 波数倍；而任务描述大多数天然有序。清单式文本（独立句为主）才是真正的受益形态。
2. **启发式未经真实语料校准**：序数/承接/顺承规则的假阴/假阳率未知（ADR-281 V5 先例：混层阈值无语料时三次读数波动 4.50/4.59/5.82，登记"有语料后须重新校准"）。
3. **行为变更面**：`dependencies` 是 Quest 结构的一部分，分解结果变化波及全部下游消费面（拓扑校验/检查点恢复/subagent/同层字典序显示）；ADR-268（`--plan-strict` 默认 Off）为同类 opt-in 先例。

**翻转前置条件（全部满足方可重议默认开）**：
- ① 真实 quest 语料出现并完成启发式规则校准（假阴/假阳率可测量）；
- ② 执行面补**全线性快速路径**（层数==任务数时退回单批，消除叙事回退）或演进为**读写分类执行模型**（任务按载荷读写性分类：只读全并行/可能写保守串行，沿 toolplan_runner declared/undeclared 双轨先例——后者是 ADR-247 D3 载荷接真实工具时的自然工作项）；
- ③ 三形态基准对比（叙事/清单/混合）证明默认开下无场景回退。

## 2. 已知语义细节（非缺陷）

`sort_tasks_topologically` 的同层字典序在 HeuristicDag 下使 10+ 任务同层独立句按 `task-N` 字典序（非数值序）显示重排；依赖先行性不变，下游结果回填由输入序槽位机制保证。

## 3. 验证

- quest-engine **411 passed / 0 failed**（新增 9 测试：5 策略单测 + 2 proptest + engine/config 回归）；chimera-cli **455 passed / 0 failed**；workspace check 绿；clippy `-D warnings` 零告警。
- 两开关语义矩阵见 WP-E 实施报告（`Linear` 默认零变化 / `HeuristicDag` 波次 / 悬空·成环 fail-fast）。

## 4. 回滚

单 commit revert 整体回退；`dependency_mode` 缺省 Linear 配置面无迁移；`topological_layers` / `dispatch_in_waves` 无其他消费方，摘除不影响既有三个 dag 公开 API。
