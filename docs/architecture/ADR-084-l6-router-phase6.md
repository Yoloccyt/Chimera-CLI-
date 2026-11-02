# ADR-084:L6 Router Phase 6 深打磨——六维调整器落点/零新事件变体/层图订正/占位治理

- **状态**:Accepted(Phase 6 W0 落档,2026-08-16;各 Wave 收尾回填门槛)
- **日期**:2026-08-16(对应实施波次 Phase 6 W0~W7)
- **决策发起方**:`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §11(L6 Router)+ §3(铁律)+ §16.4(跨层事件表)
- **关联**:ADR-049(决策 1 内嵌哲学)、ADR-033(L0 契约层)、ADR-054(三环重组/层图权威)、ADR-078/079(SegmentMetadata/TokenLedger)
- **影响架构层**:L6 Router(osa/kvbsr/faae/sesa/omega 主)+ L5(repo-wiki 协同)+ L9/L7(gea/ssra 层图订正)

## 背景(Context)

v3.4.0 规范 §4.2 宣称 L6 含 10 crate(新增 skills-progressive-loader / operator-router /
six-dimension-adjuster / harness-config-manager / parent-selector / tool-schema-pruner 六个),
但 §11 仅为其中三个给出代码,且与仓库现状存在三类冲突:

1. **实现已内嵌落地**(ADR-049 决策 1):ProgressiveSkillLoader 在 L5 repo-wiki、
   OperatorRouter 在 faae-router、ToolSchemaPruner 在 osa-coordinator——规范的新 crate
   规划与"38 crate 基线不可动摇"第一性原理(§3.3)冲突。
2. **规范自身缺口**:six-dimension-adjuster / harness-config-manager / parent-selector
   无任何算法描述;§4.2 的 parent-selector 与 §16.4"ParentSelected 由 L5 产生 L6 消费"自相矛盾。
3. **仓库腐化点**:ToolSchemaPruner 孤儿未接线;D1-D6 契约零 L6 消费方;
   omega s9_route 丢弃学习结果伪造权重;sesa 共识订阅纯日志 stub;
   gea affinity 占位;ssra latency_ms 硬编码 0。

## 决策(Decision)

1. **SixDimensionAdjuster 内嵌 osa-coordinator**(用户裁决):osa 本是"全维协调器",
   已持有 sha2 哈希基建,D1/D2 的 L6 主要受益者是 osa 自身(faae 仅经 apply_strategy
   消费 D3)。不新建 crate,不新建 harness-config-manager(其职责由调整器直接读写
   L0 `HarnessConfigContract` 承担)。
2. **零新增 NexusEvent 变体**(用户裁决):反馈闭环走已落地的 `ExperienceCardBus`
   数据面(卡片天然携带 operator+score+execution_status,正是 record_result 输入);
   与"流式数据面走 bounded mpsc 不进事件枚举"的既有哲学一致。
   ParentSelected/OperatorExecuted/VariantApproved 三变体留给规范 §19 Phase 7 的
   +14 事件波次统一治理——L7/L8 生产者落地前新增只会制造孤儿发布者。
3. **gea/ssra 层图订正 + 纳入打磨**(用户裁决):check_dependency_rules.{ps1,sh} 层图
   将 gea(L9)/ssra(L7)误标 L6,与 crate 自述头及 AGENTS.md §2.1 冲突;订正移动
   (二者零外部生产依赖方,grep 核实无 Check-B 影响)。gea 修 affinity 占位、
   ssra 修 latency/空模板占位。
4. **omega 占位本阶段修复**(用户裁决):RL 开发闸门明确"现阶段只做 Rust 侧";
   修复保持影子模式 advisory 合规,不动 R2 冻结边界(Learned 策略仍不强制下发)。
5. **Skills L6 编排落点 osa 纯规划器**:osa 不加 repo-wiki 生产依赖(避免 rusqlite
   依赖链污染全部 osa 消费方),定义 `SkillIndexEntry` 轻量投影 + 纯函数规划器;
   repo-wiki 侧补 prefetch/index_snapshot/R8;集成测试经 dev-dep 接线。
6. **D1-D6 只读消费**:L6 对 L0 契约只读消费(含调整器产出的新版本契约),
   零 L0 类型改动;§11.4 边界裁决——L5 ThreeFactorSelector 是唯一父本选择实现,
   L6 仅做消费适配(parent_context 注入 OperatorContext.parent_card)。

## 理由

- 38 crate 基线是第一性原理级约束,规范 §4.2 的 10-crate 规划在 ADR-049 决策 1
  哲学下已由内嵌落点实质取代,本 ADR 将其正式化。
- 零新事件变体使 L0/L1 涟漪为零,全仓回归风险最小化;卡片总线闭环在 L7 卡片
  生成器落地后自动接通真实反馈(前向兼容)。
- 孤儿/占位/虚假数据治理与 L3 `tier_distribution` 修复先例同构:接线优先于新增。

## 影响

- osa-coordinator 新增 six_dimension.rs / skill_plan.rs 两模块;faae 新增
  card_feedback.rs / parent_context.rs 两模块;W1~W6 波次交付。
- 依赖脚本层图:gea→L9、ssra→L7(2026-08-16 实测 EXIT=0)。
- deprecated `five_dimension_masks()` 移除(零调用方,grep 核实)。
- 下游 L7-L10:零 API 破坏(masks 类型/事件结构不变;faae 闭环 opt-in 装配期接线)。

## 备选与否决

- 新建 six-dimension-adjuster 独立 crate:违反 38 crate 基线,需改脚本
  expectedCrates 断言与 CODE_WIKI 索引,收益仅"目录语义"——否决。
- 本阶段新增三个 NexusEvent 变体:短期全为孤儿变体(L5/L7/L8 生产者未落地),
  且违背卡片总线数据面哲学——否决,留 Phase 7。
- omega 占位延后 R2 解冻:占位属"虚假数据固化"腐化类,与冻结边界(策略下发)
  正交——否决延后,本阶段修复。
