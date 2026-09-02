# ADR-170: 正式收录 OMEGA 第 10/11 定律——Ω₁₀-Card 经验卡片数据结构定律与 Ω₁₁-Synthesize 按需记忆合成算法定律

- **状态**: Accepted(2026-09-02,用户直接指令收录)
- **决策者**: E01(首席架构师)提案;用户终裁
- **关联**:
  - `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §3.1(Ω₁₀/Ω₁₁ 权威定义源)
  - ADR-049(polish-v2.7,首次声明"新增定律需 ADR 记录")
  - ADR-033(L0 契约层,`ExperienceCard` 类型契约归属)
  - ADR-160(可达性棘轮;本 ADR 新增实施点 crate 已纳入生产图)
  - `.trae/rules/nuxus规则.md` §1.1a(守恒铁律)
  - 红线 §1.4(RL 开发闸门 Rust-First)

> **编号说明**: 初稿编号拟为 ADR-161,但 ADR-161 已被 `ADR-161-island-repayment-roadmap-and-batch-ratchet.md`(2026-08-29 落档)占用;ADR-162~165 为预留编号;ADR-166/167/168/169 均已落档。按项目惯例(ADR-167→168、ADR-166→169 均因占用顺延),最终编号顺延至 **ADR-170**。编号权威以磁盘文件名为准。

## 背景

`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §3.1 已将 Ω₁₀-Card(经验卡片)与 Ω₁₁-Synthesize(按需记忆合成)作为扩展定律纳入设计蓝图,并明确其 Rust 侧实现状态为"已落地"。经全库核验,以下四个实施点均已存在且通过测试:

- **`event-bus/src/experience_card_bus.rs`** — 经验卡片的结构化事件流总线(`ExperienceCardBus`),支持 broadcast + mpsc 双通道、任务级索引、节点级索引、错误签名级检索,以及 `TokenLedgerRecorded` 等关联事件发布。
- **`nexus-contracts/src/experience_card.rs`** + **`nexus-contracts/src/skill_lifecycle.rs`** — L0 契约层定义 `ExperienceCard` 不可变数据结构、`CardMetadata` 版本化元数据、`SkillLifecycle` 技能生命周期状态机,以及 `AtomicMemoryCard`/`AtomicCardType` 原子记忆卡片契约。
- **`mlc-engine/src/experience_card_system.rs`** + **`mlc-engine/src/on_demand_synthesizer.rs`** — L2 记忆层的经验卡片系统(`ExperienceCardSystem`)与按需合成器(`OnDemandSynthesizer`),实现懒加载祖先检索、同错误签名兄弟定向检索、操作符感知的上下文选择。
- **`cmt-tiering/src/experience_card_storage.rs`** — L3 存储层的经验卡片持久化(`ExperienceCardStorage`),支持四级归档(Hot/Warm/Cold/Ice)与卡片级 TTL 清理。

这些代码模块已在多个 E2E 闭环中运行:``experience_loop_closure_e2e``、`card_persistence_closed_loop_test`、`error_card_closed_loop_test`、`on_demand_synthesizer_test` 等。但权威规则文档(`.trae/rules/nuxus规则.md`、`AGENTS.md`、`CODE_WIKI.md`)仍仅列出 Ω₁~Ω₉ 九定律,Ω₁₀/Ω₁₁ 未获正式收录,导致设计蓝图与规则文档存在 gap。

**守恒前提不变**:Ω₁~Ω₉ 为架构基座,其定义、语义与落地点不可变更(ADR-049 守恒铁律)。本 ADR 仅将已落地的 Ω₁₀/Ω₁₁ 作为**Rust 侧扩展定律**正式纳入,表述升级为"OMEGA 十一定律(九基座 + 两扩展)"。

## 决策

1. **正式收录 Ω₁₀-Card 为第 10 定律**:
   - **含义(设计定义)**:经验卡片数据结构定律——经验以不可变结构化卡片(`ExperienceCard`)为载体,卡片内含三因子评分、版本化元数据、错误签名与父节点引用;卡片流为 append-only,写入后不可变,更新即新建版本。
   - **核心约束**:
     - 不可变:单张卡片字段在创建后不可修改(如需修正,发布新卡片并链式引用旧卡片)。
     - 版本化:`CardMetadata.version` 单调递增,旧版本保留供溯源。
     - Append-only 事件流:卡片发布走 `ExperienceCardBus`,与 `NexusEvent` 同构双通道(broadcast 订阅 + mpsc 旁路关键卡片)。
   - **落点 crate**: `event-bus`(总线/索引)、`nexus-contracts`(契约)、`mlc-engine`(系统/合成器)、`cmt-tiering`(持久化)、`pvl-layer`(卡片生成/轨迹评分)、`repo-wiki`(经验银行)、`faae-router`(卡片反馈/父上下文)。

2. **正式收录 Ω₁₁-Synthesize 为第 11 定律**:
   - **含义(设计定义)**:按需记忆合成算法定律——记忆召回不采用全量预加载,而是根据当前操作符(`AtomicOperator`)与目标卡片,懒加载合成相关上下文;合成过程异步执行,不阻塞主推理流程。
   - **核心约束**:
     - 按需懒加载:仅当任务节点需要时才触发祖先链/兄弟节点检索,而非任务启动时全量预热。
     - 不阻塞主流程:`synthesize_memory_on_demand` 返回 `SynthesizedMemory`,超时回退到基础上下文(空合成),保证主路径延迟可控。
     - Debug→同错误签名兄弟定向检索:当目标卡片携带错误签名时,合成器优先检索同 `error_hash` 的兄弟卡片作为负面教训上下文,支持闭环调试。
   - **落点 crate**: `mlc-engine`(合成器核心)、`event-bus`(错误签名级检索接口)。

3. **守恒表述升级**:
   - 全项目权威规则文档中"OMEGA 九定律"统一升级为"**OMEGA 十一定律(Ω₁~Ω₉ 基座 + Ω₁₀/Ω₁₁ 扩展)**"。
   - Ω₁~Ω₉ 的不可变更性(守恒铁律)不受影响;新增扩展定律须经 ADR 记录(本 ADR 即为合规记录)。

## 否决项

- **否决方案 A:将 Ω₁₀/Ω₁₁ 作为可独立移除的 feature gate**:经验卡片总线已深度融入 `event-bus` 发布路径(含 `TokenLedgerRecorded` 事件),合成器为 `mlc-engine` 核心 API,独立 feature gate 会导致装配面碎片化且增加测试矩阵。保持现状(始终编译)。
- **否决方案 B:允许卡片可变修改模式**:若允许原地修改 `ExperienceCard` 字段,将破坏 append-only 事件流与版本化溯源,且与 `event-bus` 的不可变消息语义冲突。维持"更新即新建版本"模式。
- **否决方案 C:将合成器改为全量预加载**:全量预加载违背 Ω₂-Compress(四级窗口按需压缩)原则,会在大任务链上造成 O(N) 内存膨胀。维持懒加载按需合成。

## 与既有规则的关系

### 依赖铁律
- Ω₁₀ 落点 `event-bus`(L1)与 `nexus-contracts`(L0),Ω₁₁ 落点 `mlc-engine`(L2),均遵守 `L(N)→L(N-1)` 允许、`L(N)→L(N+1)` 禁止的铁律。
- `experience_card_bus.rs` 对 `nexus-contracts` 的 `ExperienceCard` 引用为 L1→L0,符合 ADR-033 扩展规则。
- 无新增跨层渗透;经验卡片在 L2/L3 的消费均通过 event-bus 广播或本地 API,不引入向上依赖。

### RL 开发闸门(Rust-First)
- Ω₁₀/Ω₁₁ 当前为**纯 Rust 规则式实现**:卡片评分为确定性纯函数、合成为规则式检索+选择,无神经网络训练。
- 设计蓝图 §3.1 中标注的"卡片嵌入网络"/"记忆生成网络"为未来 RL 升级路标,明确受 RL 开发闸门约束:在 R2 解冻 + 稳定性观察期通过前,**禁止实施 Python 侧训练服务**,仅保留 Rust 侧接口占位。

## 后果

- **正向**:权威规则文档与代码实现 gap 消除;Ω₁₀/Ω₁₁ 获得 ADR 级正式身份,后续变更(如合成算法升级)可直接引用本 ADR。
- **正向**:经验卡片体系纳入 OMEGA 定律对齐检查清单,任何修改须同时满足 Ω₁₀(不可变/版本化/append-only)与 Ω₁₁(懒加载/非阻塞/错误签名定向)约束。
- **无负向**:本 ADR 不修改任何代码,仅做文档层面的正式收录与表述统一;所有实施点代码已在 v2.28.0-omega 基线中运行并通过测试。

## 验证

- [x] 四个实施点源码文件存在且编译通过(见背景章枚举)
- [x] 相关 E2E/闭环测试通过(见背景章枚举)
- [x] 权威规则文档(nuxus规则.md / AGENTS.md / CODE_WIKI.md)已同步为"十一定律"表述
- [x] 守恒铁律验证:Ω₁~Ω₉ 定义未变更,仅追加 Ω₁₀/Ω₁₁
- [x] 依赖铁律验证:无新增跨层违规边
