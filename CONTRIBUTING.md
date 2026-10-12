# 贡献指南（分支 / 提交 / PR / 发布流程）

> **文档定位**：本文件是 Chimera CLI 仓库**协作流程类规则的唯一权威源**——分支模型、提交约定、PR 合并、标签与发布。
> 架构与代码类约束不在此重复，分别见：
> [agents.md](agents.md)（项目专属规则全文）、[.trae/rules/nuxus规则.md](.trae/rules/nuxus规则.md)（速查基线）、
> [docs/CONVENTIONS.md](docs/CONVENTIONS.md)（目录与命名白名单）、
> [docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md](docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md)（文档生命周期）。
>
> **最后核对**：2026-10-11（工作树现态；CI 触发与 GPG 条件以 `.github/workflows/` 现行文件为准）
> **变更触发器**：workflow 文件、PR 模板、分支保护设置、本文件任一变更时同批核对。

---

## 1. 分支模型

```
main                         主干，唯一长期分支；始终可发布
├── feat/<slug>              功能分支（仓库主流命名，见下方词表）
├── fix/<slug>               缺陷修复
├── docs/<slug>              纯文档修订
├── perf/<slug> / test/<slug> / refactor/<slug> / chore/<slug> / ci/<slug>
└── release/v<x.y.z>-omega   发布支线（仅在需要从历史点发版时切出，受"禁合并提交"保护）
```

**规则**：

1. 一切变更经 Pull Request 进入 `main`；CI（`.github/workflows/ci.yml`）仅在 **PR 目标 main** 与 **push main** 时触发。
2. 分支名用 kebab-case 英文短横线，与提交类型词对齐；仓库既有的 `feat/*` 分支命名是事实标准。
3. 一个分支只做一件事；批次拆分每波 ≤ 8 个工作项，逐 commit 独立可 revert（与 PR 模板 §7 一致）。
4. `release/v*.*-omega` 支线用于历史点发版，**禁止合并提交（merge commit）**；该支线的发布靠 tag 推送自动携带提交对象，不靠合回主干。
5. 分支保护的具体勾选项（required checks、禁止 force-push 等）在 GitHub 仓库设置中，不在仓库内，以 Settings → Branches 现设为准。

---

## 2. 提交约定

### 2.1 Conventional Commits

提交信息遵循 Conventional Commits，类型集合与 [cliff.toml](cliff.toml) 的 `commit_parsers` **逐字一致**（CHANGELOG 初稿由 git-cliff 解析，类型写错会漏分组）：

| 类型 | 用途 |
|------|------|
| `feat` | 新功能 / 新接线 |
| `fix` | 缺陷修复 |
| `test` | 仅测试改动（新增测试、修 flaky、调断言） |
| `docs` | 仅 Markdown / 文档改动 |
| `perf` | 性能改动（主张需有 criterion 数据支撑） |
| `refactor` | 不改变外部行为的重构 |
| `chore` | 杂项 / 卫生批次（台账、脚本、格式化） |
| `ci` | `.github/workflows/` 与 CI 配置 |
| `build` | 构建系统、依赖、Cargo.toml/profile |

格式：`type(scope): 主题`，scope 用 crate 短名或区域名（如 `fix(mas-sched): ...`、`docs(chimera-tui): ...`）。
仓库历史提交是这一格式的活样本（`git log --oneline -30`）。

### 2.2 硬性纪律（继承 agents.md「Git 协作」，此处为流程权威登记）

- 逐文件 `git add <path>`，**禁止** `git add -A` / `git add .`，防误纳敏感文件。
- **禁止**未经明确请求的 force-push、删除分支、`--amend`、修改 git config。
- **禁止** `--no-verify`、`--no-gpg-sign` 跳过 hooks/签名，除非用户当次明确要求。
- 提交信息用 HEREDOC 传入，避免 PowerShell 解析损坏格式。
- Windows 环境提交含中文的信息前设置 `$env:GIT_AUTHOR_ENCODING = 'utf-8'`；历史教训（v2.3.0 提交信息永久乱码）见 agents.md §10.3a——**损坏后不 amend、不 force-push，用递增 patch 的新 tag 补救**。
- 不主动提交；提交须由用户明确发起。提交时先 `git status` / `git diff` 核查，再暂存、再提交，分步执行。

### 2.3 提交内容边界

- 代码改动与文档改动同批提交时，提交信息按主导面选类型（文档跟随代码同批改是义务，见 §5）。
- 不夹带无关重格式化；不提交 `target/`、`.toolchain/`、`tmp/`、`.env*`、`*.pem`（`.gitignore` 已覆盖，仍需自查）。

---

## 3. Pull Request 流程

### 3.1 生命周期

```
开 PR（目标 main）→ 填写 .github/PULL_REQUEST_TEMPLATE.md 七节
  → CI 四 job（check / e2e / mca-feature / benchmark）
  → CODEOWNERS 覆盖的路径需 @Yoloccyt 审查
  → 按评审意见逐 commit 修订（保持每 commit 可 revert）
  → 全绿 + 审查通过 → 维护者合并
```

### 3.2 PR 描述的七节要求（模板为仓库内权威表）

模板存在的理由写在其开头：本仓反复出现"虚假还债"（用 `optional`/feature/注释把未接线宣布成已接线）。
七节要点的详细判据以 [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md) 为准，摘要：

1. **一句话动机**：还债类 PR 必须写出债的原始编号（`C-xx`/`WI-xx`/`ADR-xxx`/`G-xx`）与所在文档。
2. **两账分开（必填）**：声明面（改了哪几行台账、从几到几）与装配面（生产构造点 `file:line` + 被谁构造）；仅测试构造点的状态只能写 `DORMANT`/`DECORATIVE`，不得写 `WIRED`。
3. **棘轮方向**：只减不增型台账（`*_freeze.txt`、baseline）只能变少；确需新增须附理由与去向。
4. **门与证据**：提交前自查命令（见 §4）；`env-fail` 不等于通过；新门须带 `--selftest` 夹具与一次真变异。
5. **主张可证伪**：写"已落地/已接线"必须能指到生产构造点；文档里的计数交给工具自报，不抄无主数字。
6. **红线**：触碰六类事项之一必须先有新 ADR，不得在 PR 内自行放行（清单见模板 §6：新增 crate / 新增跨层直连 / 增删 NexusEvent 变体 / unsafe / 扩 UNLEARNABLE_SECURITY_RULES / R2·RL·GPU·Python 解冻 / 改机器派生字段）。
7. **回滚**：写清回滚路径（单 commit 可 revert / 需同步撤销的台账行 / 数据面可逆性）。

### 3.3 审查与合并

- **审查人**：[.github/CODEOWNERS](.github/CODEOWNERS) 按 L0–L10 与 tests/fuzz/scripts/.github/docs 分组登记；当前全部条目为 @Yoloccyt（单人维护）。
- **必需状态检查**：以分支保护设置为准；`ci.yml` 的 check / e2e / mca-feature 是合并前门禁，`benchmark` job 与夜间类 workflow（coverage/stress/slo）不是 PR 阻塞项。
- **合并方式**：维护者裁决；`release/*` 支线禁合并提交。main 的合并策略（merge/squash/rebase）以 GitHub 分支保护设置为准，PR 作者按"每 commit 独立可 revert"组织历史，不依赖合并时清洗。
- 平台 flake 测试可用 `#[ignore = "理由"]` 临时隔离，但必须在 PR 描述与发布说明中透明披露，且 main 已有等价覆盖。

---

## 4. 提交前自查（本地门禁）

工具链环境变量见 [.claude/CLAUDE.md](.claude/CLAUDE.md) §1（CARGO_HOME / RUSTUP_HOME / GNU 工具链）。

```powershell
# 类型 / lint / 格式
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt --all -- --check      # 长路径报 os error 206 时逐包 -p 聚合，不可跳过

# 测试（16 核宿主必须限流防编译 OOM；D 盘空间为硬前置）
cargo test --workspace --jobs 4
cargo test --workspace --release -- --ignored --test-threads=1 --nocapture   # 压测/性能红线

# 治理门（PR 模板 §4 要求）
py -3 scripts/run_gate_manifest.py            # G-01~G-98 清单执行器，默认 light 档，rc=0
py -3 scripts/check_entry_contract.py         # 改动 scripts/ 下脚本时
py -3 scripts/check_doc_symbol_claims.py      # 文档写了符号名时
pwsh -NoProfile -File scripts/check_doc_consistency.ps1   # 文档-代码三方一致性
```

`scripts/gate_manifest.toml` 是门禁清单 SSOT（每条含 id/spec/cmd/expect/kind/note）；
判据修复优先于豁免登记。完整发布前检查（audit/deny/docker/体积）见 agents.md §7.2 与
[docs/release/release_guide.md](docs/release/release_guide.md)。

---

## 5. 文档同步义务

代码与文档是同一交付物的两面，遵循 [DOCUMENT_LIFECYCLE_POLICY](docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md)：

- **计数不抄进文档**：crates 数、NexusEvent 变体数、版本号等以代码/冻结文件为权威源，文档写"由 X 现取"；
  机检由 `scripts/check_doc_drift.sh`（锁 `scripts/doc_count_freeze.txt`）与 `check_doc_consistency.ps1` 承担。
- **增删 NexusEvent 变体需"六锁同批"**（types.rs 枚举 + registry 定级 + 文档锚点 + 测试 + 分片清单等，见 PR 模板 §6）。
- **新增 crate**：更新 `Cargo.toml` members、CODE_WIKI 索引、分层映射、可达性冻结清单，且先有 ADR。
- **TUI 面板**：插删面板必须同步 `types.rs` / `app/mod.rs` / `app/tests.rs` / `tests/integration.rs` 四处断言。
- 新增 ADR 遵守编号不跳号、同 ADR 至多 5 个版本、状态变更须治理签署（政策 R 系列）。
- 历史文档不改写，只加历史快照横幅或归档至 `docs/architecture/_archive/YYYY-Qx/`。

---

## 6. 标签与发布

| 项 | 规则 | 权威源 |
|----|------|--------|
| 版本号 | `Cargo.toml` `workspace.package.version` 唯一权威；派生文档同批同步 | Cargo.toml |
| tag 格式 | `v<x.y.z>-omega`；alpha/beta/rc 后缀自动按 prerelease 发布 | release.yml 触发器 |
| 触发链路 | tag 推送 → release.yml（build 5 平台 → test 全量 → docker <100MB → release，前三全绿才建 Release）；fuzz.yml 同 tag 触发（非阻塞） | release.yml / fuzz.yml |
| 制品签名 | 5 平台二进制 + checksums.txt 的 detached GPG `.asc`；仅当 `GPG_KEY_SIGNING` secret 已配置时执行，缺省则静默不签（不是失败） | release.yml 签名 job |
| 版本校验 | `--version` 输出须匹配 `^(aether\|chimera\|chimela)`，真实入口只有 `chimera`；`aether`/`chimela` 是改名期兼容别名，不得删除 | release.yml docker 校验步 |
| 发布指南 | 完整步骤、产物清单、回滚 runbook | [docs/release/release_guide.md](docs/release/release_guide.md)、[rollback_runbook.md](docs/release/rollback_runbook.md) |

tag 重指（仅未发布、run 失败场景）：`git push origin :refs/tags/X` → `git tag -f X <sha>` → `git push origin X`；
已发布 release 的 tag 不重指。历史教训见 agents.md §10.3a 与项目记忆（v2.28.2 三次重指实战）。

---

## 7. 项目级硬约束（知情即守，细节走对应权威源）

- **Rust-First / RL 开发闸门（持续有效）**：Python 侧 RL 训练服务仅保留规划契约（`docs/architecture/rl-client-protocol.md`），
  实体禁止实施；解冻须 R2 影子期通过 + ADR-054 治理签署。R2 关键词扫描是 CI 红线。
- 全 crate `#![forbid(unsafe_code)]`；跨层依赖只允许同层/向下，跨层通信只走 Event Bus / MCP Mesh。
- 性能主张必须附 criterion benchmark；未实测数字不得写入 perf 阈值文件与 workflow。
- MCA 体系默认 off，经 `--features mca` 双轨验证（`mca-gateway` 为 GATED，不是孤岛）。

完整红线与反模式见 agents.md §4.4 / §5 / §6。
