# ADR-312: event-bus 撤主张面物理删除（B5 批次）

- 状态: Accepted
- 日期: 决策与落地 2026-11-16（B5 提交 `2abc977b`，2026-11-16T00:35:32+08:00）；本文件落档 2026-11-18
- 批次: L1 深度重构 B5（ADR-311 决策合订的 D2.1 子章节；本文件为其独立物理档）
- 决策者: NEXUS-OMEGA Team（用户裁决：物理删除，而非"保留代码但撤主张"）
- 关联提交: `2abc977b`（B5 主体）、`ce0c572b`（G-37 台账补收口）、`58ac0ca0`（ADR-311 落档 + 索引登记）、`fe6ba887`（ADR-318 改号批中的 ADR-312 表述更正）
- 关联: ADR-311 D2.1（零生产装配面判据）、ADR-222 D6（台账与代码同批）、ADR-197（序数只增不改）、ADR-160（装配面台账）、ADR-193（架构减法先例）

> **落档性质**：本决策已于 2026-11-16 由 B5 提交落地，此前仅以代码注释
> （`crates/event-bus/src/lib.rs` 等多处 `NOTE(B5/ADR-312)`）与 `adr_index.md`
> 索引行承载，磁盘无独立 ADR 文件。本文件为**补录**，不改变任何既成事实。

---

## D1 — 背景：四个"保留代码但撤主张"的子系统

`event-bus` 在其 crate 文档的「装配态口径」表里，逐一登记了在本仓**无任何生产
装配点**的子系统（判据：排除本 crate 自身 `src`、`tests`、`benches` 后全库检索
构造点）。其中四个在 B5 之前处于"代码仍在、语义主张已撤"的悬置态：

| 子系统 | 文件 | 删除前行数（`git show 2abc977b --stat`） | 消费面实测 |
|---|---|---|---|
| `membrane`（`MembraneFilter`） | `crates/event-bus/src/membrane.rs` | 1,111 | 仅本 crate bench（`publish_membrane` 方法对零生产调用方） |
| `token_ledger`（`TokenLedger`） | `crates/event-bus/src/token_ledger.rs` | 603 | 上游 `cmt-tiering` 闭环本身零生产装配 |
| `segment_per`（`SegmentAwarePER`） | `crates/event-bus/src/segment_per.rs` | 479 | 仅本 crate 测试消费 |
| `pattern_index`（`PatternIndex`） | `crates/event-bus/src/pattern_index.rs` | 321 | 仅本 crate bench 消费 |

**病灶（三选一都不经济）**：

1. **维持现状 = 死面永续**：约 2.5k 行代码 + 2 个 bench + 2 个专属测试文件长期
   占据编译面与维护面，读者会把「代码存在」误读为「能力已装配」。
   这正是 `crates/event-bus/src/lib.rs` 的「装配态口径」小节存在的理由——
   该小节自陈这些子系统"不得被当作已生效能力引用"。
2. **降级为 `pub(crate)`**：不可行。仅被 `tests/`/`benches/` 消费的公开面**不能**
   降级——`tests/`/`benches/` 是独立 crate，只能访问公开 API，降级会直接导致其
   编译失败（ADR-311 D2.1 决策 2 已裁决）。
3. **拆成独立 crate**：被 `crates/event-bus/src/lib.rs` 的 crate 文档明文否决
   （"WHY 不拆分为独立 crate：那会新增 workspace 成员，违 ADR-049 '0 新增 crate'
   与 v3.5 总账的 crate 总账冻结原则"）。

⇒ 用户在 B5 裁决「**物理删除**」，且明确 **`lib.rs` 的"不拆 crate"约束不破**
——删除是本 crate 内部的减法，不是新增成员。

**事件的连带面**：`TokenLedgerRecorded` 变体在仓内**零生产发布点**，
其存在使 `NexusEvent` 变体数与 Normal 分布锁失真，并让事件注册表保留一条
永不触发的定级行。

---

## D2 — 决策

### D2.1 物理删除四个零装配子系统

删除 `crates/event-bus/src/{membrane,pattern_index,segment_per,token_ledger}.rs`
四文件，并同步删除其专属测试与 bench：

- 测试：`crates/event-bus/tests/token_ledger_test.rs`、`crates/event-bus/tests/segment_per_test.rs`
- bench：`crates/event-bus/benches/membrane_delivery_bench.rs`、`crates/event-bus/benches/pattern_index_bench.rs`
- `crates/event-bus/Cargo.toml`：删除上述两个 `[[bench]]` 条目

`crates/event-bus/src/bus.rs` 同步删除 `use` 与 `publish_membrane` /
`publish_membrane_blocking` 方法对（原仅被 bench 自引用）。

### D2.2 `TokenLedgerRecorded` 变体退役（五点同批）

变体摘除必须**五点同批**，缺一即门红：

| 点位 | 文件 + 符号 | 变更 |
|---|---|---|
| 枚举本体 | `crates/event-bus/src/types.rs` | 删除 `TokenLedgerRecorded` 变体 |
| 注册表定级 | `crates/event-bus/src/registry.rs`（`define_event_registry!`） | 删除对应定级行 |
| 计数锁 | `crates/event-bus/tests/variant_count_test.rs`（`VARIANT_COUNT_LOCK` / `NORMAL_LOCK`） | 156→155；Normal 123→122 |
| 属性测试样本 | `crates/event-bus/tests/registry_property_test.rs` | 删除该变体样本与定级断言 |
| TUI 漂移测试 | `crates/chimera-tui/tests/event_coverage_drift_test.rs` | 同步删除引用 |

**序数 143 空洞合法**：按 ADR-197「序数只增不改」，退役变体留下的序号空洞
**保留不重排**；`variant_count_test.rs` 内以注释登记该空洞。

### D2.3 活体半边的保留改造（不做"连带误删"）

- `cmt-tiering` 的证据落盘闭环：原「TokenLedger 账本 → 导出 → 落盘」路径改为
  **直构造 L0 `TokenLedgerEntry`**，`store_evidence_batch` 的覆盖范围不丢。
- `experience_card_bus`：E2E 管线与证据链语义**保留**，仅摘除账本半边。
  相关断言改写而非删除（见 `crates/event-bus/tests/experience_card_bus_test.rs`
  的 `NOTE(B5/ADR-312)` 注记）。

### D2.4 台账同批同步

| 台账 | 变更 |
|---|---|
| `scripts/doc_count_freeze.txt` | `nexus_event_variants` 156→155 |
| `scripts/bench_inventory_freeze.txt` | 删除 `event-bus\|pattern_index_bench` 行 |
| `scripts/perf_redlines.toml` | 删除 `[[redline]]` / `[[redline_exec]]` 的 RL-07（membrane delivery） |
| `crates/event-bus/src/lib.rs` | 装配态口径表与锚行同步（`全部 155 类 NexusEvent`） |

---

## D3 — 后果

### 正面

- **死面清零**：四个子系统 + 专属测试/bench 一次性离盘，编译面与读者认知面同步收缩。
- **事件面诚实**：变体数 155，Normal 分布锁 122，注册表不再保留零发布点定级行。
- **判据可复用**：「生产构造点 grep 为零」的判据（ADR-311 D2.1）在后继批次
  （ADR-320/ADR-321）被沿用作统一口径。

### 负面与破例说明

1. **破了 ADR-222 D6「台账与代码同批」**（本批实际发生过，已在 HEAD 上留红）：
   B5 提交删除了 `crates/event-bus/src/pattern_index.rs`，但**未同批**撤销
   `scripts/check_doc_drift.sh` 的 DECLS 锚点，也**未同批**订正两处仍写 156 的
   文档声明 ⇒ `check_doc_drift.sh`（G-37）在 HEAD 即退 2（"declaration target
   missing"，不可判定）。
   - **precedent**：ADR-222 D6 本身（中间态必红纪律）；同类的"前序批次收尾
     独立成 commit"先例见 ADR-311 D3.1。
   - **收口**：`ce0c572b` 以独立 commit 撤销该 DECLS 锚点与对应夹具，
     并把 `crates/event-bus/src/topic.rs` 与
     `docs/reports/DOC-AUDIT-FACTS_2026-09-20.md` 的声明 156→155。
     该收口**不与任何 L1 批次混提**——它是前序批次的收尾，不是 B5 的内容。
2. **公开路径破坏式变更**：`event_bus::{membrane, pattern_index, segment_per, token_ledger}`
   四路径失效。实测外部生产消费为零，SemVer 影响限于本 crate 的测试/bench。
3. **测试消费面消失**：`token_ledger_test.rs` / `segment_per_test.rs` 整体删除，
   `experience_card_bus_test.rs` 的账本断言改写。这是有意的契约口径调整
   （测试是 API 契约的验证手段，不是生产依赖），不是"删测试通过门"。
4. **序数空洞永久化**：143 号不可回收，后续新增变体只能尾接（ADR-197）。
5. **`check_doc_drift.sh` 的锚点集合减少一条**：`pattern_index.rs` 不再受
   文档声明对拍保护，属预期内的检测面收缩（该文件已不存在）。

---

## D4 — 回滚

- **原子回滚**（推荐）：回滚 commit 顺序为**后提交先回滚**：
  ```powershell
  git revert ce0c572b 2abc977b
  ```
  先回滚收口提交可恢复 DECLS 锚点，再回滚 B5 主体恢复四文件、变体与台账；
  两步都落在同一提交内，不产生中间态。
- **仅回滚 B5**：`git revert 2abc977b` 会同时恢复变体锁、`doc_count_freeze.txt`、
  `bench_inventory_freeze.txt` 与 `perf_redlines.toml` 的删除行（这些都在该
  commit 内）。但 `ce0c572b` 撤销的 DECLS 锚点不会自动恢复——门仍绿，只是
  检测面比 B5 之前少一条，须手工决定是否补回。
- **单文件恢复**（不推荐）：`git show 2abc977b^:crates/event-bus/src/token_ledger.rs > ...`
  只恢复源码而不恢复变体与台账，会立刻产生"模块在盘、变体锁不含其事件"的
  半态，不构成有效回滚。
- 无不可逆点：全部为 git 跟踪文件，历史完整保留。

---

## D5 — 证据索引

| 事实 | 文件 + 符号（行号仅辅助） |
|---|---|
| 四子系统退役登记 | `crates/event-bus/src/lib.rs` + `NOTE(B5/ADR-312)`（:141-143） |
| 装配态口径表（含四子系统处置） | `crates/event-bus/src/lib.rs` + 「装配态口径」（:26-42） |
| 变体计数锁 | `crates/event-bus/tests/variant_count_test.rs` + `VARIANT_COUNT_LOCK`（:53）/ `NORMAL_LOCK`（:59） |
| 冻结基线 | `scripts/doc_count_freeze.txt` + `nexus_event_variants`（:25） |
| 注册表定级退役 | `crates/event-bus/src/registry.rs` + `NOTE(B5/ADR-312)`（:364） |
| 发布方法对退役 | `crates/event-bus/src/bus/publish.rs` + `NOTE(B5/ADR-312)`（:496） |
| bench 条目退役 | `crates/event-bus/Cargo.toml` + `NOTE(B5/ADR-312)`（:55/:86） |
| cmt-tiering 改直构造 L0 证据行 | `crates/cmt-tiering/tests/card_persistence_closed_loop_test.rs` + `B5/ADR-312`（:152） |
| 经验卡片断言改写 | `crates/event-bus/tests/experience_card_bus_test.rs` + `NOTE(B5/ADR-312)`（:56/:59/:150） |
| G-37 收口（DECLS 撤销） | `scripts/check_doc_drift.sh` + `ADR-312(B5 提交 2abc977b)` 注记（:389-392） |
| perf 红线退役 | `scripts/perf_redlines.toml` + `NOTE(B5/ADR-312)`（:54/:225） |
| TUI 漂移测试同步 | `crates/chimera-tui/tests/event_coverage_drift_test.rs` + `B5/ADR-312`（:286） |

**取数命令（本文所有数字的来源）**：

```powershell
# 变体数现取值（唯一权威源）
Select-String -Path scripts/doc_count_freeze.txt -Pattern 'nexus_event_variants'
Select-String -Path crates/event-bus/tests/variant_count_test.rs -Pattern 'VARIANT_COUNT_LOCK|NORMAL_LOCK'
# 部署前后文件与行数（删除面证据）
git show 2abc977b --stat
# 四子系统文件确已不在盘（应全部输出 False）
Test-Path crates/event-bus/src/membrane.rs
Test-Path crates/event-bus/src/pattern_index.rs
Test-Path crates/event-bus/src/segment_per.rs
Test-Path crates/event-bus/src/token_ledger.rs
```
