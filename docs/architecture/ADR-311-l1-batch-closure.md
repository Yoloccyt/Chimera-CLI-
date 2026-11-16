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

---

## D8 — 遗留与后续

1. **ADR-312 / ADR-313** 作为本 ADR 的子章节（D2.1 / D2.3）占位登记，
   编号不回收，保持 57 处引用的连续性。
2. **R2 解冻**时按 D2.2 的重建触发条件恢复 RL 预留面。
3. **`nexus-telemetry` / `nexus-compute` 的 `edition.workspace = true` 缺失**
   属配置漂移，与本 ADR 无涉，由独立批次（`edition.workspace` 一致性收口）
   处理。
   > **2026-11-16 订正**：W1-0 批次实测两 crate 在 HEAD 即已有
   > `edition.workspace = true` ⇒ **该漂移不存在**，方案假设 A7 被证伪，
   > W1-0 为空批次（未做任何改动）。
