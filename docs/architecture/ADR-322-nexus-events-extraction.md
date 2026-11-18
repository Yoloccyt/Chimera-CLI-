# ADR-322 — 事件目录迁出 `event-bus` 为独立 crate `nexus-events`

- **状态**: ✅ Accepted（2026-11-18，实施与决策同批落地）
- **架构层**: L0-B 核心基础面（机器层号 1，与 `nexus-core` / `event-bus` / `nexus-telemetry` / `nexus-compute` 同亚层）
- **取代/关联**: 无取代；关联 ADR-222 D1/D3（L0 = 0A/0B 两亚层，机器层号不重排）、ADR-309（L0-B 首次"整面迁出"先例）、ADR-314（L1 内部拆分；本 ADR **遵守**其「`NexusEvent` enum 本体不拆」约束）、ADR-160（可达性棘轮）、ADR-188（事件所有权账本）、ADR-049（"0 新增 crate" 的原始决议）
- **编号说明**: 任务原拟 ADR-314，但该号已被 2026-11-16 的「L1 内部拆分与有限公开面收缩」占用（`adr_index.md` 有行、`ADR-311-l1-batch-closure.md` D7 为载体）。取号前对 `docs/`、`scripts/`、`crates/`、根 `*.md`、`.claude/`、`.trae/` 全域 grep `ADR-322` = **0 命中**（2026-11-18 实测），磁盘最大已落盘编号 321。

## 0. 背景与动机

`event-bus` 同时承载两件**正交**的事：

1. **事件目录**（catalog）：`NexusEvent` 155 变体、结构化载荷、`EventTopic` 十类、定级注册表——纯数据契约，零 IO、零运行时；
2. **运行时**：`tokio::broadcast` 发布订阅、Critical mpsc 旁路、背压、分片、因果归因、经验卡片总线。

混在一个 crate 里的代价是可测量的：任何人读 `NexusEvent` 定义都必须先具备 tokio 心智模型；`event-bus` 的 L0-B 依赖面（G-131）被 11 个外部依赖撑开，其中 tokio/arc-swap/crossbeam-queue 全部只因运行时面而存在；任何想只拿"事件长什么样"的下游（尤其跨进程/协议侧）都被迫连带编译整个运行时栈。

## 1. 决策

**新增 workspace 成员 `crates/nexus-events`（L0-B，机器层号 1），把事件目录整面迁出；`event-bus` 只留运行时面，并保留一个发布周期的纯路径 re-export。**

### 1.1 破例理由（ADR-049「0 新增 crate」）

ADR-049 的"0 新增 crate"是 v2.7 阶段控膨胀的**预算约束**，其治理目的是"禁止为了拆而拆"。本 ADR 按已被批准的两次先例破例：

| 先例 | 做法 | 结果 |
|---|---|---|
| ADR-222（2026-10-20） | `nexus-contracts` 拆 1+6 sibling + 新建 `nexus-telemetry` | 39→46 |
| ADR-309（2026-11-15） | `nexus-core/src/compute/` 整面迁出为 `nexus-compute` | 46→47 |
| **本 ADR（2026-11-18）** | `event-bus` 的事件目录整面迁出为 `nexus-events` | **47→48** |

破例判据（三条全部满足，与 ADR-309 §2 同构）：

1. **边界是既有的、不是新造的**——迁出面是 `types.rs` + `payloads/` + `registry.rs` + `topic.rs::EventTopic` 四个早已独立成文件的单元，本 ADR 不新增任何抽象；
2. **切面两侧依赖不同**——目录面只需 `serde`/`chrono`/`nexus-contracts`，运行时面才需 tokio 系；合并会持续把运行时依赖强加给"只想读事件定义"的消费者；
3. **有机器判据而非主观主张**——"零 tokio 系依赖"由 G-131 台账锁死（`scripts/l0b_dep_freeze.txt` 只减不增），"变体数不变"由 `VARIANT_COUNT_LOCK` 与 `doc_count_freeze.txt` 双源守护。

### 1.2 迁出边界

| 进 `nexus-events`（纯类型） | 留 `event-bus`（运行时） |
|---|---|
| `NexusEvent` 155 变体（按域 7 个分组文件，见 §2） | `EventBus` / `EventReceiver` 与 `bus/` 全部子模块 |
| `payloads/`（chat / critical / evolution / metrics / quest 六文件） | `FilteredSubscriber`（持有 `EventReceiver`） |
| `registry.rs`（`define_event_registry!` 定级注册表） | `backpressure` / `critical_sink` / `credit_flow` / `shard` / `causal` / `attribution` / `experience_card_bus` / `dynamic_registry` / `logging` / `subscriber` |
| `topic.rs::EventTopic` + `EventTopic::all()` | 全量广播、mpsc 旁路、分片、序列化编解码（`bus/codec.rs`） |
| `NexusEvent::certify_claim_ref()`（自 `bus/mod.rs` 迁入的纯只读访问器） | `CRITICAL_MPSC_VARIANTS` / `LANE_FORBIDDEN_SHARD` 等运行时清单 |

依赖方向：`event-bus(L0-B) → nexus-events(L0-B) → nexus-contracts(L0-A)`，同亚层向下，无环；`nexus-events` 对 `event-bus` **零依赖**（反向即 crate 环，G-125 拦截）。

### 1.3 兼容策略：直断 + 一个发布周期的纯路径 re-export（compat-2x）

不做 N-1 双轨。`event-bus` 在 `lib.rs` 增加**纯路径 re-export**（无 feature gate、默认开启）：

- 根路径：`pub use nexus_events::{NexusEvent, EventMetadata, EventSeverity, ...}` 等 15 项（与迁出前根导出逐字一致）；
- `event_bus::types::*`：保留同名别名模块（`pub use nexus_events::{payloads::*, types::*}`），覆盖下游 `use event_bus::types::{NexusEvent, RollbackTriggerType}` 一类写法；
- `event_bus::payloads::*`：同上；
- `event_bus::topic::EventTopic`：由 `topic.rs` 的 `pub use nexus_events::topic::EventTopic` 提供；
- `event_bus::registry`：迁出前即为**私有** `mod registry`（零 pub 项），故无路径需要转出——下游从未可引用，无需垫片。

**退役时点：v2.31.0-omega**。届时下游按 `nexus_events::` 机械改写（L1-E-2 波次），本段整段删除。**退役预案见 ADR-324**（含现取符号清单与机器前置检查；2026-11-19 实测全限定内联路径仍有 89 处残留，退役动作阻塞于该前置）。

### 1.4 155 变体如何拆分而不改契约

Rust 不允许跨文件拆分 enum 定义，也不允许宏在枚举变体位展开（`macros cannot expand to enum variants`）。因此采用**令牌累加链**：

- `types/<域>.rs` 各定义一个 `pub(crate) macro_rules! variants_<域>`，把本段变体追加到累加器 `$($acc)*` 后调用下一段；
- 链尾调用 `types/mod.rs` 的 `define_nexus_event!`，由它一次性落成**单一** `pub enum NexusEvent`（derive 与 serde 属性只有这一个声明点）。

结果：**声明顺序、变体名、字段、doc、serde 线格式、序数完全逐字不变**（"enum 本体不拆"——与 ADR-314 的约束一致）。这是文件长度红线（单文件 ≤900 行）与"单一枚举"契约之间的唯一兼顾解；代价是 IDE 逐变体跳转弱于单文件形态，已在分组文件头注记录。

## 2. 最终模块布局

| 文件 | 行数 | 内容 |
|---|---|---|
| `crates/nexus-events/src/lib.rs` | 59 | 契约块 + 模块声明 + 根 re-export + prelude |
| `src/types/mod.rs` | 86 | 装配宏 + 枚举声明 + `certify_claim_ref` |
| `src/types/contract.rs` | 206 | `UserIntentEncoded` → `TelemetryReporterDegraded` |
| `src/types/lifecycle.rs` | 268 | `NexusStateChanged` → `SandboxViolation` |
| `src/types/execution.rs` | 547 | `OperationProduced` → `ExpertUnregistered` |
| `src/types/governance.rs` | 566 | `DebateStarted` → `QuestPriorityAdjusted` |
| `src/types/interface.rs` | 468 | `DecayMetricsReported` → `TuiHelloAck` |
| `src/types/evolution.rs` | 396 | `R1ShadowRegressionDetected` → `ParliamentStrategyCapChanged` |
| `src/types/runtime.rs` | 603 | `ModelAffinitySelected` → `SecurityInterceptionReported` |
| `src/payloads/{mod,chat,critical,evolution,metrics,quest}.rs` | 原样平移 | 结构化载荷六域 |
| `src/registry.rs` | 374 | 定级注册表（宏 + 155 行定级） |
| `src/topic.rs` | 252 | `EventTopic` + `all()` + 主题映射测试（11 例） |

依赖面（`Cargo.toml`）：`nexus-contracts` · `serde` · `chrono`。**零 tokio / arc-swap / dashmap / crossbeam-queue / tracing / rayon**。

## 3. 台账与门禁同批变更（否则中间态必红）

| 台账 / 门 | 变更 |
|---|---|
| `Cargo.toml` `workspace.members` / `[workspace.dependencies]` | +`crates/nexus-events`；+`nexus-events = { path = ... }` |
| `Cargo.lock` | `cargo check` 自动生成（未手工编辑） |
| `scripts/dep_edge_freeze.txt` | +`event-bus|nexus-events`、+`nexus-events|nexus-contracts` |
| `scripts/crate_reachability_freeze.txt` | **不改**——`nexus-events` 经 `event-bus` 生产边可达（棘轮实测 48 成员 / 40 可达 / 0 孤岛） |
| `scripts/l0b_dep_freeze.txt` + `check_l0b_dep_surface.py` / `check_pub_api_leak.py` 的 L0-B 名册 | +`nexus-events`；外部行 `nexus-events::chrono` / `nexus-events::serde`（**零签名泄露**，G-135 实测 sites=rows=18） |
| `scripts/check_dependency_rules.{sh,ps1}` | `layer_of`=1 / `sublayer_of`=0B / `layered_crates` / `$layerMap` / `$subLayerMap` / `expected_crates` 47→48 |
| `scripts/check_doc_drift.sh` + `doc_count_freeze.txt` | `nexus_event_variants` 改测 `crates/nexus-events/src/types/`（分组目录求和，目录缺失/空退 2）；`event_topic_variants` 改测 `nexus-events/src/topic.rs`；DECLS 的 registry 锚点改路径；`workspace_crates` 47→48；fixture/自测同批迁移（`--selftest` PASS） |
| `scripts/check_doc_consistency.ps1` F1 | 单文件 `pub enum` 扫描改为分组目录扫描（≥4 空格缩进的不同变体名），实测 155 |
| `scripts/check_event_ownership.py` | 权威路径改 `nexus-events`；变体解析支持宏分组；**并修一处既有判据缺陷**（外部 `#[cfg(test)] mod tests;` 文件未被划入测试域，导致 L1-D 拆分后 7 条假 STALE——`--selftest` 24 断言 PASS） |
| `scripts/crate_contract.py --check` | `nexus-contracts` CONSUMERS 34→35（本 crate 成为其消费者）；`nexus-events` 契约块按台账方言登记 |
| `scripts/doc_count_freeze.txt` / `DOC-AUDIT-FACTS_2026-09-20.md` / `CODE_WIKI.md` / `README.md` / `.claude/CLAUDE.md` / `nuxus规则.md` | 计数与索引同批同步 |

### 3.1 契约块方言说明（`BACKEND`）

任务要求 `BACKEND: pure`，但 `crate_contract.py` 的合法取值集是 `sqlite | fs | memory | none`（无 `pure`），且 `--check` 会与磁盘派生值比对。为保持"门禁判据不被放宽"，本 crate 登记 `BACKEND: memory`（派生口径：无 rusqlite、无 `std::fs`），并在 `lib.rs` 契约块下方以散文明确"纯类型、零 IO 零运行时"。**不以沉默失败换取 `pure` 字样**。

## 4. 验证判据（全部实测）

| 判据 | 命令 | 结果 |
|---|---|---|
| 155 变体 | `cargo test -p event-bus --test variant_count_test` | 4 passed（`VARIANT_COUNT_LOCK = 155`，序数/分布锁同绿） |
| 变体文本实证 | 分组目录 `^ {4,}[A-Z]\w* *[{,(]` 计数 | 155 |
| 零 tokio 系 | `Select-String -Path crates/nexus-events/Cargo.toml -Pattern 'tokio\|arc-swap\|dashmap\|crossbeam\|tracing\|rayon'` | 0 命中 |
| `nexus-events` 自检 | `cargo test -p nexus-events` | 11 unit + 1 doctest passed |
| 下游零改动 | `cargo check --workspace --all-targets --jobs 4` | rc=0 |
| 依赖方向 | `bash scripts/check_dependency_rules.sh` | `[C] disk crates 48 / layer map 48`，rc=0 |
| L0-B 依赖面 | `py -3 scripts/check_l0b_dep_surface.py` | crates=5 external_deps=29，rc=0 |
| 签名泄露面 | `py -3 scripts/check_pub_api_leak.py` | sites=rows=18，rc=0 |
| 依赖边棘轮 | `py -3 scripts/check_dep_edge_freeze.py` | 164/164，rc=0 |
| 可达性棘轮 | `py -3 scripts/check_crate_reachability.py` | 48 成员 / 40 可达 / 0 孤岛，rc=0 |
| 文档三方一致 | `powershell -File scripts/check_doc_consistency.ps1` | all pass（48 crates / F1=155），rc=0 |
| 计数漂移 | `bash scripts/check_doc_drift.sh`（含 `--selftest`） | rc=0 / PASS |
| 事件所有权 | `py -3 scripts/check_event_ownership.py`（含 `--selftest`） | rc=0 / 24 断言 PASS |

## 5. 回滚

1. `git revert <本批 commit>`（代码 + 台账 + 文档同批，无拆步残留）；
2. `cargo check`（`Cargo.lock` 由 cargo 自行回收 `nexus-events` 条目）；
3. 无数据迁移、无运行时状态、无线格式变更 ⇒ **回滚零兼容负担**。

迁移过程中不存在"半迁移可运行"的中间态：`Cargo.toml` 成员、层表、台账与 crate 实体互为前提，任何拆分提交都会让 G-125/G-131/G-33 立即变红（与 ADR-309 §6 同纪律）。

## 6. 后果

- **正**：事件契约可被"只要类型"的消费者单独依赖；L0-B 依赖面从 11 个外部依赖（event-bus 现值）中隔离出 tokio 系；`types.rs` 2859 行单文件降为 7 个 ≤603 行的分组文件。
- **负**：变体定义经宏装配，IDE 级跳转与 rustfmt 覆盖弱化（rustfmt 不格式化宏体，分组文件内的缩进由生成脚本保证）；一个发布周期内 `event-bus` 保留 compat-2x re-export，共两份路径需要维护。
- **待办（L1-E-2 波次）**：下游 37 个 crate 的 `use event_bus::...` 机械改写为 `nexus_events::...`，随后删除 `event-bus/src/{types,payloads}.rs` 别名与 `lib.rs` 的 compat-2x 段（v2.31.0-omega）。
