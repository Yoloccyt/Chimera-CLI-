# Week 5 深度复审 Spec

> **复审目标**:对 Week 5 已实现的 7 个 crate(含 4 个新实现 + 3 个扩展)进行跨周横向深度审计,识别架构漂移、跨层集成缺陷、隐式技术债、并发安全隐患、性能瓶颈、测试盲区与文档失同步问题,为 Week 6(L2+L10 适配 + 进化 + 多模态)的健康推进奠定基础。

---

## 1. 复审范围

### 1.1 Week 5 新实现/扩展 crate 清单(7 个,按层级分组)

| 层级 | Crate | 类型 | 主要内容 | 源码估算 |
|------|-------|------|---------|---------|
| L1 | event-bus | 扩展 | 新增 8 个事件类型 + 复用 1 个(ThinkingModeSwitched 扩展 reason 字段) | ~300 行新增 |
| L1 | nexus-core | 扩展 | path_util.rs 测试竞态修复(提取内部函数参数化注入) | ~30 行修改 |
| L3 | decb-governor | 新实现 | 双档预算治理(BudgetTier/DecbGovernor/OverflowDetector) | ~1500 行 |
| L4 | seccore | 扩展 | ASA 对抗性自我审计(AsaAuditor/AsaSandboxCoordinator) | ~800 行新增 |
| L4 | qeep-protocol | 扩展 | 测试加固(20→40 个测试,覆盖超时/孤儿/并发/边界) | ~600 行测试新增 |
| L8 | parliament | 新实现 | 5 角色议会 + Skeptic 否决 + AHIRT 红队 + DpoPairGenerator | ~3500 行 |
| L9 | quest-engine | 扩展 | TTG 思考切换治理(TtgGovernor/复杂度评估/预算联动/手动覆盖) | ~1000 行新增 |

**合计**:~7700 行新增/修改源码

### 1.2 复审范围边界

**纳入复审**:
- Week 5 新实现的全部源码(parliament、decb-governor)
- Week 5 扩展的增量代码(seccore/asa.rs、quest-engine/ttg.rs、event-bus 新事件、qeep-protocol 新测试)
- Week 5 新增的测试文件(E2E/CSA/安全免疫/proptest/error_paths)
- Week 5 更新的文档(CHANGELOG/CODE_WIKI/project_memory)

**不纳入复审**(已在 Week 1-4 横向复审中覆盖):
- Week 1-4 已实现的源码(仅作为依赖参考)
- 未变更的骨架 crate

### 1.3 基线指标(Week 5 验收时已验证)

| 指标 | 当前值 | 状态 |
|------|--------|------|
| 全量测试通过 | 2023 passed / 0 failed | ✅ |
| cargo clippy 零警告 | 0 warnings | ✅ |
| cargo build --release | 通过 | ✅ |
| `#![forbid(unsafe_code)]` 覆盖率 | 35/35 crate | ✅ |
| 安全免疫率 | 100%(100 载荷) | ✅ |
| CSA 端到端延迟 | < 300ms | ✅ |
| qeep-protocol 测试数 | 40(≥20 要求) | ✅ |

---

## 2. 复审维度与验收标准

### 2.1 维度 A:架构一致性审计

**目标**:验证 Week 5 实现与 `AETHER_NEXUS_OMEGA_ULTIMATE.md` 设计文档的一致性。

**检查项**:
- A1: Parliament 5 角色权重(0.25/0.30/0.20/0.15/0.10)与设计文档一致性
- A2: 十层架构依赖方向规则(L8→L4 向下允许,L8→L9 向上禁止,通过 EventBus 解耦)
- A3: OMEGA 四定律在 Week 5 代码中的体现(Ω-Event 事件驱动、Ω-Evolve 对抗进化)
- A4: 跨层通信合规性(Parliament→GSOE/AutoDPO 通过 ConsensusReached 事件,AHIRT→Decay 通过 RedTeamAudit 事件)
- A5: 命名模式(*Governor/*Engine/*Router/*Protocol/*Registry/*RuleBook)的规范遵循
- A6: newtype 类型安全(RoleId/BudgetCoefficient/ComplexityScore 等 ID 类型使用 newtype)
- A7: `#![forbid(unsafe_code)]` 和 `#![warn(missing_docs, clippy::all)]` 在所有新 crate 的覆盖

**验收标准**:列出所有架构漂移点,按严重程度分级(Critical/Major/Minor)。

### 2.2 维度 B:跨层集成审计

**目标**:验证 EventBus 事件契约的完整性与 Week 5 新增事件的跨层通信正确性。

**检查项**:
- B1: Week 5 新增 9 个事件类型(DebateStarted/SkepticVeto/RedTeamAudit/ThinkingModeSwitched 扩展/BudgetAdjusted/AsaIntervention/AhirtProbeCompleted/RoleRegistered/BudgetStatsReported)的字段完整性
- B2: 每个新事件的 source 字段与发布者 crate 的对应关系
- B3: Critical 事件(SkepticVeto/RedTeamAudit)的背压保护策略(是否使用 mpsc 通道保证投递)
- B4: AsaIntervention 事件的 severity 静态判定问题(统一 Normal,Block 级别应通过 Critical 通道发送)
- B5: 事件发布与订阅的配对完整性(无孤儿事件、无幽灵订阅)
- B6: ThinkingModeSwitched 向后兼容性(#[serde(default)] reason 字段)
- B7: BudgetAdjusted 与 BudgetExceeded 的语义区分(档位切换 vs 超限告警)
- B8: 跨层 API 边界的类型安全(无 String 弱类型传递关键 ID)

**验收标准**:事件流图完整,无孤儿事件,无类型逃逸,向后兼容性验证通过。

### 2.3 维度 C:技术债与代码质量审计

**目标**:识别 Week 5 代码中的隐式技术债和代码质量问题。

**检查项**:
- C1: 伪实现/桩函数识别(ASA 评分模型占位、Opinion 生成规则化、AHIRT 探测的占位逻辑)
- C2: 硬编码常量(应配置化的阈值、超时、容量,如 Parliament 5 秒超时、DECB 10 秒滞后)
- C3: 错误处理一致性(anyhow vs thiserror 的使用边界、错误传播链完整性)
- C4: 注释完整性(WHY 注释覆盖隐藏约束、变通方案、反直觉行为)
- C5: 函数长度合规性(≤200 行规则)
- C6: 模块组织规范性(lib.rs → types.rs → config.rs → error.rs → 功能模块)
- C7: unwrap()/expect() 使用合规性(非测试代码禁用,锁中毒场景使用 unwrap_or_else)
- C8: Box<dyn Trait> 使用 avoidance(优先 impl Trait 或 enum dispatch)

**验收标准**:技术债清单按修复成本(S/M/L)分级,代码质量评分≥4/5。

### 2.4 维度 D:并发与性能审计

**目标**:识别 Week 5 代码中的并发安全隐患和性能瓶颈。

**检查项**:
- D1: 锁竞争热点(RoleRegistry RwLock、VoteCounter、DecbGovernor Mutex、TtgGovernor Mutex)
- D2: 死锁风险(锁顺序、async 持锁、跨锁引用)
- D3: async 正确性(Parliament::deliberate 的 FuturesUnordered、5 秒超时、Skeptic 辩论前否决)
- D4: 分配热点(不必要的 Vec/HashMap 分配、热路径上的 clone)
- D5: 算法复杂度(MaliciousIntentRuleBook 规则匹配、AHIRT 载荷库查询、DECB 预算计算)
- D6: 并发原语选择(FuturesUnordered vs join_all、mpsc vs broadcast)
- D7: check-then-act 模式原子化(DecbGovernor 档位判定与切换、TtgGovernor 模式选择)
- D8: 滞后机制实现(DECB 档位切换 10 秒滞后、TTG 联动切换 10 秒滞后)

**验收标准**:无死锁风险,无 async 正确性缺陷,性能热点清单可量化。

### 2.5 维度 E:测试覆盖盲区审计

**目标**:识别 Week 5 测试覆盖的盲区与薄弱环节。

**检查项**:
- E1: 错误路径测试覆盖率(ParliamentError/DecbError/QuestError/SecCoreError 的每个变体)
- E2: 边界条件测试(空提案、0 票、全弃权、预算系数 0.0/1.0、复杂度 0)
- E3: 集成测试缺口(Parliament ↔ DECB ↔ TTG 联动、AHIRT ↔ SecCore 协同闭环)
- E4: proptest 不变量验证的充分性(加权赞成率、预算系数、复杂度评分、safety_score、探测率)
- E5: 测试隔离性(无共享状态、无顺序依赖、无环境变量竞态)
- E6: 测试代码质量(无过度 mock、无脆弱断言、min-of-N 减少噪声)
- E7: E2E 测试覆盖度(Quest→TTG→DECB→Parliament→Skeptic→ASA→AHIRT 全链路)
- E8: 安全免疫测试载荷多样性(4 类各 25 个,覆盖典型变体)
- E9: CSA 延迟测试的稳定性(min-of-N 5 次,标记 #[ignore])

**验收标准**:盲区清单按风险分级,关键路径 100% 覆盖。

### 2.6 维度 F:文档同步审计

**目标**:验证 Week 5 文档与代码的一致性。

**检查项**:
- F1: CODE_WIKI.md 新增 4 个模块说明(parliament/decb-governor/seccore-ASA/quest-engine-TTG)与实际实现一致
- F2: CHANGELOG.md Week 5 章节与实际功能对应
- F3: crate 级 lib.rs 文档注释与模块功能一致
- F4: spec 文档(spec.md/tasks.md/checklist.md)与实现状态一致
- F5: project_memory.md Week 5 经验教训的时效性与准确性
- F6: 关键 API 文档(Parliament::deliberate/DecbGovernor::compute_budget/TtgGovernor::select_mode/AsaAuditor::audit/AhirtRedTeam::probe)

**验收标准**:文档失同步点清单,关键差异≤3 个。

---

## 3. 团队组建与职责分配

### 3.1 专家团队(6 名子代理)

| 角色 | 资质要求 | 负责维度 | 验收产出 |
|------|---------|---------|---------|
| 架构师 | 10 年+ 系统架构经验,熟悉 Rust workspace 与事件驱动架构 | 维度 A | 架构漂移清单 |
| 集成专家 | 10 年+ 分布式系统集成经验,熟悉事件总线与跨层通信 | 维度 B | 事件流图与缺陷清单 |
| 代码质量专家 | 10 年+ 代码审计经验,熟悉 SOLID/DRY 与技术债管理 | 维度 C | 技术债分级清单 |
| 并发专家 | 10 年+ 并发编程经验,熟悉 Tokio async 与锁竞争分析 | 维度 D | 并发风险与性能热点清单 |
| 测试专家 | 10 年+ 测试工程经验,熟悉 TDD/proptest/边界测试 | 维度 E | 盲区清单与补测建议 |
| 文档专家 | 10 年+ 技术文档经验,熟悉文档同步与一致性审计 | 维度 F | 文档失同步清单 |

### 3.2 协作机制

- **并行执行**:维度 A/B/C/D 可并行(独立检查路径),E/F 依赖前四者输出
- **结构化思考**:每个维度采用"假设→验证→结论"流程
- **充分探讨**:发现跨维度问题时,触发跨专家会审
- **严谨验证**:所有结论必须有代码证据(文件:行号)

### 3.3 RACI 责任矩阵

| Task | 架构师 | 集成专家 | 代码质量 | 并发专家 | 测试专家 | 文档专家 |
|------|--------|---------|---------|---------|---------|---------|
| Task 1 架构审计 | R/A | C | C | I | I | I |
| Task 2 集成审计 | C | R/A | I | C | I | I |
| Task 3 技术债审计 | C | I | R/A | C | I | I |
| Task 4 并发审计 | I | C | C | R/A | C | I |
| Task 5 测试审计 | I | I | C | I | R/A | C |
| Task 6 文档审计 | I | I | I | I | C | R/A |
| Task 7 汇总报告 | C | C | C | C | C | C |

> R=Responsible, A=Accountable, C=Consulted, I=Informed

---

## 4. 任务分解

### Task 1:架构一致性审计(维度 A)
- 1.1 Parliament 5 角色权重与设计文档一致性检查
- 1.2 十层依赖方向合规性验证(L8→L4/L3 允许,L8→L9 禁止)
- 1.3 OMEGA 四定律(Ω-Event/Ω-Evolve)在 Week 5 代码中的体现审计
- 1.4 跨层通信合规性(ConsensusReached/RedTeamAudit 事件解耦)
- 1.5 命名模式与新类型安全审计
- 1.6 `#![forbid(unsafe_code)]` 覆盖验证

### Task 2:跨层集成审计(维度 B)
- 2.1 Week 5 新增 9 个事件类型字段完整性验证
- 2.2 事件 source 字段与发布者对应关系验证
- 2.3 Critical 事件(SkepticVeto/RedTeamAudit)背压保护审计
- 2.4 AsaIntervention severity 静态判定问题评估
- 2.5 事件发布/订阅配对完整性
- 2.6 ThinkingModeSwitched 向后兼容性验证
- 2.7 BudgetAdjusted vs BudgetExceeded 语义区分验证
- 2.8 跨层 API 类型安全审计

### Task 3:技术债与代码质量审计(维度 C)
- 3.1 伪实现/桩函数识别(ASA 评分/Opinion 生成/AHIRT 探测)
- 3.2 硬编码常量盘点(超时/阈值/容量)
- 3.3 错误处理一致性审计
- 3.4 注释完整性评估(WHY 注释)
- 3.5 函数长度合规性检查(≤200 行)
- 3.6 模块组织规范性检查
- 3.7 unwrap()/expect() 使用合规性
- 3.8 Box<dyn Trait> 使用 avoidance

### Task 4:并发与性能审计(维度 D)
- 4.1 锁竞争热点分析
- 4.2 死锁风险评估(锁顺序/async 持锁/跨锁)
- 4.3 async 正确性验证(FuturesUnordered/超时/否决)
- 4.4 分配热点识别
- 4.5 算法复杂度审计
- 4.6 并发原语选择评估
- 4.7 check-then-act 原子化验证
- 4.8 滞后机制实现审计

### Task 5:测试覆盖盲区审计(维度 E)
- 5.1 错误路径覆盖率分析(每个 Error 变体)
- 5.2 边界条件测试检查(空/0/最大值)
- 5.3 集成测试缺口识别
- 5.4 proptest 充分性评估
- 5.5 测试隔离性验证(无环境变量竞态)
- 5.6 测试代码质量评估
- 5.7 E2E 测试覆盖度评估
- 5.8 安全免疫测试载荷多样性评估
- 5.9 CSA 延迟测试稳定性评估

### Task 6:文档同步审计(维度 F)
- 6.1 CODE_WIKI.md 一致性检查
- 6.2 CHANGELOG.md 对应性检查
- 6.3 lib.rs 文档注释一致性
- 6.4 spec 文档状态一致性
- 6.5 project_memory.md 时效性
- 6.6 关键 API 文档完整性

### Task 7:汇总报告与修复建议
- 7.1 跨维度问题汇总
- 7.2 优先级排序(Critical/Major/Minor)
- 7.3 修复成本评估(S/M/L)
- 7.4 Week 6 前置条件确认

---

## 5. 执行原则

1. **证据驱动**:所有结论必须引用具体代码位置(文件:行号)
2. **长期主义**:关注架构健康度,而非短期修复
3. **分布式分析**:各专家独立检查,定期同步发现
4. **多轮验证**:初步发现→交叉验证→最终确认
5. **质量优先**:宁可多花时间,不放过潜在风险

---

## 6. 验收标准

### 6.1 复审完成条件

- [ ] 6 个维度全部完成检查
- [ ] 每个维度产出结构化清单
- [ ] 跨维度问题完成会审
- [ ] 汇总报告生成
- [ ] 优先级修复建议明确
- [ ] Week 6 前置条件确认

### 6.2 质量要求

- 所有发现可追溯到代码证据
- 无误报(False Positive)
- Critical 级问题零遗漏
- 修复建议可操作(具体到文件和行号)

---

## 7. 风险评估

| 风险 | 可能性 | 影响 | 应对措施 |
|------|--------|------|---------|
| 伪实现过多导致复审范围扩大 | 中 | 中 | 优先识别伪实现,标注 TODO(Week 6/8) |
| 跨层集成缺陷修复成本高 | 低 | 高 | 仅识别不修复,记录到 Week 6 任务 |
| 并发问题难以复现 | 中 | 高 | 静态分析为主,动态测试为辅 |
| 文档失同步范围扩大 | 低 | 低 | 以代码为准,文档差异记录到修复清单 |

---

## 8. 参考文献

- `AETHER_NEXUS_OMEGA_ULTIMATE.md` §5.2 数据流参考
- `AETHER_NEXUS_OMEGA_ULTIMATE.md` §10.1 核心领域类型
- `AETHER_NEXUS_OMEGA_ULTIMATE.md` §10.3 ADR 决策记录
- `.trae/specs/week1-4-cross-review/spec.md` Week 1-4 横向复审参考
- `.trae/specs/week5-parliament-security-budget/spec.md` Week 5 实现规范
- `CODE_WIKI.md` 代码 Wiki
- `CHANGELOG.md` 变更日志
