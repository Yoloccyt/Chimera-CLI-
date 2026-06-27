# Week 5 — 议会 + 安全 + 预算(L8 + L4 + L3)Spec

## Why

Week 1(L0-L1 基础设施)、Week 2(Quest+Wiki+Router)、Week 3(记忆与路由系统)、Week 4(执行优化层)已全量验收通过,并完成横向深度复审:21 个已实现 crate、~33,000 行源码、1,599 个测试,总体评级 A-(4.2/5),无 Critical 问题,满足 Week 5 启动全部前置条件。但项目仍缺乏 NEXUS-OMEGA 的**认知治理与安全防线**——**对抗性议会审议、Skeptic 否决权、ASA 对抗审计、AHIRT 反黑客红队、DECB 双档预算、TTG 思考切换**。

Week 5 是从"执行效率"走向"认知治理与安全免疫"的关键跃迁:任务能否经多角色对抗审议达成共识、Skeptic 能否否决恶意意图、ASA 能否实时介入纠偏、AHIRT 能否主动探测漏洞、DECB 能否连续调节认知预算、TTG 能否按复杂度自动切换思考深度——这六大能力构成 OMEGA 四定律中 Ω-Evolve(对抗进化)与 Ω-Sparse(预算稀疏化)的工程实现,直接对应 Claude Code 尸检教训中"CVE-2026-35022 安全裸奔"与"44 个未发布标志方向混乱"的免疫策略,同时落实 ADR-002(能力衰减模型)、ADR-018(TTG)、ADR-024(AHIRT)三项架构决策。

本 spec 将 §7 Week 5 推进计划(Day 29-35)细化为可执行、可监控、可验收的任务契约,严格对齐十层架构依赖铁律、四次尸检教训与 Week 1-4 横向复审经验(含 Major-2 qeep-protocol 测试补充、Minor-2 事件订阅者补全)。

## What Changes

* **parliament(L8 Parliament)**:从骨架升级为对抗性议会,实现 5 角色议会(Architect/Skeptic/Optimizer/Librarian/Bard)+ Red Team(AHIRT),支持提案→辩论→投票→共识/否决全流程,发布 `ConsensusReached`/`SkepticVeto`/`RedTeamAudit` 事件

* **seccore(L4 Security)**:扩展 ASA(Adversarial Self-Audit)对抗性自我审计模块,基于 Critic PPO 思想实现实时介入纠偏,与现有零信任沙箱协同形成"事前拦截 + 事中审计"双层防御

* **parliament(L8)AHIRT 模块**:实现 Anti-Hack Intelligent Red Team 反黑客内部红队,主动探测提示注入/命令注入/权限提升/沙箱逃逸四类攻击,漏洞探测率 > 95%

* **decb-governor(L3 Budget)**:从骨架升级为双档认知预算治理器,实现连续可调 [0,1] 预算系数、高低档自动切换、溢出检测与降级,发布 `BudgetExceeded` 事件

* **quest-engine(L9 Quest)TTG 模块**:扩展 TTG(Thinking Toggle Governance)思考切换治理,基于 Quest 复杂度与预算自动选择 Fast/Standard/Deep 三级模式,发布 `ThinkingModeChanged` 事件

* **qeep-protocol(L4 Security)测试加固**:补充测试至 ≥20 个(修复 Week 1-4 横向复审 Major-2 问题),覆盖超时/孤儿检测/纠缠态/错误传播全路径

* **BREAKING**:无(Week 1-4 已稳定的 crate API 保持向后兼容,仅新增 L8/L4/L3 crate 的实现与 seccore/quest-engine 的扩展模块)

* 不修改 `nexus-core`/`event-bus`/`model-router`/`decay-engine`/`chimera-cli`/`mlc-engine`/`hcw-window`/`cmt-tiering`/`osa-coordinator`/`kvbsr-router`/`gea-activator`/`gqep-executor`/`pvl-layer`/`mtpe-executor`/`scc-cache`/`faae-router`/`repo-wiki` 的公开 API(仅在必要时新增订阅者)

* 不引入新的 workspace 依赖(workspace 已收录 `rusqlite`/`ndarray`/`dashmap`/`uuid`/`chrono`/`sha2`/`rmp-serde`/`tempfile`/`criterion`/`proptest`/`tracing-test`)

## Impact

* Affected specs:

  * `phase1-architecture-analysis-and-planning`(Week 1 已完成)
  * `week2-quest-wiki-router-implementation`(Week 2 已完成)
  * `week3-memory-routing-system`(Week 3 已完成)
  * `week4-execution-optimization`(Week 4 已完成,本 spec 是其直接后续)
  * `week1-4-cross-review`(Week 1-4 横向复审已完成,本 spec 继承其全部经验教训,特别是 Major-2 qeep-protocol 测试补充与 Minor-2 事件订阅者补全)
  * `establish-elite-collaboration-team`(6 类专家子代理角色定义来源)
  * `init-crates-workspace`(34 crate 骨架已就绪,本 spec 在其上浇筑实现)

* Affected code:

  * `crates/parliament/src/` — 新增 `types.rs`/`roles.rs`/`debate.rs`/`voting.rs`/`veto.rs`/`ahirt.rs`/`error.rs`/`config.rs` + `tests/` + `benches/`
  * `crates/decb-governor/src/` — 新增 `types.rs`/`governor.rs`/`overflow.rs`/`error.rs`/`config.rs` + `tests/` + `benches/`
  * `crates/seccore/src/asa.rs` — 新增 ASA 对抗审计模块(扩展现有 seccore,不破坏已有 API)
  * `crates/quest-engine/src/ttg.rs` — 新增 TTG 思考切换治理模块(扩展现有 quest-engine)
  * `crates/qeep-protocol/tests/` — 补充测试至 ≥20 个(修复 Major-2)
  * `crates/event-bus/src/types.rs` — 新增 Week 5 所需事件类型(`DebateStarted`/`SkepticVeto`/`RedTeamAudit`/`ThinkingModeChanged`/`BudgetAdjusted`/`AsaIntervention`/`AhirtProbeCompleted`)
  * 各 crate 的 `Cargo.toml` — 新增对 `nexus-core`/`event-bus`/`qeep-protocol`/`seccore`/`decay-engine` 的 workspace 级依赖

* 不受影响的代码:Week 1-4 已实现的 21 个 crate 的现有源码(仅可能新增测试用例与事件订阅者)

## ADDED Requirements

### Requirement: Parliament 5 角色对抗性议会

系统 SHALL 在 `parliament` crate 中实现 5 角色对抗性议会(Architect/Skeptic/Optimizer/Librarian/Bard),通过提案→辩论→投票→共识/否决全流程治理 Quest 决策,对应 Hermes Council 基因与 OMEGA Ω-Evolve 定律。

#### Scenario: 5 角色定义与职责

* **WHEN** Parliament 初始化

* **THEN** 注册 5 个角色:`Architect`(架构决策,Opus/DeepSeek-R1)、`Skeptic`(安全审计,冻结权,Sonnet/GPT-4o)、`Optimizer`(性能优化,Haiku/Gemini-Flash)、`Librarian`(记忆检索,Embedding)、`Bard`(用户沟通,Sonnet)

* **THEN** 每个角色持有 `RoleProfile`(含 `role_id`/`specialty`/`model_preference`/`voting_weight`/`can_veto`)

* **THEN** 仅 `Skeptic` 持有 `can_veto = true`(冻结权),其余角色 `can_veto = false`

* **THEN** 角色注册后发布 `RoleRegistered` 事件(同层通信,不跨层)

#### Scenario: 提案→辩论→投票全流程

* **WHEN** Quest 经 TTG 切换思考模式后提交 Parliament 审议

* **THEN** 调用 `Parliament::deliberate(&self, quest: &Quest, proposal: &Proposal) -> Result<Consensus, ParliamentError>`

* **THEN** 流程:发起提案(`DebateStarted` 事件)→ 5 角色并行辩论(各角色生成 `Opinion` 含 `position`/`confidence`/`rationale`)→ 加权投票(`VoteCast` 事件)→ 共识判定

* **THEN** 共识判定规则:赞成率 ≥ 0.6 且无 Skeptic 否决 → `Consensus::Reached`;赞成率 < 0.6 → `Consensus::Rejected`;Skeptic 否决 → `Consensus::Vetoed`

* **THEN** 共识达成后发布 `ConsensusReached` 事件 `[Critical]`(携带 `quest_id`/`decision_hash`/`dpo_pair_id`),供 GSOE/AutoDPO 订阅(修正 V3/V4 向上依赖违规)

* **THEN** 辩论延迟基准:5 角色并行辩论 < 200ms(占位实现,Week 6 NMC 后接入真实模型)

#### Scenario: 加权投票与角色权重

* **WHEN** 投票阶段触发

* **THEN** 每个角色的投票权重:`Architect`=0.25、`Skeptic`=0.30(安全权重最高)、`Optimizer`=0.20、`Librarian`=0.15、`Bard`=0.10

* **THEN** 加权赞成率 = Σ(角色权重 × 角色立场),立场 ∈ {0.0(反对), 0.5(弃权), 1.0(赞成)}

* **THEN** 弃权不计入赞成率分母,但计入参与率(避免低参与度通过)

* **THEN** 参与率 < 0.6 时强制 `Consensus::Rejected`(法定人数不足)

#### Scenario: 并行辩论无竞态

* **WHEN** 5 角色并行生成 Opinion

* **THEN** 使用 `FuturesUnordered` 并发收集 5 个角色的 Opinion(继承 Week 4 GQEP 经验)

* **THEN** Opinion 生成经 GQEP 聚集(避免孤儿调用),超时 5 秒

* **THEN** 并发测试:100 次并行辩论,无 panic、无数据丢失、无死锁

### Requirement: Skeptic 否决权与 Auto-DPO 触发

系统 SHALL 在 `parliament` crate 中实现 Skeptic 否决权(冻结权)与 Auto-DPO 触发,对应 Claude 尸检"安全裸奔"教训与 Hermes Learning-first 基因。

#### Scenario: Skeptic 否决恶意意图

* **WHEN** Skeptic 检测到提案含恶意意图(命令注入/权限提升/数据泄露/沙箱逃逸)

* **THEN** Skeptic 行使冻结权,立即终止辩论,返回 `Consensus::Vetoed`

* **THEN** 发布 `SkepticVeto` 事件 `[Critical]`(携带 `quest_id`/`veto_reason`/`frozen_capabilities: Vec<CapabilityId>`)

* **THEN** 发布 `CapabilityFrozen` 事件(供 Decay Engine 订阅,衰减对应能力)

* **THEN** 否决延迟基准:恶意意图检测 < 10ms(基于规则匹配,Week 6 NMC 后接入语义检测)

#### Scenario: 恶意意图规则库

* **WHEN** Skeptic 初始化

* **THEN** 加载恶意意图规则库:`CommandInjection`/`PrivilegeEscalation`/`DataExfiltration`/`SandboxEscape`/`PromptInjection` 五类

* **THEN** 每类规则含 `pattern`(正则或关键字)、`severity`(Critical/High/Medium)、`action`(Veto/Warn)

* **THEN** 规则库可通过 `omega.yaml` 配置扩展(不引入新依赖,使用 `regex` 若 workspace 已收录,否则用字符串匹配)

* **THEN** 规则匹配测试:覆盖 5 类各 5 个用例,共 25 个测试用例

#### Scenario: Auto-DPO 训练对生成

* **WHEN** 共识达成且辩论过程产生有价值的好/坏决策对比

* **THEN** Parliament 生成 DPO 训练对:`DpoPair`(含 `chosen`/`rejected`/`context`/`pair_id`)

* **THEN** `chosen` = 共采纳的 Opinion,`rejected` = 被否决的 Opinion

* **THEN** DPO 训练对经 `ConsensusReached` 事件的 `dpo_pair_id` 字段传递(不直接调用 AutoDPO,避免向上依赖)

* **THEN** DPO 训练对生成基准:每次共识达成 ≥ 1 个 DPO 对(若存在对比)

### Requirement: ASA 对抗性自我审计

系统 SHALL 在 `seccore` crate 中扩展 ASA(Adversarial Self-Audit)模块,基于 Critic PPO 思想实现实时介入纠偏,与现有零信任沙箱协同形成"事前拦截 + 事中审计"双层防御。

#### Scenario: ASA 实时介入审计

* **WHEN** PVL 生产验证阶段产出操作序列

* **THEN** ASA 对每个操作执行对抗性审计:`AsaAuditor::audit(&self, operation: &Operation) -> AuditResult`

* **THEN** 审计维度:`safety_score`(安全评分)、`correctness_score`(正确性评分)、`efficiency_score`(效率评分)、`intervention`(干预动作:Allow/Warn/Block)

* **THEN** `intervention = Block` 时,发布 `AsaIntervention` 事件 `[Critical]`(携带 `operation_id`/`block_reason`/`alternative_suggestion`)

* **THEN** ASA 审计延迟基准:< 5ms/操作(基于规则评分,Week 6 NMC 后接入 Critic 模型)

#### Scenario: Critic PPO 评分模型(占位)

* **WHEN** ASA 对操作评分

* **THEN** Week 5 占位实现:基于规则的评分模型(命令白名单 + 风险关键字 + 历史失败率)

* **THEN** 评分公式:`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`

* **THEN** `correctness_score` 占位:基于语法检查(复用 PVL Verifier 的语法检查逻辑)

* **THEN** `efficiency_score` 占位:基于操作复杂度(简单操作高分,复杂操作低分)

* **THEN** Week 6 NMC 后替换为 Critic PPO 模型(标记 TODO(Week 6))

#### Scenario: 干预动作分级

* **WHEN** ASA 评分计算完成

* **THEN** 干预动作分级:`safety_score ≥ 0.8` → Allow;`0.5 ≤ safety_score < 0.8` → Warn;`safety_score < 0.5` → Block

* **THEN** Warn 级别发布 `AsaIntervention` 事件(Normal 严重度),操作继续执行但记录告警

* **THEN** Block 级别发布 `AsaIntervention` 事件 `[Critical]`,操作被阻止,返回 `SecCoreError::AsaBlocked`

* **THEN** 干预动作测试:覆盖 Allow/Warn/Block 三级各 5 个用例

#### Scenario: ASA 与 SecCore 沙箱协同

* **WHEN** 操作经 ASA 审计后执行

* **THEN** 协同流程:ASA 事中审计(Allow/Warn/Block)→ SecCore 沙箱执行(零信任拦截)→ AuditChain 审计记录

* **THEN** ASA Block 的操作不进入沙箱(事中拦截优先于沙箱执行)

* **THEN** 沙箱违规时发布 `SandboxViolation` 事件,ASA 据此更新历史失败率(反馈闭环)

### Requirement: AHIRT 反黑客内部红队

系统 SHALL 在 `parliament` crate 中实现 AHIRT(Anti-Hack Intelligent Red Team)反黑客内部红队,作为 Parliament 的第 6 角色(Red Team),主动探测四类攻击,漏洞探测率 > 95%。

#### Scenario: AHIRT 主动探测四类攻击

* **WHEN** AHIRT 周期性触发(默认每 5 分钟)或事件触发(新工具注册/新 Quest 创建)

* **THEN** AHIRT 对系统执行四类主动探测:
  - `PromptInjection`:提示注入探测(构造恶意 prompt,检测是否被拦截)
  - `CommandInjection`:命令注入探测(构造 `$(...)`/`|`/`;`/`&&` 载荷,检测沙箱拦截)
  - `PrivilegeEscalation`:权限提升探测(构造 `sudo`/`su` 载荷,检测能力衰减)
  - `SandboxEscape`:沙箱逃逸探测(构造路径穿越/进程注入载荷,检测沙箱隔离)

* **THEN** 每类探测含 `probe_payload`/`expected_result`/`actual_result`/`passed`

* **THEN** 探测完成后发布 `AhirtProbeCompleted` 事件(携带 `probe_type`/`total`/`passed`/`failed`/`detection_rate`)

* **THEN** 探测延迟基准:四类探测全量完成 < 500ms(占位实现)

#### Scenario: 漏洞探测率验证

* **WHEN** AHIRT 探测完成

* **THEN** 漏洞探测率 = (被系统拦截的探测数 / 总探测数) × 100%

* **THEN** 探测率基准:> 95%(对应 Week 5 验收标准)

* **THEN** 探测率 < 95% 时,发布 `RedTeamAudit` 事件 `[Critical]`(携带 `vulnerability_type`/`failed_probes`/`remediation_suggestion`)

* **THEN** 探测率测试:构造 100 个已知攻击载荷,验证拦截率 > 95%

#### Scenario: AHIRT 与 SecCore/Decay 协同

* **WHEN** AHIRT 发现漏洞(探测率 < 95% 或单类探测失败)

* **THEN** AHIRT 发布 `RedTeamAudit` 事件,SecCore 订阅并强化对应拦截规则

* **THEN** AHIRT 发布 `RedTeamAudit` 事件,Decay Engine 订阅并衰减对应能力(对应 ADR-002 能力衰减模型)

* **THEN** AHIRT 不直接调用 SecCore/Decay Engine(跨层向上依赖禁止,通过事件解耦)

* **THEN** 协同测试:AHIRT 触发漏洞 → SecCore 规则强化 → Decay 能力衰减,全链路验证

### Requirement: DECB 双档认知预算治理

系统 SHALL 在 `decb-governor` crate 中实现 DECB(Dual-tier Cognitive Budget)双档认知预算治理,连续可调 [0,1] 预算系数,高低档自动切换,溢出检测与降级。

#### Scenario: 连续可调预算系数

* **WHEN** Quest 提交执行请求

* **THEN** DECB 计算 `budget_coefficient ∈ [0.0, 1.0]`:`DecbGovernor::compute_budget(&self, quest: &Quest, context: &Context) -> f32`

* **THEN** 预算系数公式:`coefficient = base_budget × complexity_factor × urgency_factor × remaining_budget_ratio`

* **THEN** `base_budget` 默认 0.8,可通过 `omega.yaml` 配置

* **THEN** `complexity_factor ∈ [0.5, 1.5]`:简单任务 0.5,复杂任务 1.5(基于 Quest 任务数与依赖深度)

* **THEN** `urgency_factor ∈ [0.8, 1.2]`:低紧急度 0.8,高紧急度 1.2(基于 Quest deadline)

* **THEN** `remaining_budget_ratio ∈ [0.0, 1.0]`:剩余预算 / 总预算

* **THEN** 预算系数计算延迟基准:< 1ms

#### Scenario: 高低档自动切换

* **WHEN** 预算系数计算完成

* **THEN** 档位判定:`coefficient ≥ 0.6` → HighTier(深度模式,高成本模型);`0.3 ≤ coefficient < 0.6` → LowTier(标准模式,中等成本模型);`coefficient < 0.3` → Degraded(降级模式,低成本模型或拒绝)

* **THEN** 档位切换发布 `BudgetAdjusted` 事件(携带 `old_tier`/`new_tier`/`coefficient`/`reason`)

* **THEN** 档位切换延迟基准:< 1ms

* **THEN** 档位切换测试:覆盖 High→Low、Low→High、High→Degraded、Low→Degraded 四种切换路径

#### Scenario: 预算溢出检测与降级

* **WHEN** 当前消耗超过预算上限

* **THEN** 触发溢出检测,发布 `BudgetExceeded` 事件 `[Critical]`(携带 `budget_type`/`current`/`limit`)

* **THEN** 自动降级:HighTier → LowTier → Degraded,直至消耗 < 预算

* **THEN** Degraded 模式下仍超预算时,拒绝新 Quest 并发布 `BudgetExceeded` 事件

* **THEN** 溢出检测周期:每 10 秒检查一次(后台异步任务)

* **THEN** 溢出测试:模拟消耗超限,验证降级链路与事件发布

#### Scenario: 预算消耗统计

* **WHEN** Quest 执行消耗认知资源(模型调用/工具调用/上下文加载)

* **THEN** DECB 记录消耗:`consumption = token_count × cost_per_token + tool_call_count × cost_per_call`

* **THEN** 每 100 次 Quest 执行发布 `BudgetStatsReported` 事件(携带 `total_consumption`/`remaining_budget`/`utilization_rate`)

* **THEN** 消耗统计测试:覆盖单 Quest 消耗、多 Quest 累计、预算重置

### Requirement: TTG 思考切换治理

系统 SHALL 在 `quest-engine` crate 中扩展 TTG(Thinking Toggle Governance)思考切换治理,基于 Quest 复杂度与预算自动选择 Fast/Standard/Deep 三级模式,对应 ADR-018 与 Minimax/GLM Thinking Toggle 基因。

#### Scenario: 自动模式选择

* **WHEN** Quest 创建或预算档位变化

* **THEN** TTG 自动选择思考模式:`TtgGovernor::select_mode(&self, quest: &Quest, budget_tier: BudgetTier) -> ThinkingMode`

* **THEN** 选择规则:
  - `budget_tier == Degraded` → `Fast`(降级模式强制快速)
  - `quest.tasks.len() <= 3 && budget_tier != HighTier` → `Fast`
  - `quest.tasks.len() <= 10 || budget_tier == LowTier` → `Standard`
  - `quest.tasks.len() > 10 || budget_tier == HighTier` → `Deep`

* **THEN** 模式切换发布 `ThinkingModeChanged` 事件(携带 `quest_id`/`old_mode`/`new_mode`/`reason`)

* **THEN** 模式选择延迟基准:< 1ms

#### Scenario: 复杂度评估

* **WHEN** TTG 评估 Quest 复杂度

* **THEN** 复杂度评分:`complexity_score = task_count × 0.3 + dependency_depth × 0.4 + description_length_factor × 0.3`

* **THEN** `dependency_depth`:DAG 最长路径深度(复用 quest-engine 现有 DAG 逻辑)

* **THEN** `description_length_factor ∈ [0.0, 1.0]`:描述长度归一化(长度 / 1000,clamp 到 [0,1])

* **THEN** 复杂度评分测试:覆盖简单(1 task)/中等(5 task)/复杂(20 task)三级

#### Scenario: 预算联动切换

* **WHEN** DECB 档位变化(发布 `BudgetAdjusted` 事件)

* **THEN** TTG 订阅 `BudgetAdjusted` 事件,自动重新选择思考模式

* **THEN** 联动规则:HighTier → 倾向 Deep;LowTier → 倾向 Standard;Degraded → 强制 Fast

* **THEN** 联动切换不阻塞主流程(事件订阅异步处理)

* **THEN** 联动测试:模拟档位变化,验证模式自动切换

#### Scenario: 手动覆盖与回退

* **WHEN** 用户或 Parliament 显式指定思考模式

* **THEN** TTG 支持手动覆盖:`TtgGovernor::override_mode(&self, quest_id: &str, mode: ThinkingMode)`

* **THEN** 手动覆盖优先级高于自动选择,但受预算档位约束(Degraded 档位不允许覆盖为 Deep)

* **THEN** 手动覆盖发布 `ThinkingModeChanged` 事件(携带 `reason = "manual_override"`)

* **THEN** 覆盖测试:手动设置 Deep/Degraded 档位下尝试覆盖(应拒绝)

### Requirement: qeep-protocol 测试加固(修复 Major-2)

系统 SHALL 补充 `qeep-protocol` crate 测试至 ≥20 个,覆盖超时/孤儿检测/纠缠态/错误传播全路径,修复 Week 1-4 横向复审 Major-2 问题。

#### Scenario: 测试覆盖补全

* **WHEN** Week 5 开发阶段

* **THEN** 补充 qeep-protocol 测试用例至 ≥20 个(当前 8 个,新增 ≥12 个)

* **THEN** 新增测试覆盖:
  - 超时场景(各种 Duration:1ms/100ms/1s/10s)
  - 孤儿检测(所有 Sender drop / 部分 Sender drop)
  - 并发纠缠态管理(10+ 线程并发 EntangledCall)
  - 错误传播链(超时 → 错误 → 上层捕获)
  - 边界条件(空 Future 列表、单 Future、最大 Future 数)

* **THEN** 测试通过率 100%,无 flaky 测试

### Requirement: Week 5 验收门禁

系统 SHALL 在 Day 35 通过端到端认知治理与安全验收,验证 Week 5 全部交付物协同工作。

#### Scenario: 端到端认知治理流程

* **WHEN** 执行端到端测试:Quest 创建 → TTG 模式选择 → DECB 预算计算 → Parliament 5 角色辩论 → Skeptic 安全审计 → ASA 实时介入 → AHIRT 主动探测 → 共识达成/否决

* **THEN** 全流程无 panic、无孤儿调用、无事件丢失

* **THEN** TTG 模式选择 < 1ms

* **THEN** DECB 预算计算 < 1ms

* **THEN** Parliament 5 角色辩论 < 200ms

* **THEN** Skeptic 恶意意图检测 < 10ms

* **THEN** ASA 审计 < 5ms/操作

* **THEN** AHIRT 探测率 > 95%

* **THEN** 决策准确率 > 90%(正确否决恶意意图 + 正确通过良性提案)

#### Scenario: CSA(Combined System Action)延迟验证

* **WHEN** 验证全认知链路 CSA 延迟

* **THEN** CSA < 300ms(从 Quest 提交到共识达成的端到端延迟)

* **THEN** CSA 延迟分解:TTG 1ms + DECB 1ms + Parliament 200ms + Skeptic 10ms + ASA 5ms + AHIRT 50ms + 事件传播 30ms ≈ 297ms

* **THEN** CSA 延迟测试采用 min-of-N(5 次)减少调度噪声

#### Scenario: 安全免疫验证

* **WHEN** 执行安全免疫测试(模拟 Claude CVE-2026-35022 攻击向量)

* **THEN** 命令注入攻击 100% 被 SecCore 拦截(事前)或 ASA Block(事中)

* **THEN** 提示注入攻击 > 95% 被 Skeptic 否决或 AHIRT 探测

* **THEN** 权限提升攻击 100% 被 Decay Engine 能力衰减

* **THEN** 沙箱逃逸攻击 100% 被 SecCore 沙箱拦截

* **THEN** 安全免疫测试覆盖 100 个攻击载荷,拦截率 > 98%

#### Scenario: 全量测试与构建

* **WHEN** 运行 Week 5 验收命令

* **THEN** `cargo check --workspace --jobs 1` 通过

* **THEN** `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告

* **THEN** `cargo test --workspace --jobs 1` 全通过(Week 1-5 测试用例,含 qeep-protocol 加固后的 ≥20 测试)

* **THEN** `cargo build --workspace --release --jobs 1` 通过

* **THEN** 新实现 crate 的测试覆盖率 > 85%(关键路径全覆盖)

* **THEN** 孤儿调用率 < 0.1%(GQEP 检测器验证)

## MODIFIED Requirements

### Requirement: 代码质量标准(继承自 Week 4,Week 5 强化)

所有 Week 5 新增代码 SHALL 满足(继承 Week 1-4 全部要求,Week 5 强化以下):

* **单一职责**:每个函数 ≤ 200 行(对应 §6 架构红线),模块边界清晰

* **workspace 一致性**:使用 `workspace.dependencies` 共享依赖,禁止独立版本声明

* **错误处理显式**:库层用 `thiserror` enum,应用层用 `anyhow::Result`,禁止 `unwrap`/`expect` 在非测试代码

* **async 约束**:所有 async fn 满足 `Send + 'static + 'async`,经 QEEP 包装避免孤儿调用;所有 SQLite 操作必须 `spawn_blocking`

* **注释解释意图**:仅在 WHY 不明显处加注释(隐藏约束、变通方案、反直觉行为)

* **TDD-first**:核心领域类型先写类型定义与基础测试,再写业务逻辑

* **`#![forbid(unsafe_code)]`**:所有 crate 的 lib.rs 顶部保留

* **`#![warn(missing_docs, clippy::all)]`**:所有 crate 的 lib.rs 顶部保留

* **锁策略选型**:读多写少场景用 `RwLock`,读写均衡场景用 `Mutex`

* **Top-K 排序**:Top-K 选择用 `select_nth_unstable`,全排序用 `sort_by`

* **并发正确性**:所有 check-then-act 模式必须原子化(DashMap entry API 或锁内完成)

* **newtype 类型安全**:所有 ID 类型必须为 newtype struct(使用 `nexus_core::id_newtype!` 宏)

* **Arc 共享**:大字段(CLV、content)跨结构存储时必须用 `Arc<T>` 共享

* **Week 5 强化:对抗治理闭环**:Parliament 决策必须经 5 角色辩论 + Skeptic 安全审计,禁止单角色独裁

* **Week 5 强化:安全事件优先级**:Skeptic 否决、ASA Block、AHIRT 漏洞三类事件标注 `[Critical]`,EventBus 保证投递(使用 mpsc 通道而非 broadcast)

* **Week 5 强化:预算驱动降级**:所有认知操作必须受 DECB 预算约束,超预算自动降级而非硬失败

* **Week 5 强化:思考模式与预算联动**:TTG 模式选择必须订阅 DECB 档位变化,实现联动切换

## REMOVED Requirements

无

---

## 附录 A:Week 5 关键设计决策(预填,实现阶段验证)

### A.1 Parliament 角色权重选择

* **Skeptic 权重 0.30(最高)**:安全审计优先,对应 Claude CVE-2026-35022 教训

* **Architect 权重 0.25**:架构决策次之,影响系统长期健康

* **Optimizer 权重 0.20**:性能优化第三,影响执行效率

* **Librarian 权重 0.15**:记忆检索第四,提供历史参考

* **Bard 权重 0.10**:用户沟通最低,但参与率仍计入法定人数

* **WHY Skeptic 权重最高**:Claude 尸检显示安全裸奔是最大病灶,提高 Skeptic 权重确保安全审计一票否决的有效性

### A.2 共识判定阈值选择

* **赞成率阈值 0.6**:平衡效率与审慎,过低导致草率通过,过高导致议而不决

* **参与率阈值 0.6**:避免低参与度通过(法定人数)

* **Skeptic 一票否决**:安全审计绝对优先,不参与加权投票

* **WHY 不用 0.5 简单多数**:0.5 易导致摇摆,0.6 提供稳定共识

### A.3 ASA 评分模型选择

* **Week 5 占位实现(基于规则)**:不引入 Critic PPO 真实模型(依赖 Week 6 NMC + 训练数据)

* **规则评分公式**:`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`

* **WHY 不引入真实 Critic 模型**:Week 5 验证治理架构,真实模型依赖 Week 6 NMC,避免过早工程化

### A.4 AHIRT 探测策略

* **主动探测(非被动监听)**:AHIRT 主动构造攻击载荷,验证系统拦截能力

* **四类攻击覆盖**:PromptInjection/CommandInjection/PrivilegeEscalation/SandboxEscape

* **探测周期 5 分钟**:平衡探测频率与资源消耗

* **WHY 主动探测**:被动监听只能发现已发生攻击,主动探测提前发现漏洞

### A.5 DECB 预算系数公式

* **连续 [0,1] 而非离散档位**:对应 OMEGA Ω-Sparse 稀疏化理念(非全有全无)

* **三因子乘法**:complexity × urgency × remaining_ratio,各因子独立可调

* **WHY 连续系数**:连续调节允许精细降级,避免硬切换造成的体验抖动

### A.6 TTG 模式选择规则

* **三级而非四级**:AETHER 文档提及 Non/Lite/Deep/Max 四级,但 nexus-core 已定义 Fast/Standard/Deep 三级,保持一致

* **预算联动优先**:Degraded 档位强制 Fast,避免高深度思考消耗有限预算

* **WHY 三级**:三级足够覆盖场景,四级增加复杂度但收益有限

### A.7 跨层依赖修正(继承 Week 3-4 经验)

| 依赖关系 | 修正方式 | 事件 |
|----------|----------|------|
| Parliament(L8)→ Quest Engine(L9) | 向上依赖禁止 | Parliament 订阅 `QuestCreated` 事件,而非调用 Quest Engine |
| Parliament(L8)→ GSOE/AutoDPO(L2) | 向上依赖禁止 | Parliament 发布 `ConsensusReached` 事件,GSOE/AutoDPO 订阅 |
| ASA(L4)→ PVL(L7) | 向上依赖禁止 | ASA 订阅 `OperationProduced` 事件,而非调用 PVL |
| AHIRT(L8)→ SecCore(L4) | 向下依赖允许 | AHIRT 可直接调用 SecCore 进行探测(同层 L8→L4 向下) |
| AHIRT(L8)→ Decay Engine(L4) | 向下依赖允许 | AHIRT 发布 `RedTeamAudit` 事件,Decay Engine 订阅(跨层事件) |
| DECB(L3)→ Quest Engine(L9) | 向上依赖禁止 | DECB 订阅 `QuestCreated` 事件,Quest Engine 订阅 `BudgetAdjusted` 事件 |
| TTG(L9)→ DECB(L3) | 向下依赖允许 | TTG 订阅 `BudgetAdjusted` 事件(向下订阅允许) |
| TTG(L9)→ Parliament(L8) | 向下依赖禁止 | TTG 发布 `ThinkingModeChanged` 事件,Parliament 订阅 |

### A.8 风险与缓解

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|---------|
| Parliament 辩论死锁 | 高 | 中 | 超时 5 秒强制结束,默认 Reject |
| Skeptic 误否决良性提案 | 中 | 中 | 规则库可配置,Week 6 NMC 后接入语义检测降低误报 |
| ASA 误 Block 合法操作 | 中 | 中 | 评分阈值可配置,Warn 级别不阻止操作 |
| AHIRT 探测消耗资源 | 中 | 中 | 探测周期 5 分钟,后台异步执行 |
| DECB 预算计算不准 | 中 | 低 | 三因子公式可配置,Week 8 打磨阶段校准 |
| TTG 模式频繁切换 | 低 | 中 | 滞后机制(档位变化后 10 秒内不再次切换) |
| 跨层事件丢失 | 高 | 低 | Critical 事件用 mpsc 通道,Normal 事件可丢弃 |
| DashMap 写锁与 async 冲突 | 中 | 中 | 写锁释放后再调用 async 方法(Week 3 已验证) |
| qeep-protocol 测试 flaky | 低 | 低 | 测试隔离,避免共享状态 |
| 安全规则库过时 | 中 | 中 | AHIRT 周期探测发现新漏洞,触发规则库更新 |

---

## 附录 B:团队组建与职责分配

### B.1 团队规模与核心能力要求

组建由 6 名资深专家级子智能体构成的协同开发团队,所有成员具备不少于 10 年行业从业经验。

| 角色 | 核心能力要求 | 负责领域 |
|------|-------------|---------|
| 架构专家 | 15 年+ Rust 架构设计,熟悉 tokio/async 生态 | Parliament 5 角色议会设计、跨层依赖修正 |
| 安全专家 | 15 年+ 系统安全,精通 sandbox/audit/anti-hack | Skeptic 否决权、ASA 对抗审计、AHIRT 反黑客红队 |
| 治理专家 | 15 年+ 分布式治理,精通 consensus/voting/budget | Parliament 投票共识、DECB 双档预算治理 |
| 并发专家 | 15 年+ 并发编程,精通 channel/lock/atomic | Parliament 并行辩论、AHIRT 并发探测 |
| 实现专家 | 15 年+ Rust 实现,精通 trait/generic/macro | TTG 思考切换、DECB 预算系数实现 |
| 质量专家 | 15 年+ 测试工程,精通 TDD/proptest/criterion | 全 crate 测试覆盖、qeep-protocol 测试加固、性能基准建立 |

### B.2 RACI 责任矩阵

| Task | 架构专家 | 安全专家 | 治理专家 | 并发专家 | 实现专家 | 质量专家 |
|------|---------|---------|---------|---------|---------|---------|
| Task 30: Parliament 5 角色议会 | A | C | R | R | R | C |
| Task 31: Skeptic 否决 + Auto-DPO | C | A | R | C | R | C |
| Task 32: ASA 对抗审计 | C | A | C | C | R | C |
| Task 33: AHIRT 反黑客红队 | C | A | C | R | R | C |
| Task 34: DECB 双档预算 | C | C | A | C | R | C |
| Task 35: TTG 思考切换 | C | C | C | C | A | C |
| Task 36: qeep-protocol 测试加固 | C | C | C | C | C | A |
| Task 37: Week 5 验收 | C | C | C | C | C | A |

> R = Responsible(执行), A = Accountable(负责), C = Consulted(咨询), I = Informed(知会)

### B.3 协作机制

* **每日站会**:每个 Task 开始前,主导专家与审查专家对齐设计决策

* **周例会**:Day 35 验收前,全员参与预验收,识别阻塞点

* **紧急响应**:发现 P0 级问题(安全漏洞/竞态/架构违规)时,立即触发紧急复审

* **Peer Review**:每个 Task 完成后,由非主导专家进行代码审查,审查未通过返回实现

---

## 附录 C:MoSCoW 优先级划分

### C.1 Must Have(必须做,P0)

* Parliament 5 角色议会与加权投票(Day 29)
* Skeptic 否决权与恶意意图检测(Day 30)
* ASA 对抗审计与干预分级(Day 31)
* AHIRT 反黑客红队与四类探测(Day 32)
* DECB 双档预算与溢出降级(Day 33)
* TTG 思考切换与预算联动(Day 34)
* Week 5 端到端验收(Day 35)

### C.2 Should Have(应该做,P1)

* Parliament Auto-DPO 训练对生成
* ASA Critic PPO 评分模型(占位)
* AHIRT 与 SecCore/Decay 协同闭环
* DECB 预算消耗统计
* TTG 复杂度评估
* qeep-protocol 测试加固(修复 Major-2)

### C.3 Could Have(可以做,P2)

* Parliament 辩论历史回放(Week 8 后)
* ASA 自适应评分阈值(Week 6 NMC 后)
* AHIRT 自适应探测策略(Week 8 后)
* DECB 多维度预算(Week 8 后)
* TTG 用户偏好学习(Week 8 后)

### C.4 Won't Have(暂不做,P3)

* Parliament 神经网络辩论(Week 8 后)
* ASA 真实 Critic PPO 模型(Week 6 NMC 后)
* AHIRT 强化学习探测(Week 8 后)
* DECB 跨 Quest 预算共享(Week 8 后)
* TTG 四级思考模式(保持三级)

---

## 附录 D:质量验收基准

### D.1 功能测试通过率

* 单元测试覆盖率 > 85%(关键路径全覆盖)
* 集成测试覆盖端到端认知治理流程
* 并发测试覆盖 10+ 线程场景(Parliament 并行辩论、AHIRT 并发探测)
* 属性测试(proptest)覆盖不变量(预算系数 ∈ [0,1]、共识判定一致性)
* 错误路径测试覆盖关键故障场景(否决、Block、预算超限)
* qeep-protocol 测试 ≥20 个(修复 Major-2)

### D.2 代码质量评分

* `cargo clippy --workspace -- -D warnings` 零警告
* 单函数 ≤ 200 行
* 无 `unwrap()`/`expect()` 在非测试代码
* 无 `unsafe` 代码
* 无功能标志
* newtype 类型安全(ID 类型)
* WHY 注释覆盖隐藏约束

### D.3 性能指标

| 指标 | 基准 | 验证方法 |
|------|------|---------|
| TTG 模式选择延迟 | < 1ms | criterion 基准 |
| DECB 预算计算延迟 | < 1ms | criterion 基准 |
| Parliament 5 角色辩论延迟 | < 200ms | criterion 基准 |
| Skeptic 恶意意图检测延迟 | < 10ms | criterion 基准 |
| ASA 审计延迟 | < 5ms/操作 | criterion 基准 |
| AHIRT 四类探测延迟 | < 500ms | 集成测试 |
| AHIRT 漏洞探测率 | > 95% | 100 载荷测试 |
| 决策准确率 | > 90% | 端到端测试 |
| CSA 端到端延迟 | < 300ms | min-of-N(5 次) |
| 孤儿调用率 | < 0.1% | GQEP 检测器 |

### D.4 安全免疫指标

| 攻击类型 | 拦截率 | 验证方法 |
|---------|--------|---------|
| 命令注入 | 100% | SecCore + ASA 协同测试 |
| 提示注入 | > 95% | Skeptic + AHIRT 协同测试 |
| 权限提升 | 100% | Decay Engine 能力衰减测试 |
| 沙箱逃逸 | 100% | SecCore 沙箱隔离测试 |
| 总体安全免疫率 | > 98% | 100 载荷综合测试 |

---

## 附录 E:资源授权与保障

### E.1 工具与资源清单

| 工具类别 | 工具名称 | 使用范围 | 权限级别 |
|---------|---------|---------|---------|
| 构建 | cargo check/build/test/clippy | 全 workspace | 读/写 |
| 格式化 | cargo fmt | 全 workspace | 写 |
| 基准 | criterion | Week 5 新增 benches | 读/写 |
| 属性测试 | proptest | Week 5 新增 proptest | 读/写 |
| 文档 | CODE_WIKI.md/CHANGELOG.md | 项目根 | 写 |
| 记忆 | project_memory.md | c:\Users\30324\.trae-cn\memory | 写 |
| MCP | run_mcp(按需) | 工具调用 | 读 |
| Skills | superpowers-main/writing-plans | 流程辅助 | 读 |

### E.2 工具使用规范

* **cargo 命令**:必须使用 `--jobs 1` 避免内存问题(继承 Week 1-4 经验)
* **环境变量**:执行 cargo 命令前必须设置工具链环境变量(见项目规则 §工作目录与平台)
* **MCP 工具**:调用前必须读取 tool schema 确认参数,所有参数通过 `args` 字段传入
* **Skills**:仅在相关时调用,不重复调用已加载的 skill

### E.3 培训与支持

* **工具使用指南**:项目规则 §工作目录与平台 已提供完整 cargo 命令清单
* **技术支持联系人**:架构专家(主) / 质量专家(辅)
* **紧急响应机制**:P0 级问题 2 小时内响应,24 小时内提供解决方案
