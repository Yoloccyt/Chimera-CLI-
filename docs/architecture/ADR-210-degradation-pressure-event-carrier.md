# ADR-210: 降级链压力事件的 NexusEvent 载体（B-6/N-8 红线可执行化）

- **Status**: Accepted（用户 2026-10-11 授权；采纳 **Option C**——把红线措辞与现网对齐，由活体 `EfficiencyAlertTriggered`/`BudgetExceeded` 承载；**不**新增变体、零生产代码）
- **Date**: 2026-10-11
- **Deciders**: 主协调（本席，架构+取证）；待 chimera-architect / security / rust 会签
- **关联**: OMEGA Ω₁-Sparse/Ω₄-Event/Ω₈-Assess；红线「降级路径必须发布对应事件」（`agents.md` §8 降级路径 / `nuxus规则.md`）；v1.0 §5.1-S5、v2.0 §13-bis #8；ADR-197（新增事件先例）、ADR-072（字段扩展优先于新变体）
- **取号**: 磁盘最大 202；203–209 已被决策包预留（203 session-store…209 三类压力）；210/211 双通道全域查重 0 命中 → 取 **ADR-210**。**勘误**：上批 ADR-202 §0 曾写「203+ 顺延」，未先查 `adr_index.md` 预留表，属过度取号，本批评正。

## 0. Context（全 A 级，逐条 file:line）

- **能力已在码内**：`crates/chimera-mas/src/stability.rs:157-170` `enum PressureSource { MemoryNearBudget(161), ExpertOverload(165), ArchiveIoContention(169) }`，每态经 `stability.rs:33-37` 文档映射到有序 `DegradationStep` 序列（`MemoryNearBudget → [HcwCompress, TierDemote, RejectNewAgent]` 等），由 `DegradationChain::apply()` 返回。
- **但红线字面不可执行**：红线要求「降级路径必须发布对应 `NexusEvent`（含三压力源）」；实测 `MemoryNearBudget|ExpertOverload|ArchiveIoContention|PressureSource` 在 `crates/event-bus/src/` 全文 **0 命中**（本轮复确认）。⇒ 三压力源**不是 `NexusEvent` 变体**，`PressureSource` 是 `chimera-mas` 内部枚举、不经总线。⇒ 「发布对应事件」这一步**无载体、不可执行**（v1.0 S5/B-6、v2.0 §13-bis #8）。
- **事件面结构**：`crates/event-bus/src/registry.rs` 宏 `Variant => Severity, Topic;` 单源生成 severity/topic/type_name；先例 `GatherTimedOut => Normal, Execution`（`:186`）；`OrphanCallDetected => Critical, Execution`（`:188`，broadcast-only）。新增变体须经 `variant_count_test.rs` 三层锁（编译期穷举 + 计数 + 分布），且**若定 Critical 须七点位同批**（`event_ownership_baseline.txt` + `event_ownership` 生产发出点；见 ADR-202 I10 / architect M5）。

## 1. 为什么这是"调度系统化"的关键一环

v2.0 元诊断：调度病是「机制—装配—开关」后两段缺失。降级链是**机制已在码内、只差事件载体**的典型——补上载体即把一条**长期不可执行的红线**转为可观测、可证伪的闭环（Ω₄-Event + Ω₈-Assess：压力→证据）。它也是 v2.0 §6.2-3 的四件目标态之一。

## 2. Decision Options

### Option A（推荐）— 新增单变体 `PressureDetected { source, applied_steps, remaining_budget_mb? }`
- 语义匹配 `PressureSource`（三态 + 已选 `DegradationStep` 序列 + 预算余量）；**append-only + 默认 `Normal` + 复用现有 Topic（不新增 EventTopic，守 §0.3/三层锁）**。
- Topic 选 **`Quest`**（降级是 MAS 运行时行为，`stability.rs` 属 chimera-mas/L9；复用既有 Topic 免 10→11 三层锁）；备选 `System`。
- **代价（须诚实）**：新变体 = 触及 v1.0 §0.3「三层锁 + I8c 散文计数追猎」+ 生产发出点 `prod_ctor`。故实施批须**同批**改：`event-bus/src/types.rs` 变体 + `registry.rs` 定级行 + `variant_count_test.rs` 计数锁（152→153）与分布锁 + `event_ownership_baseline.txt`（chimera-mas 取得生产 ctor）+ `chimera-mas` 的 `DegradationChain::apply()` 发布点。**不进 16 mpsc / 不进 20 禁分片清单**（Normal，非 Critical，避免双清单七点位）。

### Option B（最小面积，但语义弱）— 复用 `GatherTimedOut` 或 `AuditFindingRaised` 字段承载
- 依 ADR-072「字段扩展优先于新变体」+ architect M5 精神：向既有 Normal 事件加字段，**零新变体**、免计数锁/三层锁。
- 代价：`GatherTimedOut` 语义专指 gqep gather 超时，塞 MAS 压力 = **语义污染**（违反"事件名即契约"）；`AuditFindingRaised`（efficiency-monitor，五维证据）过于泛型，丢 `PressureSource` 三态离散性与降级步序列。**本席不推荐**——它把可执行红线换成"能编译但语义错位"的事件。

### Option C（回退）— 不建事件，改红线措辞为"不依赖事件"
- 承 v2.0 §9「修判据 > 加豁免」：若判定新增事件成本 > 收益，则把红线从"必须发布 `NexusEvent`"改为"必须经结构化可观测出口（含总线或 metrics）"，并给 `PressureSource`→metrics 路径。
- 代价：弱化 Ω₄-Event（跨层通信只走总线）；但**诚实反映"三态本是 L9 内部枚举、未必非要上总线"**。适用条件：若 `efficiency-monitor` 已有 metrics 出口可承载。

### 推荐
**Option A**（新增 `PressureDetected`，Normal/复用 Quest/append-only），以 §4 I1–I5 为实施批验收门；若成本评审判重，退 **C**（改红线措辞），**不采 B**（语义污染）。

## 3. Consequences / 影响面
| 维度 | 影响 | 处置 |
|---|---|---|
| 事件面 | 新增 1 变体（152→153） | `variant_count_test.rs` 计数+分布锁 + `doc_count_freeze.txt` 三值回写（走 `check_doc_drift`/G-04 类门） |
| 依赖方向 | chimera-mas(L9)→event-bus(L1) 向下边，合法；不新增反向/跨层直连 | 无新 `dep_edge` |
| 双清单 | 定 Normal ⇒ **不进** 16 mpsc / 20 禁分片 | 免七点位同批（这是选 Normal 而非 Critical 的核心理由） |
| EventTopic | 复用 Quest/System，**不新建** | 免 10→11 三层锁 |
| 装配 | 发布点在 `DegradationChain::apply()`，该路径当前是否有生产消费者需实施批核 | 若无生产调用方则接 `gated_wiring` 判定，避免"发布点零调用"= 新装饰 |

## 4. 不变量（实施批必须断测兑现，不接受注释）
- **I1** `PressureDetected` 定级 = `Normal`、Topic ∈ {Quest,System}；`variant_count_test` 计数锁同步至新值且 `severity()` 无通配。
- **I2** append-only：仅新增、不改/删既有变体；不触 16/20 双清单。
- **I3** 三态穷举：`source` 携 `MemoryNearBudget|ExpertOverload|ArchiveIoContention` 之一，与 `stability.rs:157` 一一对应（编译期可判定，禁字符串自由值）。
- **I4** 生产发出点存在（`event_ownership` `prod_ctor≥1`），且发布**不持锁跨 await**（若 `apply()` 在持锁上下文，走 `publish_blocking` 同步版）。
- **I5** 三值（39 crate/152→153 event/10 topic）与 `doc_count_freeze`/`check_doc_drift` 一致；不新增 EventTopic。

## 5. 边界声明（不可自决）
1. 本 ADR 为草案，**不改码、不标 ACCEPTED**；实施须人类 ACCEPTED 后另批。
2. 若实施评审判"新变体成本 > 收益"，按 §2-C 改红线措辞（亦属文档裁决，需批准）。
3. 编号：203–209 已预留，本 ADR 取 210（若届时 210 被占则顺延并回抛）；实施批同步更新 `adr_index.md` 主表 + 预留表。
4. 不动 `.gitignore` `*.md` 策略（治理决策，另批）。

## 6. 决策记录（Accepted 2026-10-11，用户授权）
- **采纳 Option C**：把红线从“必须发布含 MemoryNearBudget/ExpertOverload/ArchiveIoContention 的 NexusEvent”改为“必须由**已存在的活体事件**承载”——规则/风险告警走 `EfficiencyAlertTriggered`（`efficiency-monitor/monitor.rs` `publish_rule_alert`/`publish_paradox_risk_alert`/`publish_critical_alert` 真实发布），预算耗尽走 `BudgetExceeded`（Critical+mpsc）。三份规则派生副本（`.qoder/rules/chimera2.md` + `.claude/CLAUDE.md`）已逐字同步。
- **否决 Option A**（承重理由，非成本）：降级链唯一生产者 `DegradationChain::apply()`（`stability.rs:438`）全仓**无生产调用方**（命中仅文档注释/re-export/字符串测试），新增 `PressureDetected` = 第二个零生产者装饰变体，且无法满足本 ADR 自身 **I4（prod_ctor≥1）**——与本仓反幽灵门纪律相违。
- **未来独立项（非本 ADR 范围）**：若确需把压力源升为一等总线事件，**前置=先为 `DegradationChain::apply()` 接生产调用方**（属独立接线工作，可能需另立 ADR），再按 v1.0 §0.3 七点位同批（含 `variant_count_test` 计数锁、`doc_count_freeze.txt`）。不得先加零生产者变体。
- **一致性**：本决策**不新增变体**（三值 39/152/10 不变）、**不改 `doc_count_freeze.txt`**、**不碰** `agents.md`/`nuxus规则.md`/任何 `.rs`。
