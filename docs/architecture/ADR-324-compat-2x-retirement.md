# ADR-324 — compat-2x 事件类型 re-export 退役（v2.31.0-omega）

- **状态**: ✅ Accepted（2026-11-19 登记；同日完成全限定路径清零，退役动作已解锁 —— 见 §1.3-B 的 2026-11-19 追记）
- **架构层**: L0-B 核心基础面（`event-bus` 的兼容垫片；被转出方为 `nexus-events`）
- **取代/关联**: 落实 ADR-322 §1.3「一个发布周期的纯路径 re-export」承诺；关联 ADR-312（event-bus 撤主张面退役的零消费判据先例）、ADR-311 D2.1（零生产装配面处置判据）、ADR-242（回滚开关统一台账的登记先例）、ADR-160（可达性棘轮）
- **编号说明**: 本 ADR 取号前实测 `ls docs/architecture/ADR-32*.md` 现取最大为 ADR-323，且 `Select-String -Path docs/architecture/adr_index.md -Pattern 'ADR-324'` = **0 命中**（2026-11-19），故 ADR-324 未被占用。

## 1. Context

### 1.1 ADR-322 的承诺

ADR-322（2026-11-18）把事件目录（`NexusEvent` 155 变体 + `payloads/` + `registry.rs` + `EventTopic`）从 `event-bus` 整面迁出为独立 crate `nexus-events`，并在 `event-bus/src/lib.rs` 保留**纯路径 re-export（compat-2x）**，承诺「退役时点：v2.31.0-omega」，届时下游按 `nexus_events::` 机械改写后**整段删除**。

### 1.2 compat-2x 现取清单（2026-11-19）

现取命令：

```powershell
Select-String -Path crates/event-bus/src/lib.rs -Pattern 'compat-2x'
Get-Content crates/event-bus/src/lib.rs | Select-Object -Skip 207 -First 12
```

**根路径 re-export（`crates/event-bus/src/lib.rs:214-218`）= 16 个符号**：

| # | 符号 | # | 符号 |
|---|---|---|---|
| 1 | `ActionSource` | 9 | `EventMetadata` |
| 2 | `AgentStatus` | 10 | `EventSeverity` |
| 3 | `BudgetMetricsPayload` | 11 | `NexusEvent` |
| 4 | `ChatStatus` | 12 | `QuestStatus` |
| 5 | `ClvSummary` | 13 | `RouterStatsPayload` |
| 6 | `CompatLevel` | 14 | `TaskPriority` |
| 7 | `ConsultUrgency` | 15 | `TuiChatMessagePayload` |
| 8 | `CriticalEventDropped` | 16 | `VoteValue` |

> 计数订正：ADR-322 §1.3 与 §0 背景写「15 项」，**现取为 16 项**（逐个数清，见上表）。ADR-322 该数字为登记时点笔误，本 ADR 以代码为事实源。

**别名模块（同批待删）**：

| 路径 | 载体 | 内容 |
|---|---|---|
| `event_bus::types::*` | `crates/event-bus/src/types.rs:10-11` | `pub use nexus_events::payloads::*;` + `pub use nexus_events::types::*;` |
| `event_bus::payloads::*` | `crates/event-bus/src/payloads.rs:8` | `pub use nexus_events::payloads::*;` |
| `event_bus::registry` | — | 迁出前即为**私有** `mod registry`（零 pub 项），无垫片可删 |

**`topic` 模块的特殊性**：`crates/event-bus/src/topic.rs` **不是纯别名模块**——它承载运行时类型 `FilteredSubscriber`（持有 `EventReceiver`），只在 `:25` 有一行 `pub use nexus_events::topic::EventTopic;` 路径转发。退役批须**保留该模块**，仅摘除该转发行，并把 `lib.rs:207` 的 `pub use topic::{EventTopic, FilteredSubscriber};` 收敛为 `pub use topic::FilteredSubscriber;`（`EventTopic` 改由下游直连 `nexus_events::topic::EventTopic`）。具体行号以退役实施批实测为准。

### 1.3 「零生产消费者」取证（2026-11-19 现取）

**A. `use` 语句面（L1-E-2 机械改写已完成）**：

```powershell
py -3 scripts/migrate_l1_event_imports.py --check
```

输出（原文）：`[CHECK] 0 use statement(s) pending across 0 crate(s); 0 symbol(s) migrate` / `[OK] no downstream import still reaches the event catalogue through event_bus` → **EXIT=0**。

**B. 全限定内联路径面（未清，阻塞退役）**：`migrate_l1_event_imports.py` 的显式边界是「只改 `use` / `pub use` 行；**full paths are untouched**」（脚本 docstring 第 28 行）。对全限定路径 `event_bus::<符号>` 现取（排除 `event-bus` / `nexus-events` / `nexus-contracts*` 自身，排除 `#[cfg(test)] mod X;` 外部测试模块，排除注释行）：

```powershell
# 现取脚本（临时内联，不落盘）: 对 crates/*/src/**/*.rs 逐行匹配 event_bus::<16 符号> 与 event_bus::{types,payloads}::
```

结果：**89 处生产引用 / 29 文件 / 12 crate**（2026-11-19 实测）：

| crate | 命中 | crate | 命中 |
|---|---|---|---|
| `chimera-tui` | 21 | `parliament` | 6 |
| `chimera-mas` | 15 | `decay-engine` | 4 |
| `efficiency-monitor` | 15 | `nexus-core` | 4 |
| `chimera-cli` | 11 | `seccore` | 4 |
| `repo-wiki` | 3 | `faae-router` | 2 |
| `mcp-mesh` | 2 | `mlc-engine` | 2 |

按符号拆分（生产面，已剔注释）：`NexusEvent` 27 · `EventMetadata` 24 · `ClvSummary` 12 · `event_bus::types::`（别名模块）12 · `QuestStatus` 4 · `RouterStatsPayload` 4 · `EventTopic` 3 · `ActionSource` 2 · `AgentStatus` 1 · 其余 7 个符号 0。合计 **89**。

**结论（如实登记）**：**「compat-2x 今日已零生产消费者」不成立**。`use` 语句面为 0 仅说明 L1-E-2 的机械改写覆盖了 import 行；**全限定内联路径仍有 89 处真实编译依赖**，今天删除 compat-2x 会使 workspace 编译失败。因此本 ADR 是**退役预案登记**，不是可立即执行的退役命令。

### 追记（2026-11-19，同日清零）

登记后发现 `migrate_l1_event_imports.py` 的边界只覆盖 `use` 行。当批即扩展该工具并执行了最后一公里：

- **工具扩展**：新增 `rewrite_fullpaths()` 与 `FULLPATH_RE`，识别 `event_bus::[types::|payloads::|topic::]Sym` 形态，保留模块前缀（`nexus_events::types::X`），跳过注释行；selftest 由 21 条断言增至 **27 条**（新增：内联改写、前缀保留、运行时类型不动、注释不误改、内联计数、二次幂等）。
- **依赖补齐**：`nexus-core` 增 `nexus-events = { workspace = true }`（此前 33 个 crate 已在 L1-E-2 补齐，`nexus-core` 是唯一漏项）。台账 `dep_edge_freeze.txt` 同批登记该边并写明「类型消费者不应绑定 bus 运行时」的理由。
- **执行结果**：`--apply` 改写 **29 文件**；`--check` 现为 **0 use + 0 inline / 0 crate**。
- **编译实证**：`cargo check --workspace --all-targets --jobs 1` = **0**（7m57s，0 error）。
- **复测口径**：`py -3 scripts/migrate_l1_event_imports.py --check` 退出码 0 即代表两类面（`use` 行 + 全限定内联）**双双归零**。

因此 §3 前置清单第 2 项 **已满足**，本 ADR 转 Accepted；退役动作可在 v2.31.0-omega 版本推进时直接执行。

## 2. Decision

**在 v2.31.0-omega 删除 `crates/event-bus/src/lib.rs` 的 compat-2x 段，并删除 `types` / `payloads` 两个纯别名模块；`topic` 模块保留（仅摘除 `EventTopic` 转发行）。**

删除范围（实施批须逐项核对）：

1. `lib.rs` 的 `// === compat-2x:纯路径 re-export(ADR-322,v2.30 引入 / v2.31.0-omega 移除)===` 注释块与随后的 `pub use nexus_events::{...}`（16 符号，现取 `lib.rs:208-218`）；
2. `lib.rs` 的 `pub mod payloads;` / `pub mod types;` 模块声明（`lib.rs:145` / `:166`）与两文件本体；
3. `topic.rs:25` 的 `pub use nexus_events::topic::EventTopic;`，及 `lib.rs:207` 对 `EventTopic` 的再导出；
4. `prelude` 中经 `crate::types::{...}` 取名的 12 个事件符号（`lib.rs:229-232`）改为 `nexus_events::` 路径（或直接从 prelude 摘除——**保留哪些属实施批裁决**，不在本 ADR 预判）。

**不删除**：`event-bus` 的运行时面（`EventBus` / `EventReceiver` / `FilteredSubscriber` / 背压 / 分片 / mpsc 旁路 / `experience_card_bus` 等）。

## 3. 退役前置检查清单（可机器执行，逐条）

| # | 检查项 | 命令 | 通过判据 | 2026-11-19 现取 |
|---|---|---|---|---|
| 1 | import 语句面零残留 | `py -3 scripts/migrate_l1_event_imports.py --check` | EXIT=0 | ✅ 0 pending |
| 2 | 生产面全限定路径零残留 | `py -3 scripts/migrate_l1_event_imports.py --check`（工具已扩围覆盖两类面） | EXIT=0 且报告 `0 use + 0 inline` | ✅ **0 use + 0 inline / 0 crate**（2026-11-19 同日清零，29 文件已改写） |
| 3 | 删除后全仓编译 | `cargo check --workspace --all-targets` | EXIT=0 | 未取数（删除后才有意义）；**改写后**已实测 = 0（7m57s） |
| 4 | 签名泄露面 | `py -3 scripts/check_pub_api_leak.py` | EXIT=0 | 未执行（删除后复测） |
| 5 | 依赖边棘轮 | `py -3 scripts/check_dep_edge_freeze.py` | EXIT=0；若删别名模块导致边变化须同批登记 | ✅ 198/198 绿（含新增 `nexus-core\|nexus-events` 边） |
| 6 | 版本推进三方一致 | `Cargo.toml` / `CHANGELOG.md` / `docs/architecture/README.md` / `.claude/CLAUDE.md` = `2.31.0-omega`；由 `bash scripts/check_doc_consistency.sh` 守护 | EXIT=0 | ❌ 现为 `2.29.0-omega`（版本推进是发布动作，非本 ADR 范围） |

> **准入裁决**：第 2 项已于 2026-11-19 满足（工具扩围到全限定路径并执行清零），退役动作正式解锁。剩余门槛只有第 6 项 —— 版本号推进到 `2.31.0-omega`，那是发布批次动作。

## 4. Rollback

1. 首选：`git revert <退役提交>`（删除动作应为**单一原子提交**；无运行期状态、无线格式、无数据迁移）。
2. 若无法 revert，按下列清单手工恢复：
   - 恢复文件 `crates/event-bus/src/types.rs`、`crates/event-bus/src/payloads.rs`（内容为 §1.2 两行/三行 `pub use`）；
   - 恢复 `lib.rs` 的 `pub mod types;` / `pub mod payloads;` 声明与 §1.2 的 16 符号 `pub use nexus_events::{...}`；
   - 恢复 `topic.rs:25` 的 `pub use nexus_events::topic::EventTopic;` 与 `lib.rs:207` 的 `EventTopic` 再导出；
   - `cargo check --workspace --all-targets` 复验。
3. 该垫片无 feature gate、无运行时开关（`lib.rs` 注释明确「无 feature gate、默认开启」），故**不登记**到 ADR-242 回滚开关台账（台账只收 `CHIMERA_*` 运行期开关）；回滚手段仅为 git revert。

## 5. Evidence（文件路径 + 符号名）

| 证据 | 路径 / 符号 | 现取结果（2026-11-19） |
|---|---|---|
| compat-2x 根导出 | `crates/event-bus/src/lib.rs:214-218` | 16 符号（见 §1.2 表） |
| 别名模块 types | `crates/event-bus/src/types.rs:10-11` | `nexus_events::{payloads,types}::*` |
| 别名模块 payloads | `crates/event-bus/src/payloads.rs:8` | `nexus_events::payloads::*` |
| topic 转发 | `crates/event-bus/src/topic.rs:25` | `pub use nexus_events::topic::EventTopic;` |
| import 面零残留 | `py -3 scripts/migrate_l1_event_imports.py --check` | EXIT=0 / 0 pending |
| 全限定路径残留 | `git grep -n "event_bus::NexusEvent" -- "crates/**/*.rs"` 等 | 89 处 / 12 crate（§1.3-B） |
| 退役承诺原文 | `docs/architecture/ADR-322-nexus-events-extraction.md` §1.3 | 「退役时点：v2.31.0-omega」 |
| 迁移脚本边界 | `scripts/migrate_l1_event_imports.py:28` | 「only `use` / `pub use` lines; full paths are untouched」 |

## 6. 后果

- **正**：v2.31.0-omega 后 `event-bus` 只剩运行时面，事件目录单一权威源（`nexus-events`）不再有双入口；`event_bus::NexusEvent` 一类旧路径彻底失效，避免「两处都能 import、改一处另一处静默可用」的漂移面。
- **负**：退役批必须清完 89 处全限定路径，属跨 12 crate 的机械改写；漏改会以编译错误暴露（fail-closed，无静默降级）。
- **待办**：扩展 `migrate_l1_event_imports.py` 或等价工具以覆盖 full paths（当前 docstring 明确不改 full paths），并在 L1-E 波次内完成清零。
