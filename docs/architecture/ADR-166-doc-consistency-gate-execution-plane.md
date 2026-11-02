# ADR-166: 文档一致性门禁的执行面重划（CI 降级为代码面断言，本地 `.ps1` 为发布权威执行体）

* 状态：**Proposed（待架构委员会终批）** — 起草 2026-08-31（Phase R R1-T11）
* 关联：ADR-085（双态治理：已发布权威基线 vs 在途开发）、`docs/governance/DOCUMENT_LIFECYCLE_POLICY.md`（R7 正文不改写）、`.github/workflows/ci.yml:57`、用户级 SCM 策略"所有 `*.md` 仅存本地不入库"

## 背景与实测证据

`.gitignore` 第 54-56 行排除 `*.md`、`.claude/`、`.trae/`。实测（2026-08-31）：

* `git ls-files | Select-String '\.md$'` → **1 个文件**（`docs/reports/toolchain-analysis-report.md`，历史例外）
* `git ls-files --error-unmatch CHANGELOG.md` → `error: pathspec ... did not match`（`check_doc_consistency.sh:116` 注释亦自陈"bb471f9 移除跟踪"）

后果：`ci.yml:57` 的 `bash scripts/check_doc_consistency.sh` 在 CI 的 checkout 下**看不到** `CHANGELOG.md` / `CODE_WIKI.md` / `adr_index.md` / `AGENTS.md` / `CLAUDE.md` / `docs/architecture/ADR-*.md` 中的任何一个，于是：

| 检查 | 脚本位置 | CI 下的真实行为 |
|---|---|---|
| A1 members vs 磁盘目录 | `:60-70` | **有效**（Cargo.toml 与 crates/ 均入库） |
| A2 version 字段存在 | `:71-77` | **有效** |
| B1/B2 架构文档 crate 数对齐 | `:78-102` | 文件不存在 → `[x-warn] 跳过`，**不计失败** |
| C1 CHANGELOG 汇总章节 | `:114-127` | 同上 |
| D1/D2 ADR 文件数与声明数 | `:135-161` | `compgen -G` 空 → `[WARN-D1]`；D2 warn |
| E1/E2 规则文档基线 | `:203-212` | 同上 |

即：**发布清单第 1、2 项（version 与 tag 一致、CHANGELOG 章节存在）在 CI 上不可判定**，而 `.claude/CLAUDE.md` 旧稿写着"CI 已在 ci.yml 自动执行该巡检"——这是一条会导致误信任的陈述。本地实测对照：`.sh` 5 类 12 项 PASS、`.ps1` 6 类 14 项 PASS（含 F2：`types.rs` 实测 144 变体 = `CODE_WIKI.md` 声明值）。

## 决策

1. **不把 `*.md` 纳入 git 跟踪**。理由：用户级 SCM 策略明确"文档/报告/规则只存本地，版本控制聚焦代码、配置、workflow、manifest"；本 ADR 只治"声明与执行面不符"，不改存储策略。
2. **CI 侧明示降级而不是伪装通过**：`check_doc_consistency.sh` 的 warn 分支必须在 job summary 中输出"本环境缺少 md 权威源，B/C/D/E 类未判定（设计如此，见 ADR-166）"，把"跳过"从隐性变成显性。理由：当前形态下 CI 绿会被读作"文档已核验"，这比不检查更危险。
3. **文档一致性的权威执行体 = 本地 `scripts/check_doc_consistency.ps1`**，并写入发布前检查清单新增的**第 2.5 项**（在 `cargo check` 之前）。发布流程的硬约束是"第 2.5 项未全 PASS 不得打 tag"。
4. **A1/A2 继续留在 CI**（它们判的是入库对象，真实有效），并在 `ci.yml:56` 的注释中把该步的名称从"文档一致性巡检"改述为"入库面对齐（members + version）"，消除名称带来的过度承诺。
5. **可判定子集外移到代码面**：凡能用入库对象表达的断言（如 `Cargo.toml` version ↔ 待发布 tag、`fuzz/Cargo.toml` target 数 ↔ `fuzz.yml` 声明、`crates/` 目录 ↔ 层图）继续留在 CI；只有"散文文档里的数字"必然依赖未入库 md，接受其本地判定。
6. 本 ADR 与 `ADR-160 决策 5`（`event_types.rs` 镜像退役，分类真值源收敛为 `types.rs` 单表）同属一类动作：**把真值源从"两份互相抄、其中一份不生效"改成"一份权威 + 一道可执行断言"**。

## 备选项与否决理由

* **把 CHANGELOG/CODE_WIKI/adr_index 纳入跟踪**：语义上最能根治，但直接违反用户级 SCM 策略，且会把大量内部诊断报告推入公开远端仓库。否决。
* **在 CI 里用 artifact 上传的文档做校验**：需要额外的"文档上传→下载→校验"链路，且校验时点与发布时点分离，容易形成新的"看起来有门"。否决。
* **删掉 CI 的这步**：会连带丢失真实有效的 A1/A2 判定。否决。

## 后果

正面：不再有"CI 绿 = 文档已核验"的误信任；发布门禁的执行面与其实际能力一致；本地 `.ps1` 从"可选自查"升为权威环节，因而值得为它修历史遗留问题（如硬编码 `python3` 导致仅有 `py` 启动器时 EXIT=127）。

代价：文档一致性不再由 PR 自动化保障，依赖"发布前本地跑一次"的纪律。缓解：该项进入 `agents.md §7.2` 与 `.claude/CLAUDE.md` 清单第 2.5 项，且 `docs/governance/RK-P_risk_register.md` 的 RK-P30（"写了不跑"）把"任何新增门必须有阴性对照红证"设为通用验收条件。

## 验证

* 本 ADR 批准后，`check_doc_consistency.sh` 在 CI 的 job summary 必须出现"缺少 md 权威源"字样（可用一个只改 `.github/` 的 PR 观测）；
* 本地 `.ps1` 全项 PASS 作为 tag 前置（**项数取脚本末尾自报值**，不在本文写死），且**故意**把 `adr_index.md` 的 ADR 声明数改错一位 → `.ps1` 必须判红（证明权威执行体有牙齿）。
