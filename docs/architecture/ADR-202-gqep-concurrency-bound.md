# ADR-202: gqep-executor 并发上界（max_concurrency）生效决议

- **Status**: **Accepted**（用户 2026-10-11 在「B 段全量实施」计划批准中显式授权；采纳 **Option B** 分批入流。决策记录见 §11；草案期边界声明保留于 §9 / §10.4，由 §11 承接）
- **Date**: 2026-10-11（草案）/ 2026-10-11（ACCEPTED 并实施）
- **Deciders**: chimera-security-engineer（本席：资源治理红线取证）；待 chimera-architect / rust-engineer / perf-engineer 交叉验证
- **关联红线**: §6.1「1M Token 暴力加载」/ §6.2 红线 13「孤儿调用」/ UNLEARNABLE #3、#6
- **依据立项书**: `docs/reports/p1-preflight0-correctness-fixes-2026-10-11.md` #4 + §3.3 + §8

## 0. 取号查重（Step 0，强制前置）

- 磁盘实体档最大主编号 = **ADR-201**（`docs/architecture/ADR-201-model-view-event-taxonomy.md`）；`ADR-202-*.md` 磁盘 **0 命中**
- 非 md 全域（`*.py / *.ps1 / *.sh / *.toml / *.rs / *.json / *.yml`，排除 `target`、`.tower`、`__pycache__`）：`ADR-202` **0 命中**
- `.trae/`、`.claude/`、`.qoder/`、`.qoder-cn/` 四目录**全文件类型**扫描（本批补测，口径修正）：`.claude/` 3 文件 **0 命中**、`.qoder/` 89 文件 **1 命中**、`.qoder-cn/` 1 文件 **0 命中**、`.trae/` 355 文件 **0 命中**。唯一命中 = `.qoder/canvases/preflight0-completion-report.canvas.tsx:85`，原文「1) #4 gqep max_concurrency 生效是否授权立 ADR-202（重构 gather 并发上界 + entangle/超时/孤儿语义交互）」——与本 ADR **同用途**，属**支持性授权链佐证**而非异义占用。**诚实纠正**：上一轮该面仅按「目录内 md」口径扫描故记 0 命中，漏掉 `.tsx`；Spec Step 0 要求目录级查重（不限扩展名），本批按全类型重扫并修正本条，结论（202 空闲）不变。
- md 内命中均为「新号自 ADR-202 起」的**取号指令**（`ADR-033:307` / `ADR-188:223` / `ADR-195:114` / `fusion-concept-map:102` / 调度层 v1.0 `:113`、`:565` / p1 报告 `:89` / v3.5 总账 `:410`），**非占用**
- ⚠ **同号异义风险登记（如实，不粉饰）**：`docs/reports/v3.5-w66-decision-packages-2026-10-09.md:24` 曾把 ADR-202 预想为「层表 SSOT / HTS 阈值标定窗口」用途。
  本 ADR 裁定：**202 由「gqep 并发上界」占用**——因 p1 立项书 `:89` 已把 #4 明确判给「取号自 ADR-202 起」的首个待授权项，且该项是六条红线中唯一仍开放的**机制型**缺口。
  「HTS 阈值标定」「层表 SSOT」等其余候选用途**顺延取 203+**，并已在 `docs/architecture/adr_index.md`「编号预留登记」表显式声明 202 已占用，防止双占复发（该表 §编号预留 记录本仓已发生两次撞号）。
- ⇒ 同号异义的**两个对向预想**本批已对齐并留证：`.qoder/canvases/preflight0-completion-report.canvas.tsx:85`（#4 gqep `max_concurrency` ⇒ 立 ADR-202）与本 ADR **同侧**，构成授权链佐证；`v3.5-w66:24`（层表 SSOT / HTS 阈值标定）为**对向**，裁定顺延 203+。二者均为文档面预想、无实体档与 index 占用，故 202 可取。

## 1. Context（全 A 级证据：逐条可复核 file:line）

### 1.1 死字段自陈（差距的源头）
`crates/gqep-executor/src/config.rs:24-34` 字段注释即宣告未生效：「NOTE:当前实现暂未强制限流(`FuturesUnordered` 一次性放入所有 future)…保留此字段用于未来扩展与配置一致性」；`:33` `#[allow(dead_code)]`；`:34` `pub max_concurrency: usize`。默认值 `:62` = **100**。`:83-97` `test_config_serde_roundtrip` **断言该字段序列化往返** ⇒ 配置面公开、运行时消费点为零。

### 1.2 放大面本体（N 个 entangle 同时在飞）
`gatherer.rs:17` 导入的是 **`futures::stream::FuturesUnordered`**（非 tokio 版）。`gatherer.rs:145-158`：新建空流后 `for future in futures { … stream.push(entangled) }` **全量入流，无任何上界判定**；`:154-155` 每个元素被 `qeep.entangle(mapped)` 包裹。`futures` 版 `FuturesUnordered` 在**首次 `poll_next` 即 poll 全部已注册 future** ⇒ 一次 gather 的 N 个操作**同时**进入在飞态。

单个在飞 future 的资源足迹（`crates/qeep-protocol/src/protocol.rs`，`entangle` 为 `async fn` @ `:110`）：

| 足迹 | 证据 | 随 N 的放大 |
|---|---|---|
| `pending_calls` 注册 | `protocol.rs:119-127` `insert(id, CallRecord{Pending})` | 每操作 1 条 map entry |
| Ack 三元组 + 状态迁移 | `protocol.rs:129-139`（含二次 `get_mut`） | 每操作 1 次额外 map 访问 |
| `OrphanGuard` + `Arc<AtomicBool>` | `protocol.rs:141-147` | 每操作 1 组分配 |
| 单操作定时器 | `protocol.rs:152` `tokio::time::timeout(self.default_timeout, future)` | **每操作 1 个 timer 条目** |
| 底层 IO/LLM 通道 | `agents.md:510` 语义 + `gatherer.rs:150-156` 包裹链 | 每操作 1 路上游并发 |

⇒ 放大三重面：**内存**（entry + timer + Box::pin）、**句柄/连接**（N 路并发）、**上游限流面**（N 路同时打向 provider）。

### 1.3 外层 deadline 是绝对值，不随 N / 并发度缩放
`gatherer.rs:241` 读 `self.config.gather_deadline_ms`；`:261-282` 以 `tokio::time::timeout(deadline, collect_all)` 包裹整个收集循环；默认 5000ms（`config.rs:63`）。`config.rs:43-45` 注释自称「5s deadline 足以覆盖大规模 gather」，但实现中**不含任何 `ceil(N/bound) × per-op` 缩放项** ⇒ 「大规模」的主张无机器支撑。`:245-259` 另规定 `deadline_ms == 0` 即禁用全局超时（无上限运行）。

### 1.4 第二面同构缺口（口径一致性）
`crates/gqep-executor/src/toolplan_runner.rs:111-113`「WI-16 门禁:无冲突层内全并行」+ `:138-153`：每层 `FuturesUnordered::new()` 后对层内节点全量 `push`，**同样无并发上界**。它仅被**准入界**间接封顶：`crates/nexus-contracts/src/tool_plan.rs:127` `MAX_STEPS = 64`，`:148-149` `nodes.len() > max_steps ⇒ Err(TooManySteps)`；`:140-143` 的 `guards.timeout_ms`（`:129` `MAX_TIMEOUT_MS = 30_000`）只在**层间**检查、不在层内。⇒ 只治 gatherer 会留下同 crate 内第二面的口径不一致。

### 1.5 红线权威出处与「同一条红线在别处已生效」的对照
- `agents.md:510`：「内存爆炸? | **1M Token 暴力加载** | 必须经 HCW 分层 + OSA 稀疏化后再加载(1M = 128K 实际 + 8× 稀疏压缩)」
- `agents.md:508`：「所有 async 必须 await 或 spawn 管理」；`agents.md:415`：「**并发收集用 `FuturesUnordered`** — 优于 `join_all`,减少内存占用,支持流式结果」（张力见 §6）
- `.trae/rules/nuxus规则.md:166`：红线 **13. 孤儿调用(异步无 GQEP 聚集/超时)**
- **同红线已生效先例**：`crates/event-bus/src/bus.rs:34-40` `CRITICAL_CHANNEL_CAPACITY = 4096`，注释明写「同时提供硬上限防止慢消费者导致 OOM(**§6.1 红线 "1M Token 暴力加载"**)」⇒ 【事实】**同一条 1M 红线在 L4 事件面是硬常量，在 L7 gather 面是死字段**
- **不可学习红线**：`crates/seccore/src/policy.rs:318-325`（`:338` 机器锁定 `len == 6`）——`:321`「零孤儿调用保证(QEEP)不可绕过」、`:324-325`「BudgetExceeded severity = Critical 不可降级」
- **门禁缺口**：`scripts/perf_redlines.toml` 仅 RL-01~RL-11 + SLO，其中 `:62-65` RL-08 = `50agent_mem_peak <=130MB` 是唯一资源帽；**全表无「并发度 / 资源耗尽」条目**。`scripts/check_parallel_redlines.ps1` 四条禁令（自旋 / IO-on-rayon / rayon::join / 持锁跨 await）亦**不含并发上界**。`tests/security/owasp_top10.rs` 的 A05 映射为「安全配置错误」而非 DoS/资源耗尽。⇒ 【事实】「并发度限制」六条红线中唯一的一条**机制型红线，今日无机器门**。

- **立项授权链（v2.0 三处，本批补入取证）**：`…重构方案_v2.0.md:573`（§12 追加表 **#19【事实·确认】**「`gqep-executor/config.rs:34 max_concurrency` `#[allow(dead_code)]` 不生效」）、`:436`（**§7-bis 前置 0 由 3 项扩为 4 项**，第 ④ 项原文「`gqep config.rs:34 max_concurrency` 死字段**要么生效要么删**」——本 ADR Options B=生效 / C=删 即该二择的直接承接）、`:605`（**§13-bis 移交清单项 #4**，同表标注前三项实施状态见 p1 报告，唯第四项即本 ADR 待裁决）。
### 1.6 现状真实烈度（诚实定级，不夸大）
唯一生产消费者：`crates/chimera-cli/src/commands/doctor.rs:175` `const PROBE_COUNT: usize = 11`、`:202` `ctx.gqep.gather_collected(probes)`；装配根 `crates/chimera-cli/src/composition.rs:177` 使用 `GqepConfig::default()`。`crates/gqep-executor/src/lib.rs` CRATE-CONTRACT 记 CONSUMERS = 1（另 `crates/chimera-mas/src/delegation.rs:790-794` 刻意绕开 gqep 用 `tokio::time::timeout`）。

⇒ 【判定】**即便今日让 `max_concurrency = 100` 生效，对现有生产路径也是 no-op（11 ≪ 100）**。本 ADR 处置的是**潜伏设计缺陷 + 面向未来调用方（N > 100）的防御缺口 + 红线账挂账**，不是活跃耗尽事故。任何把它写成"修复线上 OOM"的表述都是粉饰。

## 2. OMEGA 定律一致性与事件面关系

### 2.1 Ω₁-Sparse —【推断：相悖（该面缺载体）】
`.trae/rules/nuxus规则.md:51`：Ω₁ = 「策略稀疏性:全维稀疏掩码 + 按需激活」，载体 `osa-coordinator`(五维度掩码) + `sesa-router`。预算维确有实现（`crates/osa-coordinator/src/parallel.rs` `budget_mask_core`；`six_dimension.rs:16` 规则表「`BudgetExceeded` → max_tools_per_step −1（收紧工具面）D2」）。
但 `crates/gqep-executor` **不依赖 osa-coordinator、不消费任何掩码**：`gatherer.rs:145-158` 是无条件全量入流。⇒ 【推断】「按需激活」在 L7 gather 面无载体，Ω₁ 在该面的落地断言为**缺口**——不是被违反，而是**从未接线**。

### 2.2 Ω₈-Assess —【事实：相悖，本仓 UnusedCapability 的教科书案例】
`.trae/rules/nuxus规则.md:58` 指定权威源 `efficiency-monitor/src/auditor.rs`(EvidenceGap)；判定规则 `auditor.rs:341-343`：未登记 → `EvidenceGap`(High)；已登记且使用数 = 0 → **`UnusedCapability`(Medium)**；使用数 > 0 → `VerifiedCapability`(Info)。变体定义 `:87/:89/:91`，`:20` 注释自述「`UnusedCapability` Finding 提示**配置与实际行为脱节**」。
`max_concurrency` 精确命中「配置在（`config.rs:34`）+ serde 往返测试在（`:83-97`）+ 运行时消费点 0（`#[allow(dead_code)]` `:33`）」。⇒ 【事实】Ω₈ 的证据纪律对本面判**不合规**；且 `config.rs:30-34` 的自陈注释等于**已知并登记了该脱节**却未偿还。

### 2.3 三事件与该面的关系 —【事实，逐条定级/通道取证】

| 事件 | 定级 / 通道 | 权威证据 | 与并发上界面的关系 |
|---|---|---|---|
| `GatherTimedOut` | **Normal**, Execution | `crates/event-bus/src/registry.rs:186`；载荷含 `deadline_ms / elapsed_ms / total / abandoned`（`types.rs:777-794`）；发布点 `gatherer.rs:292-298` | 全局超时的**唯一批次级**信号，直接量化放弃面（`abandoned` 由 `gatherer.rs:289` `total.saturating_sub(succeeded+failed)` 算出）。定级 Normal ⇒ 背压下可被 Lagged / 降采样丢弃 |
| `OrphanCallDetected` | **Critical，但 broadcast-only** | `registry.rs:187-188`（注释「[Critical·broadcast] … §6.1 红线观测面,历史 Critical 只走 broadcast」）；`bus.rs:59-64` `CRITICAL_TOTAL = 20 = 16(mpsc) + 4(历史 Critical 只走 broadcast: CheckpointSaved / ConsensusReached / SlowConsumerDropped / **OrphanCallDetected**)`；`bus.rs:77` `LANE_FORBIDDEN_SHARD` 列其名，但 `bus.rs:136-156` `is_critical_mpsc_event` 的 matches! 臂**不含**它；发布点 `gatherer.rs:320-332` 用 `self.event_bus.publish(...)`（**非 `publish_critical`**） | 放弃面（abandoned）**唯一的 Critical 归因出口**，却是 4 个"历史 Critical 不入 mpsc 旁路"之一 ⇒ **确保送达通道在此失效** |
| `BudgetExceeded` | **Critical + mpsc 旁路** | `registry.rs:143-145`（注释「标 Normal 会在背压场景被丢弃,导致超限无人响应、Quest 持续消耗直至 OOM」）；`bus.rs:141` 旁路臂；发布点 `crates/decb-governor/src/governor.rs:456-464` `publish_blocking`；`:469-476` Degraded 仍超 ⇒ `Err(DegradedModeRejected)` | 【事实】**`gqep-executor` 内零 `BudgetExceeded` 发布点**——gather 面的并发/内存压力与该预算事件**无连线**。UNLEARNABLE #6（`policy.rs:324-325`）只锁 severity，不锁「gqep 必须发它」 |

⇒ 【推断】**结构性弱点**：gqep 面一旦被并发/超时压力击穿，(i) 批次级信号 `GatherTimedOut` = Normal 可丢；(ii) 唯一 Critical 出口 `OrphanCallDetected` 走可丢的 broadcast；(iii) 预算事件压根不连线。「压力 → 证据」链条在 L7 **三面皆薄**。这正是 Ω₈ 证据纪律要防的形态。

### 2.4 文档—证据链错位（顺带抓出，本 ADR 不修但须登记）
`gatherer.rs:291`、`event-bus/src/types.rs:782`（注释「供 **efficiency-monitor** 等订阅者记录全局超时指标」）等处均主张 `GatherTimedOut` 由 efficiency-monitor 消费。实测：`Grep GatherTimedOut crates/efficiency-monitor` ⇒ **0 命中**；真实消费者是 `crates/chimera-tui/src/data/sync.rs:1546-1581`（`GqepTimeoutStats{per_op, global, orphan}`），并被 `crates/chimera-tui/tests/event_coverage_drift_test.rs:212` 判为 `Consumed`。⇒ 【事实】文档主张的订阅者错位；属 `EvidenceGap` 类（`auditor.rs:341`），登记为待办，不在本草案内改码。

## 3. Decision Options（三选一，附推荐与否决理由）

### Option A — 用 `buffer_unordered(max_concurrency)` 替换现流式
- 直观优点：语义直白、改动集中在 `gatherer.rs:145-164`；仓内 `buffer_unordered` 全库仅 1 处**注释**命中（`config.rs:31`），无实现先例，改动面小是假象
- **致命缺陷（A 级，否决理由）**：`buffer_unordered` 的"未拉取源项"**从未被 poll**。而 `entangle` 是 `async fn`（`protocol.rs:110`），其注册（`:119-127`）与 `OrphanGuard` 创建（`:141-147`）都发生在 **async 体首次执行**；`protocol.rs:283-284` 已把这条写进文档：「**调用者不 await**:`entangle()` 返回的 future 被 drop 而未 poll（注:**此时 `OrphanGuard` 尚未创建,因为 async fn 体未执行**）」。
  ⇒ 全局超时 drop 整个流时，**未启动的 future 静默消失：零 `pending_calls` 条目、零 timer、零 `OrphanCallDetected`**。这与 `types.rs:31-34` 的书面承诺（「被放弃(未完成)的 future 不计入 `succeeded`/`failed`,它们通过 `OrphanCallDetected` 事件(Critical)被独立报告」）**直接冲突**，并使 UNLEARNABLE #3（`policy.rs:321`「零孤儿调用保证(QEEP)不可绕过」）的可观测性**名存实亡**。
- 【判定】**否决**。方案 A 会把"当前完全可观测的放弃面"换成"不可观测的静默丢失"，属**安全回退**，不是限流改进。

### Option B（推荐）— 分批入流（chunk = 上界）+ 显式"未启动"归因
保留 `entangle` + `FuturesUnordered` + 双层超时 + `OrphanGuard` 的现有全部结构（`gatherer.rs:145-164` 语义不动），仅把「一次性全量 push」改为「按上界分批 push、每完成一个补一个」，与仓内**已生效的真限流实现**同构：`crates/pvl-layer/src/concurrency.rs:59`「信号量切片：同时最多 `max_concurrency` 个任务在飞（洪峰防抖）」、`:77-89` 首波 `while next < n && inflight < self.max_concurrency`、`:130-133` 消费后补波、`:116-119` 超时分支 drain 残留并 `saturating_sub` 归零。
- **B 的强制前提（缺一条即退化成 A 的破洞）**：滞留在 backlog（尚未 push 入流的 `Vec` 尾部）中的 future **同样未被 poll** ⇒ 必须与 §4 **I5（三段守恒 + 未启动显式计数与事件出口）**同时落地。
- 优点：最小侵入；不新增跨层依赖边；限流逻辑与孤儿检测时序可**分离验证**；可复用现成探针 `gatherer.rs:337-339` `pending_count()` 做峰值断测
- 代价：需按 I4 重算全局 deadline 与流式时间断言；`GatherResult`（`types.rs:35-51`）需扩"未启动/已准入"口径

### Option C — 删除字段 + 声明「gqep 面不限流，并发由 rayon/pvl 统一治理」
- 效果：诚实消除 `UnusedCapability`（Ω₈ 账面转绿），但红线「并发度限制」**不闭合**；gather 面 N 路同时打开仍无界，`MAX_STEPS = 64`（`tool_plan.rs:127`）只约束 toolplan 面、不约束 `gather_collected` 的入参 N
- **可接受条件**：仅当同时给出**书面二维界证明**（步数 × 累计 token × 墙钟三者独立取严）才成立。现状缺：`docs/architecture/Chimera_CLI_调度层与ADR_从规则集合到调度系统_架构分析与重构方案_v1.0.md:204`【推断】已判「**无累计界**：并发上限与步数上限、墙钟三者不成二维界」，`:344` 把它列为目标态
- 【判定】可作 **fallback**，但必须先补二维界推导，否则等于用文档掩盖红线

### 推荐
**Option B**，以 §4 I1–I11 **全绿**为验收门，其中 **I5 与 I6 为红线级**（缺一即打回）。A 否决。C 仅在 B 被判不可实施且补齐二维界时启用。

## 4. 并发上界的安全不变量清单（实施批必须钉的断测项 I1–I11）

> 纪律：**每条不变量都必须以断言（断测）兑现，不接受注释或文档承诺**。I5 / I6 为红线级。

| # | 不变量 | 断测（建议测试名） | 仓内先例 / 依据（file:line） |
|---|---|---|---|
| **I1** | **下界 ≥ 1**：上界为 0 不得 panic、不得死锁、不得等价"无限制"；构造期钳制为 1 | `test_max_concurrency_zero_clamps_to_one`、`test_max_concurrency_one_equivalent_serial` | `pvl-layer/src/concurrency.rs:45` `max_concurrency.max(1)`；`chimera-mas/src/chunker.rs:250` `clamp(1, MAX_QUADRANT_FANOUT)`；`chunker.rs:720-722` 既有 0⇒1 测 |
| **I2** | **上界有静态帽**：`bound <= GQEP_MAX_FANOUT`（常量入 `nexus-contracts`，与 `MAX_STEPS` 同处），防"配置写 10^9"形同无界 | `test_max_concurrency_capped_to_fanout_ceiling` | `chunker.rs:250`（钳到 `MAX_QUADRANT_FANOUT`=4，`:222` 注明受 INV-3 约束）；`tool_plan.rs:127` `MAX_STEPS=64` 常量入 L0 的先例 |
| **I3** | **内存自适应**：`effective = min(bound, free_mem_mb / op_budget_mb).max(1)`，含除零回退；与 130MB 资源帽呼应 | `test_effective_concurrency_min_formula`（照抄 `chunker.rs:694-711` 四情形，含除零）| `chunker.rs:324-331` `checked_div(...).unwrap_or(max_concurrency)` 后 `clamp(1, max_concurrency)`；`scripts/perf_redlines.toml:62-65` RL-08 `50agent_mem_peak <=130MB` |
| **I4** | **与 `gather_deadline` 协同**：上界收紧后，全局 deadline 不得**必然**误报。要么显式 `deadline' = f(N, bound, per_op)` 并写进 `config.rs:36-49` 文档，要么在类型/文档上声明"上界与 deadline 由调用方共同负责"并给出真实调用方核算 | `test_deadline_scaled_for_two_waves`、`test_bound_n_gt_total_behavior_unchanged`（等价今日） | 现状反证：`gatherer.rs:241`+`:261-282` 绝对 5s、`config.rs:63`；`config.rs:43-45` 主张无机器支撑（§1.3） |
| **I5** �� | **三段守恒 + 未启动显式归因**：`succeeded + failed + abandoned + not_started == total`，且 `not_started` **必须有事件出口**（禁止静默 drop） | `test_conservancy_three_segment_under_bound_and_global_timeout`、`test_not_started_futures_are_attributed` | 破洞来源：`protocol.rs:283-284`（未 poll ⇒ 无 guard，**自述**）+ `:119`/`:143` 注册时机；承诺面：`types.rs:31-34`；现算式：`gatherer.rs:289` `saturating_sub`；红线：`policy.rs:321` UNLEARNABLE #3、`nuxus规则.md:166` 红线 13 |
| **I6** 🔴 | **在飞峰值 ≤ bound**（断**峰值**不是均值） | `test_bounded_concurrency_peak_le_bound`（`AtomicUsize::fetch_add + fetch_max` 峰值锁存） | 现成探针 `gatherer.rs:337-339` `pending_count()`；模板 `pvl-layer/src/concurrency.rs:194-198` + `:184-209` `bounded_concurrency` |
| **I7** | **不新增静默拒绝路径**：上界不得导致"操作凭空消失"；若引入显式准入拒绝（如 `GqepError::AdmissionDenied`），必须（a）枚举扩 5→6 且 `map_gqep_to_qeep` 穷尽 match 显式处理（b）拒绝可归因、非超时、非静默 | `test_admission_denied_is_explicit_and_counted` | 现枚举恰 5 变体：`error.rs:22` `OperationTimeout`/`OperationFailed`/`BatchAtomicFailure`/`OrphanCallDetected`/`GlobalTimedOut`（**无拒绝类变体**）；穷尽 match 面 `gatherer.rs:377-389`（`:383-385` 注释自述"新增错误变体必须显式处理,防止遗漏"）；"拒绝即一等公民"先例 `mas-sched/src/scheduler.rs:136-138（提交 bdc966f 后重取行号）` `ClaimOutcome::Denied(DenyReason::QuotaExceeded)` |
| **I8** | **流式语义不回退为串行**：`bound >= N` 时与今日逐行等价（向后兼容判据）；`bound == 1` 时等价串行且无死锁 | `test_backward_compat_bound_ge_total`、`test_bound_one_no_deadlock` | 现流式断言基线 `tests/concurrent.rs:94-126`（50 fast + 50 slow@50ms，`elapsed < 500ms`）、`:158-174`（100 ops `< 100ms`） |
| **I9** | **既有时间/规模断言须重算，不得为过而放宽阈值**：N=200 测（> 默认 100）会变为 2 波 | `test_concurrent_large_scale_no_orphan`（`:132-153`，含 `:151` `orphan_count()==0`）在限流下仍绿 | `tests/concurrent.rs:135` `(0..200)`、`:149-152`；`tests/proptest.rs:60` 用 `GqepConfig::default()`、`:56-57` n ≤ 25 |
| **I10** | **事件面四处同批一致（若新增/改道事件）**：`severity()` + `is_critical_mpsc_event()` + `CRITICAL_MPSC_VARIANTS` + `CRITICAL_TOTAL` + `LANE_FORBIDDEN_SHARD` + `check_perf_redlines.py` **I8c** 必须同批，不得只改一处 | 复用既有 `test_critical_double_list_d8_counts`、`mpsc_bypass_exact_*`、`variant_count_test.rs` | `bus.rs:42-57`（I8c 说明）、`:57` =16、`:59-64` =20、`:66-77` LANE、`:136-156` matches! 臂 |
| **I11** | **卫生面不回退**：`#![forbid(unsafe_code)]` 保留；无持锁跨 `await`；`await_guard_allowlist.txt` **不得新增豁免**；proptest 生成域须扩到"含慢 future + 收紧 deadline"，并把两条守恒断言改为**三段守恒**（而非改松） | `cargo clippy -D warnings` + `check_parallel_redlines.ps1` + 改造后的 `test_gather_succeeded_plus_failed_equals_total` | `policy.rs:323`（forbid unsafe 不可移除，UNLEARNABLE #5）；`scripts/check_parallel_redlines.ps1:58-84` 四禁令；`tests/proptest.rs:74-81`、`:86-90`（现两断言，见 §7 R3） |

### 4.1 形式化归属（不改 `PropertyCategory` 枚举）
并发上界不变量归 L0 FormalVerifier 的 **`InvariantPreservation`** 类别（`crates/nexus-contracts/src/formal_props.rs` 现有 4 变体：`LineageIntegrity` / `ScoreMonotonicity` / `ConsensusSafety` / `InvariantPreservation`）。⇒ 【建议】实施批以 `formal_props.rs` 属性 + proptest 双层落 I5/I6，**不新增枚举变体**（避免触 `check_formal_property_ledger.py` 台账面）。


## 5. Consequences（影响面）

| 维度 | 影响 | 证据 / 处置 |
|---|---|---|
| 执行语义 | 并发上界首次成为真约束；「层内全并行」在 gather 面变为「有界并行」 | `toolplan_runner.rs:111-113` 的措辞需在实施批同步更正，否则文档与实现口径分叉 |
| 孤儿检测时序 | **最高风险面**：注册发生在首次 poll（`protocol.rs:119`/`:143`，自述 `:283-284`） | 由 I5 硬约束；任何"未 poll 即 drop"必须显式归因 |
| 双层超时交互 | 内层 `default_timeout_ms`（`protocol.rs:152`）逐操作；外层 `gather_deadline_ms` 绝对（`gatherer.rs:241`/`:282`） | 由 I4 协同；`deadline_ms == 0` 禁用分支（`gatherer.rs:245-259`）须定义限流下的语义 |
| 性能 | 吞吐/延迟双刃（详见 §7 R1/R4） | **必须 criterion 同轮配对实测；未实测数字不得入库**（`ADR-195` D7 反幽灵 + `p1-preflight0` 诚实数据纪律） |
| 依赖方向 | 不新增跨层边；`GQEP_MAX_FANOUT` 若入 `nexus-contracts` 属既有向下依赖 | 合规；须过 `check_dev_dep_direction.py` / `dep_edge_freeze.txt` |
| 向后兼容 | 真实消费者仅 doctor（N=11，`doctor.rs:175`），`GqepConfig` serde 面不变 | `GatherResult`（`types.rs:35-51`）**未标 `#[non_exhaustive]`**，新增字段属结构性变更 ⇒ 【建议】优先 `#[non_exhaustive]` 或新增 `gather_stats_v2()` 而非直接加字段 |
| 治理账 | 红线「并发度限制在 gqep 面不生效」由**公开但不生效**转为闭合 | 现状判定见 `docs/architecture/Chimera_CLI_调度层与ADR_…_v1.0.md:201`【事实】、`:549` (#29) 三态总表 |


### 5.1 关联 / 一致性（Spec 点名项）

- **不新增第二派发入口**：本 ADR 只收紧既有 gather 面的在飞上界，不新建调度器 / crate / 入口（`…重构方案_v2.0.md:68`）。**派发唯一入口既有权威 = ADR-127**（实体归并在合并档 `ADR-095-134-phase1-consolidation.md:43`「RuntimeSwitcher 合并入 ComputeBridge L-f route()」，依 `adr_index.md:10` 合并档定位规则，**单独 glob 必落空非缺失**）+ **ADR-196 D1**（`ADR-196…:83` 逐字「ADR-127 已裁决唯一入口」，Accepted 2026-10-07）。二者论域为 **CPU 计算派发面**（`ComputeBridge::route()`/`TaskKind`/`HtsTable`）；本 ADR 论域为 **L7 gqep 聚集面**（`gqep-executor/Cargo.toml:9-27` 不经 ComputeBridge），故**不落入 127/196 论域、不构成第二派发入口**、不改其任何阈值/表项。〔**勘误**：本席初稿与主协调复核均曾误判「ADR-127 查无实体」，实为合并档定位规则所漏，architect 会签纠正。〕
- **OMEGA**：Ω₁-Sparse（§2.1）、Ω₄-Event、Ω₈-Assess（§2.2）；**不动 Critical mpsc 双清单**（§2.3 + I10），不改 `severity()`/`is_critical_mpsc_event()`/`CRITICAL_MPSC_VARIANTS=16`/`CRITICAL_TOTAL=20`。
## 6. 与既有规范文本的冲突裁决（不可回避）

`agents.md:415` 现规定：「**并发收集用 `FuturesUnordered`** — 优于 `join_all`,减少内存占用,支持流式结果」。表面与本 ADR（引入 `buffer_unordered` / 分批）存在张力。

【裁决】本 ADR **不否决** `agents.md:415`：
1. 该条约束的是**数据结构选型**（`FuturesUnordered` vs `join_all`），不构成「必须一次性全量入流、不得设上界」的授权；
2. Option B **保留** `FuturesUnordered` 作为流式载体，只改入流节奏，与该条完全相容；
3. Option A（`buffer_unordered`）之所以否决，理由不是"违反 `agents.md:415`"，而是它破孤儿归因（§3-A）——**否决依据必须是安全语义而非文本教条**。
⇒ 【建议】实施批给 `agents.md:415` 追加限定语「并发收集用 `FuturesUnordered`，**并须施加并发上界**」；本席不改码，登记为待办。

## 7. 风险提示（拒绝 / 饥饿 / backpressure）

| # | 风险 | 判定 | 机理与证据 | 缓解 |
|---|---|---|---|---|
| **R1** | **GlobalTimedOut 级联（吞吐自杀）**：bound 越小 ⇒ 完成越慢 ⇒ 绝对 5s deadline 越易触发 ⇒ `abandoned` 激增 ⇒ `OrphanCallDetected` 风暴 | 【推断】中-高 | `gatherer.rs:241`/`:282`（绝对 deadline）+ `:289`（abandoned 算式）+ `:292-298`/`:320-332`（两类事件）；默认值 `config.rs:61-63` 双 5000ms 无缩放关系 | **I4** 协同 + 默认 bound 保守不动（100）+ 上界只在内存压力时自适应下调（I3） |
| **R2** | **孤儿归因破洞（红线级）**：未 poll 的 future drop 后**零报告**，放弃面从"可观测"退化为"静默丢失" | 【事实】机理已在源码自述 | `protocol.rs:283-284` + `:119`/`:141-147`；冲突承诺 `types.rs:31-34`；红线 `policy.rs:321` | 否决 Option A；**I5** 为红线级验收门（先落归因，再落限流） |
| **R3** | **现有守恒断言本已"条件成立"**：限流把 slow 路径推进全局超时，两条 proptest 断言随之失效 | 【事实】 | `tests/proptest.rs:74-81`（`succeeded+failed==total`）与 `:86-90`（`errors.len()==failed`）在 `GqepConfig::default()`（`:60`）、n≤25、全快 future 下不触全局超时；而 `types.rs:45-49` 已自述全局超时时 `errors.len() == failed + 1`（`gatherer.rs:306-309` push `GlobalTimedOut`）⇒ 该例外今日即存在，只是测不到 | **不得改松断言**；改为三段守恒（I5）并扩生成域含慢 future（I11） |
| **R4** | **尾部饥饿**：真正饿死点不在 gather 内部（`FuturesUnordered` 无公平性但会 poll 全部），而在**外层 deadline 提前放弃靠后的波次** | 【推断】 | `gatherer.rs:265-282`（单一全局 deadline 覆盖所有波）；`concurrent.rs:94-126` 现仅测单波余量 | I4 + 固定 FIFO 波序 + 记录每波剩余预算；禁止按结果价值重排（引入不确定性） |
| **R5** | **背压可丢性**：把压力证据寄托在两个"可丢"信道上 | 【事实】 | `GatherTimedOut` = **Normal**（`registry.rs:186`）⇒ Lagged/降采样可丢；`OrphanCallDetected` = Critical 但 **broadcast-only**（`bus.rs:61-64`、发布用 `publish` 而非 `publish_critical`，`gatherer.rs:328`）；本仓背压策略面 `backpressure.rs`（`LagThreshold{max_lag:256}` / `DropOldest` / `CriticalMpsc{4096}`） | 【建议】验收判据用**同步断测**（I6 峰值断言、I5 守恒断言），**不用事件订阅**做门；本 ADR **不擅自改** 4 个历史 Critical 的通道归属（属既定设计推演 D-8，改动触 I10 四处同批，须另立 ADR） |
| **R6** | **口径不一致（第二面遗留）**：只改 gatherer，`toolplan_runner` 每层仍无界 | 【事实】 | `toolplan_runner.rs:138-153`；仅 `MAX_STEPS=64` 间接界（`tool_plan.rs:127`/`:148-149`） | 同一 ADR 内声明两面上界语义，或显式登记 follow-up + 现间接界推导 |
| **R7** | **拒绝路径扩张**：若新增 `AdmissionDenied` 类错误 ⇒ 触达穷尽 match 面与错误语义映射 | 【建议】 | `error.rs:22` 现恰 5 变体、无拒绝变体；`gatherer.rs:377-389` 必须穷尽处理 | 优先**不新增拒绝变体**（限流是"延后"而非"拒绝"，与 `mas-sched/scheduler.rs:136-138` 的显式拒绝语义区分开）；确需则走 I7 |

## 8. 安全判定：收紧防御（安全增益）vs 引入新拒绝路径（DoS 自伤 / 饥饿）

**结论**：【判定】**主体是收紧防御（安全增益）；但在未配套 I4/I5 时，它会同时引入一个新的失败面。二者不是二选一——"是否增益"取决于**是否以 I5/I6 为前置门**，而非取决于限流本身。**

增益侧证据（"仓内已有同姿态先例"）：
- 同一条 1M 红线在 L4 已以**硬常量上界**形式生效：`bus.rs:34-40`（4096 容量帽 + 注释直指 §6.1 红线）⇒ 给并发面上界是**补齐同类既有实践**，不是新发明
- OSA 的「压力 → 收紧」是仓内既定安全姿态且**到界 no-op 而非报错**：`osa-coordinator/src/six_dimension.rs:16`（`BudgetExceeded` → `max_tools_per_step −1`）、`:257-266` `tighten_tools_per_step` 带下限 `min_max_tools_per_step`、到界返回 no-op ⇒ 收紧**本身不被建模为拒绝路径**，这正是 gqep 应当采用的语义
- 并发上限作为「控制面硬约束、非丢失」的先例：`mas-sched/src/scheduler.rs:370`（测注「其余 48 被 QuotaExceeded 拒绝（并发上限是控制面硬约束,非丢失）」）——注意其前提是**拒绝可归因**（`:136-138` 显式 `Denied`），gqep 若无 I5 就不满足这个前提
- 线程预算侧的先例是**主动预留而非满载**：`nexus-core/src/compute/bridge.rs:560-571` `compute_threads()` = `(n−2).max(MIN_THREADS)`，注释「预留 2 核给 tokio worker,防 CPU 计算饿死 IO」

自伤侧证据：
- 绝对 deadline 不缩放（§1.3）⇒ R1 级联；未 poll 零归因（§3-A）⇒ R2 破洞
- 【判定】把"资源耗尽"故障换成"批量放弃 + 归因缺失"故障 = **净损失**；故 I5/I6 必须是红线级门，缺一打回

不做会怎样（现状代价）：
- 红线继续"公开但不生效"（`config.rs:30-34` 自陈 + `…v1.0.md:201`【事实】），Ω₈ `UnusedCapability` 挂账（`auditor.rs:342`），未来调用方 N>100 无防护，`toolplan_runner` 第二面同构缺口持续（§1.4）

## 9. 边界声明（不可自决 / 本席权限边界）

1. 本 ADR 为**草案**：Status = Proposed，**不合入、不标 ACCEPTED、不改 `gatherer.rs` / `config.rs` / `toolplan_runner.rs` 任何语义**。本轮工作树唯一变更是 Phase 1 两个已验修复提交（`bdc966f` mas-sched / `b3b67a9` nexus-core），与本 ADR 无关。
2. 实施须人类 **ACCEPTED** 后另起独立批次——依据 `docs/reports/p1-preflight0-correctness-fixes-2026-10-11.md` §3.3 判定：让 `max_concurrency` 生效「须同时触碰 entangle 孤儿检测时序 / 双层超时交互 / 流式收集与层内全并行执行模型……属**生产 gather 执行语义变更**」，故「**未实现——需 ADR，不擅自改**」。
3. 不动 `.gitignore:54` 的 blanket `*.md` 策略（治理决策，需另批授权）⇒ 本 ADR 与报告**本地保留、不入远程**，`adr_index.md` 亦同。
4. 不触 R2 冻结面（ADR-042 无条件冻结 / ADR-053 rev4 解冻程序）：本 ADR 与 gsoe-evolution / auto-dpo 无关，且**未使用**任何 R2 扫描关键词。
5. 不擅自改 Critical 通道归属（`OrphanCallDetected` broadcast-only → mpsc 会触 I10 四处同批 + 属推演 D-8 既定设计），如需改动另立 ADR。
6. 未实测性能数字不入库（本文一律标【待验证】）。

## Annex A — 红线出处 ↔ 现状差距对账表（本席输出 (a)）

| 红线 / 权威出处 | 现状态 | 差距性质 |
|---|---|---|
| §6.1「1M Token 暴力加载」——`agents.md:510`；已生效硬上界先例 `event-bus/src/bus.rs:34-40` | gather 面全量入流无界：`gatherer.rs:145-158`；上界字段死：`config.rs:33-34` | 【事实】**同一红线，L4 生效 / L7 失效** |
| §6.2 红线 13「孤儿调用(异步无 GQEP 聚集/超时)」——`nuxus规则.md:166` | 聚集/超时**有**；但限流后"未 poll 即静默 drop"成为新孤儿形态：`protocol.rs:283-284` | 【推断】红线**精神**风险，需 I5 闭合 |
| UNLEARNABLE #3「零孤儿调用保证(QEEP)不可绕过」——`seccore/src/policy.rs:321`（`:338` 锁 len=6） | 今日满足（全量 poll ⇒ 全可观测）；**方案 A 会破坏它** | 【事实】方案 A = 安全回退，故否决 |
| UNLEARNABLE #6「BudgetExceeded severity = Critical 不可降级」——`policy.rs:324-325`；定级 `registry.rs:145`；旁路 `bus.rs:141` | 定级合规；但 **gqep 面零发布点**（发布仅见 `decb-governor/src/governor.rs:456-464`） | 【事实】压力↔预算事件**无连线**（不降级、只缺连线） |
| Ω₁-Sparse「全维稀疏 + 按需激活」——`nuxus规则.md:51`；预算维 `parallel.rs budget_mask_core`、`six_dimension.rs:16` | gather 面不依赖 OSA、不消费掩码 | 【推断】Ω₁ 在 L7 **从未接线** |
| Ω₈-Assess 证据纪律——`nuxus规则.md:58`；`auditor.rs:341-343` | `max_concurrency` = `UnusedCapability` 教科书案例（配置+serde 测在、消费点 0） | 【事实】判不合规，且 `config.rs:30-34` 已自认 |
| 机器门禁「并发度限制」 | `scripts/perf_redlines.toml` RL-01~11（`:62-65` RL-08 是唯一资源帽）**无并发条目**；`check_parallel_redlines.ps1:58-84` 四禁令**不含上界**；`tests/security/owasp_top10.rs` A05 映射为"安全配置错误"非 DoS | 【事实】**该红线无机器门**（这也是它长期挂账不红的根因）⇒ 【建议】另立门号 + 新 ADR 取号（203+） |
| 二维终止界（步数 × token × 墙钟） | `…v1.0.md:204`【推断】「无累计界」；`:344` 目标态 | 【事实】Option C 的前置条件不满足 |

## Annex B — 交叉验证分工（本批实际状态）
- `chimera-rust-engineer`：✅ 已交付三方案实现取证（落点/改动面/与 entangle+双层超时+孤儿检测的兼容性，独立亦判 A 因未 poll 破孤儿归因、推荐分批实现 B）
- `chimera-security-engineer`（本席）：✅ 红线↔差距对账(Annex A) + I1–I11 + A 否决承重证据 `protocol.rs:283-284`
- 主协调：✅ 独立读源复核承重项——`seccore/policy.rs:321` UNLEARNABLE #3 逐字、`qeep-protocol/protocol.rs:283-284`「未 poll 无 OrphanGuard」自述、`gqep-executor/types.rs:31-34`「被放弃经 OrphanCallDetected 独立报告」承诺、`mas-sched/scheduler.rs:138` 行号 post-commit 复核
- `chimera-architect`：✅ 会签——取号程序合规；但 §5.1「ADR-127 查无实体」**为误判**（实体在合并档 `ADR-095-134:43`，承继经 `ADR-196:83` 取证），并揪出 `gather_atomic`（`batch.rs:85-124`）**既存**未-poll 零归因（测 `:262-269` total=10/succeeded=4/failed=1 ⇒ 5 op 静默消失）⇒ **I5 须覆盖 gather/gather_collected/gather_atomic 三入口**；A 否决理由由「必然破 #3」改为「可证性更弱 + 零先例 + 结构重构面」；编号预留表须补全对向清单（总账:51/299、w66:24/192、fusion:102、CHANGELOG:31）。
- `chimera-perf-engineer`：✅ 会签——三轴影响定性（内存↓活跃/延迟 bound≥N 不变·bound<N 上升/吞吐两可）；criterion 方案 = `gather_bounded_vs_unbounded` 同轮双组交替，**测点必扩到 N>bound + bound<100**（现 `benches/gather.rs` 的 10/50/100 全 ≤100 ⇒ 对默认 bound 限流零可观测）；「无退化」仅钉 bound≥N 区 + I6 在飞峰值（`fetch_max` 主、`pending_count()` 降为校验）；数值阈值挂 CI 首跑关 `calibration_pending` 后落地，本地 100ms 兜底与未实测数字不入库。

## Annex C — 未决待裁决（实施批前须闭合）
1. 是否引入显式拒绝变体（触 `error.rs:22` 5→6 + `gatherer.rs:377-389` 穷尽 match）——本席倾向**否**（限流 = 延后，非拒绝）
2. `GQEP_MAX_FANOUT` 静态帽的具体取值（须与 RL-08 130MB / `doctor` N=11 余量、以及 R1 联动推导，**不接受拍脑袋数字**）
3. 门禁落地：是否另立 RL/SLO 并发条目 + 门号（新 ADR 取号自 203+，先查 `adr_index.md`「编号预留登记」）
4. `OrphanCallDetected` 通道归属是否随本面一并复议（本 ADR 默认**不动**）
5. 【architect 会签补】`gather_atomic`（`batch.rs:85-124`）既存未-poll 零归因：I5 三段守恒须扩至 gather/gather_collected/gather_atomic **三入口**，或显式登记为「同 UNLEARNABLE #3 残留缺口 + 后续门号」
6. 【architect 会签补】契约变更登记：入参 future 由「必被 poll」变「可能完全不执行」，须在 `lib.rs`/`gatherer.rs` doc 明示，且 not_started 兼具**重试幂等判据**（副作用/计费调用方须能区分"未执行 vs 执行失败"）
7. 【architect 会签补】编号 202+ 段现有**三**下游认领者（层表 SSOT 全表裁决 / GPU 解冻 supersession 链 / 并发门号）——ACCEPTED 须所有者认证 + 回写对向源 + index 预留表把 203/204 显式分派，防 `adr_index.md:24` 警告的撞号复发

## 10. 实施蓝图（ACCEPTED 后另批；本批不落码）
> 把 Annex C 的 C2/C6/C7/M5/M7 收敛为**可据以实施 + 可断测**的次序；**先断测转绿、再落限流**，缺 I5/I6 不放行（architect C 系前置）。

### 10.1 分批入流骨架（Option B 具体化，gatherer.rs:145-158 一处）
- 以 `effective = clamp(1, min(bound, mem_adaptive))` 为上界，对 `futures: Vec<GqepFuture<T>>` 按 `bound` 切片；每片仍构造 `FuturesUnordered` + `entangle` 包裹 + 现有 `collect_with_deadline`，**批间共享一个绝对 deadline 时刻**（`Instant`，非每片重置 `gather_deadline_ms`）——即 I4：总墙钟上界恒 = 原 deadline，杜绝 `deadline × 批数` 放大。`batch.rs:85-124` 的 `gather_atomic` 早退 skipped 面同批纳入 I5 记账（Annex C-5）。
- `not_started` = `total - (succeeded + failed + abandoned)`，落 `GatherTimedOut` **字段级扩展**（禁新变体，承 M5/ADR-072）；`bound >= total` 路径 = 今日逐位一致（I8 向后兼容判据，含生产 doctor N=11）。

### 10.2 红线级门 I5 / I6（先写断测→红→实现→绿）
- **I6 在飞峰值 ≤ bound**：在 `entangle` 包裹体进出处 `AtomicUsize::fetch_add/fetch_sub + fetch_max` 锁存峰值为主判据；`gatherer.rs:337` `pending_count()` 降为交叉校验（M7）。
- **I5 三段（四段）守恒** `succeeded + failed + abandoned + not_started == total` 覆盖 gather / gather_collected / gather_atomic 三入口；`not_started` 兼具**重试幂等判据**（副作用/计费调用方须区分"未执行 vs 执行失败"，C6）。

### 10.3 criterion 判据（未实测数字不入库）
`gather_bounded_vs_unbounded` 同轮双组交替，测点扩到 N>bound + bound<100；"无退化"仅钉 bound≥N 区；数值阈值挂 CI 首跑关 `calibration_pending` 后落地（perf 会签）。

### 10.4 门禁（不可自决）
- **本批不落 gqep 码、不标 ACCEPTED**。用户选 D=欲认证；但认证=所有者决策、architect 列 C1–C10 为 ACCEPTED 前置。实施须**用户显式确认 ACCEPTED 并授权**后另起批次，按 10.1→10.2（先断测绿）→10.3 次序落地。

## 11. 决策记录（ACCEPTED 与实施批，2026-10-11）

### 11.1 边界释放与授权链
本节所在的**实施批**经计划批准取得用户显式授权（原话选定：「本批：B 全做 + A 只交决策包」），从而释放 §9.1 与 §10.4 的「本批不落码、不标 ACCEPTED」草案期约束。该约束在草拟时点正确且**不得静默覆盖**，故以本节显式承接：认证属所有者决策，本席仅在收到授权后才落码。

### 11.2 采纳与否决
- **采纳 Option B**（分批入流 + `not_started` 显式归因）：与 `pvl-layer/src/concurrency.rs:77-89` 已生效真限流同构，不新增跨层依赖边。
- **维持否决 Option A**（承重理由未变）：`buffer_unordered` 未拉取源项从未 poll ⇒ 无 `OrphanGuard`（`protocol.rs:283-284` 源码自述）⇒ 放弃面静默丢失，触 `policy.rs:321` UNLEARNABLE #3。**属安全回退而非限流改进。**
- **Option C 不成立**：`v1.0.md:204` 已判无累计界，二维界前置不满足。

### 11.3 Annex C 逐条闭合
| # | 待裁决项 | 本批裁定 | 依据 |
|---|---|---|---|
| C1 | 是否新增显式拒绝变体 | **否** | 限流语义为「延后」非「拒绝」；`error.rs:22` 保持 5 变体，`:377-389` 穷尽 match 不动 |
| C2 | 静态帽取值 | **256**，且**定义在 gqep-executor 本 crate**（偏离见 §11.4-D1） | 见 §11.4-D2 推导 |
| C3 | 是否另立 RL/SLO 并发条目 + 门号 | **本批不申请** | RL 阈值需 criterion 实测，本批不发明数字；I5/I6 以 `cargo test` 断测进 CI test job 即已构成机器门 |
| C4 | `OrphanCallDetected` 通道归属 | **不动** | 改动触 I10 四处同批，属推演 D-8 既定设计，需另立 ADR |
| C5 | I5 覆盖三入口 | **已覆盖** gather / gather_collected / gather_atomic | `batch.rs:85-124` 早退 skipped 面（architect 会签补登） |
| C6 | 契约变更登记 | **已登记** | `types.rs` 结构体注释 + `gatherer.rs`/`lib.rs` 函数文档均明示「入参 future 可能完全不执行」，并声明 `not_started` 具重试幂等判据 |
| C7 | 编号治理 | **不占用 203–209 预留段** | 本批无新 ADR（帽常量不入 L0、不申请门号）；ADR-211 属改写现有档而非新取号 |

### 11.4 两处有据偏离（与本文草拟建议不一致，必须留下理由）
**D1：帽常量不入 `nexus-contracts`，改落 `gqep-executor`**（I2 草拟建议为「常量入 L0 与 `MAX_STEPS` 同处」）

机械门取证定死了这个选择：`scripts/gate_manifest.toml:1002` G-91 note 逐字——nexus-contracts 全部 pub 类型按**消费方 crate 数**统计，`<2` 者须入 `scripts/contract_consumer_baseline.txt` 只减登记册，「**NEW 未登记必红**」。`MAX_GATHER_FANOUT` 只有 gqep-executor 一个消费者 ⇒ 若入 L0，**必然要求新增一行只减台账**，直接违反 v2.0 §9-6「台账只减不增」与优先序「删除>合并>下沉>抽象>新增」。单消费者常量本就不该进契约层。

**D2：帽值 256 的推导（拒绝拍脑袋）**
- 下界约束：现默认 `max_concurrency = 100`（`config.rs:62`），帽低于默认会静默改写默认语义，违 I8「`bound >= N` 逐位等价」⇒ 帽必须 `>= 100`。
- 先例对齐：取仓内既有上限常量同值 `nexus-contracts/src/quota.rs:97 MAX_TASK_QUEUE_LENGTH = 256`。
- 职能界定：I2 静态帽的职责是**截断 10^9 型误配置**（「形同无界」），**不是资源帽**；资源帽属 I3。
- **不收紧到 `<= 50`**：RL-08（`perf_redlines.toml:62-65`）测的是 MAS 50-agent 路径，**不是 gqep 单操作**；gqep 无 per-op 内存实测 ⇒ 按 §8-bis「未实测数字禁入 perf 三表」不得据此推帽。

**D3：I3 内存自适应本批不实现**（从 I1–I11 清单中显式挂起一项，而非谎称已做）
- gqep 无 per-op 内存预算实测；
- 仓内同类实现 `chimera-mas/src/chunker.rs:324 effective_concurrency` **自身零生产调用方**（其宿主 `BatchExecutor` 无消费者），复制它等于复制病症而非继承实践；
- 引入 sysinfo 探针到 L7 gather 路径属新增依赖且无实测支撑。
⇒ 本批只落 I1（下界钳制）+ I2（静态帽），I3 登记为「待 per-op 实测后另批」。

### 11.5 未闭合项（如实）
1. RL/SLO 并发阈值待 criterion 实测后入库（本批测点已按 perf 会签扩到 `N > bound`）。
2. I3 内存自适应待 per-op 实测。
3. R6 第二面 `toolplan_runner.rs:138-153` 层内无界仍开放（本批已在文档口径上声明两面差异，未改其语义）。
4. R1 GlobalTimedOut 级联的烈度只由断测边界护住，未做生产带实测。
