# Week 5 验收检查清单 — 议会 + 安全 + 预算(L8 + L4 + L3)

> 本检查清单对应 `tasks.md` 的 Task 30-37,共 48 个 SubTask。
> 每个 SubTask 完成后,验证对应检查项并勾选。
> 全部检查项通过后,Week 5 验收完成。

---

## Task 30:Parliament 5 角色对抗性议会

- [x] 30.1 Parliament 核心类型定义完整(RoleId/Role/RoleProfile/Proposal/Opinion/Consensus/DebateResult)
- [x] 30.1 ParliamentError enum 定义完整(RoleNotFound/DebateTimeout/QuorumNotMet/VetoFailed/ConfigError)
- [x] 30.1 ParliamentConfig 含 5 角色权重(Architect=0.25/Skeptic=0.30/Optimizer=0.20/Librarian=0.15/Bard=0.10)
- [x] 30.1 `cargo check -p parliament` 通过
- [x] 30.2 RoleRegistry 注册 5 个默认角色,权重总和 = 1.0
- [x] 30.2 Skeptic 角色 `can_veto = true`,其余 `can_veto = false`
- [x] 30.2 角色注册后发布 `RoleRegistered` 事件 <!-- ✅ 修复于 Week 6 复审(2026-06-27):parliament/roles.rs 新增 with_event_bus 构造器,register() 通过 publish_blocking 发布事件 -->
- [x] 30.3 `Parliament::deliberate` 实现提案→辩论→投票全流程
- [x] 30.3 5 角色并行辩论使用 `FuturesUnordered`,超时 5 秒
- [x] 30.3 辩论延迟 < 200ms(占位实现)
- [x] 30.4 加权赞成率计算正确(Σ 角色权重 × 角色立场)
- [x] 30.4 共识判定:赞成率 ≥ 0.6 且无 Skeptic 否决 → Reached
- [x] 30.4 参与率 < 0.6 时强制 Rejected(法定人数)
- [x] 30.4 共识达成后发布 `ConsensusReached` 事件 `[Critical]`
- [x] 30.5 并发测试:10 线程同时 deliberate,无 panic、无数据竞争
- [x] 30.5 criterion 基准:warmup 10 次 + P50/P99 100 次测量
- [x] 30.5 性能断言测试标记 `#[ignore = "perf: run with --ignored"]`

## Task 31:Skeptic 否决权与 Auto-DPO 触发

- [x] 31.1 MaliciousIntentType 定义 5 类(CommandInjection/PrivilegeEscalation/DataExfiltration/SandboxEscape/PromptInjection)
- [x] 31.1 MaliciousIntentRuleBook 加载 5 类规则,每类含 pattern/severity/action
- [x] 31.1 规则库可通过 `omega.yaml` 配置扩展(不引入 regex 依赖)
- [x] 31.1 规则匹配测试:5 类各 5 个用例,共 25 个测试用例
- [x] 31.2 Skeptic 检测恶意意图时行使冻结权,返回 `Consensus::Vetoed`
- [x] 31.2 发布 `SkepticVeto` 事件 `[Critical]`(携带 quest_id/veto_reason/frozen_capabilities)
- [x] 31.2 发布 `CapabilityFrozen` 事件(供 Decay Engine 订阅)
- [x] 31.2 否决延迟 < 10ms(基于规则匹配)
- [x] 31.3 DpoPairGenerator 生成 DPO 训练对(chosen/rejected/context/pair_id)
- [x] 31.3 DPO 对经 `ConsensusReached` 事件的 `dpo_pair_id` 字段传递(不直接调用 AutoDPO)
- [x] 31.3 每次共识达成 ≥ 1 个 DPO 对(若存在对比)
- [x] 31.4 集成测试:恶意提案 → Skeptic 否决 → CapabilityFrozen 事件 → Decay 订阅
- [x] 31.4 集成测试:良性提案 → Skeptic 通过 → 正常辩论流程

## Task 32:ASA 对抗性自我审计

- [x] 32.1 AuditResult 定义完整(safety_score/correctness_score/efficiency_score/intervention)
- [x] 32.1 InterventionAction 定义 3 级(Allow/Warn/Block)
- [x] 32.1 SecCoreError 扩展 `AsaBlocked` 变体
- [x] 32.1 `cargo check -p seccore` 通过
- [x] 32.2 AsaAuditor::audit 实现基于规则的评分模型(占位)
- [x] 32.2 评分公式:`safety_score = 1.0 - risk_weight × keyword_count - history_failure_rate`
- [x] 32.2 标记 TODO(Week 6):替换为 Critic PPO 模型
- [x] 32.2 审计延迟 < 5ms/操作
- [x] 32.3 干预分级:safety_score ≥ 0.8 → Allow;0.5 ≤ score < 0.8 → Warn;score < 0.5 → Block
- [x] 32.3 Warn 级别发布 `AsaIntervention` 事件(Normal 严重度)
- [x] 32.3 Block 级别发布 `AsaIntervention` 事件 `[Critical]`,返回 `SecCoreError::AsaBlocked`
- [x] 32.3 干预动作测试:Allow/Warn/Block 三级各 5 个用例,共 15 个测试用例
- [x] 32.4 协同流程:ASA 事中审计 → SecCore 沙箱执行 → AuditChain 审计记录
- [x] 32.4 ASA Block 的操作不进入沙箱(事中拦截优先)
- [x] 32.4 沙箱违规时发布 `SandboxViolation` 事件,ASA 据此更新历史失败率
- [x] 32.4 ASA 订阅 `OperationProduced` 事件实现事中审计
- [x] 32.5 并发测试:10 线程并发 audit,无 panic、无数据竞争
- [x] 32.5 criterion 基准:审计延迟
- [x] 32.5 性能断言测试标记 `#[ignore]`

## Task 33:AHIRT 反黑客内部红队

- [x] 33.1 ProbeType 定义 4 类(PromptInjection/CommandInjection/PrivilegeEscalation/SandboxEscape)
- [x] 33.1 ProbePayloadLibrary 加载 4 类探测载荷,每类 25 个,共 100 个
- [x] 33.1 载荷库可通过 `omega.yaml` 配置扩展
- [x] 33.2 AhirtRedTeam::probe 实现 4 类主动探测
- [x] 33.2 PromptInjection 探测:构造恶意 prompt,检测 Skeptic 拦截
- [x] 33.2 CommandInjection 探测:构造 `$(...)`/`|`/`;`/`&&` 载荷,检测 SecCore 拦截
- [x] 33.2 PrivilegeEscalation 探测:构造 `sudo`/`su` 载荷,检测 Decay 能力衰减
- [x] 33.2 SandboxEscape 探测:构造路径穿越/进程注入载荷,检测沙箱隔离
- [x] 33.2 AHIRT 可直接调用 SecCore(L8→L4 向下依赖允许)
- [x] 33.2 探测完成后发布 `AhirtProbeCompleted` 事件
- [x] 33.2 探测延迟 < 500ms(四类全量)
- [x] 33.3 漏洞探测率 = (拦截数 / 总数) × 100% > 95%
- [x] 33.3 探测率 < 95% 时发布 `RedTeamAudit` 事件 `[Critical]`
- [x] 33.3 100 个已知攻击载荷测试,拦截率 > 95%
- [x] 33.4 AHIRT 发现漏洞 → 发布 `RedTeamAudit` 事件 → SecCore 强化规则
- [x] 33.4 AHIRT 发现漏洞 → 发布 `RedTeamAudit` 事件 → Decay Engine 衰减能力
- [x] 33.4 AHIRT 不直接调用 Decay Engine(跨层通过事件解耦)
- [x] 33.5 周期探测:默认每 5 分钟全量探测(后台异步 `tokio::spawn`)
- [x] 33.5 事件触发:新工具注册/新 Quest 创建时触发对应类探测
- [x] 33.5 并发测试:10 线程并发探测,无 panic、无数据竞争

## Task 34:DECB 双档认知预算治理

- [x] 34.1 BudgetTier 定义 3 档(HighTier/LowTier/Degraded)
- [x] 34.1 BudgetCoefficient ∈ [0,1] 包装
- [x] 34.1 DecbConfig 含 base_budget(0.8)/high_tier_threshold(0.6)/low_tier_threshold(0.3)
- [x] 34.1 `cargo check -p decb-governor` 通过
- [x] 34.2 DecbGovernor::compute_budget 实现连续可调预算系数
- [x] 34.2 预算系数公式:`coefficient = base_budget × complexity_factor × urgency_factor × remaining_budget_ratio`
- [x] 34.2 complexity_factor ∈ [0.5, 1.5](基于 Quest 任务数与依赖深度)
- [x] 34.2 urgency_factor ∈ [0.8, 1.2](基于 Quest deadline)
- [x] 34.2 remaining_budget_ratio ∈ [0.0, 1.0]
- [x] 34.2 预算系数 clamp 到 [0.0, 1.0]
- [x] 34.2 预算系数计算延迟 < 1ms
- [x] 34.3 档位判定:coefficient ≥ 0.6 → HighTier;0.3 ≤ coefficient < 0.6 → LowTier;< 0.3 → Degraded
- [x] 34.3 档位切换发布 `BudgetAdjusted` 事件
- [x] 34.3 档位切换滞后机制:10 秒内不再次切换
- [x] 34.3 档位切换延迟 < 1ms
- [x] 34.4 溢出检测:消耗超过预算上限时触发
- [x] 34.4 发布 `BudgetExceeded` 事件 `[Critical]`
- [x] 34.4 自动降级:HighTier → LowTier → Degraded
- [x] 34.4 Degraded 模式下仍超预算时拒绝新 Quest
- [x] 34.4 溢出检测周期:每 10 秒检查一次(后台异步)
- [x] 34.5 预算消耗统计:`consumption = token_count × cost_per_token + tool_call_count × cost_per_call`
- [x] 34.5 每 100 次 Quest 执行发布 `BudgetStatsReported` 事件
- [x] 34.6 并发测试:10 线程并发 compute_budget,无 panic、无数据竞争
- [x] 34.6 criterion 基准:预算系数计算延迟
- [x] 34.6 性能断言测试标记 `#[ignore]`

## Task 35:TTG 思考切换治理

- [x] 35.1 TtgConfig 含 simple_task_threshold(3)/complex_task_threshold(10)/lag_interval_ms(10000)
- [x] 35.1 QuestError 扩展 `TtgOverrideRejected` 变体
- [x] 35.1 复用 `nexus_core::ThinkingMode`(Fast/Standard/Deep)
- [x] 35.1 `cargo check -p quest-engine` 通过
- [x] 35.2 TtgGovernor::evaluate_complexity 实现复杂度评分
- [x] 35.2 复杂度公式:`task_count × 0.3 + dependency_depth × 0.4 + description_length_factor × 0.3`
- [x] 35.2 dependency_depth:DAG 最长路径深度(复用 quest-engine 现有逻辑)
- [x] 35.2 description_length_factor ∈ [0.0, 1.0](长度/1000,clamp)
- [x] 35.3 TtgGovernor::select_mode 实现 4 种选择规则
- [x] 35.3 Degraded 档位强制 Fast
- [x] 35.3 模式切换发布 `ThinkingModeChanged` 事件
- [x] 35.3 模式选择延迟 < 1ms
- [x] 35.4 TTG 订阅 `BudgetAdjusted` 事件实现联动切换
- [x] 35.4 联动规则:HighTier→Deep 倾向;LowTier→Standard 倾向;Degraded→Fast 强制
- [x] 35.4 联动切换不阻塞主流程(事件订阅异步)
- [x] 35.4 联动切换滞后机制:10 秒内不再次切换
- [x] 35.5 TtgGovernor::override_mode 支持手动覆盖
- [x] 35.5 Degraded 档位不允许覆盖为 Deep
- [x] 35.5 手动覆盖发布 `ThinkingModeChanged` 事件(reason = "manual_override")
- [x] 35.6 并发测试:10 线程并发 select_mode,无 panic、无数据竞争
- [x] 35.6 criterion 基准:模式选择延迟
- [x] 35.6 性能断言测试标记 `#[ignore]`

## Task 36:qeep-protocol 测试加固

- [x] 36.1 覆盖盲区清单完成(超时边界/孤儿检测边界/并发纠缠态/错误传播链)
- [x] 36.2 超时测试:1ms/100ms/1s/10s 四种 Duration
- [x] 36.2 超时边界测试:刚好超时 / 刚好未超时
- [x] 36.2 超时错误传播测试:超时 → GqepError → 上层捕获
- [x] 36.3 所有 Sender drop → recv 返回 None 测试
- [x] 36.3 部分 Sender drop → recv 仍可接收剩余消息测试
- [x] 36.3 孤儿调用检测器触发测试(模拟 spawn 未 await)
- [x] 36.4 10+ 线程并发 EntangledCall 测试,无 panic、无数据竞争
- [x] 36.4 空 Future 列表/单 Future/最大 Future 数(1000)边界测试
- [x] 36.4 错误传播链测试:超时 → 错误 → 上层捕获 → 恢复
- [x] 36.4 总测试数 ≥ 20 个(原 8 个 + 新增 ≥12 个)
- [x] 36.4 测试通过率 100%,无 flaky 测试

## Task 37:Week 5 端到端验收

- [x] 37.1 event-bus 新增 9 个事件类型(DebateStarted/SkepticVeto/RedTeamAudit/ThinkingModeChanged/BudgetAdjusted/AsaIntervention/AhirtProbeCompleted/RoleRegistered/BudgetStatsReported) <!-- ✅ Week 7 Task 8.4 核验(2026-06-27):实际为 8 个新变体 + 1 个字段扩展。ThinkingModeSwitched 是复用扩展(新增 reason 字段),非新增变体;event-bus/types.rs 代码注释明确写"8 个新变体"。AsaIntervention 在 severity() 中返回 Normal(Block 语义等价 Critical,发布者负责通过 Critical 通道发送)。描述已同步修正于 CHANGELOG Week 5 章节 -->
- [x] 37.1 事件 metadata()/severity()/type_name() match 分支更新
- [x] 37.1 `cargo test -p event-bus` 通过(新事件序列化/反序列化、severity 正确)
- [x] 37.2 端到端流程测试:Quest 创建 → TTG → DECB → Parliament → Skeptic → ASA → AHIRT → 共识
- [x] 37.2 全流程无 panic、无孤儿调用、无事件丢失
- [x] 37.3 CSA 延迟 < 300ms(min-of-N 5 次)
- [x] 37.3 CSA 延迟分解:TTG 1ms + DECB 1ms + Parliament 200ms + Skeptic 10ms + ASA 5ms + AHIRT 50ms + 事件 30ms ≈ 297ms
- [x] 37.3 性能断言测试标记 `#[ignore]`
- [x] 37.4 命令注入攻击 100% 被 SecCore/ASA 拦截
- [x] 37.4 提示注入攻击 > 95% 被 Skeptic/AHIRT 拦截
- [x] 37.4 权限提升攻击 100% 被 Decay Engine 衰减
- [x] 37.4 沙箱逃逸攻击 100% 被 SecCore 沙箱拦截
- [x] 37.4 100 个攻击载荷综合测试,拦截率 > 98%
- [x] 37.5 `cargo check --workspace --jobs 1` 通过
- [x] 37.5 `cargo clippy --workspace --jobs 1 -- -D warnings` 零警告
- [x] 37.5 `cargo test --workspace --jobs 1` 全通过(Week 1-5 测试,含 qeep-protocol ≥20 测试)
- [x] 37.5 `cargo build --workspace --release --jobs 1` 通过
- [x] 37.5 新实现 crate 测试覆盖率 > 85%
- [x] 37.5 孤儿调用率 < 0.1%(GQEP 检测器)
- [x] 37.6 CHANGELOG.md 新增 "## Week 5 议会 + 安全 + 预算" 章节
- [x] 37.6 CODE_WIKI.md 新增 parliament/decb-governor/seccore(ASA)/quest-engine(TTG) 模块说明
- [x] 37.6 project_memory.md 记录 Week 5 经验教训
- [x] 37.7 Parliament proptest:加权赞成率 ∈ [0,1]、共识判定一致性
- [x] 37.7 DECB proptest:预算系数 ∈ [0,1]、档位切换单调性
- [x] 37.7 TTG proptest:复杂度评分 ∈ [0, ∞)、模式选择与档位一致性
- [x] 37.7 ASA proptest:评分 ∈ [0,1]、干预动作分级一致性
- [x] 37.7 AHIRT proptest:探测率 ∈ [0,1]
- [x] 37.7 5 个 crate 各补充 5 个错误路径测试,共 25 个

---

## 跨任务验收检查

### 架构合规性

- [x] 所有 crate 顶部保留 `#![forbid(unsafe_code)]`
- [x] 所有 crate 顶部保留 `#![warn(missing_docs, clippy::all)]`
- [x] 无跨层向上依赖违规(Parliament L8 → Quest L9 禁止,通过事件)
- [x] 无跨层向上依赖违规(ASA L4 → PVL L7 禁止,通过事件订阅)
- [x] Critical 事件使用适当通道保证投递(SkepticVeto/AsaIntervention/RedTeamAudit/BudgetExceeded)

### 代码质量

- [x] 单函数 ≤ 200 行(项目铁律)
- [x] 无 `unwrap()`/`expect()` 在非测试代码
- [x] 无 `unsafe` 代码
- [x] 无功能标志
- [x] newtype 类型安全(ID 类型使用 newtype struct)
- [x] WHY 注释覆盖隐藏约束
- [x] workspace 级依赖(无独立版本声明)

### 并发安全

- [x] 所有 async fn 满足 `Send + 'static + 'async`
- [x] 所有 SQLite 操作 `spawn_blocking`
- [x] 无持锁调用 async 的死锁风险
- [x] DashMap 写锁释放后再调用 async 方法
- [x] check-then-act 模式原子化

### 性能指标

- [x] TTG 模式选择 < 1ms
- [x] DECB 预算计算 < 1ms
- [x] Parliament 5 角色辩论 < 200ms
- [x] Skeptic 恶意意图检测 < 10ms
- [x] ASA 审计 < 5ms/操作
- [x] AHIRT 四类探测 < 500ms
- [x] AHIRT 漏洞探测率 > 95%
- [x] 决策准确率 > 90%
- [x] CSA 端到端延迟 < 300ms
- [x] 孤儿调用率 < 0.1%

### 安全免疫

- [x] 命令注入拦截率 100%
- [x] 提示注入拦截率 > 95%
- [x] 权限提升拦截率 100%
- [x] 沙箱逃逸拦截率 100%
- [x] 总体安全免疫率 > 98%

### 文档同步

- [x] CHANGELOG.md 更新 Week 5 章节
- [x] CODE_WIKI.md 更新 4 个模块说明
- [x] project_memory.md 记录 Week 5 经验教训
- [x] spec/tasks/checklist 文档完整

---

## 验收签字

| 维度 | 验收人 | 通过标准 |
|------|--------|---------|
| Task 30 Parliament | 架构专家 | 5 角色议会 + 加权投票 + 共识判定 |
| Task 31 Skeptic | 安全专家 | 否决权 + 恶意意图检测 + Auto-DPO |
| Task 32 ASA | 安全专家 | 对抗审计 + 干预分级 + 沙箱协同 |
| Task 33 AHIRT | 安全专家 | 4 类探测 + 探测率 > 95% + 协同闭环 |
| Task 34 DECB | 治理专家 | 预算系数 + 档位切换 + 溢出降级 |
| Task 35 TTG | 实现专家 | 模式选择 + 预算联动 + 手动覆盖 |
| Task 36 qeep-protocol | 质量专家 | 测试 ≥20 个 + 覆盖盲区 |
| Task 37 验收 | 质量专家 | E2E + CSA + 安全免疫 + 全量构建 |

**总体通过标准**:全部检查项勾选,Week 5 验收完成,可进入 Week 6(适配 + 进化 + 多模态)。
