# ADR-311: L1 批次收口 —— B1~B5 破坏式重构的决策合订

- 状态: Accepted
- 日期: 2026-11-16
- 批次: L1 深度重构 W0（ADR 补录批）
- 设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §6（L1 Core）与本次实施前的现状测绘
- 关联: ADR-222（L0-A/L0-B 双层 + 层号不重排）、ADR-223（G-131 L0-B 外部依赖面台账）、ADR-225 D6（G-135 公开签名泄露台账）、ADR-226（loom 仅 dev-dep）、ADR-309（`nexus-compute` 迁出，shim 一个版本窗）、ADR-310（L0 facade 单源化 + L0 门单入口）

> **本 ADR 的补录性质**：B1~B5 五批破坏式重构已于 2026-11-15 落地并合入主线
> （提交序列见 D1 表），但其决策依据仅由台账注释与 `adr_index.md` 片段承载，
> **ADR-311/312/313 三个编号在磁盘上无物理文件**——被 57 处代码注释与冻结台账
> 引用（`scripts/dep_edge_freeze.txt` 13 处、各 crate `lib.rs` 13 处、
> `scripts/nexus_core_dep_freeze.txt` 3 处、测试 12 处、其他 16 处）。
>
> 这构成「可回溯」原则的缺口：既成事实无正式决策记录。本 ADR 按 Q6 裁决
> **合并为一份 ADR-311**，ADR-312/313 作为其子章节占位登记（编号不回收，
> 保持引用连续性）。

---

## D1 — 背景：五批在途重构的事实

B1~B5 是 L0 深度重构（W4-1 起）向 L1 收口的连续五批，每批均为破坏式变更：

| 批次 | 提交 | 主题 | 破坏面 |
|---|---|---|---|
| B1 | `bb9b3e53` + `f3875485` | **compute shim 退役** —— 消费方直连 `nexus_compute`，`nexus-core` 摘除计算面依赖 | `nexus_core::compute::*` 旧路径失效 |
| B2 | `ee4f838f` + `48d99c8c` + `b9253437` | **RL 预留面物理删除** —— `stat_learning` / `rl_client` / `resilience` 退役 | `nexus_core::stat_learning::*` 等三路径失效 |
| B3 | `361901d5` | **ChimeraConfig 迁回 chimera-cli** —— nexus-core 摘除 CLI 域配置（30 个配置类型） | `nexus_core::config::*` 失效 |
| B4 | `a6fa40ab` + 工作树 | **B4a+b+c 领域类型直连 L0**（45+ 处 use 迁移 `nexus_contracts`）；**B4d `types` 纯 re-export 汇聚点退役** | `nexus_core::{Quest, Task, TaskStatus, ThinkingMode, UserIntent, Checkpoint, MultimodalInput}` 七个根路径导出失效 |
| B5 | `2abc977b` | **event-bus 撤主张面物理删除** —— `membrane` / `pattern_index` / `segment_per` / `token_ledger` 四子系统退役，`TokenLedgerRecorded` 变体摘除（156→155） | `event_bus::{membrane, pattern_index, segment_per, token_ledger}` 四路径失效；事件变体数 156→155 |

**共同的决策模式**：五批全部遵循「**生产装配点为零 ⇒ 物理删除而非降级**」，
而非「保留代码但撤主张」。判据是 **grep 生产构造点**（其他 crate 的 `src/`
+ 本 crate 的非 `mod tests` 段），不是「模块看起来有没有用」。

---


### D1.1 — W1 批次裁决（死参数「赋义」，而非删参）

**批次**：W1（前置 W1-0 经实测为**空批次**，见 D8 第 3 项）。

**处置**：`nexus-core::state::NexusState::update_quest_progress(&self, quest_id,
_completed, _total)` 的两个死参数（带 `_` 前缀、不参与逻辑）**赋义**而非删除：

| 裁决 | 依据 |
|---|---|
| 语义：`completed > total` 是**物理不可能**的进度组合 ⇒ 入口即拒 | 参数名自带「完成数 / 总数」语义；不参与逻辑本身就是缺陷（调用方可传任意值） |
| 错误面：新增 `NexusError::InvalidProgress { completed, total }` | 全仓 `NexusError` 消费方均用 `matches!` 宏（非穷尽 match）⇒ **加变体零破坏**（已逐个 grep 核实） |
| 选「赋义」而非「删参」 | 删参会丢失调用方的「完成数 / 总数」意图并需改所有调用点；赋义保留意图并把它变成**可校验的不变量** |

**验证**：`cargo test -p nexus-core --lib` ⇒ **74 passed / 0 failed**（新增 4 用例：
负例 ×2 + 边界 + 「载荷错优先于存在性」的判定顺序）。

### D1.2 — W2 批次裁决（错误模型核实：方案前提被证伪 ⇒ 零代码改动）

**批次**：W2（方案 §5.4 的 BC-09 / BC-10：错误模型统一）。

**方案前提**：BC-09 称 `event-bus/src/critical_sink.rs` 有 11 处 `expect()`，
BC-10 称 `nexus-compute` 有 2 处 `panic!()`，均需改为 `Result` 传播。

**实测结论：两处前提均不成立 ⇒ W2 为验证性批次，零代码改动。**

| 方案前提 | 实测 | 裁决 |
|---|---|---|
| `critical_sink.rs` 11 处 `expect()` 在生产面 | **生产面（1~174 行）零 `expect`**；11 处全在 `#[cfg(test)] mod file_wal_tests` 内，且测试模块有 `#[allow(clippy::unwrap_used, clippy::expect_used)]` 豁免 | **不改**。改动会把「测试惯用法」误升格为生产面错误模型变更，属无谓扩大改动面 |
| `nexus-compute` 2 处 `panic!()` 需改 `Result` | `bridge.rs:147`（rayon 建池失败）注释载明「进程级资源耗尽 = 进程不可用，无运行期降级路径」；`hts.rs:395`（SequentialTest 配置非法）同时提供 `try_new` 的 Result 版本，且生产调用点为 0（`calibration.rs:242` 用 `try_new`） | **不改**。两处均为有充分论证的设计意图，且已有 Result 替代路径 |
| `G-81`/`G-82` lint 棘轮 | `scripts/check_lint_ratchet.py` 实跑：47 crate 全 opt-in、**0 offenders** | 棘轮为绿 ⇒ 无需任何登记 |

⇒ W2 的价值在**证伪**（避免把测试惯用法与有意设计意图当成缺陷去"修"），
证据即 `check_lint_ratchet.py` 的 0 offenders 实跑结果。

## D2 — 决策

### D2.1 零生产装配面的处置判据（ADR-312 子章节）

1. **判据 = 生产构造点 grep**，三条件同时成立方判「零装配」：
   - 其他 crate 的 `src/`（**不含** `tests/`/`benches/`/`examples/`）零引用；
   - 本 crate 的非 `mod tests` 生产段零引用；
   - **上游数据面也不产生它**（如 `token_ledger` 的上游 `cmt-tiering` 闭环
     本身零生产装配）。
2. **测试消费不构成保留理由**——测试是 API 契约的验证手段，不是生产依赖。
   但**反之**：仅被测试消费的公开面**不自动降级为 `pub(crate)`**，因为
   `tests/`/`benches/`/`examples/` 是独立 crate，只能访问公开 API，降级会
   直接导致其编译失败。⇒ 处置是「**保留 pub**」而非「降级」。
3. **退役即物理删除**，不保留 shim（与 ADR-309 的 shim 一个版本窗例外不同：
   ADR-309 迁出的是**仍在生产消费**的路径，需要过渡窗；B1~B5 删除的是
   **零生产消费**的路径，无过渡需求）。
4. **连带清理**：退役子系统的事件变体（`TokenLedgerRecorded`）、台账行
   （`nexus_core_dep_freeze.txt` / `l0b_dep_freeze.txt`）、计数锁
   （`VARIANT_COUNT_LOCK` 156→155、`doc_count_freeze.txt`）必须**同批提交**。

### D2.2 RL / 学习预留面的处置（ADR-311 核心）

1. **B2 退役 `stat_learning` / `rl_client` / `resilience`** 的依据：
   - `stat_learning`（`StatLearningPolicy` / `SlidingWindowPolicy` /
     `UCBPolicy` / `ActionStats`）与 `resilience` 的生产消费者数为 **0**；
   - `rl_client` 的 Rust 侧实现（`RLClient` trait / `RulePolicyFallback`）
     生产消费者数为 **0**；实际传输通道在 L10 侧
     `crates/chimera-cli/src/rl_client.rs`（140 行，`rl-client` feature
     **默认 off**，承载 `POST /experiences` 与 `GET /health`）。
   - 权威文档 §6.3 描述的 `StatLearningPolicy` 与 §6.4 的 `crates/rl-client/`
     **均已不在盘**。
2. **重建触发条件**（写入本 ADR，作为不可静默漂移的锚）：
   - **R2 解冻**前置六要素全 Satisfied（ADR-053 rev4）+ ADR-054 治理签署；
   - 且**按 L0 契约重建**——`nexus_contracts` 的 `rl_hooks` 面
     （`RLHook` / `SerializedPolicy` / `RLTrajectory` / `RLStateVector` /
     `RLActionVector`）是唯一契约源，实现不得反向定义在 L1；
   - L10 侧 `chimera-cli::rl_client` 保留 feature 门，不随 L1 重建而合并。
3. **本次任务明确不涉及 RL**（业主指令）。

### D2.3 领域类型直连 L0 的终局（ADR-311 核心 + ADR-313 互补）

1. **B4d 退役 `nexus_core::types` 纯 re-export 汇聚点**：全部领域类型
   （`Quest` / `Task` / `UserIntent` / `Checkpoint` / `TaskStatus` /
   `ThinkingMode` / `MultimodalInput`）由消费方**直连 L0**：
   - `use nexus_contracts::domain::{Quest, Task, ...}`
   - `use nexus_contracts::task::TaskStatus`
   - `use nexus_contracts::checkpoint::Checkpoint`
2. **ADR-033 决策 6 + ADR-222 D2 的终局落点**：`nexus-contracts-core`
   （L0-A sibling）承接领域类型，`nexus-core` 回到「核心基础面」本义
   ——**只承载** `NexusState` / `CLV` / 7 个 ID newtype / `PragmaCapable` /
   衰减纯函数。
3. **编译期等价性守卫随模块退役**：`reexported_types_are_identical_to_contracts`
   （原守卫 re-export 与 L0 类型同一性）随模块退役，类型一致性改由 L0 自身
   `tests/type_consistency_test.rs` 承担——**守卫责任不落空，只是换了归属**。
4. **B3（ADR-313 互补）**：`ChimeraConfig` 等 30 个 CLI 域配置类型迁回唯一
   真实消费者 `chimera-cli`（L2-L9 生产代码零消费）。
   `chimera_cli::config::X` 路径经其 `config.rs` re-export **保持零 diff**。

### D2.4 计算面迁出后的 shim 退役（与 ADR-309 的衔接）

1. ADR-309 规定 `nexus-core` 保留 `pub mod compute { pub use nexus_compute::*; }`
   shim **一个版本窗**。**B1 即为该窗口的到期处置**：shim 退役，消费方直连
   `nexus_compute`，`nexus-core` 对 `nexus-compute` 的依赖行从
   `nexus_core_dep_freeze.txt` 同批删除。
2. **反向边被拒**：`nexus-compute → nexus-core` 会构成 crate 环（ADR-309 D4
   红线），`nexus-core` 不得借 dev-dependency 成环。
3. **消费方影响面**：`chimera-cli` / `decay-engine` / `faae-router` /
   `gsoe-evolution` / `hcw-window` 等的 `nexus_core::compute::*` 改为
   `nexus_compute::*`，**类型同一性**（同一 rlib）由编译保证。

---

## D3 — 台账与门禁的同批同步（可回溯纪律）

B1~B5 每批必须同步的台账（**缺一即门红**，这是 ADR-222 D6「中间态必红」
纪律的机器化）：

| 台账 / 门 | B1 | B2 | B3 | B4 | B5 |
|---|---|---|---|---|---|
| `scripts/nexus_core_dep_freeze.txt`（G-90） | ✅ 删 `nexus-compute` 行 | ✅ 删 `arc-swap`/`async-trait`/`rand`/`rayon` | ✅ | ✅ | — |
| `scripts/l0b_dep_freeze.txt`（G-131） | ✅ | ✅ 删 `nexus-core::{arc-swap,async-trait,rand,rayon}` | ✅ | ✅ | — |
| `scripts/dep_edge_freeze.txt`（G-32） | ✅ 改直连 | ✅ | ✅ | ✅ | — |
| `scripts/doc_count_freeze.txt`（G-37） | ✅ crate 数 | — | — | — | ✅ 变体 156→155 |
| `crates/event-bus/tests/variant_count_test.rs` | — | — | — | — | ✅ `VARIANT_COUNT_LOCK` |
| `crates/*/src/lib.rs` CRATE-CONTRACT 头 | ✅ | ✅ | ✅ | ✅ CONSUMERS 21→19 | — |

**纪律**：台账与代码**同批提交**；禁 `git checkout --` / `git reset --hard` /
`git stash` 绕过门（沿 ADR-222 D6）。

**W6 追加的同批同步项**（见 D7.6）：

| 台账 / 门 | W6 变更 |
|---|---|
| `scripts/l0b_pub_signature_freeze.txt`（G-135） | ✅ 1 行模块路径迁移（`event-bus::bus::` → `event-bus::bus/subscribe::`） |
| `crates/event-bus/src/bus/mod.rs`（模块可达性 G-109） | ✅ 4 个新子文件均有 `mod` 指向（`live=0 listed=0`） |

### D3.1 前序批次留红的同批修复（HEAD 既有缺陷，非 W6 引入）

W6 批次开工前的基线门禁跑批发现 **`scripts/check_doc_drift.sh`（G-37）在 HEAD 即红**
（exit 2），根因是**前序 B5 提交（`2abc977b`，ADR-312 撤主张面）破坏了 ADR-222 D6
的「台账与代码同批提交」纪律**：该提交物理删除了
`crates/event-bus/src/pattern_index.rs` 并把变体锁 156→155，但**未同批**撤销
`check_doc_drift.sh` 的 DECLS 锚点、也未同批订正两处文档声明 ⇒ 门退 2（不可判定）。

| 缺陷 | 修复 | 验证 |
|---|---|---|
| DECLS 锚点指向已删除文件 | 撤销该锚点 + 同步 selftest 夹具（夹具文件一并删除） | 门 EXIT=0；`--selftest` 8 类判定全过（RESULT: PASS） |
| `crates/event-bus/src/topic.rs` 文档声明仍写 156 | 改为 155 | 同上 |
| `docs/reports/DOC-AUDIT-FACTS_2026-09-20.md` 文档声明仍写 156 | 改为 155 + 补记 ADR-312/B5 的 156→155 退役轨迹 | 同上 |

⇒ 该修复**独立成 commit**（`scripts/check_doc_drift.sh` + `crates/event-bus/src/topic.rs`
+ `docs/reports/DOC-AUDIT-FACTS_2026-09-20.md`），不与任何 L1 批次混提：
它是前序批次的收尾，不是 W6 的内容。

---

## D4 — 与权威文档 §6 的差异裁决

本次实测发现权威文档 §6 与代码实况存在 8 处差异（详见
`docs/reports/` 对应批次报告的差异表）。裁决原则：

> **凡代码已通过 ADR 正式裁决并留痕者，以代码为事实源**；文档降级为
> 「历史设计稿」，须补更新。**若差异未被任何 ADR 覆盖，则判为治理缺口**，
> 列入风险登记册。

| 差异 | 裁决 | 处置 |
|---|---|---|
| §6.1 `ExperienceCardBus` 的 11 处实现偏离 | **不回滚** | 见 D5 |
| §6.2 `SegmentAwarePER` / §6.3 `StatLearningPolicy` / §6.4 `crates/rl-client/` | **代码为事实源** | 文档标注「已退役于 ADR-311/312，重建条件见 D2.2」 |
| §6.1 import 示例 `use nexus_contracts::{ExperienceCard, ...}` | **代码为事实源** | 文档改为 `nexus_contracts::experience_card::{...}`（ADR-222 L0-A sibling 拆分后遗留，示例已不可编译） |
| §6 的「L1 Core」层名 | **代码为事实源** | ADR-222 D1 已将 L1 命名为「L0-B 核心基础面」，**机器层号 0/1 不重排**（116+ ADR 层引用证据链）——文档补注记 |
| §6 完全未提 `nexus-compute` | **代码为事实源** | 文档补 §6.5「L0-B 计算面（ADR-309）」 |

---

## D5 — `ExperienceCardBus` 相对 §6.1 的偏离裁决（不回滚）

权威文档 §6.1 的设计稿与落地实现存在 11 处差异。**逐条裁决：10 处为有意的
风险消除，不回滚**；1 处为文档笔误，修正文档。

| # | §6.1 设计 | 落地实现 | 裁决理由 |
|---|---|---|---|
| 1 | `base_bus: EventBus` 字段 + `new(base_bus)` | 无 `base_bus`，`new()` 无参（独立数据面） | ADR-065 决策 4：经验卡片流不混入 `NexusEvent` 枚举 |
| 2 | Critical = `mpsc::UnboundedSender` | `Arc<Mutex<Vec<mpsc::Sender>>>` bounded fan-out（4096） | **unbounded 在慢消费者下 OOM**；bounded + 满载采样丢弃计数是安全替代 |
| 3 | `pub async fn publish(...) -> Result<(), BusError>` | 同步 `pub fn publish(&self, card)`，无返回值 | 投递路径不含 IO；async 化只增加无谓 await |
| 4 | 定义 `enum BusError { CriticalChannelClosed, BroadcastError }` | **该类型不存在**（全仓零命中） | 文档自造类型，与本 crate 的 `EventBusError` 体系不兼容 |
| 5 | `get_cards_by_error_hash -> Vec<ExperienceCard>` | `get_card_ids_by_error_hash -> Vec<String>` | 返回 ID 列表避免大对象 clone；调用方按需取卡 |
| 6 | 索引键 `String` | 索引键 `Box<str>` | 零拷贝承载 |
| 7 | Top-K 用 `sort_by`（O(n log n)） | `select_nth_unstable_by`（O(n)） | **性能**，红线 R8 |
| 8 | 无 Critical 丢弃计数 | 新增 `critical_dropped_count` | 运维可观测性必需 |
| 9 | `use nexus_contracts::{ExperienceCard, ThreeFactorScore, ExecutionStatus}` | 实际须 `nexus_contracts::experience_card::{...}` | ADR-222 L0-A sibling 拆分后遗留（**文档笔误**） |
| 10 | 文档未声明索引上界 | 实际明示**四索引无上界 / 无 TTL / 无淘汰**（豁免注记 C-69/M54） | 诚实登记；**不实现淘汰**（加淘汰=行为变更，与「高分必达」语义冲突，须另立 ADR） |
| 11 | 投递分级阈值 >0.8 / >0.5 | 一致 | 无差异 |

**裁决**：**代码为事实源，文档更新**。不回滚任何偏离——回滚会重新引入
unbounded channel 的 OOM 风险与 O(n log n) 的 Top-K 回归。

---

## D6 — 回滚

- **B1~B5 每批独立 commit** ⇒ `git revert <sha>` 即可逐批回滚；
- **B1 shim 退役**的回滚需注意：ADR-309 的 shim 已删除，回滚 B1 需同时
  回滚 ADR-309 的两 commit（ADR + 施工）——ADR-309 已记录该回滚路径；
- **无不可逆点**：全部为 git 跟踪的文件增删，git 历史完整保留。

---

## D7 — W6 批次裁决（`event-bus` 内部拆分 + 发布路径收敛 + 载荷域下沉）

> 批次：W6（BC-01 / BC-02 / BC-03 / BC-05）。方案 §5.4 的 BC-01~BC-05 与 §6 批次表
> 是本次的权威依据；本节记录**实测证伪的方案前提**与**逐条偏离裁决**。
> 凡与方案字面不一致处，以本节为准（裁决原则：**代码为事实源，证据优先于方案原文**）。
>
> 决策归属：**ADR-314**（L1 内部拆分与有限公开面收缩，W6/W7）——本节是 ADR-314 的
> 决策明细载体（ADR-314 无独立物理文件，按索引行登记于本文件）。

### D7.1 BC-01：`bus.rs` → `bus/` 五职责拆分（按方案执行）

**落地结构**（`crates/event-bus/src/bus/`）：

| 子文件 | 职责 | 迁入内容 |
|---|---|---|
| `mod.rs` | 总线本体 + 类型 | `EventBus` 结构、`EventReceiver`、`ShardWorkerGuard`、构造器、统计面、变体枚举辅助 |
| `codec.rs` | 纯编解码 | `serialize_msgpack` / `deserialize_msgpack` / `serialize_json` / `deserialize_json` |
| `publish.rs` | 发布面 | `publish` / `publish_blocking` / `publish_batch` / `publish_critical` / 3 个待退役方法 + `dispatch_one_impl` 宏 + `dispatch_batch` |
| `subscribe.rs` | 订阅面 | `subscribe` / `subscribe_filtered` / `subscriber` / `critical_subscriber` / `subscribe_critical_events` + `send_critical_mpsc` |
| `shard_ctl.rs` | 分片控制面 | `without_sharding` / `try_enable_sharding` / `enable_sharding` / `shutdown_sharding` / `sharding_enabled` / `shadow_stats` / `try_shard_publish` / `resolve_shard_override` |

**偏离与理由**：

- `ShardWorkerGuard` **留在 `mod.rs`**（方案未指定）：它是 `EventBus` 结构字段
  `shard_guard: Arc<OnceLock<ShardWorkerGuard>>` 的类型定义；移走需降为
  `pub(crate)`，而 `shutdown_sharding` 的公开 rustdoc 内链 `[`ShardWorkerGuard`]`
  指向它 ⇒ 降级会产生 rustdoc 私有内链告警。留在 `mod.rs` 同时保住 rustdoc
  链接与可见性。
- 跨子模块的私有方法提升为 `pub(super)`（`try_shard_publish` / `ds_admit` /
  `dispatch_batch` / `send_critical_mpsc` / `resolve_shard_override`）：Rust
  隐私规则下「父模块私有项对子模块可见」，`pub(super)` 是为让 `bus::tests`
  （`mod tests` 与 `bus::shard_ctl` 是兄弟模块）也能访问。
- `dispatch_one_impl` 宏迁至唯一使用方 `publish.rs`，**不**用 `#[macro_export]`
  （避免扩大 crate root 公开面）。

**内容守恒证明**（行级多重集差分，脚本级 fail-closed）：
`HEAD:crates/event-bus/src/bus.rs` 逐行 vs 拆分后 5 文件逐行，差异仅 15 项，
全部为有意变更（宏内 `Ordering` 全限定、失效 rustdoc 链接修正、`pub(super)`
可见性、import 按模块拆分），**零内容丢失**。

**过程中发现并修复的两处隐性损失**：

1. **宏的论证性 rustdoc 丢失（41 行）**：`dispatch_one_impl` 宏上挂着
   「WHY 必须是宏，不能改回函数」的**实测基准数据**（同步路径普通 `#[inline]`
   退化 +5.1% p<0.05；异步路径 `publish_batch_vs_serial/serial/5` 退化 +6.4%
   p=0.00）与发布顺序说明。初次切割丢失 ⇒ 已**逐字恢复**。删掉它等于让后人
   重复踩「把宏改回函数 ⇒ 性能回归」的坑。
2. **既有文档错位**：`publish` 的 rustdoc 曾被误挂在 `subscriber_count` 上
   （本次切割引入）⇒ 已就地纠正；`publish` 的说明现位于 `bus/publish.rs`。

### D7.2 BC-02：发布路径废弃 —— 实测只废弃 3 个（方案列 5 个）

**方案前提被证伪**：方案 §5.2.1 称这 5 个方法「各自零外部消费」。实测
**4 个有生产调用点**：

| 方法 | 生产调用点 | 语义 | 裁决 |
|---|---|---|---|
| `publish_batch_blocking` | 0（仅本 crate test 1 处） | 与 `publish_batch` 共用 `dispatch_batch` 内核 | **废弃**（test 调用点保留守护 + `#[allow(deprecated)]`） |
| `publish_claim` | 1（`chimera-cli/src/sched_claim.rs`） | 方法体逐字 = `publish(event).await` | **废弃** + 调用点迁 `publish` |
| `publish_declaim` | 1（`chimera-cli/src/sched_claim.rs`） | 方法体逐字 = `publish(event).await` | **废弃** + 调用点迁 `publish` |
| `publish_certify` | 1（`parliament/src/debate/mod.rs`） | = `publish_critical(event).await` | **保留不废弃** |
| `publish_critical_blocking` | 2（`chimera-mas/src/context/budget.rs`、`budget_model.rs`） | **无条件** mpsc 旁路 | **保留不废弃** |

**`publish_certify` 保留的依据**：`scripts/check_certify_whitelist.py` 以
`\.publish_certify\s*\(` 作为「Certify 生产签发面」的**静态可检索 token**
（裁决权外置的落地形态）。废弃或改名（迁到 `publish_critical`）都会让该门的
真实命中数归零 —— 门仍是绿的，但**已不测任何东西**（比红更坏：失效的门）。
方案 §5.2.1 本身即要求「保留（`certify_claim_ref` 是公开契约的一部分）」。

**`publish_critical_blocking` 保留的依据**：它**无条件**走 mpsc 旁路，而
`publish_blocking` 仅对 `is_critical_mpsc_event` 清单内事件（18 类）走旁路。
其两个生产调用点发布的 `NexusEvent::AgentContextOverflow`（chimera-mas 预算
越界告警）**不在**旁路清单 ⇒ 迁到 `publish_blocking` 会**丢失 mpsc 旁路投递
保证**——那正是调用点选择该方法的理由（见 `budget.rs` 的 `maybe_publish_overflow`
注释）。且它是 `publish_critical` 的 **sync 孪生**：async 侧有显式 Critical
通道，sync 侧（无法 await）必须有对称物。

**退役时序**：本批只打 `#[deprecated]` 标记（阶段①）；物理删除（阶段②）须在
**一个版本窗（≥2 个 CI 周期 + 1 次发布）之后**、且**独立 commit**
（ADR-222 D6 原子纪律）。本批次**不删**。

### D7.3 BC-03：`payloads.rs` 按 topic 域下沉（按方案执行）

**落地结构**：`crates/event-bus/src/payloads/` = `mod.rs`（facade）+ 5 个域子模块
（`quest` / `chat` / `metrics` / `evolution` / `critical`）。

**契约零变更**：子模块为**私有** `mod`（真正的下沉：域内路径不外泄）；全部
13 个类型经 facade 的 `pub use` 逐一转出 ⇒ `event_bus::payloads::X` 路径
**逐字不变**；`types.rs` 的 `pub use crate::payloads::*;`（glob）与 crate 根
`pub use types::{...}` 均继续覆盖全部类型。

### D7.4 BC-05：可见性收缩 —— 实测只降级 1 个（方案列 4 个）

`SIGN_FLOOR` 实测 = **120**（`scripts/check_pub_api_leak.py`），L0-B 实测扫描
`pub fn` 签名 = 346（HEAD 347）⇒ 降级不会让 G-135 退 2「不可判定」。但方案的
「外部 0 + 本 crate tests/benches/examples 均 0」口径**漏算两类消费者**：

| 符号 | 裁决 | 依据 |
|---|---|---|
| `pressure_latch_next` | **降级 `pub(crate)`** ✓ | 外部 `src/`/`tests/`/`benches/`/`examples/`/doctest 引用数**全为 0**（2026-11-16 实测）；唯一生产调用点是同 crate 的 `bus::ds_admit`。同时撤出 crate 根 re-export。 |
| `CriticalSubscribed` | **保留** | 外部集成测试 `crates/event-bus/tests/subscribe_spawn_test.rs` 调用 `.critical_subscriber().subscribe()`；`subscribe()` 必须 `pub` ⇒ 其类型参数必须 `pub`（否则 `private_interfaces` 告警）。 |
| `CausalBuffer` / `PendingEvent` | **保留** | `pub mod causal` 是公开模块，且 `causal.rs` 内有**可编译 doctest** 以 `event_bus::causal::{CausalBuffer, VectorClock}` 示范公开用法 ⇒ 降级即破坏文档契约（方案的「外部 0」口径漏算 doctest）。 |

⇒ BC-05 实际收益 = **1 个符号**。与方案 §5.1/§5.5 的「≈228」估计仍有差距，
但方案已自陈「公开面减法的真实空间需要另一个判据」，本节据实登记。

### D7.5 新发现并修复：批量路径与单发路径的投递语义分叉

> 这不是方案项，是**等价性验收（方案 §7.2 W6 行）逼出来的真实缺陷**。

`publish_batch`（内部 `dispatch_batch`）此前与 `publish`（`dispatch_one_impl`）
有**三处分叉**：

1. **不递增 `published_total` / `critical_total`** ⇒ ADR-191 §16.5 定义的
   「发布入口计数」漏计批量发布 ⇒ L1 吞吐量指标**低报**（唯一生产批量发布方是
   parliament 的辩论批量投递）；`critical_total` 前哨亦失真。
2. **完全跳过降采样闸门 `ds_admit`** ⇒ 压力区内批量发布绕过「每 every 条放行
   1 条」策略，且降级量不入 `downsampled_total`（运维看不见降级量）。
3. **闸门即便被调用也不生效**：批量路径的 `queued` 是**批前一次性采样**（此时
   一条都还没发出去 ⇒ 恒为 0）⇒ `pressure_latch_next` 永不锁存 ⇒
   **降采样策略在批量发布下被完全绕过**。

**修复**：批量路径补齐入口计数（位置对齐 `publish`：信用扣减与分片路由之前）
+ 调用 `ds_admit`（位置对齐 `publish`：mpsc 旁路之后、broadcast 发送之前）
+ 降采样策略启用时**逐条重采**占用水位（默认策略下 `ds_gate_active=false`，
保留「每批一次」的摊销收益；默认策略是全仓唯一生产配置）。

**验收**（新增 `crates/event-bus/tests/publish_path_equivalence_test.rs`，5 用例）：

- 三条自动路由路径（`publish` / `publish_blocking` / `publish_batch`）对同一
  事件序列产生**逐条相同**的订阅者可见序列；
- 负控 1：仅一条不同的分叉序列必须被判为不等；
- 负控 2：顺序被改写的序列必须被判为不等（模拟「批量路径走了不同投递路径」）；
- 批量路径的 `published_total` / `critical_total` 与逐条发布一致；
- 批量路径**不得绕过**降采样闸门（`downsampled_total > 0`）。

**等价性的适用范围（诚实边界）**：断言在**默认背压策略**下成立——这是全仓唯一
的生产配置（`EventBus::with_policy` 的降采样策略生产调用方为 0，2026-11-16
实测）。降采样策略下 `publish_batch` **刻意**摊销采样开销，其 latch 演进与
逐条采样不可能逐位相同；本 ADR 不假装二者等价，而是另行锁定「批量路径不得
绕过闸门」。

### D7.6 门禁与台账的同批同步

- **G-135 签名泄露台账**（`scripts/l0b_pub_signature_freeze.txt`）：1 行模块
  路径迁移（`event-bus::bus::subscribe_critical_events::tokio::ret` →
  `event-bus::bus/subscribe::subscribe_critical_events::tokio::ret`，因其从
  `bus/mod.rs` 迁到 `bus/subscribe.rs`）。**泄露本身未增减**（18 观察点 /
  18 台账行）。
- **扫描器盲区对账**：`scanned_pub_fn` 台账头记录 413（历史快照）→ 实测 346。
  逐文件对账（HEAD vs 当前树）证明：**本次拆分零盲区**——`bus.rs` 52 → 5 文件
  24+4+9+6+9 = **52**；`payloads.rs` 11 → 6 文件 1+0+1+7+2+0 = **11**；净 -1
  恰为有意降级的 `pressure_latch_next`；其余差额来自仓库此前的减法批次
  （B4d 退役 `nexus-core/src/types.rs`、membrane 子系统退役等），**与本次无关**。
- **G-109 零孤儿源文件**：新文件全部有 `mod` 指向（`live=0 listed=0`）。
- **G-89 分片红线**：分片键派生（`type_name` + `fnv1a`）、几何 64×256、
  静默 send 恰好 1 处 —— 拆分后不变。
- **G-81/G-82 lint 棘轮**：47 crate 全 opt-in，0 offenders。

### D7.7 W6 验证记录（实测）

| 项 | 结果 |
|---|---|
| `cargo check -p event-bus -p chimera-cli -p parliament -p chimera-mas --all-targets` | EXIT=0，0 warning |
| `cargo clippy -p event-bus --all-targets` | 0 warning |
| `cargo test -p event-bus` | **527 passed / 0 failed**（33 个测试二进制，含 doctest） |
| `check_pub_api_leak.py`（G-135） | [OK] sites=18 rows=18 scanned_pub_fn=346 |
| `check_src_module_reachability.py`（G-109） | [OK] live=0 listed=0 |
| `check_bus_shard_hotspot.py`（G-89） | OK |
| `check_lint_ratchet.py`（G-81/82） | [OK] 47 crate / 0 offenders |

### D7.8 W7（BC-04）：`nexus-compute::hts` 拆分（路径不变）

**背景**：方案 §5.2.4 的目标是 `hts.rs` → `hts/{mod,cgroup,sequential_test}.rs`。
实测 `hts.rs`（1199 行）里 `sequential_test`（序贯检验）与 `cgroup`（平台校正）
是**内联公开模块**，与阈值表结构（`Entry` / `ThresholdSource` / `HtsTable`）
交织。

**落地**：`crates/nexus-compute/src/hts/` = `mod.rs`(768 行) + `cgroup.rs`(107 行)
+ `sequential_test.rs`(345 行)。

**契约零变更**：两个子模块**保持 `pub`** ⇒
`nexus_compute::hts::cgroup::CgroupProbe`、
`nexus_compute::hts::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision, HtsError}`
路径**逐字不变**；`nexus_compute::hts::{HtsTable, ThresholdSource, Entry}` 不变；
`lib.rs` 的 `pub mod hts;` 声明**无需改动**（Rust 模块解析对 `hts.rs` 与
`hts/mod.rs` 同构）。

**与方案 §5.2.4 的差异**：方案的示意结构写的是
`mod cgroup;` / `mod sequential_test;`（私有）+ `hts` 层级 `pub use` 转出。
本批**不采用**该形态——那会把公开路径从 `hts::cgroup::X` 变成 `hts::X`，
属**破坏式 API 变更**（与方案「路径不变」的总约束冲突）。故保留公开子模块，
只做物理拆分。

**验证**：`cargo check -p nexus-compute --all-targets` EXIT=0 / 0 warning；
`cargo test -p nexus-compute` ⇒ **103 passed / 0 failed**；
`check_src_module_reachability.py`（G-109）live=0（三个新文件均有 `mod` 指向）；
`check_pub_api_leak.py`（G-135）sites=18 rows=18 scanned_pub_fn=346（模块键迁移
但泄露面未增减）。

**验证盲区（诚实登记）**：`cgroup.rs` 的 `#[cfg(target_os = "linux")]`
`effective_cores()` 分支**在本机（Windows）不可编译验证** ⇒ 须以 CI Linux
runner 的 `cargo test -p nexus-compute` 结果为最终判据（方案 §7.5 的 R-09）。

---

## D8 — 遗留与后续

1. **ADR-312 / ADR-313** 作为本 ADR 的子章节（D2.1 / D2.3）占位登记，
   编号不回收，保持 57 处引用的连续性。
   > **2026-11-18 落档追记**：ADR-312 与 ADR-313 已补录独立物理文件
   > （`ADR-312-event-bus-claim-surface-retirement.md` /
   > `ADR-313-chimera-config-return-to-chimera-cli.md`），本 ADR 的 D2.1 / D2.3
   > 仍为其决策依据；编号连续性按原裁决不变。ADR-315 亦于同日补录
   > `ADR-315-gate-manifest-domainization-and-exclusive.md`。
2. **R2 解冻**时按 D2.2 的重建触发条件恢复 RL 预留面。
3. **`nexus-telemetry` / `nexus-compute` 的 `edition.workspace = true` 缺失**
   属配置漂移，与本 ADR 无涉，由独立批次（`edition.workspace` 一致性收口）
   处理。
   > **2026-11-16 订正**：W1-0 批次实测两 crate 在 HEAD 即已有
   > `edition.workspace = true` ⇒ **该漂移不存在**，方案假设 A7 被证伪，
   > W1-0 为空批次（未做任何改动）。
4. **BC-02 的 3 个 deprecated 方法的物理删除**（`publish_batch_blocking` /
   `publish_claim` / `publish_declaim`）：受**版本窗**时间门约束（≥2 个 CI 周期
   + 1 次发布），且删除 commit 必须与打标记 commit **分离**（D7.2）。届时
   同步复核 `crates/chimera-cli/src/sched_claim.rs` 的模块头文档链接
   （已在本批改为指向 `EventBus::publish`）。
5. **BC-05 的 3 个未降级符号**（`CriticalSubscribed` / `CausalBuffer` /
   `PendingEvent`）：若未来外部消费面归零且 doctest 契约同步改写，可重新评估
   降级（D7.4 已登记各自的阻塞原因）。
6. **`publish_batch` 在降采样策略下的逐位等价性**：批量路径**刻意**摊销采样，
   与逐条发布不可能逐位相同（D7.5）。若未来要求逐位等价，须放弃摊销或改用
   「批量整体采样一次 + 按批内序号推演」的确定性策略——属独立设计议题。
7. **`nexus-compute::hts` 拆分（W7）**：属独立可回滚批次，路径不变
   （`nexus_compute::hts::X` 保持可达）。
