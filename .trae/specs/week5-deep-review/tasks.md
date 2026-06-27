# Tasks — Week 5 深度复审

> 本任务列表基于 6 位资深专家(架构/集成/代码质量/并发/测试/文档)的分布式深度分析结果制定。
> 共 7 个 Task,对应 6 个审计维度 + 1 个汇总报告。
> Task 1-4 可并行执行,Task 5-6 依赖前四者输出,Task 7 为汇总。
> 严格遵循证据驱动原则:所有结论必须引用具体代码位置(文件:行号)。

---

## Task 1:架构一致性审计(维度 A,P0)

验证 Week 5 实现与 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 设计文档的一致性。

- [x] SubTask 1.1:Parliament 5 角色权重与设计文档一致性检查
  - 读取 `crates/parliament/src/config.rs` 验证 5 角色权重(Architect=0.25/Skeptic=0.30/Optimizer=0.20/Librarian=0.15/Bard=0.10)
  - 对照 `AETHER_NEXUS_OMEGA_ULTIMATE.md` §5.2 数据流参考,验证角色职责与设计一致
  - 输出:一致性确认或漂移清单(文件:行号)
  - 验证:权重总和 = 1.0,角色职责匹配

- [x] SubTask 1.2:十层依赖方向合规性验证
  - 检查 `crates/parliament/Cargo.toml` 的 [dependencies],确认无 L8→L9 向上依赖
  - 检查 `crates/parliament/Cargo.toml` 的 [dev-dependencies],确认测试依赖合规(dev-dependencies 可豁免)
  - 检查 `crates/decb-governor/Cargo.toml` 的 [dependencies],确认 L3 仅依赖 L1/L2
  - 检查 `crates/seccore/Cargo.toml` 的 [dependencies],确认 L4 仅依赖 L1
  - 检查 `crates/quest-engine/Cargo.toml` 的 [dependencies],确认 L9 仅依赖 L1-L8
  - 输出:依赖方向合规性报告
  - 验证:无生产代码向上依赖违规

- [x] SubTask 1.3:OMEGA 四定律(Ω-Event/Ω-Evolve)在 Week 5 代码中的体现审计
  - Ω-Event:验证 Parliament/DECB/TTG/ASA/AHIRT 通过 EventBus 通信
  - Ω-Evolve:验证 AHIRT 红队探测 → GSOE 进化的对抗进化闭环
  - 输出:四定律体现清单
  - 验证:Week 5 代码体现 Ω-Event 与 Ω-Evolve

- [x] SubTask 1.4:跨层通信合规性(ConsensusReached/RedTeamAudit 事件解耦)
  - 验证 Parliament 不直接 import GSOE/AutoDPO(通过 ConsensusReached 事件解耦)
  - 验证 AHIRT 不直接调用 Decay Engine(通过 RedTeamAudit 事件解耦)
  - 验证 TTG 订阅 BudgetAdjusted 事件(而非直接调用 DECB)
  - 输出:跨层通信合规性报告
  - 验证:无跨层向上直接依赖

- [x] SubTask 1.5:命名模式与新类型安全审计
  - 检查命名模式:*Governor(DecbGovernor/TtgGovernor)、*Registry(RoleRegistry)、*RuleBook(MaliciousIntentRuleBook)、*RedTeam(AhirtRedTeam)、*Counter(VoteCounter)、*Generator(DpoPairGenerator)
  - 检查 newtype:RoleId、BudgetCoefficient、ComplexityScore
  - 输出:命名合规性报告
  - 验证:命名符合项目规范,newtype 类型安全

- [x] SubTask 1.6:`#![forbid(unsafe_code)]` 覆盖验证
  - 检查 `crates/parliament/src/lib.rs`、`crates/decb-governor/src/lib.rs` 顶部 `#![forbid(unsafe_code)]`
  - 检查 `crates/seccore/src/lib.rs`、`crates/quest-engine/src/lib.rs`、`crates/event-bus/src/lib.rs` 顶部 `#![forbid(unsafe_code)]`
  - 输出:覆盖清单
  - 验证:所有 Week 5 crate 覆盖 `#![forbid(unsafe_code)]`

---

## Task 2:跨层集成审计(维度 B,P0)

验证 EventBus 事件契约的完整性与 Week 5 新增事件的跨层通信正确性。

- [x] SubTask 2.1:Week 5 新增 9 个事件类型字段完整性验证
  - 读取 `crates/event-bus/src/types.rs`,验证 9 个事件类型的字段完整性
  - 验证 DebateStarted/SkepticVeto/RedTeamAudit/ThinkingModeSwitched(扩展)/BudgetAdjusted/AsaIntervention/AhirtProbeCompleted/RoleRegistered/BudgetStatsReported
  - 输出:事件字段清单
  - 验证:每个事件字段完整且语义清晰

- [x] SubTask 2.2:事件 source 字段与发布者对应关系验证
  - 验证 DebateStarted 的 source = "parliament"
  - 验证 SkepticVeto 的 source = "parliament"
  - 验证 RedTeamAudit 的 source = "parliament"
  - 验证 BudgetAdjusted 的 source = "decb-governor"
  - 验证 AsaIntervention 的 source = "seccore"
  - 验证 AhirtProbeCompleted 的 source = "parliament"
  - 输出:source 对应关系清单
  - 验证:每个事件的 source 与发布者 crate 对应

- [x] SubTask 2.3:Critical 事件(SkepticVeto/RedTeamAudit)背压保护审计
  - 验证 SkepticVeto/RedTeamAudit 标记为 EventSeverity::Critical
  - 验证 Critical 事件是否通过 mpsc 点对点通道保证投递(或文档说明)
  - 输出:背压保护策略清单
  - 验证:Critical 事件有投递保证

- [x] SubTask 2.4:AsaIntervention severity 静态判定问题评估
  - 验证 AsaIntervention 统一返回 Normal(同步函数无法动态判定)
  - 评估 Block 级别应通过 Critical 通道发送的文档说明是否充分
  - 输出:severity 判定评估报告
  - 验证:文档说明充分,或有改进建议

- [ ] SubTask 2.5:事件发布/订阅配对完整性(**结转 Week 7**)
  - 检查每个新事件是否有发布者(通过 Grep 搜索事件名)
  - 检查每个新事件是否有订阅者(通过 Grep 搜索 subscribe)
  - 识别孤儿事件(有发布无订阅)和幽灵订阅(有订阅无发布)
  - 输出:配对完整性清单
  - 验证:无孤儿事件,无幽灵订阅
  - **核验结果(Task 9)**:8/9 事件已发布(DebateStarted/SkepticVeto/RedTeamAudit/AhirtProbeCompleted/BudgetAdjusted/BudgetStatsReported/AsaIntervention/ThinkingModeSwitched),但 `RoleRegistered` 未发布(roles.rs:124 仅 TODO 注释 + tracing 日志);生产代码无任何订阅者(仅测试代码 subscribe)。结转 Week 7 补齐订阅者与 RoleRegistered 发布。

- [x] SubTask 2.6:ThinkingModeSwitched 向后兼容性验证
  - 验证 #[serde(default)] reason 字段
  - 验证旧格式(无 reason)反序列化为空字符串
  - 验证 quest-engine/src/engine.rs 的 ThinkingModeSwitched 发布已包含 reason
  - 输出:兼容性验证报告
  - 验证:向后兼容性通过
  - **核验通过(Task 9)**:types.rs:155 `#[serde(default)] reason` ✓;types.rs:1580-1603 旧格式反序列化测试通过(reason="")✓;engine.rs:408-415 发布携带 reason="manual_switch" ✓;ttg.rs:271/289/307/324 三级切换(Fast/Standard/Deep)实现完整 ✓

- [x] SubTask 2.7:BudgetAdjusted vs BudgetExceeded 语义区分验证
  - 验证 BudgetAdjusted(档位切换通知,Normal)与 BudgetExceeded(超限告警,Critical)的语义区分
  - 验证发布场景:档位切换发 BudgetAdjusted,超限发 BudgetExceeded
  - 输出:语义区分验证报告
  - 验证:语义区分清晰

- [x] SubTask 2.8:跨层 API 类型安全审计
  - 检查事件 payload 中是否有 String 弱类型传递关键 ID(应使用 newtype 或强类型)
  - 检查跨 crate 公开 API 的参数类型安全
  - 输出:类型安全清单
  - 验证:无关键 ID 使用 String 弱类型

---

## Task 3:技术债与代码质量审计(维度 C,P0)

识别 Week 5 代码中的隐式技术债和代码质量问题。

- [x] SubTask 3.1:伪实现/桩函数识别
  - 识别 ASA 评分模型占位(基于规则,TODO Week 6 替换为 Critic PPO)
  - 识别 Opinion 生成规则化(基于 Quest 特征,TODO Week 6 接入真实模型)
  - 识别 AHIRT 探测的占位逻辑(若存在)
  - 识别 MaliciousIntentRuleBook 的字符串匹配(不引入 regex 依赖)
  - 输出:伪实现清单(标注 TODO 周次)
  - 验证:所有伪实现都有 TODO 标注

- [x] SubTask 3.2:硬编码常量盘点
  - 检查 Parliament 5 秒超时(debate_timeout_ms = 5000)
  - 检查 DECB 10 秒滞后(lag_interval_ms = 10000)
  - 检查 TTG 10 秒滞后(lag_interval_ms = 10000)
  - 检查 AHIRT 5 分钟周期探测
  - 检查阈值常量(0.6 共识阈值、0.8 Allow 阈值、0.5 Warn/Block 阈值)
  - 输出:硬编码常量清单(标注是否应配置化)
  - 验证:关键阈值已配置化或标注理由

- [x] SubTask 3.3:错误处理一致性审计
  - 验证 ParliamentError/DecbError/QuestError/SecCoreError 使用 thiserror
  - 验证错误传播链完整性(? 操作符使用正确)
  - 验证错误变体覆盖所有失败场景
  - 输出:错误处理一致性报告
  - 验证:错误处理一致且完整

- [x] SubTask 3.4:注释完整性评估(WHY 注释)
  - 检查 WHY 注释覆盖隐藏约束(滞后机制、预算优先、依赖方向)
  - 检查 WHY 注释覆盖变通方案(ASA 占位实现、规则匹配顺序)
  - 检查 WHY 注释覆盖反直觉行为(Skeptic 权重最高、Degraded 强制 Fast)
  - 输出:注释完整性评估报告
  - 验证:WHY 注释覆盖关键隐藏约束
  - **核验通过(Task 9)**:AHIRT 周期与探测率阈值已有 WHY 注释(ahirt.rs:9/13/36/148/171/218/261/296/360/371/383/404/405/407/410/458/471/475);滞后机制 WHY(governor.rs:39/79/99/840);预算优先 WHY(debate.rs:224/249/260/285);依赖方向 WHY(ttg.rs:9/13);规则匹配顺序 WHY(asa.rs:10/145/159/172/187/196/232)。原 checklist FAIL 描述"AHIRT 周期与探测率阈值缺 WHY 注释"与实际不符。

- [x] SubTask 3.5:函数长度合规性检查(≤200 行)
  - 使用 Grep 或手动检查 Week 5 新增函数的长度
  - 重点关注 Parliament::deliberate、AhirtRedTeam::probe、DecbGovernor::compute_budget
  - 输出:超长函数清单(若有)
  - 验证:所有函数 ≤ 200 行

- [x] SubTask 3.6:模块组织规范性检查
  - 检查 parliament:lib.rs → types.rs → error.rs → config.rs → roles.rs → voting.rs → debate.rs → veto.rs → ahirt.rs
  - 检查 decb-governor:lib.rs → types.rs → error.rs → config.rs → governor.rs → overflow.rs
  - 检查 seccore/asa.rs 模块组织
  - 检查 quest-engine/ttg.rs 模块组织
  - 输出:模块组织规范性报告
  - 验证:模块组织符合项目规范
  - **核验通过(Task 9)**:parliament(9 文件:lib/types/error/config/roles/voting/debate/veto/ahirt)✓;decb-governor(6 文件:lib/types/error/config/governor/overflow)✓;quest-engine(8 文件:lib/types/error/config/engine/ttg/dag/checkpoint)✓;seccore(7 文件:lib/types/error/asa/audit/policy/sandbox,无独立 config.rs 但配置内嵌)✓。均符合标准布局。

- [x] SubTask 3.7:unwrap()/expect() 使用合规性
  - Grep 搜索 Week 5 新增源码中的 unwrap()/expect()
  - 验证非测试代码无 unwrap()/expect()
  - 验证锁中毒场景使用 unwrap_or_else(|p| p.into_inner())
  - 输出:unwrap()/expect() 使用清单
  - 验证:非测试代码无 unwrap()/expect()(锁中毒场景豁免)

- [x] SubTask 3.8:Box<dyn Trait> 使用 avoidance
  - Grep 搜索 Week 5 新增源码中的 Box<dyn Trait>
  - 验证优先使用 impl Trait 或 enum dispatch
  - 输出:Box<dyn Trait> 使用清单(若有)
  - 验证:无 Box<dyn Trait>(或有合理理由)

---

## Task 4:并发与性能审计(维度 D,P0)

识别 Week 5 代码中的并发安全隐患和性能瓶颈。

- [x] SubTask 4.1:锁竞争热点分析
  - 分析 RoleRegistry RwLock(读多写少,合理)
  - 分析 VoteCounter 锁(投票期间持有,可能热点)
  - 分析 DecbGovernor Mutex(compute_budget 时持有,预算计算 < 1ms,低风险)
  - 分析 TtgGovernor Mutex(select_mode 时持有,模式选择 < 1ms,低风险)
  - 输出:锁竞争热点清单
  - 验证:无高竞争锁热点

- [x] SubTask 4.2:死锁风险评估
  - 检查锁顺序(是否存在多锁获取)
  - 检查 async 持锁(锁内是否调用 async 方法)
  - 检查跨锁引用(是否存在锁 A 内获取锁 B)
  - 输出:死锁风险清单
  - 验证:无死锁风险
  - **核验通过(Task 9)**:全局搜索 `lock().*await` 模式无匹配(parliament/decb-governor/quest-engine/seccore 均 ✓);decb-governor/src/governor.rs:520-529 spawn 任务用内层块限制 `tier_state.lock()` 作用域,锁在块结束自动释放,await 在块外执行(有 WHY 注释说明);无跨锁引用嵌套。无死锁风险。

- [x] SubTask 4.3:async 正确性验证
  - 验证 Parliament::deliberate 的 FuturesUnordered 使用正确
  - 验证 5 秒超时机制(tokio::time::timeout)
  - 验证 Skeptic 辩论前否决(同步否决,不进入辩论)
  - 验证所有 async fn 满足 Send + 'static
  - 输出:async 正确性报告
  - 验证:无 async 正确性缺陷

- [x] SubTask 4.4:分配热点识别
  - 检查热路径上的 Vec/HashMap 分配
  - 检查不必要的 clone(尤其在大对象上)
  - 输出:分配热点清单
  - 验证:无关键分配热点
  - **核验通过(Task 9)**:Week 5 crates 无 DashMap 使用(仅 qeep-protocol 用 dashmap 但属 Week 1);asa.rs:430 `command clone` 有 WHY 注释说明必要_owned_;热路径 Vec/HashMap 分配为常规用法。无关键分配热点(Week 6 NMC 接入后需复评 quest.clone,与原 checklist 4.4 结论一致)。

- [x] SubTask 4.5:算法复杂度审计
  - MaliciousIntentRuleBook 规则匹配:O(n) 线性扫描,25 条规则,可接受
  - AHIRT 载荷库查询:O(1) HashMap 或 O(n) 线性扫描,验证实现
  - DECB 预算计算:O(1) 公式计算,低复杂度
  - TTG 复杂度评估:DAG 最长路径深度,O(V+E),验证实现
  - 输出:算法复杂度清单
  - 验证:无高复杂度热点

- [x] SubTask 4.6:并发原语选择评估
  - FuturesUnordered vs join_all:验证 Parliament 使用 FuturesUnordered(流式结果)
  - mpsc vs broadcast:验证事件发布使用合适通道
  - 输出:并发原语选择评估
  - 验证:并发原语选择合理

- [x] SubTask 4.7:check-then-act 原子化验证
  - DecbGovernor 档位判定与切换:验证原子化(Mutex 保护)
  - TtgGovernor 模式选择:验证原子化(Mutex 保护)
  - 输出:check-then-act 原子化报告
  - 验证:check-then-act 模式原子化

- [x] SubTask 4.8:滞后机制实现审计
  - DECB 档位切换 10 秒滞后:验证实现(时间戳记录 + 比对)
  - TTG 联动切换 10 秒滞后:验证实现(与 DECB 一致)
  - 验证滞后机制不会阻塞主流程
  - 输出:滞后机制审计报告
  - 验证:滞后机制实现正确

---

## Task 5:测试覆盖盲区审计(维度 E,P0)

识别 Week 5 测试覆盖的盲区与薄弱环节。

- [x] SubTask 5.1:错误路径覆盖率分析
  - 验证 ParliamentError 每个变体(RoleNotFound/DebateTimeout/QuorumNotMet/VetoFailed/ConfigError)有测试
  - 验证 DecbError 每个变体(BudgetExceeded/InvalidCoefficient/DegradedModeRejected/ConfigError)有测试
  - 验证 QuestError 的 TtgOverrideRejected 变体有测试
  - 验证 SecCoreError 的 AsaBlocked 变体有测试
  - 输出:错误路径覆盖率清单
  - 验证:每个 Error 变体有测试覆盖

- [x] SubTask 5.2:边界条件测试检查
  - 空提案测试(0 角色/0 票)
  - 全弃权测试(立场 = 0.5)
  - 预算系数 0.0/1.0 边界
  - 复杂度 0 边界(0 任务)
  - safety_score 0.0/1.0 边界
  - 探测率 0.0/1.0 边界
  - 输出:边界条件测试清单
  - 验证:关键边界条件有测试

- [ ] SubTask 5.3:集成测试缺口识别(**结转 Week 7**)
  - Parliament ↔ DECB ↔ TTG 联动测试(E2E 测试覆盖)
  - AHIRT ↔ SecCore 协同闭环测试(安全免疫测试覆盖)
  - ASA ↔ SecCore 沙箱协同测试(E2E 测试覆盖)
  - 识别未覆盖的集成场景
  - 输出:集成测试缺口清单
  - 验证:关键集成场景已覆盖
  - **核验结果(Task 9)**:parliament/tests/e2e.rs 有 5 个 E2E 测试(良性/恶意/降级/AHIRT/ASA),其中 test_e2e_budget_degradation_flow:261-262 验证 DECB→TTG 联动(on_budget_adjusted 同步调用);但事件流 E2E 缺失(无 subscribe 验证事件发布);AHIRT↔SecCore 闭环无独立测试(仅各自单元测试)。结转 Week 7 补齐事件流 E2E。

- [ ] SubTask 5.4:proptest 充分性评估(**结转 Week 7**)
  - 验证 5 个 crate 各有 proptest 文件
  - 验证不变量覆盖(加权赞成率/预算系数/复杂度评分/safety_score/探测率)
  - 评估 proptest 迭代次数(默认 256)
  - 输出:proptest 充分性报告
  - 验证:proptest 覆盖关键不变量
  - **核验结果(Task 9)**:4/5 Week 5 crate 有 proptest.rs(parliament/decb-governor/seccore/quest-engine ✓);qeep-protocol/tests/ 仅 qeep.rs,**无 proptest.rs**,Cargo.toml 也无 proptest 依赖。结转 Week 7 补齐 qeep-protocol proptest。

- [x] SubTask 5.5:测试隔离性验证
  - 检查无共享状态(无 static mut)
  - 检查无顺序依赖(每个测试独立)
  - 检查无环境变量竞态(已修复 path_util 竞态,验证无其他竞态)
  - 输出:测试隔离性报告
  - 验证:测试隔离性良好

- [x] SubTask 5.6:测试代码质量评估
  - 检查无过度 mock(Week 5 应无 mock,使用真实组件)
  - 检查无脆弱断言(避免精确数值断言,使用范围断言)
  - 检查 min-of-N 减少噪声(CSA 延迟测试)
  - 输出:测试代码质量报告
  - 验证:测试代码质量良好
  - **核验通过(Task 9)**:Week 5 测试使用真实组件(无 mock);断言使用范围/边界检查(如 ahirt.rs:1107 `detection_rate > 0.95`);CSA 延迟测试用 min-of-N 5 次减噪(quest-engine/tests/e2e.rs:473 test_e2e_performance_benchmarks);error_paths.rs 用 match 模式匹配而非 unwrap_err(见 e2e.rs:373)。

- [ ] SubTask 5.7:E2E 测试覆盖度评估(**结转 Week 7**)
  - 验证 Quest→TTG→DECB→Parliament→Skeptic→ASA→AHIRT 全链路
  - 验证 5 个 E2E 测试用例(良性/恶意/降级/AHIRT/ASA)
  - 识别未覆盖的 E2E 场景
  - 输出:E2E 覆盖度报告
  - 验证:关键 E2E 场景已覆盖
  - **核验结果(Task 9)**:parliament/tests/e2e.rs 有 5 个 E2E 测试用例(test_e2e_benign_quest_consensus / test_e2e_malicious_quest_vetoed / test_e2e_budget_degradation_flow / test_e2e_ahirt_security_audit / test_e2e_asa_intervention_block)✓;但事件流 E2E 缺失(无 subscribe 验证事件发布链路);DegradedModeRejected 仅在 decb-governor/tests/error_paths.rs:121-128 有单元测试,无 E2E 覆盖。结转 Week 7 补齐。

- [x] SubTask 5.8:安全免疫测试载荷多样性评估
  - 验证 4 类攻击各 25 个载荷(共 100 个)
  - 评估载荷覆盖典型变体(命令注入 6 种/提示注入 6 类/权限提升 5 种/沙箱逃逸 5 种)
  - 识别未覆盖的攻击变体
  - 输出:载荷多样性报告
  - 验证:载荷覆盖典型变体
  - **核验通过(Task 9)**:parliament/tests/security_immunity.rs:63-218 含 4 类 × 25 个 = 100 个载荷(命令注入 25 个第 66 行 / 提示注入 25 个第 102 行 / 权限提升 25 个第 139 行 / 沙箱逃逸 25 个第 176 行),覆盖典型变体,与 CHANGELOG 声称一致。

- [x] SubTask 5.9:CSA 延迟测试稳定性评估
  - 验证 min-of-N 5 次减少噪声
  - 验证标记 #[ignore = "perf: run with --ignored"]
  - 验证延迟分解(TTG 1ms + DECB 1ms + Parliament 200ms + Skeptic 10ms + ASA 5ms + AHIRT 50ms + 事件 30ms)
  - 输出:CSA 延迟测试稳定性报告
  - 验证:CSA 延迟测试稳定可靠

---

## Task 6:文档同步审计(维度 F,P0)

验证 Week 5 文档与代码的一致性。

- [x] SubTask 6.1:CODE_WIKI.md 一致性检查
  - 验证 parliament 模块说明与实际实现一致(5 角色/Skeptic/AHIRT)
  - 验证 decb-governor 模块说明与实际实现一致(双档预算/溢出降级)
  - 验证 seccore(ASA)模块说明与实际实现一致(对抗审计/干预分级)
  - 验证 quest-engine(TTG)模块说明与实际实现一致(思考切换/预算联动)
  - 输出:CODE_WIKI 一致性清单
  - 验证:4 个模块说明与实现一致

- [ ] SubTask 6.2:CHANGELOG.md 对应性检查(**结转 Week 7**)
  - 验证 Week 5 章节记录与实际功能对应
  - 验证性能指标(7 项)与实测一致
  - 验证安全免疫率(100%)与实测一致
  - 验证测试统计(2023)与实际一致
  - 输出:CHANGELOG 对应性报告
  - 验证:Week 5 章节准确反映实现
  - **核验结果(Task 9)**:CHANGELOG.md:8-136 Week 5 章节存在;安全免疫率 100%(第 100 行)与实测一致 ✓;测试统计 2023(第 105 行)与实测一致 ✓;性能指标(第 82-90 行)有基准值但实测仅标 ✓(未给具体 ms 数,部分不准确);第 76-78 行声称"新增事件类型(9 个)"但实际仅 8/9 已发布(RoleRegistered 未发布,roles.rs:124 TODO)。结转 Week 7 修正 CHANGELOG 事件集成声明与性能指标实测数值。

- [x] SubTask 6.3:lib.rs 文档注释一致性
  - 验证 parliament/src/lib.rs 文档注释与模块功能一致
  - 验证 decb-governor/src/lib.rs 文档注释与模块功能一致
  - 验证 seccore/src/lib.rs 文档注释包含 ASA 模块
  - 验证 quest-engine/src/lib.rs 文档注释包含 TTG 模块
  - 输出:lib.rs 文档注释清单
  - 验证:文档注释与实现一致

- [ ] SubTask 6.4:spec 文档状态一致性(**结转 Week 7**)
  - 验证 week5-parliament-security-budget/spec.md 与实现一致
  - 验证 week5-parliament-security-budget/tasks.md 所有 Task 已勾选
  - 验证 week5-parliament-security-budget/checklist.md 所有检查项已勾选
  - 输出:spec 文档状态报告
  - 验证:spec 文档与实现状态一致
  - **核验结果(Task 9)**:week5-parliament-security-budget/tasks.md Task 30-37 全勾选 ✓;checklist.md 第 17 行"30.2 角色注册后发布 RoleRegistered 事件"已勾选,但实际 roles.rs:124 仅 TODO 注释 + tracing 日志,**事件未实际发布** - 状态不一致;checklist.md 188+ 项全勾但 RoleRegistered 发布未实现。结转 Week 7 修正 spec 文档状态(取消 30.2 RoleRegistered 勾选或补齐发布实现)。

- [x] SubTask 6.5:project_memory.md 时效性
  - 验证 Week 5 经验教训的时效性(10 条)
  - 验证经验教训与实际遇到的问题对应
  - 识别需更新的经验教训
  - 输出:project_memory 时效性报告
  - 验证:经验教训准确且有时效性

- [x] SubTask 6.6:关键 API 文档完整性
  - 验证 Parliament::deliberate API 文档(参数/返回值/异常)
  - 验证 DecbGovernor::compute_budget API 文档
  - 验证 TtgGovernor::select_mode API 文档
  - 验证 AsaAuditor::audit API 文档
  - 验证 AhirtRedTeam::probe API 文档
  - 输出:API 文档完整性清单
  - 验证:关键 API 文档完整

---

## Task 7:汇总报告与修复建议(P0)

汇总 6 个维度的发现,生成最终复审报告。

- [x] SubTask 7.1:跨维度问题汇总
  - 汇总 Task 1-6 的所有发现
  - 按维度分类整理
  - 输出:跨维度问题汇总表
  - 验证:无遗漏
  - **核验通过(Task 9)**:checklist.md:106-117 "验收签字"表格已按 6 维度汇总(Task 1-7 各维度 PASS 数与 Critical/Major/Minor 问题);checklist.md:119-120 "总体状态"汇总 35/55 通过、2 项 Critical。无遗漏。

- [x] SubTask 7.2:优先级排序(Critical/Major/Minor)
  - 对所有问题按严重程度分级
  - Critical:阻塞 Week 6 启动的问题
  - Major:影响功能正确性或性能的问题
  - Minor:代码质量或文档问题
  - 输出:优先级排序清单
  - 验证:分级合理
  - **核验通过(Task 9)**:checklist.md 各 SubTask 的 FAIL 标注已含分级(Critical:8 孤儿事件/BudgetExceeded 未标 Critical;Major:TTG 直接依赖;Minor:ttg.rs 7 处 expect/文档失同步);验收签字表格列明各维度 Critical/Major/Minor 计数。分级合理。

- [x] SubTask 7.3:修复成本评估(S/M/L)
  - S(Small):< 1 小时修复
  - M(Medium):1-4 小时修复
  - L(Large):> 4 小时修复
  - 输出:修复成本评估表
  - 验证:成本评估合理

- [x] SubTask 7.4:Week 6 前置条件确认
  - 确认无 Critical 级问题(或已制定修复计划)
  - 确认 Week 5 代码健康度满足 Week 6 启动条件
  - 输出:Week 6 前置条件确认报告
  - 验证:Week 6 可启动

---

## Task Dependencies

- Task 1(架构审计)→ 无依赖,优先执行
- Task 2(集成审计)→ 无依赖,可与 Task 1 并行
- Task 3(技术债审计)→ 无依赖,可与 Task 1/2 并行
- Task 4(并发审计)→ 无依赖,可与 Task 1/2/3 并行
- Task 5(测试审计)→ 依赖 Task 1-4 输出(参考架构/集成/技术债/并发的发现)
- Task 6(文档审计)→ 依赖 Task 1-4 输出(参考架构/集成/技术债/并发的发现)
- Task 7(汇总报告)→ 依赖 Task 1-6 全部完成

## 优先级执行顺序

1. **第一批(并行)**:Task 1(架构)+ Task 2(集成)+ Task 3(技术债)+ Task 4(并发)
2. **第二批(并行)**:Task 5(测试)+ Task 6(文档)
3. **第三批**:Task 7(汇总报告)

## 关键路径

Task 1-4(并行)→ Task 5-6(并行)→ Task 7(汇总)

复审深度取决于 Task 1-4 的发现质量,需优先保证前四者的证据充分性。

## WBS 工作分解结构

```
Week 5 深度复审交付物
├── Task 1: 架构一致性审计 (维度 A)
│   ├── 1.1 角色权重一致性
│   ├── 1.2 依赖方向合规性
│   ├── 1.3 OMEGA 四定律体现
│   ├── 1.4 跨层通信合规性
│   ├── 1.5 命名模式与类型安全
│   └── 1.6 forbid(unsafe_code) 覆盖
├── Task 2: 跨层集成审计 (维度 B)
│   ├── 2.1 事件字段完整性
│   ├── 2.2 source 对应关系
│   ├── 2.3 Critical 事件背压
│   ├── 2.4 AsaIntervention severity
│   ├── 2.5 发布/订阅配对
│   ├── 2.6 向后兼容性
│   ├── 2.7 BudgetAdjusted vs BudgetExceeded
│   └── 2.8 跨层 API 类型安全
├── Task 3: 技术债与代码质量审计 (维度 C)
│   ├── 3.1 伪实现/桩函数
│   ├── 3.2 硬编码常量
│   ├── 3.3 错误处理一致性
│   ├── 3.4 注释完整性
│   ├── 3.5 函数长度合规性
│   ├── 3.6 模块组织规范性
│   ├── 3.7 unwrap()/expect() 合规性
│   └── 3.8 Box<dyn Trait> avoidance
├── Task 4: 并发与性能审计 (维度 D)
│   ├── 4.1 锁竞争热点
│   ├── 4.2 死锁风险
│   ├── 4.3 async 正确性
│   ├── 4.4 分配热点
│   ├── 4.5 算法复杂度
│   ├── 4.6 并发原语选择
│   ├── 4.7 check-then-act 原子化
│   └── 4.8 滞后机制实现
├── Task 5: 测试覆盖盲区审计 (维度 E)
│   ├── 5.1 错误路径覆盖率
│   ├── 5.2 边界条件测试
│   ├── 5.3 集成测试缺口
│   ├── 5.4 proptest 充分性
│   ├── 5.5 测试隔离性
│   ├── 5.6 测试代码质量
│   ├── 5.7 E2E 覆盖度
│   ├── 5.8 载荷多样性
│   └── 5.9 CSA 延迟稳定性
├── Task 6: 文档同步审计 (维度 F)
│   ├── 6.1 CODE_WIKI 一致性
│   ├── 6.2 CHANGELOG 对应性
│   ├── 6.3 lib.rs 文档注释
│   ├── 6.4 spec 文档状态
│   ├── 6.5 project_memory 时效性
│   └── 6.6 API 文档完整性
└── Task 7: 汇总报告与修复建议
    ├── 7.1 跨维度问题汇总
    ├── 7.2 优先级排序
    ├── 7.3 修复成本评估
    └── 7.4 Week 6 前置条件确认
```
