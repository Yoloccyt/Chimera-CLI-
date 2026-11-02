# ADR-193：架构减法 · crate 数 41→39（两批次整 crate 退役 / 整并）

- **状态**：✅ Accepted（2026-09-26，**追认落档**——两批减法已于 `1ead632` / `bd3b10a` 合入 `main`，此前仅存在于 commit message 与根 `Cargo.toml:95` 版本注引用，缺物理 ADR 与索引行；本 ADR 补齐，消除悬空引用）
- **决策者**：E01（架构）；`Chimera架构重构与优化方案_v2.3` §(a) A2/A5 核验触发
- **影响面**：
  - 批次一（`1ead632`）：删除 `crates/router-traits/` 整 crate（41→40）
  - 批次二（`bd3b10a`）：`crates/lsct-tiering/` 分层能力整并入 `crates/cmt-tiering/`（40→39）
  - 根 `Cargo.toml` `members`（39）、`scripts/lint_ratchet_crates.txt`（39）、`check_dependency_rules.sh` 分层四源、`scripts/event_ownership_baseline.txt` 等以 39 crate 为基线的登记面
- **承袭**：ADR-033（L0 契约/strangler 渐进）、ADR-182（可达性冻结/减法治理）、v2.3 §(a) A2/A5 核验结论

## 1. 背景与判据（减法须可证伪，不留悬空引用）

四维深审对 39-crate 基线的核验（`v2.3` §(a) A2）确认：基线由 **41→40→39 两批架构减法**得到，两批均已合入 `main`：

- **批次一 · router-traits 整 crate 退役**（commit `1ead632 refactor(arch): ADR-193 批次一·router-traits 整 crate 退役(41→40)`）：`router-traits` 是路由的纯 trait 抽象层，长期**只有唯一实现**（无第二 provider 落地），属过早抽象（YAGNI）；trait 与实现同 crate 内联即可，独立 crate 无收益却增依赖面与 reachability 维护成本。整 crate 退役，其 trait/类型下沉至实际使用方。
- **批次二 · lsct-tiering 整并 cmt-tiering**（commit `bd3b10a refactor(arch): ADR-193 批次二·lsct-tiering 整并 cmt-tiering(40→39)`，现 `main` HEAD）：`lsct-tiering` 与 `cmt-tiering` 均表达「能力分层冷热」职责，边界重叠造成双写与语义漂移；将 lsct 分层语义并入单一 `cmt-tiering`（warm/cold/ice 三层）消除重叠。

减法后仓库机器四源（`check_dependency_rules.sh` 的 layer_of / ps1 表 / `members` / CRATE-CONTRACT）一致收敛于 **39**（`v2.3` §(a) A10）。**遗留缺陷**：两批 commit message 与根 `Cargo.toml:95` 版本注均以「ADR-193 架构减法」为权威引用，但 `docs/architecture/` 下无 `ADR-193-*.md`、`adr_index.md` 亦无对应行——即 `v2.3` §(a) A5 实锤的「193 仅 Cargo.toml 注释引用、无物理文件无 adr_index 行」悬空引用。本 ADR 追认落档即消此债。

## 2. 决策

- **D1 维持两批减法为既成事实**：不回退（无第二 router 实现、双分层重叠均已消除，回退无收益）。
- **D2 追认 ADR-193 为本 ADR**：以物理文件 + 索引行落地「减法批次」的授权与理由，使 `Cargo.toml:95` 与两条 commit 的引用可解析、`adr_index_sync.py` 文件↔索引对账一致。
- **D3 基线锚定 39 crate**：所有以 crate 数为口径的登记面（lint ratchet、依赖四源、reachability freeze）以 39 为权威基线；未来 crate 增减须另立 ADR，不得静默改动本数。
- **D4 附带项**：`crates/lsct-tiering/` 若残留空目录（`git` 不跟踪空目录，仅文件系统层面），列 Wave-0 待办清理（`v2.3` §(a) R3-2），不属本 ADR 行为变更范围。

## 3. 后果

- **正面**：crate 基线 41→39 有据可查；`Cargo.toml` 悬空引用闭合；分层语义单源（cmt-tiering）。
- **代价 / 约束**：追认型 ADR 无新代码风险（改动已在 `1ead632`/`bd3b10a` 落地并过 CI）；须保持 `adr_index_sync.py` 与文件集一致（本 ADR 同步补索引行）。
- **不触碰**：RL 闸门、R2 冻结（承袭 ADR-042/053，本 ADR 为纯结构减法，与学习面正交）。
