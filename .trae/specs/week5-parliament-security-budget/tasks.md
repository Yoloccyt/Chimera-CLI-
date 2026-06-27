# Tasks — Week 5 议会 + 安全 + 预算(L8 + L4 + L3)

> 本任务列表基于 6 位资深专家(架构/安全/治理/并发/实现/质量)的分布式深度分析结果制定。
> 按 Day 29-35 时间线排序,共 8 个 Task(Task 30-37),48 个 SubTask。
> 每个 SubTask 完成后立即勾选对应 checklist 项。
> 严格遵循 MoSCoW 优先级:P0(Must Have)→ P1(Should Have)→ P2(Could Have)。

---

## Task 30:Parliament 5 角色对抗性议会(Day 29,P0)

实现 `parliament` crate 的 5 角色议会(Architect/Skeptic/Optimizer/Librarian/Bard),提案→辩论→投票→共识全流程。

- [x] SubTask 30.1:定义 Parliament 核心类型与错误类型
  - 在 `crates/parliament/src/types.rs` 定义 `RoleId`(newtype)、`Role`、`RoleProfile`(含 `role_id`/`specialty`/`model_preference`/`voting_weight`/`can_veto`)、`Proposal`、`Opinion`(含 `position`/`confidence`/`rationale`)、`Consensus`(enum:Reached/Rejected/Vetoed)、`DebateResult`
  - 在 `crates/parliament/src/error.rs` 定义 `ParliamentError` enum(thiserror),含 `RoleNotFound`/`DebateTimeout`/`QuorumNotMet`/`VetoFailed`/`ConfigError`
  - 在 `crates/parliament/src/config.rs` 定义 `ParliamentConfig`,含 5 角色权重(Architect=0.25/Skeptic=0.30/Optimizer=0.20/Librarian=0.15/Bard=0.10)、`consensus_threshold`(默认 0.6)、`quorum_threshold`(默认 0.6)、`debate_timeout_ms`(默认 5000)
  - 文件:`crates/parliament/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p parliament` 通过

- [x] SubTask 30.2:实现 5 角色定义与注册
  - 在 `crates/parliament/src/roles.rs` 实现 `RoleRegistry` 结构体
  - 注册 5 个角色:Architect(架构决策)、Skeptic(安全审计,can_veto=true)、Optimizer(性能优化)、Librarian(记忆检索)、Bard(用户沟通)
  - `RoleRegistry::new() -> Self`:初始化 5 角色默认配置
  - `RoleRegistry::register(&self, profile: RoleProfile)`:动态注册新角色
  - 持有 `RwLock<HashMap<RoleId, RoleProfile>>`(读多写少)
  - 角色注册后发布 `RoleRegistered` 事件(同层通信)
  - 文件:`crates/parliament/src/roles.rs`
  - 新增单元测试:5 角色默认注册、动态注册、角色查询
  - 验证:角色权重总和 = 1.0

- [x] SubTask 30.3:实现提案→辩论→投票全流程
  - 在 `crates/parliament/src/debate.rs` 实现 `Parliament::deliberate(&self, quest: &Quest, proposal: &Proposal) -> Result<Consensus, ParliamentError>`
  - 流程:发起提案(发布 `DebateStarted` 事件)→ 5 角色并行辩论(各角色生成 `Opinion`)→ 加权投票(发布 `VoteCast` 事件)→ 共识判定
  - 5 角色并行辩论使用 `FuturesUnordered`(继承 Week 4 GQEP 经验),超时 5 秒
  - Opinion 生成占位实现:基于 Quest 特征的规则化生成(Week 6 NMC 后接入真实模型)
  - 文件:`crates/parliament/src/debate.rs`、`crates/parliament/src/lib.rs`
  - 新增单元测试:全赞成通过、全反对拒绝、部分赞成共识、超时拒绝
  - 验证:辩论延迟 < 200ms(占位实现)

- [x] SubTask 30.4:实现加权投票与共识判定
  - 在 `crates/parliament/src/voting.rs` 实现 `VoteCounter` 结构体
  - 加权赞成率 = Σ(角色权重 × 角色立场),立场 ∈ {0.0(反对), 0.5(弃权), 1.0(赞成)}
  - 共识判定:赞成率 ≥ 0.6 且无 Skeptic 否决 → Reached;赞成率 < 0.6 → Rejected;Skeptic 否决 → Vetoed
  - 参与率 < 0.6 时强制 Rejected(法定人数不足)
  - 共识达成后发布 `ConsensusReached` 事件 `[Critical]`(携带 `quest_id`/`decision_hash`/`dpo_pair_id`)
  - 文件:`crates/parliament/src/voting.rs`
  - 新增单元测试:权重计算、共识判定、法定人数、Skeptic 否决优先
  - 验证:共识判定逻辑正确

- [x] SubTask 30.5:Parliament 并发测试与性能基准
  - 新增并发测试:10 线程同时 deliberate,无 panic、无数据竞争
  - 新增 `benches/debate.rs` criterion 基准:5 角色辩论延迟(warmup 10 次 + P50/P99 100 次测量)
  - 性能断言测试标记 `#[ignore = "perf: run with --ignored"]`
  - 文件:`crates/parliament/tests/concurrent.rs`、`crates/parliament/benches/debate.rs`
  - 验证:辩论延迟 < 200ms,并发无竞态

---

## Task 31:Skeptic 否决权与 Auto-DPO 触发(Day 30,P0)

实现 Skeptic 否决权(冻结权)、恶意意图规则库、Auto-DPO 训练对生成。

- [x] SubTask 31.1:定义 Skeptic 类型与恶意意图规则库
  - 在 `crates/parliament/src/veto.rs` 定义 `MaliciousIntentType`(enum:CommandInjection/PrivilegeEscalation/DataExfiltration/SandboxEscape/PromptInjection)、`IntentRule`(含 `pattern`/`severity`/`action`)、`VetoReason`
  - 实现 `MaliciousIntentRuleBook` 结构体,加载 5 类恶意意图规则
  - 每类规则含 `pattern`(字符串匹配,不引入 regex 依赖)、`severity`(Critical/High/Medium)、`action`(Veto/Warn)
  - 规则库可通过 `omega.yaml` 配置扩展
  - 文件:`crates/parliament/src/veto.rs`
  - 新增单元测试:5 类规则各 5 个用例,共 25 个测试用例
  - 验证:规则匹配正确

- [x] SubTask 31.2:实现 Skeptic 否决权
  - 在 `crates/parliament/src/veto.rs` 实现 `Skeptic::detect_malicious_intent(&self, proposal: &Proposal) -> Option<VetoReason>`
  - Skeptic 检测到恶意意图时,行使冻结权,立即终止辩论,返回 `Consensus::Vetoed`
  - 发布 `SkepticVeto` 事件 `[Critical]`(携带 `quest_id`/`veto_reason`/`frozen_capabilities`)
  - 发布 `CapabilityFrozen` 事件(供 Decay Engine 订阅,衰减对应能力)
  - 否决延迟基准:< 10ms(基于规则匹配)
  - 文件:`crates/parliament/src/veto.rs`、`crates/parliament/src/debate.rs`
  - 新增单元测试:恶意意图否决、良性提案通过、5 类攻击各否决一次
  - 验证:否决延迟 < 10ms

- [x] SubTask 31.3:实现 Auto-DPO 训练对生成
  - 在 `crates/parliament/src/debate.rs` 新增 `DpoPairGenerator` 结构体
  - 共识达成且辩论过程产生好/坏决策对比时,生成 `DpoPair`(含 `chosen`/`rejected`/`context`/`pair_id`)
  - `chosen` = 共采纳的 Opinion,`rejected` = 被否决的 Opinion
  - DPO 训练对经 `ConsensusReached` 事件的 `dpo_pair_id` 字段传递(不直接调用 AutoDPO,避免向上依赖)
  - 文件:`crates/parliament/src/debate.rs`
  - 新增单元测试:DPO 对生成、无对比时不生成、pair_id 唯一性
  - 验证:每次共识达成 ≥ 1 个 DPO 对(若存在对比)

- [x] SubTask 31.4:Skeptic 集成测试
  - 新增集成测试:恶意提案 → Skeptic 否决 → CapabilityFrozen 事件 → Decay 订阅
  - 新增集成测试:良性提案 → Skeptic 通过 → 正常辩论流程
  - 文件:`crates/parliament/tests/skeptic_integration.rs`
  - 验证:Skeptic 否决权正确行使,事件正确发布

---

## Task 32:ASA 对抗性自我审计(Day 31,P0)

在 `seccore` crate 中扩展 ASA 模块,基于 Critic PPO 思想实现实时介入纠偏。

- [x] SubTask 32.1:定义 ASA 核心类型与错误类型
  - 在 `crates/seccore/src/asa.rs` 定义 `AuditResult`(含 `safety_score`/`correctness_score`/`efficiency_score`/`intervention`)、`InterventionAction`(enum:Allow/Warn/Block)、`AsaConfig`(含 `safety_threshold_allow`(默认 0.8)/`safety_threshold_warn`(默认 0.5)/`safety_threshold_block`(默认 0.5))
  - 扩展 `SecCoreError` enum,新增 `AsaBlocked` 变体
  - 文件:`crates/seccore/src/asa.rs`、`crates/seccore/src/error.rs`
  - 验证:`cargo check -p seccore` 通过

- [x] SubTask 32.2:实现 ASA 实时审计与评分模型(占位)
  - 在 `crates/seccore/src/asa.rs` 实现 `AsaAuditor::audit(&self, operation: &Operation) -> AuditResult`
  - Week 5 占位实现:基于规则的评分模型
  - 评分公式:`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`
  - `correctness_score` 占位:基于语法检查(复用 PVL Verifier 逻辑,通过事件订阅获取)
  - `efficiency_score` 占位:基于操作复杂度(简单操作高分,复杂操作低分)
  - 标记 TODO(Week 6):替换为 Critic PPO 模型
  - 文件:`crates/seccore/src/asa.rs`
  - 新增单元测试:Allow/Warn/Block 三级评分、边界值、历史失败率影响
  - 验证:审计延迟 < 5ms/操作

- [x] SubTask 32.3:实现干预动作分级与事件发布
  - 干预动作分级:`safety_score ≥ 0.8` → Allow;`0.5 ≤ safety_score < 0.8` → Warn;`safety_score < 0.5` → Block
  - Warn 级别发布 `AsaIntervention` 事件(Normal 严重度),操作继续执行但记录告警
  - Block 级别发布 `AsaIntervention` 事件 `[Critical]`(携带 `operation_id`/`block_reason`/`alternative_suggestion`),返回 `SecCoreError::AsaBlocked`
  - 文件:`crates/seccore/src/asa.rs`
  - 新增单元测试:Allow/Warn/Block 三级各 5 个用例,共 15 个测试用例
  - 验证:干预动作分级正确

- [x] SubTask 32.4:实现 ASA 与 SecCore 沙箱协同
  - 协同流程:ASA 事中审计(Allow/Warn/Block)→ SecCore 沙箱执行(零信任拦截)→ AuditChain 审计记录
  - ASA Block 的操作不进入沙箱(事中拦截优先于沙箱执行)
  - 沙箱违规时发布 `SandboxViolation` 事件,ASA 据此更新历史失败率(反馈闭环)
  - ASA 订阅 `OperationProduced` 事件(PVL 发布),实现事中审计(跨层 L4→L7 向下订阅允许)
  - 文件:`crates/seccore/src/asa.rs`、`crates/seccore/src/lib.rs`
  - 新增集成测试:ASA Allow → 沙箱执行、ASA Block → 沙箱跳过、沙箱违规 → ASA 更新失败率
  - 验证:协同流程正确

- [x] SubTask 32.5:ASA 并发测试与性能基准
  - 新增并发测试:10 线程并发 audit,无 panic、无数据竞争
  - 新增 `benches/asa_audit.rs` criterion 基准:审计延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/seccore/tests/asa_concurrent.rs`、`crates/seccore/benches/asa_audit.rs`
  - 验证:审计延迟 < 5ms/操作,并发无竞态

---

## Task 33:AHIRT 反黑客内部红队(Day 32,P0)

在 `parliament` crate 中实现 AHIRT 反黑客红队,主动探测四类攻击,漏洞探测率 > 95%。

- [x] SubTask 33.1:定义 AHIRT 核心类型与探测载荷库
  - 在 `crates/parliament/src/ahirt.rs` 定义 `ProbeType`(enum:PromptInjection/CommandInjection/PrivilegeEscalation/SandboxEscape)、`ProbePayload`(含 `probe_type`/`payload`/`expected_result`)、`ProbeResult`(含 `probe_type`/`passed`/`actual_result`)、`AhirtStats`(含 `total`/`passed`/`failed`/`detection_rate`)
  - 实现 `ProbePayloadLibrary` 结构体,加载 4 类探测载荷(每类 25 个,共 100 个)
  - 载荷库可通过 `omega.yaml` 配置扩展
  - 文件:`crates/parliament/src/ahirt.rs`
  - 新增单元测试:载荷库加载、载荷查询、载荷计数
  - 验证:4 类各 25 个载荷

- [x] SubTask 33.2:实现 AHIRT 主动探测
  - 在 `crates/parliament/src/ahirt.rs` 实现 `AhirtRedTeam::probe(&self, probe_type: ProbeType) -> ProbeResult`
  - 对系统执行四类主动探测:
    - PromptInjection:构造恶意 prompt,检测是否被 Skeptic 拦截
    - CommandInjection:构造 `$(...)`/`|`/`;`/`&&` 载荷,检测 SecCore 沙箱拦截
    - PrivilegeEscalation:构造 `sudo`/`su` 载荷,检测 Decay Engine 能力衰减
    - SandboxEscape:构造路径穿越/进程注入载荷,检测 SecCore 沙箱隔离
  - AHIRT 可直接调用 SecCore(L8→L4 向下依赖允许)进行探测
  - 探测完成后发布 `AhirtProbeCompleted` 事件(携带 `probe_type`/`total`/`passed`/`failed`/`detection_rate`)
  - 文件:`crates/parliament/src/ahirt.rs`
  - 新增单元测试:4 类探测各 5 个用例,共 20 个测试用例
  - 验证:探测延迟 < 500ms(四类全量)

- [x] SubTask 33.3:实现漏洞探测率验证
  - 漏洞探测率 = (被系统拦截的探测数 / 总探测数) × 100%
  - 探测率基准:> 95%
  - 探测率 < 95% 时,发布 `RedTeamAudit` 事件 `[Critical]`(携带 `vulnerability_type`/`failed_probes`/`remediation_suggestion`)
  - 文件:`crates/parliament/src/ahirt.rs`
  - 新增单元测试:100 个已知攻击载荷,验证拦截率 > 95%
  - 验证:探测率 > 95%

- [x] SubTask 33.4:实现 AHIRT 与 SecCore/Decay 协同
  - AHIRT 发现漏洞时,发布 `RedTeamAudit` 事件
  - SecCore 订阅 `RedTeamAudit` 事件,强化对应拦截规则
  - Decay Engine 订阅 `RedTeamAudit` 事件,衰减对应能力(对应 ADR-002)
  - AHIRT 不直接调用 Decay Engine(跨层 L8→L4 通过事件解耦,虽允许向下依赖但事件更松耦合)
  - 文件:`crates/parliament/src/ahirt.rs`
  - 新增集成测试:AHIRT 触发漏洞 → SecCore 规则强化 → Decay 能力衰减,全链路验证
  - 验证:协同闭环正确

- [x] SubTask 33.5:AHIRT 周期探测与并发测试
  - 实现周期探测:默认每 5 分钟全量探测一次(后台异步任务 `tokio::spawn`)
  - 支持事件触发:新工具注册/新 Quest 创建时触发对应类探测
  - 新增并发测试:10 线程并发探测,无 panic、无数据竞争
  - 文件:`crates/parliament/src/ahirt.rs`、`crates/parliament/tests/ahirt_concurrent.rs`
  - 验证:周期探测不阻塞主流程,并发无竞态

---

## Task 34:DECB 双档认知预算治理(Day 33,P0)

实现 `decb-governor` crate,连续可调 [0,1] 预算系数、高低档自动切换、溢出检测与降级。

- [x] SubTask 34.1:定义 DECB 核心类型与错误类型
  - 在 `crates/decb-governor/src/types.rs` 定义 `BudgetTier`(enum:HighTier/LowTier/Degraded)、`BudgetCoefficient`(f32 包装,∈ [0,1])、`BudgetConsumption`(含 `token_count`/`tool_call_count`/`context_load_count`/`total_cost`)、`BudgetStats`
  - 在 `crates/decb-governor/src/error.rs` 定义 `DecbError` enum,含 `BudgetExceeded`/`InvalidCoefficient`/`DegradedModeRejected`/`ConfigError`
  - 在 `crates/decb-governor/src/config.rs` 定义 `DecbConfig`,含 `base_budget`(默认 0.8)、`high_tier_threshold`(默认 0.6)、`low_tier_threshold`(默认 0.3)、`total_budget_limit`、`overflow_check_interval_ms`(默认 10000)
  - 文件:`crates/decb-governor/src/{types.rs,error.rs,config.rs,lib.rs}`
  - 验证:`cargo check -p decb-governor` 通过

- [x] SubTask 34.2:实现连续可调预算系数计算
  - 在 `crates/decb-governor/src/governor.rs` 实现 `DecbGovernor::compute_budget(&self, quest: &Quest, context: &Context) -> f32`
  - 预算系数公式:`coefficient = base_budget × complexity_factor × urgency_factor × remaining_budget_ratio`
  - `complexity_factor ∈ [0.5, 1.5]`:基于 Quest 任务数与依赖深度(复用 quest-engine DAG 逻辑,通过事件订阅获取 Quest 信息,避免向上依赖)
  - `urgency_factor ∈ [0.8, 1.2]`:基于 Quest deadline
  - `remaining_budget_ratio ∈ [0.0, 1.0]`:剩余预算 / 总预算
  - 预算系数 clamp 到 [0.0, 1.0]
  - 文件:`crates/decb-governor/src/governor.rs`、`crates/decb-governor/src/lib.rs`
  - 新增单元测试:简单任务低系数、复杂任务高系数、紧急任务加成、预算不足降级
  - 验证:预算系数计算延迟 < 1ms

- [x] SubTask 34.3:实现高低档自动切换
  - 档位判定:`coefficient ≥ 0.6` → HighTier;`0.3 ≤ coefficient < 0.6` → LowTier;`coefficient < 0.3` → Degraded
  - 档位切换发布 `BudgetAdjusted` 事件(携带 `old_tier`/`new_tier`/`coefficient`/`reason`)
  - 档位切换滞后机制:档位变化后 10 秒内不再次切换(避免频繁切换)
  - 文件:`crates/decb-governor/src/governor.rs`
  - 新增单元测试:High→Low、Low→High、High→Degraded、Low→Degraded 四种切换路径、滞后机制
  - 验证:档位切换延迟 < 1ms

- [x] SubTask 34.4:实现预算溢出检测与降级
  - 当前消耗超过预算上限时,触发溢出检测
  - 发布 `BudgetExceeded` 事件 `[Critical]`(携带 `budget_type`/`current`/`limit`)
  - 自动降级:HighTier → LowTier → Degraded,直至消耗 < 预算
  - Degraded 模式下仍超预算时,拒绝新 Quest 并发布 `BudgetExceeded` 事件
  - 溢出检测周期:每 10 秒检查一次(后台异步任务)
  - 文件:`crates/decb-governor/src/overflow.rs`、`crates/decb-governor/src/governor.rs`
  - 新增单元测试:溢出触发、降级链路、Degraded 拒绝、检测周期
  - 验证:溢出降级正确

- [x] SubTask 34.5:实现预算消耗统计
  - DECB 记录消耗:`consumption = token_count × cost_per_token + tool_call_count × cost_per_call`
  - 每 100 次 Quest 执行发布 `BudgetStatsReported` 事件(携带 `total_consumption`/`remaining_budget`/`utilization_rate`)
  - 文件:`crates/decb-governor/src/governor.rs`
  - 新增单元测试:单 Quest 消耗、多 Quest 累计、预算重置、统计事件发布
  - 验证:消耗统计正确

- [x] SubTask 34.6:DECB 并发测试与性能基准
  - 新增并发测试:10 线程并发 compute_budget,无 panic、无数据竞争
  - 新增 `benches/budget_compute.rs` criterion 基准:预算系数计算延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/decb-governor/tests/concurrent.rs`、`crates/decb-governor/benches/budget_compute.rs`
  - 验证:预算计算 < 1ms,并发无竞态

---

## Task 35:TTG 思考切换治理(Day 34,P0)

在 `quest-engine` crate 中扩展 TTG 模块,基于 Quest 复杂度与预算自动选择 Fast/Standard/Deep 三级模式。

- [x] SubTask 35.1:定义 TTG 核心类型与错误类型
  - 在 `crates/quest-engine/src/ttg.rs` 定义 `TtgConfig`(含 `simple_task_threshold`(默认 3)/`complex_task_threshold`(默认 10)/`lag_interval_ms`(默认 10000))、`ComplexityScore`(f32 包装)
  - 扩展 `QuestError` enum,新增 `TtgOverrideRejected` 变体
  - 复用 `nexus_core::ThinkingMode`(Fast/Standard/Deep)
  - 文件:`crates/quest-engine/src/ttg.rs`、`crates/quest-engine/src/error.rs`
  - 验证:`cargo check -p quest-engine` 通过

- [x] SubTask 35.2:实现复杂度评估
  - 在 `crates/quest-engine/src/ttg.rs` 实现 `TtgGovernor::evaluate_complexity(&self, quest: &Quest) -> f32`
  - 复杂度评分:`complexity_score = task_count × 0.3 + dependency_depth × 0.4 + description_length_factor × 0.3`
  - `dependency_depth`:DAG 最长路径深度(复用 quest-engine 现有 DAG 逻辑)
  - `description_length_factor ∈ [0.0, 1.0]`:描述长度归一化(长度 / 1000,clamp 到 [0,1])
  - 文件:`crates/quest-engine/src/ttg.rs`
  - 新增单元测试:简单(1 task)/中等(5 task)/复杂(20 task)三级、依赖深度影响、描述长度影响
  - 验证:复杂度评分正确

- [x] SubTask 35.3:实现自动模式选择
  - 在 `crates/quest-engine/src/ttg.rs` 实现 `TtgGovernor::select_mode(&self, quest: &Quest, budget_tier: BudgetTier) -> ThinkingMode`
  - 选择规则:
    - `budget_tier == Degraded` → `Fast`(降级模式强制快速)
    - `quest.tasks.len() <= 3 && budget_tier != HighTier` → `Fast`
    - `quest.tasks.len() <= 10 || budget_tier == LowTier` → `Standard`
    - `quest.tasks.len() > 10 || budget_tier == HighTier` → `Deep`
  - 模式切换发布 `ThinkingModeChanged` 事件(携带 `quest_id`/`old_mode`/`new_mode`/`reason`)
  - 文件:`crates/quest-engine/src/ttg.rs`
  - 新增单元测试:4 种选择规则、边界值、模式切换事件
  - 验证:模式选择延迟 < 1ms

- [x] SubTask 35.4:实现预算联动切换
  - TTG 订阅 `BudgetAdjusted` 事件(DECB 发布),自动重新选择思考模式
  - 联动规则:HighTier → 倾向 Deep;LowTier → 倾向 Standard;Degraded → 强制 Fast
  - 联动切换不阻塞主流程(事件订阅异步处理)
  - 联动切换滞后机制:档位变化后 10 秒内不再次切换(与 DECB 滞后机制一致)
  - 文件:`crates/quest-engine/src/ttg.rs`
  - 新增单元测试:HighTier→Deep、LowTier→Standard、Degraded→Fast、滞后机制
  - 验证:联动切换正确

- [x] SubTask 35.5:实现手动覆盖与回退
  - TTG 支持手动覆盖:`TtgGovernor::override_mode(&self, quest_id: &str, mode: ThinkingMode) -> Result<(), QuestError>`
  - 手动覆盖优先级高于自动选择,但受预算档位约束(Degraded 档位不允许覆盖为 Deep)
  - 手动覆盖发布 `ThinkingModeChanged` 事件(携带 `reason = "manual_override"`)
  - 文件:`crates/quest-engine/src/ttg.rs`
  - 新增单元测试:手动设置 Deep、Degraded 档位下尝试覆盖(应拒绝)、覆盖事件
  - 验证:手动覆盖正确

- [x] SubTask 35.6:TTG 并发测试与性能基准
  - 新增并发测试:10 线程并发 select_mode,无 panic、无数据竞争
  - 新增 `benches/ttg_select.rs` criterion 基准:模式选择延迟
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/quest-engine/tests/ttg_concurrent.rs`、`crates/quest-engine/benches/ttg_select.rs`
  - 验证:模式选择 < 1ms,并发无竞态

---

## Task 36:qeep-protocol 测试加固(Day 29-35 并行,P1)

补充 `qeep-protocol` crate 测试至 ≥20 个,修复 Week 1-4 横向复审 Major-2 问题。

- [x] SubTask 36.1:分析现有测试覆盖盲区
  - 读取 `crates/qeep-protocol/src/lib.rs` 与 `crates/qeep-protocol/tests/qeep.rs`
  - 识别当前 8 个测试的覆盖盲区:超时边界、孤儿检测边界、并发纠缠态、错误传播链
  - 文件:无(分析步骤)
  - 验证:覆盖盲区清单完成

- [x] SubTask 36.2:补充超时场景测试
  - 新增测试:超时 1ms/100ms/1s/10s 四种 Duration
  - 新增测试:超时边界(刚好超时 / 刚好未超时)
  - 新增测试:超时错误传播(超时 → GqepError → 上层捕获)
  - 文件:`crates/qeep-protocol/tests/qeep.rs`
  - 验证:超时测试覆盖完整

- [x] SubTask 36.3:补充孤儿检测测试
  - 新增测试:所有 Sender drop → recv 返回 None
  - 新增测试:部分 Sender drop → recv 仍可接收剩余消息
  - 新增测试:孤儿调用检测器触发(模拟 spawn 未 await)
  - 文件:`crates/qeep-protocol/tests/qeep.rs`
  - 验证:孤儿检测测试覆盖完整

- [x] SubTask 36.4:补充并发纠缠态与边界条件测试
  - 新增测试:10+ 线程并发 EntangledCall,无 panic、无数据竞争
  - 新增测试:空 Future 列表、单 Future、最大 Future 数(1000)
  - 新增测试:错误传播链(超时 → 错误 → 上层捕获 → 恢复)
  - 文件:`crates/qeep-protocol/tests/qeep.rs`
  - 验证:并发与边界测试覆盖完整,总测试数 ≥20

---

## Task 37:Week 5 端到端验收(Day 35,P0)

全认知链路集成测试、CSA 延迟验证、安全免疫验证、全量构建验收。

- [x] SubTask 37.1:新增 Week 5 所需事件类型
  - 在 `crates/event-bus/src/types.rs` 新增事件:`DebateStarted`/`SkepticVeto`/`RedTeamAudit`/`ThinkingModeChanged`/`BudgetAdjusted`/`AsaIntervention`/`AhirtProbeCompleted`/`RoleRegistered`/`BudgetStatsReported`
  - 更新 `metadata()`/`severity()`/`type_name()` match 分支
  - 文件:`crates/event-bus/src/types.rs`
  - 新增单元测试:新事件序列化/反序列化、severity 正确
  - 验证:`cargo test -p event-bus` 通过

- [x] SubTask 37.2:端到端认知治理流程测试
  - 新增集成测试:Quest 创建 → TTG 模式选择 → DECB 预算计算 → Parliament 5 角色辩论 → Skeptic 安全审计 → ASA 实时介入 → AHIRT 主动探测 → 共识达成/否决
  - 验证全流程无 panic、无孤儿调用、无事件丢失
  - 文件:`crates/parliament/tests/e2e.rs`(或独立集成测试目录)
  - 验证:全流程通过

- [x] SubTask 37.3:CSA 延迟验证
  - 新增 CSA 延迟测试:端到端延迟 < 300ms
  - CSA 延迟分解:TTG 1ms + DECB 1ms + Parliament 200ms + Skeptic 10ms + ASA 5ms + AHIRT 50ms + 事件传播 30ms ≈ 297ms
  - CSA 延迟测试采用 min-of-N(5 次)减少调度噪声
  - 性能断言测试标记 `#[ignore]`
  - 文件:`crates/parliament/tests/csa.rs`
  - 验证:CSA < 300ms

- [x] SubTask 37.4:安全免疫验证
  - 新增安全免疫测试:模拟 Claude CVE-2026-35022 攻击向量
  - 命令注入攻击 100% 被 SecCore 拦截(事前)或 ASA Block(事中)
  - 提示注入攻击 > 95% 被 Skeptic 否决或 AHIRT 探测
  - 权限提升攻击 100% 被 Decay Engine 能力衰减
  - 沙箱逃逸攻击 100% 被 SecCore 沙箱拦截
  - 安全免疫测试覆盖 100 个攻击载荷,拦截率 > 98%
  - 文件:`crates/parliament/tests/security_immunity.rs`
  - 验证:安全免疫率 > 98%

- [x] SubTask 37.5:全量测试与构建验收
  - 运行 `cargo check --workspace --jobs 1` 通过
  - 运行 `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告
  - 运行 `cargo test --workspace --jobs 1` 全通过(Week 1-5 测试用例,含 qeep-protocol 加固后的 ≥20 测试)
  - 运行 `cargo build --workspace --release --jobs 1` 通过
  - 验证新实现 crate 的测试覆盖率 > 85%
  - 验证孤儿调用率 < 0.1%(GQEP 检测器)
  - 文件:无(验证步骤)
  - 验证:全量验收通过

- [x] SubTask 37.6:更新文档(CHANGELOG/CODE_WIKI/project_memory)
  - 更新 `CHANGELOG.md`:新增 "## Week 5 议会 + 安全 + 预算" 章节
  - 更新 `CODE_WIKI.md`:新增 parliament/decb-governor/seccore(ASA)/quest-engine(TTG) 模块职责说明
  - 更新 `project_memory.md`:记录 Week 5 经验教训(对抗治理闭环、安全事件优先级、预算驱动降级、思考模式与预算联动)
  - 文件:`CHANGELOG.md`、`CODE_WIKI.md`、`c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`
  - 验证:文档更新完整

- [x] SubTask 37.7:补充 proptest 与错误路径测试
  - 每个 crate 补充 proptest(不变量验证):
    - Parliament:加权赞成率 ∈ [0,1]、共识判定一致性
    - DECB:预算系数 ∈ [0,1]、档位切换单调性
    - TTG:复杂度评分 ∈ [0, ∞)、模式选择与档位一致性
    - ASA:评分 ∈ [0,1]、干预动作分级一致性
    - AHIRT:探测率 ∈ [0,1]
  - 每个 crate 补充错误路径测试(5 个/crate,共 25 个)
  - 文件:5 个 crate 的 `tests/proptest.rs`、`tests/error_paths.rs`
  - 验证:proptest 与错误路径测试通过

---

## Task Dependencies

- Task 30(Parliament 5 角色议会)→ 无依赖,优先执行
- Task 31(Skeptic 否决)→ 依赖 Task 30(Parliament 框架)
- Task 32(ASA 对抗审计)→ 无依赖,可与 Task 30 并行(扩展 seccore,不依赖 parliament)
- Task 33(AHIRT 反黑客)→ 依赖 Task 30(Parliament 框架)+ Task 32(SecCore 协同)
- Task 34(DECB 双档预算)→ 无依赖,可与 Task 30/32 并行
- Task 35(TTG 思考切换)→ 依赖 Task 34(预算联动)
- Task 36(qeep-protocol 测试加固)→ 无依赖,可与任何 Task 并行(P1 优先级,Day 29-35 穿插执行)
- Task 37(验收)→ 依赖 Task 30-35(全部完成后验收),Task 36 可并行完成

## 优先级执行顺序

1. **第一批(并行)**:Task 30(Parliament)+ Task 32(ASA)+ Task 34(DECB)+ Task 36(qeep-protocol 测试加固)
2. **第二批(并行)**:Task 31(Skeptic,依赖 Parliament)+ Task 33(AHIRT,依赖 Parliament + SecCore)+ Task 35(TTG,依赖 DECB)
3. **第三批**:Task 37(验收,依赖全部完成)

## 关键路径

Task 30(Parliament)→ Task 31(Skeptic)/ Task 33(AHIRT)→ Task 37(验收)

Parliament 是关键路径上的核心组件,Skeptic 与 AHIRT 均依赖 Parliament 框架,需优先完成 Parliament。

## WBS 工作分解结构

```
Week 5 交付物
├── Task 30: Parliament 5 角色议会 (Day 29)
│   ├── 30.1 核心类型定义
│   ├── 30.2 角色注册
│   ├── 30.3 辩论流程
│   ├── 30.4 投票共识
│   └── 30.5 并发测试与基准
├── Task 31: Skeptic 否决 + Auto-DPO (Day 30)
│   ├── 31.1 恶意意图规则库
│   ├── 31.2 否决权实现
│   ├── 31.3 Auto-DPO 生成
│   └── 31.4 集成测试
├── Task 32: ASA 对抗审计 (Day 31)
│   ├── 32.1 核心类型定义
│   ├── 32.2 评分模型(占位)
│   ├── 32.3 干预分级
│   ├── 32.4 沙箱协同
│   └── 32.5 并发测试与基准
├── Task 33: AHIRT 反黑客红队 (Day 32)
│   ├── 33.1 探测载荷库
│   ├── 33.2 主动探测
│   ├── 33.3 探测率验证
│   ├── 33.4 SecCore/Decay 协同
│   └── 33.5 周期探测与并发
├── Task 34: DECB 双档预算 (Day 33)
│   ├── 34.1 核心类型定义
│   ├── 34.2 预算系数计算
│   ├── 34.3 档位切换
│   ├── 34.4 溢出降级
│   ├── 34.5 消耗统计
│   └── 34.6 并发测试与基准
├── Task 35: TTG 思考切换 (Day 34)
│   ├── 35.1 核心类型定义
│   ├── 35.2 复杂度评估
│   ├── 35.3 自动模式选择
│   ├── 35.4 预算联动
│   ├── 35.5 手动覆盖
│   └── 35.6 并发测试与基准
├── Task 36: qeep-protocol 测试加固 (Day 29-35 并行)
│   ├── 36.1 盲区分析
│   ├── 36.2 超时测试
│   ├── 36.3 孤儿检测测试
│   └── 36.4 并发与边界测试
└── Task 37: Week 5 端到端验收 (Day 35)
    ├── 37.1 事件类型新增
    ├── 37.2 E2E 流程测试
    ├── 37.3 CSA 延迟验证
    ├── 37.4 安全免疫验证
    ├── 37.5 全量构建验收
    ├── 37.6 文档更新
    └── 37.7 proptest 与错误路径
```
