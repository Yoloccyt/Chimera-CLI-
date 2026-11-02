# ADR-054: 三环循环元架构重组设计决策

> **状态**: Proposed; P9-T3+P9-T4 部分落地(2 条生产违规边全部消除) 2026-08-04 (v2.20.0-omega 落档; 待 Phase 9 启动后由治理委员会晋升 Accepted)
> **作者**: superpowers-main 极致深度思考 + staff-engineer-mode 工程决策 + brooks-lint 架构审查 + 8 位虚拟专家分布式分析
> **关联评估**: `docs/architecture/_blueprints/three-ring-reorg/Phase0_评估报告_v2.20.md` (P9-T1 Phase 0 评估, 38 crate 实况)
> **关联计划**: `docs/architecture/IMPLEMENTATION_PLAN_Harness_Engineering_V3.md` §Phase 9 (P9-T1~T12)
> **关联蓝图**: `docs/architecture/_blueprints/three-ring-reorg/三环循环_十层接口_元架构重组深度分析.md` (v1.0, 10 轮推演)
> **依赖**: ADR-033 (L0 nexus-contracts) / ADR-048 (gqep-executor 跨层渗透) / ADR-042 (R2 冻结) / ADR-070 (PROBE 总纲)
> **编号协调**: 本 ADR-054 = Phase 9 三环重组; ADR-053 系列历史文档中"ADR-054(解冻执行)"语义归 Phase 10 = ADR-055 (见 adr_index.md 协调声明)

---

## 1. 背景与问题

当前 38 crate 十层架构是一个**紧耦合的分布式单体**:记忆-推理-进化三个高频循环能力分散在 L2/L5/L8/L9,经 EventBus 跨层通信(12 次序列化跳转 ~3.6ms 可优化成本 + 结构性的 1500ms Parliament 辩论延迟)。v1.7 蓝图提出"内环 9 crate 共享内存 <1ms + 外环 29 crate 事件驱动"目标,但蓝图基于 34 crate 旧基线。

P9-T1 Phase 0 评估(2026-08-04,38 crate 实读)产出三项事实基础:

1. **边界裁定**(评估报告 §5): omega-learner 与 chimera-mas **均留外环**,内环边界维持 9 候选不变;
2. **病理状态**(评估报告 §6.1): D3 GSOE↔Quest 隐式循环**已消除**(事件级断链,保留 oscillation_detector 护栏)、D4"L6 Router 星型"**为蓝图误诊**(实为 L1 底座星型)、D2 跨层渗透 2 条已记录/接受(ADR-048 + optional feature)、D1 上帝 crate 实况为三底座超级节点(event-bus 34 / nexus-core 30 / nexus-contracts 19 依赖方);
3. **破坏面**(评估报告 §4): 生产级违规外环边仅 **2 条**(quest-engine→decb-governor L8、parliament→seccore L4,均为内环→低层外环向下依赖),dev 级 7 条(集中在 gea-activator),受影响上游合计 15,接口提取面已高度消化(类型大多已下沉 L0)。

**核心矛盾**: 三环重组是 v3.0.0-omega 的最大结构变更,必须裁决内环边界、违规边解耦、编译工程组织与性能验证基准,避免"为重组而重组"。

---

## 2. 决策

8 项工程决策(依据 P9-T1 评估报告 + 蓝图 10 轮推演):

### 决策 1: 内环边界 = 9 候选不变,omega-learner / chimera-mas 留外环
- **问题**: 38 crate 中哪些进内环共享内存闭包
- **决定**: 内环 9 crate 不变——**Memory Core**(mlc-engine / hcw-window / nmc-encoder)+ **Reasoning Core**(quest-engine / parliament / gea-activator)+ **Evolution Core**(gsoe-evolution / auto-dpo / repo-wiki)。
  - **omega-learner**(L6)留外环: 低频策略提供者(学习 ≤1Hz / 影子日频),与内环 9 crate **双向零 Cargo 依赖**,仅经 L0 nexus-contracts 契约值注入(PolicySink 回调),现状已是理想接入形态,零改动;
  - **chimera-mas**(L9)留外环: 生产依赖 20 个 workspace crate(含 13 个外环),进内环必违反"内环不能依赖外环";与 quest-engine 零 Cargo 依赖、纯事件归因,已是标准范本。
- **影响**: 内环共享内存 <1ms 目标可达成;冻结两类对外契约(① L0 nexus-contracts 策略契约 SelectorPolicy 族;② 内环 crate 公共 API 面,含 MAS 依赖的 5 处 + hcw-window `with_learner/update_selector_policy`)

### 决策 2: L0/L1 三底座 = 永久外环基础设施
- **问题**: 9 内环 crate **全部**生产依赖 L0/L1 三底座(nexus-contracts / nexus-core / event-bus),"内环不依赖外环"约束字面不成立
- **决定**: nexus-contracts / nexus-core / event-bus 定为**永久外环基础设施**;内环仅禁止依赖 **L2+ 业务外环**;内环经 EventBus/CoreLoopEvent 与三底座通信(依赖铁律 `L(N) → L(0)` 恒允许,ADR-033 已覆盖)
- **影响**: 约束语义精确化;依赖审计脚本范围 = "内环不依赖 L2+ 外环"而非字面"不依赖外环"

### 决策 3: 生产级违规边 2 条解耦方案裁决
- **问题**: quest-engine→decb-governor(L8)、parliament→seccore(L4)是内环独立发布的硬阻断
- **决定**:
  - **quest-engine→decb-governor**: 裁决 **类型上提 L0**——预算裁决策略契约(`BudgetVerdict`/`QuotaDecision` 等)上提 nexus-contracts,quest-engine 依赖 L0 契约而非 L8 crate;
  - **parliament→seccore**: 裁决 **L0 trait 解耦**(2026-08-04 用户裁定,修正原"事件化反转"——AHIRT `probe_command` 为同步 `validate_command` 调用,事件化会破坏同步探测语义,与 ADR-048 同步低延迟教训一致)——`CommandValidator` trait + `Command`/`CommandPolicy`/`AttackType`/`BlockedPattern` 契约类型上提 L0 nexus-contracts(`command_validation` 模块),seccore 实现 `SecCoreCommandValidator`(validate_command 逻辑保留),parliament 改 `Option<Arc<dyn>>` 注入 + 未注入优雅降级(命令探测 skipped)
- **影响**: 2 条硬阻断消解;实现工作量落 P9-T3/T4;此二边解耦优先级高于纯数量排序(带生产违规边 > 无违规边)。**落地状态(2026-08-04)**: quest→decb 类型上提**已落地**——`BudgetTier` 上提 L0 nexus-contracts(`budget_tier` 模块),decb-governor re-export 保兼容,quest-engine 依赖切换至 L0,`cargo tree -e normal` 已无 decb-governor(P9-T3 完成)。**落地状态(2026-08-04,续)**: parliament→seccore L0 trait 解耦**已落地**——`command_validation` 契约上提 L0,seccore `SecCoreCommandValidator` 实现,parliament 生产依赖移除 seccore,`cargo tree -p parliament -e normal` 已无 seccore(P9-T4 完成);**至此 2 条生产违规边全部消除**。

### 决策 4: 内环通信协议 = 三层分层(共享内存 + mpsc + EventBus)
- **问题**: 内环 9 crate 如何通信达成 <1ms
- **决定**: 蓝图 §5.2 三层通信——
  - **同一 Core 内**: 共享内存(`Arc<RwLock<T>>` / `AtomicCell` / DashMap),延迟 <50ns;
  - **Core 之间**: 消息传递(`tokio::sync::mpsc` 或 crossbeam),延迟 <500ns;
  - **内环 ↔ 外环**: EventBus(`CoreLoopEvent` 变体),延迟 <200μs
- **影响**: 硬红线——**禁止持锁跨 await**(§4.4 反模式 1);rusqlite 调用必须 spawn_blocking(内环不直接 IO)

### 决策 5: 编译工程 = 保持单 workspace + 依赖审计脚本约束
- **问题**: 蓝图 §6 主张物理拆分子 workspace(chimera-core-loop / chimera-outer-ring),工程成本与 CI 改造大
- **决定**: **第一阶段保持 38 crate 单 workspace**,用"内环 crate 集合 + 依赖审计脚本"实施约束(检查内环 Cargo.toml 无 L2+ 外环 crate 依赖);物理子 workspace 拆分**推迟**(低收益高成本,38 crate 单 workspace 已成熟,拆分需改造 release.yml/fuzz.yml/CI 全链路)
- **影响**: P9-T10 依赖铁律审计升级为自动脚本(`cargo tree --edges` 穷举 + 内环集合白名单);零 CI 基建改动。**落地状态(2026-08-04)**: 依赖审计脚本已落地——`scripts/check_dependency_rules.{ps1,sh}`(内环边界/向上依赖/完整性 3 类检查 + SelfTest 自测 + 反向验证,实况 38 crate 0 违规;内环允许集=L0/L1 全部基础设施,auto-dpo→model-router 为 L5→L1 向下合法)

### 决策 6: 病理处置状态确认(D3/D4 已消除,任务清单裁剪)
- **问题**: 蓝图 4 类病理哪些还需 Phase 9 治理
- **决定**: P9-T1 复核结论直接采纳——
  - **D3 隐式循环**: 已消除(无需原 P9-T5 事件单向化改造,**裁剪该任务**,保留 oscillation_detector 护栏);
  - **D4 星型耦合**: 蓝图误诊(实为 L1 底座星型,并入 D1 治理);
  - **D2 跨层渗透**: 已合法化(ADR-048 + pvl→seccore optional),三环重组作为根治路径,触发条件按 ADR-048 复审条款;
  - **D1 上帝 crate**: 部分缓解(nexus-contracts L0 已承接 24 个类型模块),继续向 L0 瘦身
- **影响**: Phase 9 任务清单从 12 项收敛(P9-T5 裁剪,原任务映射至决策 3 的 P9-T3 实现)。**落地状态(2026-08-04)**: D1 治理首批下沉完成——8 纯类型上提 L0(nexus-core 5 领域类型→`domain` 模块: ThinkingMode/MultimodalInput/UserIntent/Quest/Task;event-bus 3 载荷类型→`event_payload` 模块: EventSeverity/TaskPriority/AgentStatus),re-export 保兼容;event-bus 的 Quest 引用已切换至 L0(`nexus_contracts::domain::Quest`),但 event-bus→nexus-core 生产依赖边因 `ClvSummary::from_clv` 生产 API 依赖 `nexus_core::clv::CLV`(未下沉 L0)暂保留,待 CLV 下沉后解除;L1 双 god crate 负载显著下降(P9-T7)

### 决策 7: 灰度与兼容策略 = 接口冻结 + 门禁发布
- **问题**: 重组如何保证向后兼容、零停机
- **决定**: 蓝图 §9 策略适配——
  - 接口层**无破坏性变更**(L0/L1/外环公共 API 冻结,冻结清单见决策 1);
  - 内环共享内存化仅在内部实施,对外行为不变;
  - 发布门禁 6 项(见 §4): 全量回归 0 失败 + 依赖铁律全合规 + 内环延迟达标 + binary <50MB + 接口层零破坏 + 4 类病理根治/合法化
- **影响**: 灰度发布(Alpha 内部 → Beta 贡献者 → RC → GA)沿用蓝图 §9.3;feature flag 双轨(v3-engine 模式)可复用 ADR-061 经验

### 决策 8: 性能可证伪 = criterion 四基准 + A/B 对照
- **问题**: 如何证明重组成功(性能证据,拒绝主观判断)
- **决定**: criterion bench 四基准(蓝图 §10.3 适配 v2.20 基线)——
  - **内环循环延迟** <0.01ms(1000 次记忆-推理-进化循环 / 总时间);
  - **跨环事件延迟** <0.2ms;
  - **端到端 Fast** <50ms / **Deep** <1000ms(含 Parliament 辩论);
  - 重组前后 A/B 对照表(内环共址 vs EventBus 跨层),作为 P9-T9 验收证据
- **影响**: 性能可证伪原则(§3.4.1 第 6 条)落地;延迟数据入 CHANGELOG v3.0.0-omega。**落地状态(2026-08-04)**: 重组前基线已采集——内环循环 ≈19.2μs(4 事件链)/ 跨环事件 370.9ns(纯发布)/ entangle 0.41μs;共享内存化后重跑同组基准作 A/B 对照(P9-T9,详见 _blueprints/three-ring-reorg/内环延迟基线报告_v2.20.md)

---

## 3. 实施阶段(Phase 9,2026-09 启动,6 周)

| 阶段 | 任务 | 关键交付物 | 状态 |
|------|------|-----------|------|
| Phase 0 | P9-T1 评估 | `Phase0_评估报告_v2.20.md` (38 crate 依赖图 + 循环检测 + 破坏面 + 边界裁定 + 病理复核) | ✅ 已完成 2026-08-04 |
| 决策 | **P9-T2 本 ADR 落档** | ADR-054 设计决策(8 项) | ✅ 本文件 |
| 内环构建 | P9-T3 接口提取至 L0 | 决策 3 的契约类型上提 + 依赖方向验证 | ⏳ 待启动 |
| 内环构建 | P9-T4 共享内存通道 | 三层通信(决策 4)+ CoreLoopEvent | ⏳ 待启动 |
| 外环治理 | P9-T6/T7 病理处置 | ADR-048 复审 + L1 底座向 L0 瘦身(决策 6) | ⏳ 待启动 |
| 验证 | P9-T8/T9/T10 | 全量回归 + 内环延迟基准(决策 8)+ 依赖审计脚本(决策 5) | ⏳ 待启动 |
| 发布 | P9-T11/T12 | 文档同步 + v3.0.0-omega 发布(决策 7 门禁) | ⏳ 待启动 |

## 4. 发布门禁(Phase 9,沿用 IMPLEMENTATION_PLAN)

- [ ] 内环 9 crate 共享内存延迟 < 1ms(criterion 实证,决策 8)
- [ ] 4 类架构病理全部根治或合法化(决策 6:D3/D4 已消除、D2 已接受、D1 部分缓解 + 决策 3 二边解耦)
- [ ] 全量测试回归 0 失败(8455 passed 基线,新增内环集成测试)
- [ ] 依赖铁律 38 crate 全部合规(审计脚本,决策 5)
- [ ] 接口层无破坏性变更(冻结清单,决策 1/7)
- [ ] Release binary < 50MB(重组后无体积膨胀)

## 5. 验证基线

- 全量测试: **8455 passed / 1 failed / 71 ignored(303 个 test target)**(P9-T1 基线;P9-T3+ 新增内环集成测试后更新)
- clippy: 零警告(`--all-targets -D warnings`)
- fmt: 零差异
- 依赖审计脚本: `cargo tree --edges` 穷举 + 内环集合白名单(决策 5)
- 内环延迟基准: criterion 四基准 A/B 对照(决策 8)
- 零新 crate(38 → 38 不变,决策 2 底座 + 9 内环 + 27 外环结构内)

## 6. 后续路径

- **P9-T3 起代码实现**: 决策 3 二边解耦 + 决策 4 三层通信 + 决策 5 审计脚本
- **Phase 10**: R2 解冻 + Cross-Harness GRPO(v3.1.0-omega,ADR-055;ADR-053 系列"解冻执行 ADR"语义归属此号)
- **长期演进**: v3.2.0-omega Harness Engineering 终极形态

## 7. 关联 ADR

- **前置**: ADR-033 (L0 nexus-contracts) / ADR-042 (R2 冻结) / ADR-048 (gqep 跨层渗透) / ADR-070 (PROBE 总纲)
- **平行**: ADR-063 (L8 协调度量) / ADR-064 (L8 深度优化) / ADR-069 (Token 效率优化)
- **后续**: ADR-055 (Phase 10 R2 解冻执行 + Cross-Harness GRPO) / ADR-071 (召回基准,可作三环重组健康度指标)

---

> **签署**: 待 Phase 9 启动时由治理委员会晋升 Accepted
> **最后更新**: 2026-08-04

---

## 附录: P9-T5 验证记录(2026-08-04)

> **关联**: 决策 6(D3 隐式循环已消除,裁剪原 P9-T5 事件单向化改造,保留 oscillation_detector 护栏)· 本附录为裁剪声明的**实证闭环**记录,完整报告见 `docs/architecture/_blueprints/three-ring-reorg/P9-T5_GSOE_Quest_解耦验证报告_v2.20.md`

### A.1 裁剪确认(决策 6 声明获实证支持)

- 原 P9-T5 计划"通过 EventBus 单向化"解耦 GSOE↔Quest 隐式循环;决策 6 裁定 D3 已消除、**裁剪该任务**。2026-08-04 双维度实证验证了该裁定:**隐式循环在实况中不存在,裁剪合理**。

### A.2 双维度证据摘要

| 维度 | 证据 | 结论 |
|------|------|------|
| **A 编译期**(cargo tree) | `cargo tree -p gsoe-evolution -e normal`(130 行)不含 quest-engine;`cargo tree -p quest-engine -e normal`(126 行)不含 gsoe-evolution | ✅ 双向无对端依赖边 |
| **B 事件边**(grep) | ① gsoe-evolution 生产 subscribe = 0(13 处命中全为测试/注释);② `GsoePolicyUpdated` 生产消费方仅 efficiency-monitor(oscillation_detector.rs:222-234),生产者 gsoe-evolution engine.rs:246;③ quest-engine 全库 `GsoePolicyUpdated\|gsoe` = 0 命中;④ gsoe src 中 `ExecutionCompleted\|ThinkingModeSwitched\|CoordinationRatioReported` = 0 命中 | ✅ 无 gsoe ↔ quest 双向事件边 |
| 审计脚本 | `scripts/check_dependency_rules.ps1` EXITCODE=0,违规 0(不含事件环检查,事件环由 grep 覆盖) | ✅ 编译期合规 |

### A.3 护栏结论

- **护栏保留**: efficiency-monitor `PolicyOscillationDetector` 完好(lib.rs:63 模块 + monitor.rs:75-79 字段 + monitor.rs:148-164 喂入 `ThinkingModeSwitched`/`GsoePolicyUpdated` + oscillation_detector.rs:147/208/245/328 实现),模块头注明"策略抖振检测器(P2-14,GSOE↔Quest 逻辑循环监测)"
- **测试**: `cargo test -p efficiency-monitor` 172/172 通过(0 failed / 0 ignored);护栏相关 30 测试全过(13 单元 + 14 接线 + 3 proptest)
- **遗留(可选建议)**: 审计脚本暂不含事件环静态检查,建议未来扩展;非强制

### A.4 零代码改动声明

本验证未修改任何 `.rs` / `Cargo.toml` 文件;交付物 = P9-T5 验证报告 + IMPLEMENTATION_PLAN P9-T5 标记 + 本附录。

> **记录日期**: 2026-08-04 · **验证者**: P9-T5 文档同步子代理
