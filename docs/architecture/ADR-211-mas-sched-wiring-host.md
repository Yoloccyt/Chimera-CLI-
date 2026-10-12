# ADR-211: mas-sched / PriorityScheduler 接线宿主与 optional→生产边翻转（P1 流量波）

- **Status**: Accepted（2026-10-12 用户授权转正；授权链与最终落点见 §7；草案期「不改码」约束随之解除，实施批记录 §7.5）
- **Date**: 2026-10-11
- **⚠ 已于 2026-10-11 经实测修订：D2.1 落点不成立**（详见 §6 修订记录与
  `docs/reports/a-traffic-source-decision-pack-2026-10-11.md`）。本文下方 D1/D2 原文**按“历史只加注不改写”纪律保留**，
  ACCEPTED 前**不得按旧 D2.1 落点实施**。
- **Deciders**: 主协调（本席）；待 chimera-architect / rust / security 会签
- **关联**: v2.0 §7-bis P1 流量波、§6.2 任务域目标态四件、§13-bis #5/#6；ADR-145、ADR-033、ADR-160、ADR-188；G-87/G-91/G-57；G-83
- **取号**: 磁盘最大 202、203–209 预留、210 本批降级链、211 双通道查重 0 命中 → 取 ADR-211

## 0. 前置状态（现测）
- 竞态已清：mas-sched 两处 TOCTOU 由提交 bdc966f（claim() 原子事务 + 并发回归断测）修复，本 ADR 接线不再踩 v2.0 §7-bis P1「竞态未修前禁止接生产派单」硬闸。
- 装配=孤岛：mas-sched/src/lib.rs:5-7 PRODUCERS:0/CONSUMERS:0/MATURITY:TRUE；gated_wiring_baseline.txt:13 mas-sched|DECORATIVE；mas-sched/Cargo.toml:19-23 TODO(P3-T2) 预留 optional event-bus 边。
- 对照组：nexus-subagent Task Auction 已接、mas-sched Claim 未接（v1.0 §4.2）。
- 翻转点：dep_edge_freeze.txt:181 chimera-cli|mas-sched 为 feature r2_island_repayment 下 OPTIONAL 边（GATED-UNWIRED），翻生产边触只减棘轮。

## 1. Context
v2.0 判定二：任务域病理是机制在、装配/开关缺——PriorityScheduler（WSJF+老化+should_preempt）与 mas-sched 四原语齐备但零生产调用方，scheduler_dequeue SLO 测零调用方类（v1.0 §4.2:190）。接线是把"规则集合"变"调度系统"第四构件（反馈边）落地的物理前提。

## 2. Decision
### D1 宿主选择
| 选项 | 说明 | 依赖面 | 判 |
|---|---|---|---|
| H-a 组合根 chimera-cli | 装配根持 Arc<dyn PeerScheduler>，长任务派单路由到 mas-sched | chimera-cli→mas-sched optional 翻生产边（触 dep_edge_freeze） | 直接但翻转成本最高 |
| H-b chimera-mas::delegate()（推荐） | delegate()（orchestrator.rs:304）现绕过 PriorityScheduler；改经 enqueue→dequeue（v2.0 C-4：无 select），dequeue 用 WSJF/老化序 | chimera-mas↔mas-sched 同层边，不新增向上边、不翻 chimera-cli 生产边 | 最小翻转、复用 L0 契约（scheduler_contract.rs 已在盘） |
| H-c 暂不接 | 维持 DECORATIVE | 无 | 红线不闭、SLO 仍测死码 |
推荐 H-b。

### D2 接线四件（对齐 v2.0 §6.2，无新抽象）
1. 优先级进决策：delegate() 调 PriorityScheduler::enqueue→dequeue；mas-sched priority_rank（scheduler.rs:250 dead_code）去标记并在 check_quota 竞争处消费。
2. 抢占落盘：should_preempt（scheduler.rs:277）→ quest-engine LHQP checkpoint 桥。
3. 依赖序：topological_order（dag.rs:128）接 engine.rs:324 之后，须与 AgentTask.dependencies 真填充同批（否则装饰性接线，G-87 会抓）。
4. 事件面：补 optional event-bus 边 + Claim 家族（publish_claim/declaim，bus.rs:1101-1113）生产发出点。Certify 受 G-83 白名单锁 mas-sched 禁签；Handoff 无变体（v2.0 C-5，先语义替代或另立 ADR）。

### D3 开关与降级
运行期一律 env 三态（ADR-195 D4，禁 feature 作降级开关），默认 off/observe；in-place-fallback 决策失败回落旧 delegate() 直派 + warn。

## 3. Consequences / 影响面
| 维度 | 影响 | 授权 |
|---|---|---|
| 生产派单路径 | delegate() 改走调度器 | 需 ADR（本 ADR ACCEPTED 后实施） |
| 台账 | 接线成功 gated_wiring 摘 mas-sched|DECORATIVE→GATED/WIRED；视 D1 是否翻 chimera-cli 边动 dep_edge/crate_reachability | 同批登记 |
| 事件面 | Claim prod_ctor 0→≥1（event_ownership 删 STALE 行）；optional event-bus 边 L9→L1 向下合法 | 批次级起步；翻 chimera-cli 生产边则同 D1 需 ADR |
| SLO | 接线后 scheduler_dequeue 键语义须重述（不再测零调用方类） | 实施批文档同步 |
| 依赖方向 | chimera-mas↔mas-sched 同层、→event-bus 向下，均合法 | 合规 |

## 4. 不变量 / 验收（实施批断测，先红后绿）
- I1 逐位等价前置闸：无优先级压力场景与旧路径逐位一致（沿 overwindow_bridge.rs:352-372 范式）。
- I2 priority_rank 不再 dead_code；同 peer 竞争高优先获 dequeue（断测）。
- I3 should_preempt→checkpoint 闭环：Critical 打断 Low、可恢复（端到端测）。
- I4 topological_order 生效须 AgentTask.dependencies 非空样本；G-87 式 verdict 台账同批升 WIRED_COMPOSITION。
- I5 台账只减不增：gated_wiring/event_ownership 行数随接线净减（摘 DECORATIVE/删 prod_ctor=0），非新增豁免。

## 5. 边界声明（不可自决）
1. 不改码、不标 ACCEPTED；实施须人类 ACCEPTED 后另批（改生产路径+触台账=ADR 级，承 v2.0 §13-bis #5）。
2. D1-H-a（组合根翻 chimera-cli 生产边）风险更高，须本 ADR 之外显式二次授权。
3. 不新增 crate/入口/EventTopic/第二调度器（清单 A）；不重提平行调度器（ADR-195 F-2/196 D1）。
4. 编号 211；ACCEPTED 前若 211 被占则顺延并回抛。

## 6. 修订记录（2026-10-11，实测推翻 D2.1 落点；不改写原文，只加注）

### 6.1 被推翻的具体主张与证据

本文 D1 推荐 H-b（`chimera-mas::delegate()` 改经 `enqueue→dequeue`），§0 并称
「nexus-subagent Task Auction 已接」为对照组。本会话实测**两条均不成立**：

| 本文主张 | 实测 | 证据 |
|---|---|---|
| H-b 可给 PriorityScheduler 带来真实决策 | **不成立**：`delegate()` 每次只处理 1 个任务且 orchestrator 为 ephemeral，单任务 `enqueue→dequeue` 是恒等映射 | 唯一生产入口 `commands/agent.rs:178` 构造单个 `AgentTask`、`dependencies: vec![]` |
| “Task Auction 已接”（对照组） | **不成立**：`TaskAuction`/`auction` 在 `nexus-subagent` 之外零生产调用方 | 全仓 `crates/*/src` grep（排除 tests/benches）无外部命中 |
| （未列）多任务执行面已有流量 | **不成立**：`DelegationExecutor`/`BatchExecutor`/`TaskChunker` 均零生产调用方；CLI 的 Quest 路径只分解不回显不执行 | `build_quest_reply`（`chimera-cli/src/orchestrator.rs:81-92`）按 `quest.tasks` 输出编号文本 |

另：§0 「竞态已清」一项仍成立（`bdc966f`）；D2.3 的 `topological_order` 接线**已以更安全的形式完成**——
落点改到 `create_quest`（唯一有真多任务 + 真依赖的入口），commit `efb49c1`，
并保留 `validate_dag` 生产调用方以免制造新死 API。

### 6.2 改写后的实施前提

先决问题不是「接到哪」而是「流量从哪来」。三个候选及其改动面/合规性/台账影响
已整理为决策包（含本席推荐与理由），待用户选型：
`docs/reports/a-traffic-source-decision-pack-2026-10-11.md` §2。

推荐序：**候选 ③（mas-sched Claim 准入流，单任务即有真实语义）先行 → 候选 ①（打通 run/chat 的任务执行流）紧随；候选 ②（`agent spawn` 扩多任务入参）否决**（为造流量而造流量，且触方案 §9-1「不新建入口」）。

### 6.3 D2.4 事件面的补充前提

候选 ③ 若实施，`Lease` / `ClaimOutcome` 现**无 `Serialize`**（`scheduler_contract.rs:89/104`）；
一旦要让 claim 决策进事件 payload，就是 L0 公开类型变更（ADR 级，方案 §13-bis #10）。
若仅同进程内使用则无需变更——实施前须先确定该边界，不得默认。

### 6.4 不变量补充

本文 I1（逐位等价前置闸）仍适用且必须保留；新增一条同级别判据：
**任何接线批必须同时指名它兑现了哪个现有机制的消费者**。若一个批次接完后
`crates/*/src` 仍零生产调用方，则该批属装饰性接线（方案 §1.2「零载体设计名」），
即使测试全绿也不得合入——这正是 D2.1 被否决的原因。

---

## 7. 实施批：授权记录与最终落点（2026-10-12 追加；只加注不改写正文）

> 本节为实施批追加。用户于 2026-10-12 明确选定主线「P0 收尾 + P1 候选③
> （mas-sched Claim 准入流）」并授权本 ADR 由 Proposed 转 Accepted。§6 的
> 「先决问题是流量从哪来」按决策包推荐序收敛：**候选③ 先行**（单任务即有真实
> 语义）、候选② 否决、候选① 留待独立产品决策。

### 7.1 最终落点（替代被推翻的 D1-H-b）

- **接线点**：组合根 `crates/chimera-cli/src/commands/agent.rs::spawn_agent()`，
  在 `orchestrator.delegate(agent_task)` 之前做 claim 准入（`PeerScheduler::claim`
  是 total function，无 Result 面）。`spawn_agent` 签名扩收 `&AppContext`。
- **宿主装配**：`AppContext` 新增 `sched: Arc<mas_sched::SimplePeerScheduler>`
  （沿 gea/gqep 既有装配模式）；D2.1 的「delegate→enqueue→dequeue」不采用
  （§6.1 已证单任务恒等映射）。
- **不做**：不新增 crate / 入口 / EventTopic / 第二调度器（清单 A 与 §5.3 不变）。

### 7.2 开关与降级（承 D3，按实况细化）

- env 三态 `CHIMERA_SCHED_CLAIM`：`off`（**默认**，逐位等价旧路径）| `observe`
  （调 claim，拒绝 warn 不阻断）| `apply`（拒绝阻断，返回
  `ChimeraCliError::EngineError`——不新增错误变体，保 ADR-060 7 变体矩阵）；
  未知值 fail-safe 按 off（沿 `calibration.rs` 纯函数 + OnceLock 分层形制）。
- claim 为 total function，**无「调用异常」面**——唯一失败面是 `Denied`；
  D3 的「in-place-fallback」在本实现中体现为 **off 档零调用**（非异常回落）。
- broker 护栏 `with_rate_limit(60, 60_000)` 随批装配（§9-4 限定：运行期行为
  切换走 env，feature 只用于「是否参与编译/可达性记账」）；ephemeral 进程内
  单次 claim 不触发阈值，跨进程防护留待常驻宿主（serve/tui）复用实例。
  护栏**不作为**「兑现消费者」论证主体（论证落在 claim 首个生产调用 +
  ClaimIssued/ClaimRetracted 首个生产发射点 + CRATE-CONTRACT CONSUMERS 0→1）。

### 7.3 事件面（§6.3 边界正式关闭）

- **Serialize 边界结论**：`ClaimIssued{metadata, claim_id, subject, claimant}`
  与 `ClaimRetracted{metadata, claim_ref, reason}`（`types.rs:51/76`）**不含**
  `Lease`/`ClaimOutcome` 字段 ⇒ **无需**为二类型补 `Serialize`（§6.3 的
  「同进程内使用」路径成立，L0 公开类型零变更）。
- **发布方 = 组合根**：chimera-cli 持 `ctx.bus`，Granted →
  `bus.publish_claim(NexusEvent::ClaimIssued)`；delegate 返回 Err 且已 Granted →
  `bus.publish_declaim(NexusEvent::ClaimRetracted)`。mas-sched **保持零 event-bus
  依赖**（不采用 D2.4 的「mas-sched 自发事件」形态，规避 E-6 的依赖重声明成本）。
- **语义裁量**（供后续批复核）：spawn 成功路径不显式 retract（ephemeral
  进程退出即释放租约表）；retract 仅出现于 delegate 失败路径。两变体均获得
  生产构造点 ⇒ `event_ownership_baseline` 删两行（台账净减，符合 I5）。
- Certify 维持 G-83 白名单（mas-sched 禁签）；152 变体 / 16 mpsc / 20 Critical
  三计数零变更；`ClaimIssued` 维持 Normal / Parliament Topic。

### 7.4 台账联动（只减不增；均为实测驱动）

- `gated_wiring_baseline.txt`：摘 `mas-sched|DECORATIVE`（DECORATIVE→WIRED）。
- `declared_dep_exceptions.txt`：回收 `chimera-cli -> mas-sched` 行（自述「接线落地后须回收」）。
- `event_ownership_baseline.txt`：删 `ClaimIssued`/`ClaimRetracted` 两行 + total 行同步（132→134 owned / 20→18 unowned）。
- `contract_consumer_baseline.txt`：按 `check_contract_consumers.py` 现跑输出删 STALE 行
  （至少 `TodoClaim`；视实现含 `ClaimOutcome`/`DenyReason`/`Priority` 等）。
- `dep_edge_freeze.txt`：`chimera-cli|mas-sched` 行语义更新（GATED-UNWIRED → WIRED
  生产边）+ 邻近注释按「历史只加订正注记」订正。
- `crate_contract.py`：mas-sched `CONSUMERS: 0→1`（PRODUCERS 保持 0，mas-sched
  不构造 NexusEvent）；chimera-cli header PRODUCERS 刷新（+ClaimIssued/ClaimRetracted）。
- `crate_reachability_freeze.txt`：无活动行无需同步；mas-sched 由 GATED 入可达（30→31）。

### 7.5 验收判据与回滚

- I1 逐位等价（off 档不调 claim/不发事件，行为与接线前逐位一致）；I2 断测：
  observe 档 Denied 不阻断 + apply 档 Denied 阻断（EngineError）+ Granted 发 ClaimIssued
  （bus 订阅断言）+ delegate Err 发 ClaimRetracted。
- 门集复跑：check_dependency_rules / check_dep_edge_freeze / check_crate_reachability /
  check_gated_wiring_substance / check_event_ownership / check_contract_consumers /
  check_declared_dep_usage / check_certify_whitelist（全部应保持 RC=0 或按只减方向变化）。
- 回滚：代码 revert = 恢复 `optional = true` + feature 表 `dep:mas-sched` 项；
  运行期回退 = `CHIMERA_SCHED_CLAIM=off`（零行为变更，无需 revert）。
