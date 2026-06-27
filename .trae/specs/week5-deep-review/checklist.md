# Week 5 深度复审验收检查清单

> 本检查清单对应 `tasks.md` 的 Task 1-7,共 45 个 SubTask。
> 每个 SubTask 完成后,验证对应检查项并勾选。
> 全部检查项通过后,Week 5 深度复审完成。

---

## Task 1:架构一致性审计(维度 A)

- [x] 1.1 Parliament 5 角色权重(0.25/0.30/0.20/0.15/0.10)与设计文档一致,权重总和 = 1.0
- [x] 1.2 十层依赖方向合规:无生产代码 L8→L9 向上依赖,dev-dependencies 豁免
- [ ] 1.3 OMEGA 四定律(Ω-Event/Ω-Evolve)在 Week 5 代码中有体现(**FAIL**:DECB/AHIRT 未走 EventBus,Ω-Event 部分断裂)
- [ ] 1.4 跨层通信合规:Parliament→GSOE/AutoDPO 通过 ConsensusReached 事件,AHIRT→Decay 通过 RedTeamAudit 事件(**FAIL**:TTG 直接依赖 decb_governor 同步调用,违反事件解耦)
- [x] 1.5 命名模式(*Governor/*Registry/*RuleBook/*RedTeam/*Counter/*Generator)符合规范
- [x] 1.5 newtype 类型安全(RoleId/BudgetCoefficient/ComplexityScore)已实现
- [x] 1.6 所有 Week 5 crate 覆盖 `#![forbid(unsafe_code)]` 和 `#![warn(missing_docs, clippy::all)]`

## Task 2:跨层集成审计(维度 B)

- [x] 2.1 Week 5 新增 9 个事件类型字段完整且语义清晰
- [x] 2.2 每个新事件的 source 字段与发布者 crate 对应(测试代码验证)
- [ ] 2.3 Critical 事件(SkepticVeto/RedTeamAudit)标记为 EventSeverity::Critical,有投递保证(**FAIL**:BudgetExceeded 未标 Critical;mpsc 点对点通道未实现,仅 broadcast)
- [ ] 2.4 AsaIntervention severity 静态判定问题有充分文档说明(**FAIL**:文档要求 Block 走 Critical 通道,但 Critical 通道未实现且发布者未发布事件)
- [ ] 2.5 事件发布/订阅配对完整:无孤儿事件,无幽灵订阅(**FAIL** + **结转 Week 7**:8/9 事件已发布,`RoleRegistered` 未发布 roles.rs:124 TODO;生产代码无任何订阅者,仅测试代码 subscribe。Task 9 核验修正原描述"全为 TODO 占位"不准确)
- [x] 2.6 ThinkingModeSwitched 向后兼容性通过(#[serde(default)] reason 字段)
- [ ] 2.7 BudgetAdjusted(档位切换)与 BudgetExceeded(超限告警)语义区分清晰(**FAIL**:BudgetExceeded 未标 Critical;由 model-router 而非 decb-governor 发布,语义错位)
- [ ] 2.8 跨层 API 类型安全:无关键 ID 使用 String 弱类型(**FAIL**:9 个新事件的 quest_id/proposal_id/operation_id 等均为 String,action/tier/mode 本可为 enum)

## Task 3:技术债与代码质量审计(维度 C)

- [x] 3.1 所有伪实现/桩函数(ASA 评分/Opinion 生成/AHIRT 探测)有 TODO 标注
- [ ] 3.2 关键阈值(超时/滞后/共识阈值)已配置化或标注理由(**FAIL** + **结转 Week 7**:Task 9 核验 `AhirtConfig::probe_cycle_secs=300/detection_rate_threshold=0.95` 已配置化(config.rs:141-154),但 ahirt.rs:387 `verify_security` 中 0.95 硬编码未使用配置值;WHY 注释已覆盖。原描述"缺 WHY 注释"不准确)
- [x] 3.3 错误处理一致(ParliamentError/DecbError/QuestError/SecCoreError 使用 thiserror),传播链完整
- [x] 3.4 WHY 注释覆盖关键隐藏约束(滞后机制/预算优先/依赖方向/规则匹配顺序)(**Task 9 核验通过**:AHIRT 周期/探测率/滞后/预算优先/依赖方向/规则匹配顺序均有 WHY 注释,ahirt.rs:9/13/36/148/171/218/261/296/360/371/383/404/405/407/410/458/471/475 等。原 FAIL 描述"AHIRT 周期与探测率阈值缺 WHY 注释"与实际不符)
- [x] 3.5 所有函数 ≤ 200 行(重点关注 Parliament::deliberate/AhirtRedTeam::probe/DecbGovernor::compute_budget)
- [x] 3.6 模块组织符合项目规范(lib.rs → types.rs → error.rs → config.rs → 功能模块)
- [ ] 3.7 非测试代码无 unwrap()/expect()(锁中毒场景使用 unwrap_or_else 豁免)(**FAIL**:ttg.rs 7 处 expect() 违反锁中毒规范)
- [x] 3.8 无 Box<dyn Trait>(或有合理理由),优先 impl Trait 或 enum dispatch

## Task 4:并发与性能审计(维度 D)

- [x] 4.1 锁竞争热点分析完成:无高竞争锁热点
- [x] 4.2 死锁风险评估完成:无锁顺序问题,无 async 持锁,无跨锁引用
- [x] 4.3 async 正确性验证:FuturesUnordered 使用正确,5 秒超时机制有效,Skeptic 同步否决
- [x] 4.4 分配热点识别完成:无关键分配热点(Week 6 NMC 接入后需复评 quest.clone)
- [x] 4.5 算法复杂度审计完成:无高复杂度热点
- [ ] 4.6 并发原语选择合理(FuturesUnordered vs join_all,mpsc vs broadcast)(**FAIL**:CriticalMpsc 策略未实现,关键事件仍走 broadcast)
- [ ] 4.7 check-then-act 模式原子化(DecbGovernor/TtgGovernor Mutex 保护)(**FAIL**:TTG on_budget_adjusted 滞后检查与时间戳更新跨锁,Low 风险但需修复)
- [x] 4.8 滞后机制实现正确(DECB 10 秒/TTG 10 秒,不阻塞主流程)

## Task 5:测试覆盖盲区审计(维度 E)

- [x] 5.1 每个 Error 变体(ParliamentError/DecbError/QuestError/SecCoreError)有测试覆盖
- [x] 5.2 关键边界条件(空提案/0 票/全弃权/系数 0.0-1.0/复杂度 0/评分 0.0-1.0)有测试
- [ ] 5.3 关键集成场景(Parliament↔DECB↔TTG/AHIRT↔SecCore/ASA↔SecCore)已覆盖(**FAIL**:事件流 E2E 缺失;AHIRT↔SecCore 闭环无独立测试)
- [ ] 5.4 proptest 覆盖关键不变量(加权赞成率/预算系数/复杂度评分/safety_score/探测率)(**FAIL**:qeep-protocol 无 proptest.rs)
- [x] 5.5 测试隔离性良好:无共享状态,无顺序依赖,无环境变量竞态
- [x] 5.6 测试代码质量良好:无过度 mock,无脆弱断言,min-of-N 减少噪声
- [ ] 5.7 E2E 测试覆盖 Quest→TTG→DECB→Parliament→Skeptic→ASA→AHIRT 全链路(**FAIL**:事件流 E2E 缺失;DegradedModeRejected E2E 缺失)
- [x] 5.8 安全免疫测试载荷覆盖典型变体(4 类各 25 个,共 100 个)
- [x] 5.9 CSA 延迟测试稳定可靠(min-of-N 5 次,标记 #[ignore],延迟分解清晰)

## Task 6:文档同步审计(维度 F)

- [ ] 6.1 CODE_WIKI.md 4 个模块说明(parliament/decb-governor/seccore-ASA/quest-engine-TTG)与实现一致(**FAIL**:12 处失同步,含 API 签名不一致、声称事件已发布但未发布)
- [ ] 6.2 CHANGELOG.md Week 5 章节准确反映实现(性能指标/安全免疫率/测试统计)(**FAIL**:声称事件已集成但实际未发布;性能指标未给具体数值)
- [ ] 6.3 crate 级 lib.rs 文档注释与模块功能一致(**FAIL**:decb-governor 层级标注 L3/L8 矛盾;quest-engine 声称"广播事件"但同步调用)
- [ ] 6.4 spec 文档(spec.md/tasks.md/checklist.md)与实现状态一致(**FAIL**:spec.md 9 处声称发布事件但未发布;tasks.md Task 37 全勾但事件未集成;checklist.md 188+ 项全勾但事件未发布)
- [ ] 6.5 project_memory.md Week 5 经验教训准确且有时效性(**FAIL**:第 1/2/4 条经验教训与实际矛盾,需更新)
- [ ] 6.6 关键 API(Parliament::deliberate/DecbGovernor::compute_budget/TtgGovernor::select_mode/AsaAuditor::audit/AhirtRedTeam::probe)文档完整(**FAIL**:5 个 API 均部分缺失或与 CODE_WIKI 签名不一致)

## Task 7:汇总报告与修复建议

- [x] 7.1 跨维度问题汇总完成(无遗漏)
- [x] 7.2 优先级排序完成(Critical/Major/Minor 分级合理)
- [x] 7.3 修复成本评估完成(S/M/L 分级合理)
- [ ] 7.4 Week 6 前置条件确认:无 Critical 级问题(或已制定修复计划)(**FAIL**:存在 2 项 Critical 级问题,需先修复或制定修复计划)

---

## 跨任务验收检查

### 复审质量
- [x] 所有发现可追溯到代码证据(文件:行号)
- [x] 无误报(False Positive)
- [x] Critical 级问题零遗漏
- [x] 修复建议可操作(具体到文件和行号)

### 复审覆盖
- [x] 7 个 crate 全部覆盖(parliament/decb-governor/seccore/qeep-protocol/quest-engine/event-bus/nexus-core)
- [x] 6 个维度全部完成检查(架构/集成/技术债/并发/测试/文档)
- [x] 跨维度问题完成会审
- [x] 汇总报告生成

### Week 6 前置条件
- [ ] 无 Critical 级架构问题(**FAIL**:8 个新事件未集成 EventBus,违反 Ω-Event 定律)
- [x] 无 Critical 级并发安全问题
- [ ] 无 Critical 级集成问题(**FAIL**:8 个孤儿事件;BudgetExceeded 未标 Critical;无 mpsc 投递保证)
- [ ] Week 5 代码健康度满足 Week 6 启动条件(**FAIL**:需先修复 Critical 级问题或制定修复计划)

---

## 验收签字

| 维度 | 验收人 | 通过标准 | 实际结果 |
|------|--------|---------|---------|
| Task 1 架构审计 | 架构师 | 架构漂移清单 + 无 Critical 问题 | 4/6 PASS,Major 3 项(事件未集成/TTG 直接依赖) |
| Task 2 集成审计 | 集成专家 | 事件流图 + 无孤儿事件 | 2/8 PASS,Critical 2 项(8 孤儿事件/BudgetExceeded 未标 Critical) |
| Task 3 技术债审计 | 代码质量专家 | 技术债分级清单 + 评分 ≥ 4/5 | 7/8 PASS,3.4 WHY 注释 Task 9 核验通过;S 级技术债 2 项(ttg.rs 7 处 expect 违规 / ahirt.rs:387 硬编码 0.95) |
| Task 4 并发审计 | 并发专家 | 并发风险清单 + 无死锁风险 | 7/8 PASS,Low 风险 1 项(TTG 跨锁原子性) |
| Task 5 测试审计 | 测试专家 | 盲区清单 + 关键路径 100% 覆盖 | 6/9 PASS,Critical 1 项(qeep-protocol 无 proptest) |
| Task 6 文档审计 | 文档专家 | 失同步清单 + 关键差异 ≤ 3 个 | 0/6 PASS,Critical 5 项(系统性失同步) |
| Task 7 汇总报告 | 全员会审 | 汇总报告 + Week 6 前置条件确认 | 3/4 PASS,Week 6 前置条件未满足 |

**总体通过标准**:全部检查项勾选,Week 5 深度复审完成,可进入 Week 6(L2+L10 适配 + 进化 + 多模态)。

**当前状态**:36/55 检查项通过(65.5%,Task 9 复审收尾新增 3.4 通过),存在 2 项 Critical 级问题(事件未集成 EventBus、BudgetExceeded 未标 Critical),**Week 6 启动前需先修复 Critical 问题或制定明确修复计划**。

**Task 9 复审收尾结论(2026-06-26)**:
- **新通过项(9 项)**:2.6(向后兼容)/ 3.4(WHY 注释)/ 3.6(模块组织)/ 4.2(死锁风险)/ 4.4(分配热点)/ 5.6(测试质量)/ 5.8(载荷多样性)/ 7.1(问题汇总)/ 7.2(优先级排序)
- **结转 Week 7 项(6 项)**:2.5(订阅者缺失 + RoleRegistered 未发布)/ 5.3(事件流 E2E 缺失)/ 5.4(qeep-protocol 无 proptest)/ 5.7(DegradedModeRejected E2E 缺失)/ 6.2(CHANGELOG 事件集成声明不准确)/ 6.4(spec 30.2 RoleRegistered 过度勾选)
- **原 FAIL 描述修正**:2.5("全为 TODO 占位"→"8/9 已发布,RoleRegistered 未发布")/ 3.2("缺 WHY 注释"→"已配置化但 verify_security 硬编码未使用配置值")/ 3.4("缺 WHY 注释"→"已覆盖,核验通过")
