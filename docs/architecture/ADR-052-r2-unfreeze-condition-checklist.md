# ADR-052: R2 解冻条件确认清单(草案,不解冻)

- **状态**: 草案(Proposed)— **本 ADR 不执行解冻**,仅确认解冻前置条件的达成度
- **日期**: 2026-07-29
- **关联**: ADR-042(R2 冻结)/ ADR-047(FormalVerifier 路线图)/ Phase 8 实施计划
- **决策者**: 待三方评审(架构 E01 / 安全 E02 / 认知 E06)

---

## 背景

ADR-042 将 R2(GSOE×AutoDPO 约束 RL,`GsoeAutoDpoRL`)**无条件冻结**,
解冻需三阶递进(决策 3):

1. **阶段①**:FormalVerifier 落地,满足 5 项判定标准
2. **阶段②**:新建解冻 ADR 三方评审(即本 ADR)
3. **阶段③**:影子模式运行 2 周,无异常方可正式解冻

本 ADR 是阶段②的**条件确认清单**,盘点阶段①达成度,给出是否进入阶段③的建议。
**本 ADR 明确不解冻**——CI 的 5 关键词扫描(`constrained_rl` / `r2_policy` /
`train_r2` / `GsoeAutoDpoRL` / `evolve_with_constrained_rl`)继续生效。

## 阶段① 五项判定标准达成度盘点

> 依据 ADR-042 决策 3 定义的 5 项标准,逐项核对 FormalVerifier M0/M1/M2 交付。

| # | 判定标准 | 达成载体 | 状态 |
|---|---------|---------|------|
| 1 | 进化谱系有形式化完整性保证 | M0 属性 #1 LineageChecker(DAG 无环 + 回滚可达) | ✅ 已落地 |
| 2 | 评分/奖励有防黑客单调性保证 | M0 属性 #2 CriticMonotonicityChecker | ✅ 已落地 |
| 3 | 偏好学习有一致性保证 | M1 属性 #3 PreferenceConsistencyChecker(偏好对反对称) | ✅ 已落地 |
| 4 | 学习行为有单调性 + 后悔有界保证 | M1 属性 #5 LearningMonotonicityChecker | ✅ 已落地 |
| 5 | 安全层 + 全系统不变量有形式化保证 | M2 属性 #6 DecayConsistencyChecker(衰减一致性)+ 属性 #7 InvariantClosureChecker(全系统传递闭包,含 INV-7/8/9 + UNLEARNABLE 终端锚点)+ 采集接线闭环(InvariantHealthReporter) | ✅ 已落地 |

**阶段① 结论**: **5/5 判定标准已由 FormalVerifier M0/M1/M2 落地覆盖**。
属性矩阵 7/7 完成,含事件因果一致性(M1 属性 #4)作为额外保证。

## 待办(进入阶段③ 前必须补齐)

阶段① 判定标准虽全覆盖,但**以下工程前置在进入阶段③(影子模式)前仍需补齐**:

1. **后悔率自动采集管线**:M1 LearningMonotonicityChecker 验证后悔非增,但生产
   环境的后悔率**自动采集 + 持续监控管线**尚未接线(当前靠离线 bench 快照)。
   影子模式需要实时后悔率信号判定异常。
   > ✅ **已落地(2026-07-29,commit f42e072)**:`omega-learner/src/regret_pipeline.rs`
   > `RegretCollector` — 滑动窗口聚合后悔率观测,`assess_trend()` 复用
   > `verify_regret_non_increasing` 产出 `VerificationResult`,喂给前置 3 熔断器
   > (前置1→前置3 信号链闭合);11 单测 + 7 E2E 全绿;record 31ns/assess 92ns SLO 达标。
   > 剩余:与真实学习循环的 regret 观测埋点接线(待阶段③ 启动时)。
2. **验证器 CI 门禁化**:7 个验证器目前经 E2E 触发;需接入 CI 作为**每次
   进化提交的强制门**(而非仅测试),确保解冻后每次 RL 更新前自动验证。
   > ✅ **已落地(2026-07-29,commit 138b3a5)**:`gsoe-evolution/src/formal_gate.rs`
   > `FormalVerifierGate` — 聚合 7 验证器 VerificationResult 为 CiGateResult,复用既有
   > CI 门禁基础设施(新增 `CiFailureKind::FormalPropertyViolated`);消费切片规避
   > L5→L6 向上依赖;10 单测 + 7 E2E 全绿;evaluate@7 属性 11ns SLO 达标。
   > 剩余:接入 release.yml/CI workflow 作为进化提交强制门(待阶段③ 启动时)。
3. **影子模式回滚开关**:需实现"影子模式期间检测到任一形式化属性 Violated
   即自动回滚 + 冻结"的熔断开关(复用 decay-engine Freeze 语义)。
   > ✅ **已落地(2026-07-29,commit c42e83c)**:`decay-engine/src/shadow_breaker.rs`
   > `ShadowModeCircuitBreaker` — fail-closed 门控状态机,任一 Violated 永久跳闸,
   > 复用 Freeze 不可逆哲学;9 单测 + 8 E2E(消费真实 FormalVerifier 输出)全绿。
   > 剩余:与实际 RL 更新路径的接线(待阶段③ 启动时,不在 R2 冻结期实施)。
4. **解冻范围界定**:明确阶段③ 影子模式的 RL 更新边界(仅 GSOE 变体选择?
   还是含 AutoDPO 偏好更新?),避免一次性放开全部 R2 冻结面。
   > ✅ **已落地(2026-07-29,commit 0f8ae0f)**:`gsoe-evolution/src/unfreeze_scope.rs`
   > `UnfreezeScope` — fail-closed 白名单守卫,把"范围边界"从文档承诺升级为运行时
   > 可强制。`RlUpdateTarget`(GsoeVariantSelection/AutoDpoPreference/SeamPolicy);
   > 默认全冻结,逐个 `allow` 纳入避免一次性放开;11 测试(含 3 proptest)+ 7 E2E 全绿。
   > 与前置 3 熔断器构成完整封套:解冻决策 = 范围内(WHAT)AND 验证通过(WHETHER)。

## 决策

1. **不解冻**:本 ADR 仅确认阶段① 达成,**R2 冻结继续生效**,5 关键词 CI 扫描不动。
2. **阶段① 判定**: **达成**(5/5 标准由 M0/M1/M2 覆盖,7/7 属性)。
3. **阶段②→③ 门槛**:上述 4 项待办**全部补齐**后,另立 ADR-053 启动阶段③
   影子模式,并设定 2 周观察期与量化异常阈值。
   > ✅ **4/4 工程前置全部就位(2026-07-29)**:前置1 后悔率采集(f42e072)+
   > 前置2 CI 门禁(138b3a5)+ 前置3 熔断(c42e83c)+ 前置4 范围守卫(0f8ae0f)
   > 均已代码化落地并全绿。**阶段② 工程前置完成**,但进入阶段③ 仍需:
   > (a) ADR-053 三方评审(架构/安全/认知)正式决策;(b) 各前置与真实 RL 路径的
   > 接线(当前均为独立可测组件,埋点接线不在 R2 冻结期实施);(c) 影子模式 2 周观察。
   > **本 ADR 仍不解冻**——4 前置就位是"可以开始评审阶段③"而非"已解冻"。
4. **本 ADR 关键词规避**:本文档讨论解冻但**不含**任何 5 个扫描关键词的代码标识符
   (仅在背景引用处以反引号包裹说明,非代码),不触发 CI 阻断。

## 后果

**正面**:
- 阶段① 达成度有据可查(5/5),为阶段③ 决策提供依据
- 4 项待办明确了从"形式化保证已就绪"到"可安全解冻"的剩余工程距离

**负面/风险**:
- 4 项待办(尤其后悔率采集管线 + CI 门禁化)工作量不小,阶段③ 仍有距离
- 影子模式回滚开关涉及跨 crate 协同(decay-engine Freeze × RL 更新路径),需谨慎设计

## 附:与 Phase 8 的关系

本 ADR 是 Phase 8.4 的交付物之一。Phase 8 完成 FormalVerifier M2(属性 #6/#7)
+ 采集接线闭环,使阶段① 判定标准 5/5 达成。**Phase 8 不触碰 R2 冻结**,
rl-client / 在线 GRPO 仍推迟至阶段③ 完成后的 v3.x。

---

## 追加事实注记（2026-10-10 / v3.5 C-43；只加注不改写上方盘点）

本 ADR §盘点与 §决策把“`FormalVerifierGate` 聚合 7 验证器”与“阶段① 7/7 属性达成”列为
R2 解冻前置证据。v3.5 本轮把它**逐行定位后机器化**，三项事实需与上面的表述一同阅读：

1. **名册与实装集合不相等（两处，此前只有人读得出来、没有门会拦）**：名册 =
   `nexus-contracts/src/formal_props.rs::EmptyFormalProvider::collect_results()` 里的 7 个 id
   （全库唯一的 src 侧枚举）。实装 checker = 7 个，但**不是同一批 7 个**：
   - `preference-consistency` 在名册里，**全库无 checker**（其宿主 auto-dpo 已随 ADR-172 退役）；
   - `ConsensusSafetyChecker`（`parliament/src/formal/consensus_safety.rs:50`）**有实装但不在名册**。
   即“7/7”应准确读作：**7 个名册位，此中 6 个有验证器；另有 1 个已实装的验证器未被计入。**
2. **门不产生证据，它只聚合调用方传入的结果**：`FormalResultProvider` 全库**只有一个实现**
   （`EmptyFormalProvider`，恒返 Skipped）。因此“聚合门fail-closed”在盘上可保证（无证据即否决），
   但**不存在一个生产侧提供器持续给出 7 项真裁决**；真验证链分散在各 crate 的 formal 模块与 e2e 测试里。
3. **已建门与它的边界**：新增 G-95/G-96（`scripts/check_formal_property_ledger.py`）把上述错配
   固定为**不可扩大且必须可见**：新名册位无 checker 且无登记 ⇒ 红；checker 文件被剔空 ⇒ 红；
   登记失效 ⇒ 红。但本门**不擅自修名册**：换名册内容等于改变本 ADR 与 ADR-053 已签署的 R2
   证据集（“7/7”指哪七项），必须走新 ADR（建议与 `consensus-safety` 入名册同批处理）。

→ 对本 ADR 的使用建议：引用“阶段① 7/7 达成”时，同时给出上述名册/实装差异与 G-95 的现读
（名册 7 / checker 7 / 登记错配 2）；否则会把一个“属性集含一个不可验证位”的名单
当作七项均有验证器的证据使用。

---

> NEXUS-OMEGA · ADR-052 草案 · 2026-07-29 · R2 解冻阶段② 条件确认(不解冻)
