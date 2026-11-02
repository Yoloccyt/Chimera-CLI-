# ADR-XXX（编号未分配）: 内环依赖检查判据分歧 —— 状态为**未裁决**

> **SUPERSEDED by ADR-187 (2026-09-24)**：本文诊断的问题（Check A 双源分叉、CI 侧红于
> `[GAP-A] gsoe-evolution -> decay-engine`）已按 **§3 路径 3（显式特批）** 落地：
> ADR-187 从 `.sh`/`.ps1` 双侧退役 Check A（其判别力在本文 §2 记录的 2026-09-18 `.ps1` 单侧
> 改动后已塌缩为 Check B 严格子集，净损失 0 可检测违规），并在
> `scripts/dep_edge_freeze.txt` 登记 `gsoe-evolution|decay-engine` 为合法向下边，
> `agents.md §2.2` 十层律优先级定为高于 ADR-054 内环分区。本占位号（`ADR-XXX`）经此宣告
> **不构成有效治理变更**；权威裁决见 `docs/architecture/ADR-187-inner-ring-check-a-retirement.md`。
> 本文 §1/§2 的实测记录保留为历史取证。

> **Status**: ⚠ **未生效 / 待裁决**（不是 Accepted）
> **Date**: 2026-09-18　**Author**: E01 Chief Architect
> **编号状态**：文件名占位符 `ADR-XXX` 从未替换为真实编号——`docs/architecture/adr_index.md` 与
> `CODE_WIKI.md` 对 `ADR-XXX` 命中均为 **0**（2026-09-22 实测）⇒ 本文当前**不构成** `agents.md §2.2`
> 所指"ADR 记录特批"。真实编号须由治理侧分配：物理文件与 `adr_index.md` 的最大主号均为 **185**，
> 而字符串 `ADR-186` 已出现在 `CODE_WIKI.md:718`（该号尚无物理文件）⇒ **提议新编号前先 grep 文档内的
> 预留注记**，只看已落盘文件会撞号。
> **本文件未受 git 跟踪**（`git ls-files` 命中 0），仅本机存在。

## 1. 争议本体（两种规范对同一条边的裁决相反）

`crates/gsoe-evolution`（L5）→ `decay-engine`（L4）这条**向下**边：

| 规范 | 依据 | 裁决 |
|------|------|------|
| 十层依赖律 `L(N) → L(N-1) ✓` | `agents.md §2.2` | **允许** |
| 内环边界（inner-ring 只能依赖 L0/L1 base + 内环白名单） | ADR-054 决策 2/4，实现于 `scripts/check_dependency_rules.sh:253-266` | **禁止** ⇒ 报 `[GAP-A]` |

**已核实的代码事实**（本轮逐条复测）：

* 依赖声明：`crates/gsoe-evolution/Cargo.toml:24` `decay-engine = { workspace = true }`，`:22` 注释以 §2.2 自我辩护
* 真实消费：`crates/gsoe-evolution/src/engine.rs:221`（`breaker: &mut decay_engine::ShadowModeCircuitBreaker` 形参），构造点 `:606`、`:672`、`:711`
  > ⚠ 本文旧版写的"lines 221, 598, 629, 659"中后三个行号**已随代码平移失效**，现值为 606/672/711。
* 该边非装饰品：`ShadowModeCircuitBreaker` 无公开 `trip` 方法（`engine.rs:709` 注释自陈），故其接入深度受限——但边本身在生产路径上。

## 2. 本文实际做了什么（与旧版叙述的差异）

| 旧版主张 | 复测结果 |
|---------|---------|
| "Script Logic Bug：`.ps1` 实现错误，正确逻辑是 L5 可依赖 L0-L4" | 判据确实按此改了——**只改了 `.ps1`**：`check_dependency_rules.ps1:242` `if ($depLayer -gt $crateLayer -and $depLayer -ge 2)`（只判上引） |
| "`check_dependency_rules.ps1` now correctly implements iron law / False positive eliminated" | 仅对**不执行在 CI 的那一侧**成立。CI 跑 `.sh`（`ci.yml:89`，清单 G-08），其 Check A 至今**无层号比较** ⇒ 2026-09-21 实测 `bash scripts/check_dependency_rules.sh` **EXIT=1**，唯一失败 `[GAP-A] gsoe-evolution -> decay-engine` |
| "Risks: None（向后兼容、无生产码改动）" | ❌ 有两个已量化的风险：① **双源分歧**——同一 Check A 两侧判定不同，且分歧落在 CI 不跑的侧；② `.ps1` 新判据是 Check B（`:259` `$depLayer -gt $layer`）的**严格子集** ⇒ Check A 在 `.ps1` 侧成为逻辑死码 |
| Implementation Checklist 5 项全 `[ ]` | 实际 3 项已完成：本文（未编号）、`.ps1` 判据改动、`tests/dependency_rules_inner_ring_test.rs`（**文件在盘**）；**未**完成：`.sh` 侧对齐、GAP-A 在 CI 转绿 |

## 3. 待裁决（三条互斥路径，须由架构委员会选一，不可静默放宽）

1. **改规范**：修订 ADR-054 内环白名单，把"向下边 L(N)→L(N-1)"纳入内环允许集 ⇒ 两侧脚本同步加层号比较，`agents.md §2.2` 与内环边界一致化。
2. **改代码**：把 `gsoe-evolution → decay-engine` 改为事件驱动（跨层只走 event-bus），消除该边 ⇒ 两侧无需改判据。
3. **显式特批**：分配真实 ADR 编号并登记豁免条目（键粒度须写明是 `crate→crate` 边，且**唯一真值源**），`.sh`/`.ps1` 两侧同读该表。

> 已知的**反面**做法（本报告明确不采）：只改一侧脚本让本地变绿——那会把"CI 判定"与"本地判定"拆开，
> 且 `check_layer_map_parity.py` 只对拍**数据表**（层图/crate 名单/members），**看不见判据漂移**，
> 因此现有防线接不住这类分歧。裁决记录见 `docs/reports/Q3-adjudication-inner-ring-gate-2026-09-21.md`。

## 4. 变更摘要（本次文档审查）

1. 状态由"Proposed → Accepted + ARB 已批"改为"未裁决"——理由：编号未分配、索引零命中、CI 侧仍判红，"已批准"无任何可查载体。
2. 删除"Risks: None"与"False positive eliminated"两处与代码不符的主张，替换为 §2 表中实测的双源分歧与死码结论。
3. 修正失效行号（598/629/659 → 606/672/711），并勾掉 checklist 中已实际完成的 3 项、保留 2 项未完成。
4. 新增 §3 三条互斥路径 + 反面做法说明，使本文从"事后自证"变为可指导后续开发的决策件。
