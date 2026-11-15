# ADR-309: compute/ 迁出 nexus-core 为独立 crate `nexus-compute`（L0-B）

- 状态: Accepted
- 日期: 2026-11-15
- 批次: L0 深度重构 W4-1（BC-6；施工设计 §5.1 BC-6 / §5.3 / §6 W4-1 / §9）
- 设计源: `plans/aquaman-lockjaw-jessica-cruz.md`（已批准方案）§5.1 BC-6、§5.3 兼容策略总则、§6 W4-1 行、§9 风险与回滚
- 关联: ADR-222（L0-A/L0-B 双层 + `nexus-telemetry` 上提先例，本 ADR 与其 D3 同构）、ADR-223（G-131 L0-B 外部依赖面台账）、ADR-225 D6（L0-B pub 签名泄露台账）、ADR-033（L0 契约层与依赖方向铁律）、ADR-196/201/210/211（C1 计算面治理波次；W4-0 合流裁决「合流而非并行」是本分批复核前提）

## D1 — 背景

- nexus-core 承载 L0-B 的「上帝 crate 残余」：`src/compute/` 子目录 **9 文件 / 5,281 行**（`wc -l crates/nexus-core/src/compute/*.rs` 实测口径），为 nexus-core src 体量的主体部分；外部消费方 **11 crate / 14 文件 / 34 行** `nexus_core::compute::` 引用（`grep -rn` 实测口径，含 `chimera-cli`/`decay-engine`/`faae-router`/`gsoe-evolution`/`hcw-window`/`nmc-encoder`/`osa-coordinator`/`repo-wiki`）。
- compute/ 具备独立成 crate 的全部条件（实施前逐条复核）：模块间仅 `use super::` 自引用；对 nexus-core 其余模块**零** `crate::<其它模块>::` 引用（grep 实测）；外部依赖面仅 `{arc-swap, event-bus, nexus-telemetry, rand, thiserror, tokio}` 六面 —— 方向天然单向，迁出即成无环切片。
- 病灶定性见施工设计 §2.2/§5.2：L0-B 的病灶是「上帝 crate 残余」而非类型设计；compute/ 迁出是拆解第一步，nexus-core 回到「核心基础面」本义（CLV / NexusState / 类型汇聚点）。

## D2 — 候选与取舍

| 候选 | 判定 | 理由 |
|------|------|------|
| A. 留原地 | 否 | 上帝 crate 病灶不动，L0-B 语义无法回到「核心基础面」 |
| B. 并入 event-bus | 否 | 计算面（rayon 池/HTS 阈值/归约/利用率）与事件传输正交，合并违反内聚，且 event-bus 单体化本身是二期评估议题（施工设计 §5.2 R-4） |
| C. 迁出独立 crate `nexus-compute` | **采纳** | 与 ADR-222 D3（nexus-telemetry 上提）同构：依赖倒置已就位（`nexus-telemetry` 的 `SaturationSource` trait 由 compute 侧适配器实现），迁出只是落点变更 |

## D3 — 决策

1. **新建 `crates/nexus-compute`**：机器层号 **1**（L0-B 核心基础面，与 `nexus-core`/`event-bus`/`nexus-telemetry` 同亚层）；机器层号 0/1 不重排（ADR-222 D1 纪律，116+ 份 ADR 层引用证据链不得失真）。
2. **兼容 shim 一个版本窗**：nexus-core 保留
   `pub mod compute { pub use nexus_compute::*; }`；
   `nexus_core::compute::*` 旧路径（含 `compute::bridge` 等子模块路径，经 glob 再导出自然可达）继续可用，类型同一性由 `crates/nexus-core/tests/compute_shim.rs` 编译期锁定（`Option<T>` 赋值断言模式，同 `types.rs` 既有等价测试先例）。
3. **消费方零 diff**：全部生产消费 crate 本批零改动（ADR-222 D4 硬判据沿用）；shim 期消费方渐进直连 `nexus-compute`，完成后独立小批删 shim。
4. **bench 随迁**：`bridge_bench`/`hts_bench`/`reduce_bench` 随 crate 迁移（bench 名与 `[[bench]]` 名不变，阈值键不断裂；bench 内 `nexus_core::compute::` 路径改 `nexus_compute::`，`nexus_core::CLV` 测试替身改为 bench 本地 `BenchClv` —— 新 crate 对 nexus-core 零依赖是红线，不得借 dev-dep 成环）；`clv_bench`/`clv_cosine` 留在 nexus-core。`reduce_bench` 的 registered 四联同步：`perf_thresholds.toml` [[run_bench]] `crate=` 改 `nexus-compute` + `bench_check.yml` heredoc 同行（I7 机器对拍）。
5. **依赖面入册（G-131）**：生产依赖 `{arc-swap, event-bus, nexus-telemetry, rand, rayon, thiserror, tokio, tracing}`（内部边 event-bus/nexus-telemetry 归 G-125 方向门，不进 G-131 外部面）；dev `{criterion, proptest}`。其中 `rayon`/`tracing` 为施工首跑编译实测补登（`bridge.rs` 的 `rayon::ThreadPool(Builder)` 与 `utilization.rs` 的 `tracing::warn!` 均为全路径引用、无 `use` 行，侦察期 grep 面漏捕——以代码为权威源补登；二者属 ADR-222/ADR-225 明示的 L0-B 允许面 `{tokio, tracing, rayon}`，无方向违例）。
6. **台账同批（原子 commit 纪律，ADR-222 先例）**：G-90 手工登记 `nexus-compute` 行（附 reason）；G-131/G-135 `--emit` 重生成；G-125/G-126 `.sh/.ps1` 双源层表 + `expected_crates` 47；`dep_edge_freeze.txt` 登记 3 条新内部边（`nexus-core|nexus-compute`、`nexus-compute|event-bus`、`nexus-compute|nexus-telemetry`）；`impl_maturity_baseline.txt` 的 `nexus-core::compute` 行随迁改键；`lint_ratchet_crates.txt` 继承登记；`doc_count_freeze.txt` 重锁（`check_doc_drift.sh --update-baseline`）并同步 DOC-AUDIT-FACTS 锚点；`crate_contract.py --install` 派生新 crate 契约头并刷新 nexus-core 派生行；CODE_WIKI §3.1/§11 登记。

## D4 — 后果

- workspace crate 数 **46 → 47**（`doc_count_freeze.txt` 重锁，口径 = 根 `Cargo.toml` workspace.members 实测）；assembly 内部边 +3。
- **原子性约束**：门辖区（roster / 双源层表 / 各冻结台账）与新 crate 存在性互为前提，任何中间态必红（G-99/G-105/G-125/G-131/G-135 同时失守），故施工、台账、文档同一原子 commit，整体 revert（Cargo.lock 同批回退）。
- nexus-core src 体量回落（compute/ 迁出后剩余模块为 CLV/config/state 等核心基础面），`prelude` 再导出面不变。
- 风险登记（施工设计 §9）：RSK-1 合流冲突已由 W4-0 裁决前置消解（C1 波次收口后开工）；RSK-2 台账遗漏由「gate 实跑逐一暴露」机制兜底，收口判据 `run_gate_manifest.py [SUM] mismatch=0`。

## 回滚 / 撤销条件

- 回滚 = revert 两个 commit（本 ADR + 施工原子 commit），无其它步骤；shim 设计保证 revert 后 `nexus_core::compute::*` 旧路径仍可用，零消费方影响。
- shim 删除窗口的撤销条件：删除独立小批实施前，若发现 shim 面存在未登记消费方（`grep nexus_core::compute` 非零且未经评审），停止删除、回补登记并重新评审删除时点。

## 未完成 / 后续

- 消费方直连 `nexus-compute` 的渐进迁移为独立工作项，不设截止、不阻塞本 ADR 交付。
- W4-2（ChimeraConfig 迁移分析）/ W4-3（NexusState 副作用治理分析）/ W4-4（event-bus 子系统解耦评估）照旧输出决策建议，不在本批。
