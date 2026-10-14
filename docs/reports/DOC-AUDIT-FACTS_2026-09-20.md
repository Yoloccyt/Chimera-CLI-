# 代码库事实基线（文档审查唯一权威源）

**用途**：本文件是「Markdown 计划文档系统性审查与合并」的**代码事实基准**。所有计划文档的技术声明应与本文件比对；本文件内每条事实均附 `文件路径:行号` 证据，可独立复核。
**采集时间**：2026-09-20（架构减法批次 A+B 落盘后工作树）
**采集方式**：`tomllib` 解析 manifest + `awk` 定界统计 + 门禁脚本实测，非人工回忆。
**取代关系**：文档中凡重复登记下列计数处，应改为**引用本文件**（政策依据：`[CONFIG-REFACTOR-20260920]`「计数改引用不改数」）。

---

## 1. 版本与 crate 规模

| 事实 | 值 | 证据 |
|------|-----|------|
| `workspace.package.version` | **`2.28.2-omega`** | `Cargo.toml:82` |
| `workspace.members` 数量 | **41** | `Cargo.toml:27`（members 数组起点）；`tomllib` 解析实测 41 |
| 上一已发 tag | `v2.28.0-omega` → `94499b4`（2026-09-06） | `git tag` + `docs/architecture/CODE_WIKI.md:4` |
| 已退役并物理删除的 crate | `auto-dpo`、`model-router`（批次 A）、**`acb-governor`**（ADR-182） | `Cargo.toml` members 中不存在；`crates/acb-governor` 目录不存在；`Cargo.toml:215` 注「acb-governor 已按 ADR-182 退役删除」 |
| 退役依据 | `model-router` 按 ADR-172 退役；`mca-gateway` 为唯一存活 LLM 通道 | `scripts/crate_reachability_freeze.txt`（model-router 段注记） |
| ⚠ ADR-182 状态位异常 | 文件头仍写「Proposed（待终批；删除须在 clean-disk 环境执行）」，但 crate **已物理删除** | `docs/architecture/ADR-182*` 头部；属「落盘 ≠ 已终批」治理异常，另案裁决 |

> ⚠️ **计数冲突事实（2026-09-20 时点）→【2026-09-22 P112 复测：已消解】** 当时 `agents.md`/`CLAUDE.md`/`nuxus规则.md`/`CONVENTIONS.md`/`CODE_WIKI.md` 五处仍把 **43 crates** 当现势登记，与 `Cargo.toml` 的 **41** 不符（43 为 v2.28.0 时点值，架构减法批次 `38c72e6` 净减 2）。现测：四份文件已无"43 crate"字样，仅剩**版本增量史句式**"38→43"（`nuxus规则.md:40/83/133`、`INDEX.md:8`、`CODE_WIKI.md:130/155`、`agents.md:171`），属历史陈述；现势值 41 由 `CODE_WIKI.md:40` 与 `:720` 承担，并被 `scripts/check_doc_drift.sh`（自报 `workspace_crates=41`）钉住。⇒ 本条从"待修冲突"降级为"已闭合历史"。

## 2. 事件系统

| 事实 | 值 | 证据 |
|------|-----|------|
| `NexusEvent` 变体总数 | **146** | `crates/event-bus/src/types.rs:28`（`pub enum NexusEvent`）；`scripts/doc_count_freeze.txt` 锁定 `nexus_event_variants = 146` |
| `EventTopic` 类数 | **10** | `crates/event-bus/src/topic.rs:33`；`doc_count_freeze.txt` 锁定 `event_topic_variants = 10` |
| severity() 返回 Critical 总数 | **18** | `crates/event-bus/src/bus.rs:55`（`pub const CRITICAL_TOTAL: usize = 18;`） |
| 其中走 mpsc 旁路 | **14** | `crates/event-bus/src/bus.rs:48`（`CRITICAL_MPSC_VARIANTS = 14`）；权威清单 = `bus.rs:107` `is_critical_mpsc_event()` |
| 其中仅走 broadcast 的历史 Critical | **4**（= 18 − 14） | `crates/event-bus/src/bus.rs:50-52` 注释 |
| Critical 升级动因 | `FormalVerificationFailed` 升入旁路（13→14），L4 深度优化 P1-1 | `crates/event-bus/src/bus.rs:46-47` |
| mpsc 通道容量 | 4096 | `crates/event-bus/src/bus.rs:39` |
| `event_types.rs` 镜像 | **已退役**（ADR-160 决策 5），types.rs 为单表 | `agents.md` §5.3；`docs/architecture/CODE_WIKI.md:9` |

> ⚠️ **计数冲突事实（2026-09-20 时点）→【2026-09-22 P112 复测：已闭合】** 权威清单实测 `crates/event-bus/src/bus.rs:109 is_critical_mpsc_event()` **14 个 match 臂**；`nuxus规则.md:169` 现写「14 个」✓，`chimera.md` §5.3 的 14 个名字与 bus.rs **逐名同序一致**（脚本对称差 = ∅、顺序一致 = True），`agents.md` §5.3 已改指 `registry.rs` 注册表。原句保留如下不改写：（见 `crates/event-bus/src/backpressure.rs:13`、`bus.rs:319` 同源旧注释），实际为 **14**；`nuxus规则.md` 另处登记 145、`agents.md` 登记 144，实际 **146**。

**双清单同步红线**：新增 Critical 事件必须同时改 `severity()` 与 `is_critical_mpsc_event()` 两处，并使守护测试常量断言不回退（`bus.rs:43-45`）。

## 3. 装配面与可达性（ADR-160 棘轮）

| 事实 | 值 | 证据 |
|------|-----|------|
| 冻结孤岛登记条目 | **0**（清单已空，仅剩注释） | `scripts/crate_reachability_freeze.txt`（非注释行实测 0） |
| 棘轮实测（2026-09-21 现跑） | workspace **41** · 生产可达 **32** · unreachable **0** · frozen **0** · new_gaps **0** → `[OK] ratchet holds` | `bash scripts/check_crate_reachability.sh` 输出 |
| GATED（仅 feature 编译，不入默认二进制）**9 个** | `chtc-bridge` · `csn-substitutor` · `mas-sched` · `mca-gateway` · `mtpe-executor` · `nexus-hook` · `omega-learner` · `sesa-router` · `ssra-fusion` | 同上脚本 `[GATED]` 行 |
| `mas-sched` / `nexus-hook` | 经 `r2_island_repayment` feature 接线（`chimera-cli/Cargo.toml:21`），Frozen 条目已删（P0 治理批次）→ 现归类 **GATED** | `crate_reachability_freeze.txt` 首段注记 + `[GATED]` 行 |
| `mca-gateway` | **GATED**（`optional = true` + `mca` feature），不列入孤岛清单 | `crate_reachability_freeze.txt` mca-gateway 段；ADR-065 决策 6 |
| 判据 | dev-dependency 与 feature-gated 边**不计入**默认二进制装配面 | `crate_reachability_freeze.txt` 语义段 |
| 三条偿还路径 | ①组合根接线 ②optional+feature ③ADR 记录 | `crate_reachability_freeze.txt` 注释 |

> ⚠️ **冲突事实**：`agents.md` §1.2 / §2.1 仍登记「**28 生产可达 + 14 冻结孤岛 + 1 GATED**」，为 2026-09-03 时点值；当前冻结孤岛为 **0**。权威口径应为 `scripts/check_crate_reachability.sh` 实时输出，而非文档点位值。

## 4. 十层架构（L0 + L1–L10）分层实况

> 分层映射权威源为 `docs/architecture/CODE_WIKI.md §2.1`。与旧规则文件的**结构性差异**（非计数）：

| 层 | 当前成员 | 与 agents.md §2.1 的差异 |
|----|---------|------------------------|
| L1 Core | `nexus-core` · `event-bus` | `model-router` **已移除**（旧文档仍列于 L1） |
| L5 Knowledge | `repo-wiki` · `gsoe-evolution` | `auto-dpo` **已移除**（旧文档仍列于 L5） |
| L6 Router | `osa-coordinator` · `kvbsr-router` · `faae-router` · `sesa-router` · `omega-learner` · `router-traits` | `router-traits` 为批次 B 新增（旧文档未登记） |
| L10 Interface | `chimera-cli` · `chimera-tui` · `chtc-bridge` · `mcp-mesh` · `csn-substitutor` · `mca-gateway` · `nexus-app-server` | — |

## 5. CLI 命令面

`Commands` enum：`crates/chimera-cli/src/cli.rs:154`–`494`，共 **18** 个子命令。

| 命令 | 行号 | 命令 | 行号 |
|------|------|------|------|
| `Help` | 167 | `Parliament` | 304 |
| `Run` | 180 | `Mcp` | 323 |
| `Chat` | 198 | `Audit` | 342 |
| `Tui` | 213 | `Agent` | 365 |
| `Quest` | 232 | `Doctor` | 408 |
| `Config` | 253 | `Completions` | 429 |
| `Wiki` | 267 | `Llm` | 445 |
| `Grep` | 288 | `Exec` | 464 |
| `Serve` | 480 | `Acp` | 493 |

> 旧文档常见描述「`chimera llm` 一级子命令 + `chimera grep` 双通道」仍然成立；但文档未登记 `serve` / `acp` / `exec` / `agent` 等组合根命令。

## 5b. 产物与命令命名事实（多份文档在此处出错）

| 事实 | 值 | 证据 |
|------|-----|------|
| 唯一构建二进制名 | **`chimera`**（Windows 为 `chimera.exe`） | `crates/chimera-cli/Cargo.toml:156` `[[bin]] name = "chimera"`；实测 `./target/release/chimera --version` → `chimera 2.28.2-omega` |
| CI Release 产物名 | **`chimera-<平台>`**（如 `chimera-windows-x86_64.exe`） | `.github/workflows/release.yml:52` `artifact_name`、`:549` 产物表 |
| `aether` / `chimela` 的地位 | **非命令入口**。`aether` 为历史内部代号（v1.x 时期 binary 名，后重命名）；`chimela` 仅出现在 ① CI 版本校验的兼容前缀 ② `install.ps1` 默认安装目录 | `release.yml:451` 接受 `^(aether\|chimera\|chimela)`；`install.ps1:38` 默认 `-InstallDir $env:LOCALAPPDATA\Programs\chimela` |
| `generate` 子命令 | **不存在**（`cli.rs` 全文 `generate` 零命中）。一次性生成走 `run <PROMPT>` | 实测 `chimera run --help` → `Usage: chimera run [OPTIONS] <PROMPT>` |
| `config` 子动作 | `init` / `list` / `show` / `path` | 实测 `chimera config --help` |
| 默认配置文件路径 | **`~/.chimera/omega.yaml`**（可由 `--config` 覆盖） | `crates/chimera-cli/src/config.rs:34`、`cli.rs:77` |
| Docker 镜像名 | `ghcr.io/<owner>/chimera-cli` | `.github/workflows/release.yml:34` `IMAGE_NAME`（`:31` 注释解释仓库名尾 `-` 不能作为 reference 的原因） |

> ⚠ **代码侧待修（非 md，本审查未动）**：`crates/chimera-cli/src/cli.rs:521` 的 `config init` 帮助文本仍写「默认 `~/.aether/omega.yaml`」，与同文件 `:77` 及 `config.rs:34` 的 `.chimera` 矛盾——该矛盾会直接显示在 `chimera config --help` 输出里（已实测复现）。

## 6. TUI 面

| 事实 | 值 | 证据 |
|------|-----|------|
| `PanelId` 变体数 | **27** | `crates/chimera-tui/src/types.rs:50`（`pub enum PanelId`，定界计数） |
| 新面板接线四处 | `types.rs` + `app/mod.rs` + `app/tests.rs` + `tests/integration.rs` | `agents.md` §8「Concord TUI 红线」 |

> ⚠️ **冲突事实**：旧文档口径「22 面板」为 W0~W6 时点值，`agents.md` 已更新为 27；`.qoder/rules/chimera.md`（派生副本）未同步。

## 7. ADR 与基准

| 事实 | 值 | 证据 |
|------|-----|------|
| ADR 物理文件数 | **88**（`docs/architecture/ADR-*.md`；全 docs 树 95） | `ls docs/architecture/ \| grep -cE "^ADR-"` |
| ADR 最大编号 | **ADR-185** | 实枚举尾值 |
| `adr_index.md` 登记行数 | **101** 条 | `^\| *ADR-` 行计数 |
| ⚠ 编号≠文件：合并档 | 4 份覆盖 62 个编号：`ADR-095-134-phase1-consolidation` · `ADR-135-144-phase2-governance` · `ADR-145-152-phase3-governance` · `ADR-153-156-phase4-governance` | `ls` 实测 |
| ⚠ 编号无文件 | `ADR-086~094` 登记于 v4.0 文档 §2.5，本目录无文件（索引自述） | `adr_index.md` 声明行 |
| 索引导航性 | `adr_index.md` **零文件链接**；常规号靠 `ADR-NNN-*` 命名约定可 glob，但**合并区间号与无文号 glob 必落空**（实测 `ls ADR-120-*` → No such file） | 脚本 `tmp/adr_index_check.py`：links=0 |
| criterion bench 文件数 | **116** | `ls crates/*/benches/*.rs` |

> ⚠️ `agents.md` 表述「实际文件已至 ADR-182」，实测为 **ADR-185**。

## 8. 测试基线（须重测，不可引用旧值）

| 事实 | 值 | 证据 |
|------|-----|------|
| 最后登记的全量回归值 | 11587 passed / 0 failed（**2026-09-02 时点**） | `docs/reports/redundancy-R9-disposition_2026-09-02.md`；`docs/architecture/CODE_WIKI.md:9` |
| 当前有效性 | **已失效**——批次 A+B 架构减法、三层锁双清单变更后未重测 | `docs/architecture/CODE_WIKI.md:9` 自述「本轮架构减法/三层锁变更后待重测」 |
| 演进链 | v2.26.0 9954 → v2.27.0 10836 → v2.28.0 11522 → 11564 → 11587 | `docs/architecture/CODE_WIKI.md:9` |

**重测前置条件**（血泪教训，见项目记忆）：D 盘剩余空间为硬前置（`target/` 约 24GB）；须 `--jobs 4` 限流防 OOM；`#[ignore]` 压测须 `--release`；release-ignored 需 `--test-threads=1`。

## 9. 环境事实（可执行性相关）

| 事实 | 值/结论 | 证据 |
|------|---------|------|
| 工具链 | GNU `stable-x86_64-pc-windows-gnu`，linker `D:/msys64/mingw64/bin/gcc.exe` | `.cargo/config.toml`；`agents.md` §工作目录与平台 |
| `rust-toolchain.toml` | **刻意不入库**（单值配置会破坏跨平台 CI） | `agents.md` §10.5 |
| CI Rust 版本 | 4 个 Rust job 钉定 `1.97.1` | `agents.md` §7.1 注（adb3517） |
| 本机 Git Bash **无 `python3`** | `scripts/check_doc_consistency.sh` 第 **163** 行调用 `python3` → `command not found`，该门禁**本机不可运行** | 实测 2026-09-20 |
| audit 口径 | `cargo audit --deny unmaintained --deny unsound`（非 `--deny warnings`），6 个 ignore（2026-09-22 实测 `audit.yml:65` 的 `run:` 行含 6 条 `--ignore`）| `agents.md` §7.2 步骤 3；`.github/workflows/audit.yml:65` |

## 9b. 红线门禁实跑状态（2026-09-21 现测，勿引用文档口径）

> 文档里大量「零违规 / 已收口」类断言属**治理断言**，须以现跑退出码为准。以下为本轮实测。

| 门 | 命令 | 现测 | 说明 |
|----|------|------|------|
| 依赖铁律 | `bash scripts/check_dependency_rules.sh` | **EXIT=1（红）** | 唯一失败 `[GAP-A] gsoe-evolution -> decay-engine violates inner-ring boundary`（内环 crate 只可依赖 L0/L1 基座 + 内环白名单）；同跑 `[C] disk crates 41 / layer map 41`、`[D] chimera-mas internal deps 12/16` |
| 性能红线（静态） | `bash scripts/check_perf_redlines.sh --static-only` | **EXIT=1（红）** | 唯一违规 `I7: bench_check.yml run_bench lines == [[run_bench]] table (33 pairs)`；同跑 `#[ignore]` 归属门 **PASS**（70 条：manual-only 10 / slo-blocking 13 / slo-daily 47；`ratchet unignore-target 0<=0` PASS） |
| 性能红线（自证） | `bash scripts/check_perf_redlines.sh --selftest` | **EXIT=2（不可判定）** | 门无法自证有牙时，其静默成功不可引为「bench 已过」证据 |
| 计数漂移 | `bash scripts/check_doc_drift.sh` | EXIT=0 | 146/10/41 同步 |
| 文档名单 | `py -3 scripts/check_doc_symbol_claims.py` | EXIT=0 | 81 gap 全登记 |
| 跨文档数值 | `py -3 scripts/xdoc_precise.py` | **EXIT=2（不可判定）** | 本机 Git Bash 无 `pwsh`，`.ps1` 自报值取不到 |

⚠ **凡文档声称「依赖铁律零违规 / `.ps1`+`.sh` 双源 EXIT=0」者，在 GAP-A 收口前均不成立**（该门为 CI 阻塞门，见 `.github/workflows/ci.yml:89`）。裁决与在途处置见 `docs/reports/Q3-adjudication-inner-ring-gate-2026-09-21.md`（他方批次所有，本审查仅引用不改写）。

## 10. 文档治理既有基建（审查应复用，勿另造）

| 资产 | 作用 |
|------|------|
| `scripts/check_doc_drift.sh` + `scripts/doc_count_freeze.txt` | 三计数漂移棘轮；`--update-baseline` 重锁；未登记漂移 FAIL，登记后修文档未删行 FAIL as STALE |
| `scripts/check_doc_consistency.{sh,ps1}` | 文档一致性巡检（本机受 §9 `python3` 缺失影响） |
| `scripts/t2_doc_paths_verify.py` / `scripts/xdoc_precise.py` | 文档路径与跨文档表述精确校验 |
| `docs/architecture/doc-roles-matrix.md` | 每份 md 的 5 维标签（Diátaxis/角色/状态/基线/Owner） |
| `docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md` | SoT + 4 档刷新 + 3 类归档 + 6 触发器 + 7 铁律 |

**⚠ 关键约束**：`.gitignore:54` 含 `*.md` —— 文档**不受 git 保护**。删除任一 md 均不可经 git 恢复。因此「删除过时描述」一律采用：就地删改 + 整篇取代时移入 `_archive/`，**不做不可逆删除**（需删除须另行确认）。

---

## 11. 门禁锚点声明（`check_doc_drift.sh` DECLS 指向本文件）

> 本文件是「文档侧计数」的登记点：规则文件按 `[CONFIG-REFACTOR-20260920]` 政策**不再**就地登记计数，
> 故漂移门禁的两处锚点自 `agents.md` 改指本文件。下列两行是门禁 ERE 的匹配目标，**格式不可改写**
> （门禁按「匹配文本中第一个数字」与代码实测值比对）。改数须同步 `scripts/doc_count_freeze.txt`。

- Workspace × **39 crates**（`tomllib` 解析 `Cargo.toml` members 实测;ADR-193 减法批次 router-traits 退役 41→40、lsct-tiering 并入 cmt-tiering 40→39）
- 定义 152 个 `NexusEvent` 变体（计数以 `crates/event-bus/src/types.rs` 枚举实测为准，并由 `scripts/check_doc_drift.sh` 自报校验；ADR-197 新增 `ComputePoolSaturated` 后 151→152）

---

## 12. 命令可执行性事实（P46 波次现测，配 `tmp/doc_cmd_scan.py` 复现）

> 用途：任何"按文档搭环境/跑测试/跑 bench"的可执行性判定，先取本节，不要重新推导分母。
> 复现：`PYTHONIOENCODING=utf-8 py -3 tmp/doc_cmd_scan.py`（自带 8 坏例 + 8 good 例自检，未过则 `exit 3` 不出数）。

| 事实 | 实测值 | 证据 |
|------|--------|------|
| 可校验 package 数 | **42**（41 members + 根 package） | `Cargo.toml` members + `tomllib` |
| 根 package 名 | `chimera-e2e-tests`（无 `-p` 的 cargo 命令默认作用于它） | `Cargo.toml:6-7` |
| 根 package `[features]` | **仅 `mca`** | `Cargo.toml:24` |
| 根 `[[test]]` 注册 target | **37** | `grep -c '^\[\[test\]\]' Cargo.toml` |
| 顶层自动发现 `tests/*.rs` | **3**（2 个已注册 + `auto_dpo_zombie_test.rs` 未注册，属待清理项） | `ls tests/*.rs` |
| `r2_unfreeze` feature 落点 | `crates/chimera-cli/Cargo.toml:19` ⇒ **命令必须带 `-p chimera-cli`** | 同上 |
| `rl-client` feature 落点 | `crates/chimera-cli/Cargo.toml:10` | 同上 |
| chimera-tui `[[bench]]` | **12** 个（含 `render_bench` `:103-105`、`data_pipeline_bench` `:91-93`） | `crates/chimera-tui/Cargo.toml` |
| 已不存在的 test target | `tui_handshake_e2e`（文件与注册行均已删） | `docs/reports/code-health-cleanup-report.md:16` |
| 仓库不提供的脚本 | `export_clip_onnx.py` / `export_videomae_onnx.py` / `export_whisper_encoder_onnx.py`（须自行存盘） | 按 `export_*onnx*.py` 全库检索 0 命中 |

**判据形状（写死以免下一波重犯）**：
① `--test`/`--bench` 的校验上下文**必须**在无 `-p` 时回落根 package —— 本仓库 E2E 命令的惯例形态就是
`cargo test --test <name>`，只查带 `-p` 的行会漏掉全部 37 个 target 的引用面；
② `--features` 的解析**不得跨空格**（否则把后续 flag 与 target 名当成 feature 名）；
③ `-p X`/`-p <crate>` 这类**变量占位**不是包名，不参与存在性判定；
④ 否定式引用（同行含 `勿执行/不可执行/已失效/已退役/幽灵名/不存在/不提供/待实现/原命令/原写法`）
只免判据、不免分母，且**豁免项须单独打印**供人工过目（本轮 6 条）。

---

## 13. 配置面事实（P48 波次现测，权威源 `crates/chimera-cli/src/config.rs`）

| 事实 | 实测 | 证据 |
|------|------|------|
| Figment **有效**源数 | **3**：`Serialized::defaults` → `Yaml::file` → `Env::prefixed("CHIMERA_").split("__")` | `config.rs:219-221` |
| CLI 参数是否进 Figment | **否**——注释自陈"CLI 参数目前仅影响 config_path，未直接进入 Figment" | `config.rs:216-218` |
| 注册的文件 provider | 只有 `Yaml`（**无** Toml provider） | `config.rs:28` |
| 默认配置路径 | `~/.chimera/omega.yaml`（`--config` 只改路径） | `config.rs:43` |
| 内嵌默认模板 | `config.rs:253-418`（`:418` 是唯一的 `"#` 终止符）；顶层 section **14** 个：`nexus quest thinking_toggle repo_wiki model_router osa kvbsr pvl mtpe gqep seccore mcp evolution monitoring` | 与 `LazyConfig` 注释"14 个 section 按需解析"（`:470`）互证 |
| 模板落盘触发点 | `chimera config init`（`commands/config.rs:32`）或 `chimera doctor`（`commands/doctor.rs:248`）；**不会**在首次运行自动写 | `init_config_file` 无其他调用点（实测） |
| `examples/config.sample.yaml` | **9 字节**，内容仅一行标题 ⇒ 零配置项 | `wc -c` 实测 |
| `examples/config.sample.toml` | 1,625 B，但头部三处与代码不符：落地 `~/.aether/omega.toml`（非代码读取路径）、TOML 格式（无 provider）、"四源含 CLI"（见上两行） | `head -8` 实测 |

**被文档误写的配置键**（错写法 → 真名）：
`decay.high_risk_decay` → `seccore.capability_decay.high_risk_decay`（默认 0.2，`config.rs:354-364`）；
`scc.max_entries` → SCC 侧真名 `capacity`（默认 256，`crates/scc-cache/src/config.rs:22`+`:64`）；
`prediction_depth` → `default_prediction_depth` / `max_prediction_depth`（`crates/nexus-core/src/config.rs:427`+`:429`，模板 `:344-345`）；
`speculative_ttl_seconds` / `half_life_secs` / `ttl_seconds` → 在 `crates/**/*.rs` **0 命中**（既非键也非字段，勿再引）。

**探针 `tmp/doc_cfg_scan.py` 的口径边界**：接受集 = 319 个配置键叶子（取自各 `config.rs` 与 `examples/config.sample.*`）；
精度闸门 = 同行须含配置语境词（`omega.yaml|配置|config|默认值|CHIMERA_*|figment|.toml|feature`）；
自检 = 1 坏例必检出 + 1 真实叶子必接受 + 接受集规模须落在 `[50, 1500]`。
⚠ **只判"键名是否存在过"，不判层级**（层级错须按上表 14 个 section 人工对）；
且接受集把**样例自身的键**也算进去 ⇒ 样例漂移会被判"存在"（本轮 `half_life_secs` 就是这样被放过，最终靠人工 grep 定性）。

**已逐条定性为跨命名空间误判的 6 类（勿重复分诊）**：
`workspace.members`（`Cargo.toml` 路径，非 omega.yaml 键）｜`config.edit`、`panel.drill_down`
（**真实** TUI action id：`crates/chimera-tui/src/actions/domains/config.rs:13`、`app/key_dispatch.rs:975`、
`actions/codegen.rs:309`）｜`frame.render_widget`（ratatui API，`adr_index.md:114` 引的是外部框架方法）｜
`process.env`、`spec.env_whitelist`（`docs/gvisor-deployment.md:324` 里的 JS / gVisor 策略字段，非本项目配置面）。


## 14. CLI 前缀与二级子命令面（P54 波次现测，配 `tmp/doc_cli2_scan.py` 复现）

| 事实 | 值 | 证据 |
|---|---|---|
| 唯一可执行命令名 | `chimera` | `crates/chimera-cli/Cargo.toml:155-157`（`[[bin]] name = "chimera"`） |
| `aether` 的身份 | 内部代号，**不是命令名** | 只出现在 CI 对 `--version` 输出的兼容 grep —— 实测在 `.github/workflows/test-install-scripts.yml:171`（`grep -Eq '\^(aether\|chimera) [0-9]+\.[0-9]+\.[0-9]+'`）；口径说明见 `agents.md §10.2` |
| 顶层子命令 | 19 个可接受名（含别名 `zed`） | `crates/chimera-cli/src/cli.rs:154`（`enum Commands`），复现见 §12 探针 |
| 有二级子命令的父命令 | 5 个：`agent` `config` `llm` `mcp` `quest` | `cli.rs` 各变体的 `#[command(subcommand)] action: XxxAction` |
| 二级子命令名（共 **22**） | `agent`: cancel inspect list spawn｜`config`: init list path show｜`llm`: channels list set-default show strategy test｜`mcp`: call inspect list serve｜`quest`: cancel checkpoint list show | `cli.rs:564`/`:520`/`:592`/`:533`/`:498` |
| 命名规则 | clap v4 默认 kebab-case（`SetDefault` → `set-default`） | 全文件 `grep -n "rename_all\|name *="` 于 5 个 Action enum 内 **0 命中** ⇒ 无覆盖 |

**判据与已知假阳性来源（务必先读再改判据）**：

* `Name {` 的**名字与花括号之间有空格** ⇒ 用 `[,{]` 紧邻匹配、或"见到 `{` 先加深度再取名"的深度状态机，
  都会**静默丢掉全部结构体型变体**（本会话内两种写法各错一次：接受集从 22 缩到 10，
  并把合法的 `chimera agent spawn` 报成幽灵）。⇒ 正对照用**逐行人读真值**钉住（4/4/4/4/6 = 22）。
* `chimera config dump` 这个二级形态在 `ConfigAction` 里**不存在**（只有 init/list/path/show）；文档里出现于
  `docs/optimization/v1.2.0/task4_oncecell_verification_report.md:104,221`（写作 `aether config dump`）。
* 文档内 `aether <子命令>` 形态实测 **7 处**：已修 3 处（`docs/tui/README.md:140` 启动命令、
  `docs/audit/dimension_f_security.md:201`、`:202`）；余 4 处属**日期证据件**，按处置口径只登记不回改
  （`从零搭建完全指南.md:446`、`DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md:528`、
  `docs/optimization/v1.2.0/task4_oncecell_verification_report.md:104`、`:221`）。

**复现**：`PYTHONIOENCODING=utf-8 py -3 tmp/doc_cli2_scan.py`（433 份文档、13 处 `chimera <父> <子>` 命中、
接受集外命中即 `exit 1`；自检含 6 good〔覆盖结构体型与 kebab 复合名〕+ 5 bad〔含 `config ini`、`agent spawns`、
`llm setdefault` 近邻形〕）。

## 15. 全库 path:line 锚点普查（P58 波次现测，配 `tmp/anchor_bounds_sweep.py` 复现）

| 结论 | 值 | 说明 |
|---|---|---|
| 参与判定的仓库相对锚点 | **767** 个 | 只判 `crates/`、`scripts/`、`tests/`、`fuzz/`、`examples/`、`docs/`、`.github/` 前缀 |
| 判为"文件不在盘" | 39 种 | **但其中 12 种是 crate 内相对路径**（`tests/integration.rs`、`tests/cli.rs`、`tests/proptest.rs`、`tests/concurrent.rs`、`tests/lock_holding.rs`、`tests/qeep.rs`、`tests/error_paths.rs`、`tests/m3a_input_routing_test.rs`、`tests/dual_source_invariant_test.rs`、`tests/viz_components_test.rs`、`tests/slash_command_test.rs`、`tests/rl_client_test.rs`）⇒ 实体在 `crates/*/tests/` 下，**不是幻影**；1 种是**目录写成文件**（见下） |
| 真·不存在的测试文件 | `tests/command_palette_test.rs` ×2 | 全树无同名文件；引处为日期评估件 `docs/reports/tui-four-dimension-eval-2026-09-06.md:80` |
| 目录被写成文件 | `crates/chimera-tui/src/config.rs` | 实为目录 `src/config/{mod,theme,layout,persistence,tui_bible}.rs`；`TuiConfig` 在 `src/config/mod.rs:48`，**10 个 `pub` 字段**与文述"基线 10 字段"一致 ⇒ 只是**模块化后路径未跟** |
| 行号越界 | 2 种 | `osa-coordinator/src/masks.rs:47`（文件仅 **17 行**，见下）；`efficiency-monitor/src/lib.rs:71-89`（文件 **85 行**，函数已迁 `monitor.rs:50`，§13/账本 §63 已注记） |
| 已删 crate 的锚点 | `model-router`(12 处)、`acb-governor`(2)、`auto-dpo`(2) | 全部落在**日期证据件**（`docs/audit/dimension_{c,e,f}`、`ADR-044`、`RK-P_risk_register`）⇒ 属史实引用，不改写，登记即止 |

**`SparseMask` 位置迁移（本波新发现，需在两份审计件注记）**：
`crates/osa-coordinator/src/masks.rs` 现仅 17 行，`:17` 为 `pub use nexus_contracts::SparseMask;`
（P2-W5.2 类型上提 L0 以消类型分裂）。真身 = `crates/nexus-contracts/src/masks.rs`
（`struct SparseMask<T>` `:53`、`select_top_k` `:142`）；`OmniSparseMasks` = `crates/nexus-contracts/src/omni_masks.rs:46`。

**复现口径**：`PYTHONIOENCODING=utf-8 py -3 tmp/anchor_bounds_sweep.py`（内置 canary 自检：
植入越界/正常/缺文件三类，抽不全即 exit 3 不出数；canary 文件跑完即删）。

## 16. osa-coordinator 卡片类型/方法真值与符号门盲区（P59 波次现测）

**卡片真值**（`docs/architecture/CODE_WIKI.md` 的 osa-coordinator 卡，修后）：

- 类型只有两个 + 一个协调器：`OmniSparseCoordinator`、`OmniSparseMasks`
  （定义 `crates/nexus-contracts/src/omni_masks.rs:46`）、`SparseMask<T>`
  （定义 `crates/nexus-contracts/src/masks.rs:53`）。
- “五维”是**字段**而非五个类型：`routing`/`context`/`memory`/`audit`/`budget`
  = `crates/nexus-contracts/src/omni_masks.rs:48-56`，元素类型 `SparseMask<ToolId/FileId/MemoryId/OperationId/TaskId>`。
- 方法真名 `compute_all_masks()`（`crates/osa-coordinator/src/coordinator.rs:322`，async）与
  `compute_all_masks_batch()`（同文件 `:458`）。

**五个幽灵类型名**（旧卡片所列，全库零定义）：`RoutingMask`/`ContextMask`/`MemoryMask`/`AuditMask`/`BudgetMask`。
判据 = `grep -rn -E 'pub (struct|type) <名>' crates/*/src/` 命中 0（前四个）；`BudgetMask` 仅 1 处，
且是 `crates/nexus-contracts/src/affinity.rs:194` 注释里的**散文用词**，不是类型。

**符号门两个盲区（`scripts/check_doc_symbol_claims.py`）**：

1. `code_symbols()` 把源码标识符连注释一起收 ⇒ `BudgetMask` 被当作“存在的符号”而**不报缺口**；
   其余四个因先前已登记在 `scripts/doc_symbol_claim_baseline.txt`（理由列 `Q14`）而免检。
2. `IDENT` 只提取**表格行**内反引号里的 PascalCase 首段 ⇒ 方法名（`compute_masks()`）与
   非表格行的断言**不参与判定**。⇒ “门 PASS”不等于“卡片里的方法名存在”。

**L6 星型耦合现状**：`kvbsr-router`/`faae-router`/`sesa-router`/`omega-learner` 的 `Cargo.toml`
**均无 `osa-coordinator` 依赖边**（`grep -c osa-coordinator` 在非零的那些文件里命中的是 `# WHY:` 注释行），
四家一律依赖 L0 `nexus-contracts`。旧叙述“四家均依赖 osa-coordinator，形成 L6 内部星型”已失效。

**复现口径**：`py -3 tmp/…` 之外，三行命令即可自证 —— 见上「判据」引号内 `grep` 原文。

**门的执行面（本波现测，影响“门 PASS”可主张到什么程度）**：`scripts/check_doc_symbol_claims.py` 与其豁免登记
`scripts/doc_symbol_claim_baseline.txt` **均未入库**（`git ls-files scripts/` 无二者，`git status` 里基线是 `??`），
而 `.github/workflows/ci.yml:221-222` 只调 `check_doc_drift.sh`(+`--selftest`) ⇒
符号门是**本机独享**的门：它的 PASS/STALE 只能证明本地工作树自洽，**不随克隆与 CI 传播**。
入库且被 CI 调用的文档门 = `check_doc_drift.sh` 与 `check_doc_consistency.{sh,ps1}`；
`check_doc_consistency.sh` 现状 EXIT=1，`[GAP-B1]` 落在 `.claude/CLAUDE.md` 与 `.trae/rules/nuxus规则.md`
（即“计数改引用”与 B1“必须含当前数”的正反互斥，见 Q18/`agents.md` D8 待裁决项）。


## 17. 名字级普查三态分面与 Q14 债量化（P60 波次现测，配 `tmp/doc_name_sweep.py` 复现）

判据分面（对活文档反引号内 PascalCase 名，与 `crates/**/*.rs` 全量比对）：

- **A defined**：有 `pub (struct|enum|trait|type|fn|const|union|mod|static) 名` / `pub use …::名` /
  注册表 `名 =>` 行（宏展开产物）⇒ 声明成立。
- **B mentioned**：A 不成立但标识符在任意 .rs（含注释、测试）出现 ⇒ 多为外部生态类型（`LruCache` 类）或
  已退役 crate 史实词。
- **C absent**：全库 .rs 零出现 ⇒ 候选幽灵，须人判。

实测规模：`.rs` 文件 1256、去重符号 defined=5013 / mentioned=34725；
20 份活文档判定单位（文档×去重名）790 ⇒ **A=532 / B=158 / C=100**（C 去重名 89）。
交叉对照：CODE_WIKI 本探针 641 次命中 vs 符号门自报"表格行 482" —— 量纲不同（本探针扫全部非围栏行），
差值方向合理（更多），不等即先怀疑量纲而非缺陷。

**C 桶构成的量化结论（避免后波重复劳动）**：

- **77 个已在 `scripts/doc_symbol_claim_baseline.txt` 登记为 `Q14` 债**（即 CODE_WIKI 各 crate 卡片"关键类型"行
  里那批设计期命名，例：`LruCache`/`Prefetcher`/`Wal`/`CacheEntry`/`DebateChamber`/`VetoPower`）。
  本轮不重复改，偿还属独立批次（逐卡把设计名换成码内真名并删对应豁免行；符号门会用 STALE 自证进度）。
- 未登记 14 个里 **2 个是本波修掉的真幽灵**（命名模式表示例 `QuantumEntangledProtocol`→`QeepProtocol`
  `crates/qeep-protocol/src/protocol.rs:70`；`ACBGovernor`→`DecbGovernor`
  `crates/decb-governor/src/governor.rs:76`，旧名随 acb-governor 按 ADR-182 退役）。
- 其余 12 个**不是缺陷**：`CoreLoopEvent`/`InnerRing`（自带"设计名、代码零载体"注记）、
  `CHIMERA_MODEL__PRIMARY`（出现在"该示例无效"的否定句里）、`FpsGovernor`/`EchoOrchestratorConfig`
  （行内自述"未落地/重命名前旧名"）、`Dockerfile`/`LICENSE`/`ARCHITECTURE_HEALTH_AUDIT`/`CARGO_INCREMENTAL`/
  `E0428`/`FAST_FAIL_FATAL_APP_EXIT`（非 Rust 符号：文件名、env 变量、编译错误码、Windows 常量）。

**两次"先验后改"救回的假阳性**（若照 C 桶直接修就是制造破坏）：

1. `CHIMERA_MODEL_ROUTER__STRATEGY` 判 C 只因 env 名不进 .rs；实核 `ModelRouterConfig` 有 `strategy` 字段
   （`crates/nexus-core/src/config.rs:203` 起）⇒ 示例合法。
2. `ACBGovernor` 与 `AcbGovernor` 两种拼法都在 C 桶，前者是活文档命名表（该改），后者在 CODE_WIKI
   `:501-509` 的**已退役卡片**里（该留）。⇒ 同名异写要先看它所在的句子是否自带退役/否定语义。

⇒ **探针无 NEG 意识**：C 桶必须过一遍"这句话是不是在说它不存在"，才能进修复队列。


## 18. 方法名级核验配方与"两尺收敛"判据（P61 波次现测）

**为什么必须单独有一趟方法名核验**：符号门只判表格行里的 PascalCase 名（P59 已记其两盲区），
`Parliament::xxx()` 里的**方法段**与 snake_case 名一律不进判定 ⇒ CODE_WIKI parliament「关键方法」行
（`:495`）8 条声明里 **6 条全库不存在**（`submit_proposal`/`debate`/`vote`/`VetoPower::exercise`/
`consensus_quality`/`update_cap`），而门一路 PASS、锚点普查也一路无话（路径都真实存在）。

**配方（两行自证，可直接复用到任意卡片）**：

1. 抽名：`py -3` 对目标行跑 `` re.findall(r"`(?:[A-Za-z_]\w*::)?([A-Za-z_]\w*)\(", line) ``
   —— 必须用 Python，`tr '·' '\n'` 按**字节**切多字分隔符会抽空（伪"全部通过"）。
2. 验名：`grep -rn -E 'pub (async )?fn 名\(' --include='*.rs' crates/`（**必须递归**；单层 `src/*.rs` 会漏子目录，
   我一度据此把真存在的名字误列成幽灵）。
3. 归属：`awk -v L=行号 'NR<=L && /^impl /{o=$0} NR==L{print o}' 文件` 反查方法所属 `impl`，
   防止把 `Skeptic::exercise_veto()` 写成 `Parliament::exercise_veto()`（本轮实测 `exercise_veto` 在 `impl Skeptic`
   `crates/parliament/src/veto.rs:457`、`count_votes_with_threshold` 在 `impl VoteCounter` `voting.rs:134`）。

**修后真值（parliament 关键方法行；2026-09-24 B5 拆分后重取，原 `debate.rs` 三值作废）**：`Parliament::deliberate()` `debate/mod.rs:291`(async) ·
`deliberate_with_policy()` `debate/mod.rs:346` · `deliberate_with_override()` `debate/mod.rs:901` ·
`VoteCounter::count_votes_with_threshold()` `voting.rs:134` · `Skeptic::exercise_veto()` `veto.rs:457` ·
`QualityTrendAnalyzer::consensus_health_score()` `quality_trend.rs:158` ·
`StrategyCapGuard::set_max_strategy()`(配 `update_hysteresis_params()`)。

**两尺收敛判据（本波最重要的可复用结论）**：偿还后
`探针 C(CODE_WIKI)=68` 与 `符号门 registered=68`，**对称差 = ∅**。
在此之前二者差 1（`VetoPower` 只出现在**非表格行**，门看不见、探针看得见）⇒
"只按一个门的 STALE 反应驱动清理"必然漏掉它视野外的同类。
⇒ 每次由门驱动的清理，收尾都要用**另一把不同量纲的尺**复核对称差，收敛到 ∅ 才算该批完成。


## 19. 「关键方法」行普查结论与机械重写规则（P62 波次现测）

判据 = `tmp/doc_method_pair_check.py`（把 `Type::method()` 与代码里的 `impl Type { fn method }` 配对，
三态 PAIR_OK / METHOD_MISS / TYPE_MISS），比"方法名是否存在"强一层：**名字存在但挂在别的类型上同样是不符描述**。

现测：37 张卡片、124 条方法声明 ⇒ 修前 `PAIR_OK=64 / METHOD_MISS=39 / TYPE_MISS=21`（**48% 不符**）；
修后 **125/125 全 PAIR_OK**（含本轮补写的 1 条，计数含新增）。两类根因：

- **门面类型名整体不存在**（9 个 crate）：`RepoWiki`·`GsoeEngine`·`KvbsrRouter`·`PvlLayer`·
  `HierarchicalWindow`·`SsraFusionEngine`·`OmegaLearner`·`MerkleAuditChain`·`SparsityController`
  全库无同名 `pub struct/enum/trait`；真名分别是 `WikiStore`/`VectorIndex`、`GsoeEvolutionEngine`、
  `KVBlockSemanticRouter`、`Producer`/`Verifier`、`HcwWindow`、`SlimeFusionEngine`、`LinUCB`、
  （审计链无同名类型，故该行删除该条）、`SesaMask`/`SesaRouter`。
  ⇒ 与 P60/P61 的名字级 C 桶**交叉一致**（同一批名字两把尺各自独立命中）。
- **方法名虚构或挂错归属**：`EfficiencyMonitor::collect()`/`dashboard_snapshot()` 均无；
  真值是 `record_event()`（`crates/efficiency-monitor/src/monitor.rs:248`）与
  **自由函数** `render_metrics()`（`crates/efficiency-monitor/src/dashboard.rs:171`，不得写成 `类型::方法`）。

重写规则（机械、可复核、不猜；`tmp/p62_dryrun.py --apply`）：

- R1/R1b 改类型名：**只接受该 crate `lib.rs` `pub use` 导出的门面类型**为属主
  （否则 `MerkleAuditChain::record()` 会被"真但语义无关"的 `DecisionStats::record()` 顶掉）。
- R2 换方法名：**动词优先三档**（① 首 token 相同 ② 以声明首 token 开头 ③ 仅共享其他词根），档内取最短名。
- R2b（类型也要改时）只允许 ①② 两档，避免"类型+动词双错"叠加。
- R3 三档都不中 ⇒ 删条，不编。
- R4 补足到 3 条：取门面类型中"**该名字在本 crate 只属于这一个类型**"的非样板 pub 方法（按名排序）。
  ⚠ 明确**不用跨 crate 调用量**做证据：`.get()`/`.insert()`/`reset()` 这类名字的量被 `HashMap`/`Vec` 等着迷，
  量纲不可信（本波先按调用量补出一批 `HcwWindow::get()`(850)、`CmtCoordinator::get()`(794) 之类，已废弃该口径）。
- 不足 2 条时行尾写「其余公开方法见 `crates/<x>/src/lib.rs` 门面导出」；本波另手工补回 3 行主 API
  （`NmcEncoder::perceive()` `crates/nmc-encoder/src/fusion.rs:211`、
  `LsctPromoter::promote()`/`LsctDemoter::demote()` `crates/lsct-tiering/src/tiering/{promoter,demoter}.rs:54/53`、
  `HookExecutor::trigger()` `crates/nexus-hook/src/executor.rs:110`）。

---

## §20 单点事实基座：CODE_WIKI §5.5 事件表现势判定（2026-09-22 现取；后续勿重算）

**定级/载荷权威源** = `crates/event-bus/src/registry.rs`（`define_event_registry!`，每行
`Variant => Severity, Topic;`）+ `crates/event-bus/src/types.rs`（变体载荷）。
`types.rs:1158` 之类的旧锚已失效（定级不在该文件）。

| 文档断言 | 现势 | 证据 |
|---|---|---|
| `RouterStatsUpdated` | **不存在**；真名 `RouterStatsReported` | `registry.rs:227`（Normal, Routing）· `types.rs:1350` |
| `RouterStatsReported` 发布方＝L9 efficiency-monitor | **生产侧无发布方**；旧信念源自测试夹具的 `EventMetadata::new("efficiency-monitor")` 源串 | **4 处构造点全在测试态**：`chimera-tui/src/data/mod.rs:708`（`#[cfg(test)]` 起 `:72`）、`osa-coordinator/src/six_dimension.rs:383`（`mod tests` 起 `:335-336`）、`event-bus/tests/variant_count_test.rs:548`、`osa-coordinator/tests/six_dimension_test.rs:196,208`；生产消费方 3 处：`osa-coordinator/src/six_dimension.rs:133`、`chimera-tui/src/data/sync.rs:617`、`event-bus/src/membrane.rs:441` |
| `SecurityAuditCompleted`（L4→监控，audit_result/risk_score） | **从未存在**；意图由真实事件 `AuditLogged` 承担 | `seccore/src/audit.rs:423` 发布 · 载荷 `types.rs:462`（`audit_hash, severity`）· `registry.rs:151`（Normal, Security） |

**判据（可复用）**：文档里一个事件名要过三关才叫成立——
① `grep registry.rs` 有变体；② `grep types.rs` 载荷字段与文档所列一致；
③ 全库 `grep "NexusEvent::<Name>\s*{"` 有**非 `#[cfg(test)]`** 构造点（发布方），
否则只能写「消费面存在、生产侧无发布方」。
⇒ 且写「**唯一**构造点」这类极值断言前，必须把 `grep` 全部分命中列出并逐条分类测试态/生产态
（本批实测构造点 **4** 处，最初只看到 1 处即下"唯一"结论 ⇒ 已回改）。
计数门（146/10/41）对 ①②③ 全盲（F6）。

---

## §21 单点事实基座：`osa-coordinator/masks.rs` 与 `efficiency-monitor/lib.rs` 行号时效（2026-09-22 现取）

**两文件现值（`wc -l` 实测）**：`crates/osa-coordinator/src/masks.rs` = **17 行**、
`crates/efficiency-monitor/src/lib.rs` = **85 行**。

1. `masks.rs` 只剩**重导出垫片**（`masks.rs:17`）。类型实体已上提到 L0：
   `SparseMask<T>` = `crates/nexus-contracts/src/masks.rs:53`（`select_top_k` `:142`）；
   `OmniSparseMasks` = `crates/nexus-contracts/src/omni_masks.rs:46`（五维度字段
   `routing/context/memory/audit/budget` 在 `:48-56`）。
   `osa-coordinator` 侧另有 `coordinator.rs:54 pub use nexus_contracts::OmniSparseMasks;`。
   ⇒ 凡引 `masks.rs:47` / `:169-171` 的（`docs/audit/dimension_a:139`、`dimension_c:82,472`、
   `week1-8:230`）均为**收缩前点位**，按历史读。
2. `efficiency-monitor/src/lib.rs` 是模块门面（`pub mod` `:52-66`、`pub use` `:69+`），
   实现分散：`src/monitor.rs:248`、`src/auditor.rs:317`、`src/collectors.rs:78` 三处
   `record_event()`（不同聚合根）+ `src/dashboard.rs:171` 自由函数 `render_metrics()`。
   ⇒ 凡引 `lib.rs:71-89` / `:226` 的（`dimension_f:495`、`week1-8:224`）均为收缩前点位。

**复现**：`wc -l crates/osa-coordinator/src/masks.rs crates/efficiency-monitor/src/lib.rs`；
`grep -n "pub fn record_event\|pub fn render_metrics" crates/efficiency-monitor/src/*.rs`。


## §22 CODE_WIKI「主要依赖」行核验口径（波次 P84，2026-09-22）

**结论**：`docs/architecture/CODE_WIKI.md` §3 各 crate 的「主要依赖」行**只登记生产边**，且必须等于该 crate
`Cargo.toml` 的 `[dependencies]` 内部项；测试专用边一律写 `〔仅测试边〕` 后缀。

**可复跑判据（三条缺一不可）**：
1. 内部 crate 名全集 = `Cargo.toml` `workspace.members`（现 41）**∪ 已退役名**
   （由 `git ls-tree -d --name-only 38c72e6^ -- crates/` 得 42 目录，反差集出 `auto-dpo`/`model-router`，再并 `acb-governor`）。
   只认 members ⇒ 会把"依赖已删 crate"误当外部依赖而漏判（最严重的一类）。
2. manifest 解析用**权威 TOML 解析器**，并先剥 UTF-8 BOM
   （`crates/{chimera-tui,gqep-executor,mcp-mesh,nexus-app-server}/Cargo.toml` 带 BOM）。
3. 三态分桶后逐条人判；`A_RETIRE`/`C_NOWHERE` 必改，`B_DEVONLY` 改标注而非删除（信息保留、语义纠正）。

**2026-09-22 实测基线**：41 行中 12 行有错，共 `A_RETIRE=1`（chimera-mas×model-router）、
`C_NOWHERE=21`、`B_DEVONLY=3`；本波已全部收口，复跑应为 **0 / 0 /（0 未标注的）**。
其中 `session-store`/`nexus-subagent` 的幽灵 L1 边与根 `Cargo.toml` 内 ADR-160 注记
"虚假依赖声明已删"是同一事实的两端——**代码侧删边时文档侧不会自动跟**，故依赖行必须与 §89 探针同批维护。

**为何没有门**：`scripts/check_layer_map_parity.py` 只比 `check_dependency_rules.sh/.ps1/Cargo.toml` 三处计数，
不读 markdown ⇒ 依赖行的现势性当前纯靠本口径人工复跑（纳入文档侧需授权，见待裁决项 #11）。


## §23 文档侧层图与机器权威的对拍口径（波次 P85，2026-09-22）

**唯一机器权威**：`scripts/check_dependency_rules.sh` 的 `layer_of()`（41 条 `crate[|crate]*) echo N ;;`，
`.ps1` 为其镜像；**文档侧规范**：`CODE_WIKI.md §2.1` 层图块（11 行 = L0..L10，每行 `L(N) 名称 ─[─]* 名单`）。

**可复跑判据（四条）**：
1. 解析权威表后**断言恰 41 名**，少一个即说明脚本形状变了；
2. 每份文档的层图**命中行数必须 = 11**；不足 11 行先怀疑分隔符跑（本仓既有 `─` 双字符也有单字符写法），
   不要直接把"未命中"报成"文档缺项"；
3. 名单 token 先剥 `[...]`/`(...)`/`（...）` 注解再比对，否则 `[ADR-065]` 这类尾注会变成 GHOST；
4. 分桶 GHOST / RETIRED / WRONG_LAYER / MISSING / DUP 五态；RETIRED 名集必须来自
   `git ls-tree -d --name-only 38c72e6^ -- crates/` 与 members 的差集（不得手抄）。

**现势结论**：6 份权威/派生文档覆盖率均为 **41/41**；逐层归属分歧只剩 `csn-substitutor`
（文档+crate 自述头 L10 vs 门 L7），且该分歧是 `Phase0_评估报告_v2.20.md:109` 登记未裁决的规范冲突，
详见账本 §90.3 与 `CODE_WIKI §2.1` 层图下方注记。补记：`check_layer_map_parity.py` 只比**计数**（41=41=41=41），
比不出逐层归属，所以本波的对拍目前只能人工复跑——纳入 CI 需授权（待裁决项 #11）。

## §24 测试规模的两类量纲与静态口径（波次 P92，2026-09-22 现取）

**两类量纲不得混用**：

1. **动态 `passed`** —— 只能由实跑全量回归得到，权威源是 `docs/reports/` 最新回归报告；
2. **静态属性声明数** —— 可复现的代理指标，**不等于** `passed`（`#[ignore]` 项静态计入、动态计入 ignored 而非 passed；
   feature 门控模块静态一律计入）。

**已登记的动态时点值链**（全部为时点值，均不得当现势）：

- `11587 passed / 0 failed` —— 2026-09-02 全量重测，43 crates / 485 test target，
  出处 `docs/reports/redundancy-R9-disposition_2026-09-02.md`；
- `11794 passed / 0 failed` —— 2026-09-17 M13 提交态全量回归实测终值，回写见提交 `d3840ba`；
- 架构减法批次 `38c72e6` 删 `model-router` + `auto-dpo`（43→41 crates）**改动了测试面**
  ⇒ 上述两值均已过期，现势值**待重测**。

**静态口径两尺并列（2026-09-22，扫 `crates` + `tests` + `fuzz` 共 1304 个 `.rs`）**：

```
# strict 尺：属性独占一行
grep -rcE '^[[:space:]]*#\[test\][[:space:]]*$'          --include='*.rs' crates tests fuzz | awk -F: '{s+=$NF} END{print s}'   # 9296
grep -rcE '^[[:space:]]*#\[tokio::test\][[:space:]]*$'   --include='*.rs' crates tests fuzz | awk -F: '{s+=$NF} END{print s}'   # 1913
grep -rcE '^[[:space:]]*#\[tokio::test\([^\)]*\)\][[:space:]]*$' --include='*.rs' crates tests fuzz | awk -F: '{s+=$NF} END{print s}' # 23
#   合计 11232
```

- loose 尺（`grep -rhoE "#\[(tokio::)?test[^]]*\]|#\[rstest\]|#\[test_case" crates tests fuzz --include="*.rs" | wc -l`）= **11276**；
- 两尺差 **44**，来自同行多属性与 `test_log` 一类包装属性 ⇒ **引静态数必须先声明是哪一尺**，否则两次数不可比对。

**旧解释已被否证**（原 `.trae/rules/nuxus规则.md` 测试规模行：「静态计数 11433，差值为 doctest+宏展开」）：

- 可执行 doctest（文档注释内以 rust 为信息串的三反引号围栏）实测仅 **11** 处，另有 `no_run`/`ignore`/`text`/`toml` 等 94 处不计入；
- `rstest` 与 `test_case` 属性全库 **0** 处 ⇒ 不存在参数化宏展开；
- 被删两 crate 在 `38c72e6^` 同 strict 尺实测含 **327** 条测试属性：
  `11232 + 327 = 11559`，与 `11794` 仍差 **235**，而该差额**无法由 doctest(11) 或参数化宏(0) 解释**
  ⇒ 静态尺与动态 `passed` 本质不等价，且两次动态值分属不同工作树。

⇒ **规则**：文档里的测试规模行一律写成
**「动态数（现取日期 + 出处）+ 静态数（现取日期 + 尺子）+ 待重测」**，
禁止用"差值是 doctest/宏展开"这类未测解释圆场；解释不得替代重测。

**表格单元格禁令**：上面命令含 `|`，不得写进 md 表格单元格（反引号不转义竖线，会多切一列）；
命令只放正文/围栏块，表格行内写指向本节的指针。

## §25 函数长度红线的**执行面**与三族计数现测（波次 P93，2026-09-22）

**一、门在，但只守"工具牙口"，不守"仓库达标"**

- `scripts/audit_fnlen.py` 存在，跑全仓 **rc=0** ⇒ 它是**报告型**工具，退出码不编码判决；
- `.github/workflows/ci.yml:231-233` 是 WHY 注释，**真实执行体**在 `ci.yml:235-236`
  （`- name: Function-length audit scripts smoke test` / `run: bash scripts/test_fnlen_smoke.sh`）——
  该 smoke 脚本**植入**一个 >200 行 fixture 断言 `audit_fnlen.py` 与 `fn_scan.py` 都检出、且不误报干净函数
  （方向正确：守的是"工具有没有牙"）；
- 但**没有任何 CI 步骤断言"仓库内 0 个函数超 200 行"** ⇒ 红线"超过必须拆模块"目前无人判达标。
  ⇒ 登记为守护工具的文档行必须写清"报告型 / 是否判达标"，否则读者会把"有工具"读成"已守住"。

**二、实测超标 = 10 处（src 6 + tests 4），复现 `py -3 scripts/audit_fnlen.py`**

生产码 6 处（与红线直接冲突）：

- `crates/chimera-tui/src/i18n/en.rs:12-528`（517 行）`fn lookup`
- `crates/chimera-tui/src/i18n/zh.rs:281-791`（511 行）`fn lookup`
- `crates/chimera-tui/src/data/pipeline.rs:398-869`（472 行）`fn new_internal`
- `crates/chimera-cli/src/commands/tui.rs:147-446`（300 行）`fn execute_with_ctx`
- `crates/chimera-cli/src/quest_loop.rs:57-272`（216 行）`fn spawn_quest_lifecycle_bridge`
- `crates/chimera-tui/src/popup.rs:593-807`（215 行）`fn render`

测试态 4 处：`crates/event-bus/tests/variant_count_test.rs:55`（945）、
`crates/event-bus/tests/filtered_subscriber_test.rs:208`（481）、
`crates/mca-gateway/tests/token_efficiency_bench.rs:1333`（256）、
`crates/osa-coordinator/tests/e2e.rs:240`（214）。

**三、第二方法交叉对照**：本波自写的括号深度探针（先用合成夹具证明能检出 `todo!`/`allow(dead_code)`/
`spawn_blocking` 三种形状才允许出数）独立量出**生产码 6 处**，与门给出的 6 个 `file:line` **逐个相同**
⇒ 对称差为空，两法互证。

**四、另外三族计数的现测结果**

- **「零 `todo!()`/`unimplemented!()`」** —— **成立**（1304 个 `.rs` 全扫 0 命中；判据先在夹具上证明看得见该形状）。
  这是一条**已验证为真**的极值断言，后续不必重测，除非测试面结构变化。
- **error.rs** —— **37** = 41 members − 4 无独立文件者（`nexus-app-server` / `nexus-contracts` /
  `nexus-hook` / `router-traits`），**基数闭合**；37 份全部 thiserror 形状。旧值 39（架构减法前）已失效，
  `agents.md:402` 现文本已是正确的 37 + 指针式写法，本条仅为交叉确认。
- **「repo-wiki / scc-cache 79 处 `spawn_blocking`」** —— 已漂：现测 **89 行**（口径 = 含该词的行数）。
  复现：`grep -rc "spawn_blocking" --include="*.rs" crates/repo-wiki crates/scc-cache`（求和）。
  ⇒ 该数属时点值，文档行已改指针。
- **`#[allow(dead_code)]`**（账本旧实测 31 处/28 文件）—— 现测**出现 43 次 / 非注释行 40 / 文件 28**：
  文件数不变、处数增长，且"处"有两种量纲（含文档注释示例与否）。

**五、处置口径**：红线本身**不放宽**（不因有超标就改阈值）；所有登记该红线的文档行改为
**"报告型门 + 现测超标数 + 是否判达标"三件套**；是否把函数长度升级为阻塞门（并决定 6 处生产码的
豁免或拆分）**属治理裁决项 #21**，本审计不擅自决定。

## §26 不变量常量与编码位置对拍（波次 P94，2026-09-22 现取）

**目的**：指令面/权威源里"开发者会照抄"的常量与 `file:line` 编码位置，逐个对拍。判据 =
**只从 `pub const` 声明行与函数定义行取值**（见下方教训）。

**一、正向基线（已验证为真，后续不必重测）**

| 主张 | 现测 | 权威位置 |
| --- | --- | --- |
| `MAX_AGENT_DEPTH ≤ 5` | **= 5** | `crates/chimera-mas/src/orchestrator.rs:49`（`pub const MAX_AGENT_DEPTH: usize = 5`）；`crates/nexus-contracts/src/quota.rs:91` 镜像 `u32 = 5` |
| INV-7 预算 | `MEMORY_BUDGET_MB` **= 130**、`MEMORY_BUDGET_UTILIZATION` **= 0.9** ⇒ 阈值 117 MB | `invariants.rs:87` / `invariants.rs:95`；`nexus-contracts/src/quota.rs:85`+`:88` 第二处同值 |
| INV-8 层级序 | `Hot → Warm → Cold → Ice` 顺序成立 | `crates/chimera-mas/src/archive/*`（`archive/mod.rs:28` 自述 Hot 256 条 LRU） |
| `#![forbid(unsafe_code)]` | **41/41** 个 `src/lib.rs` 全含 | 但**无一在第 1 行**（前导是 `//!` 模块文档） |
| Critical mpsc | 常量 `CRITICAL_MPSC_VARIANTS = 14` == `is_critical_mpsc_event()` 列出 14 == `publish_critical()` 存在 | `event-bus/src/bus.rs`（`:48` / `:109` / `:854`） |
| `sqlite-vec` 禁用 | **成立**：仅存在于注释化的降级说明与注释掉的依赖行 | `crates/repo-wiki/Cargo.toml:57-62`（含 WHY：`0.1.9` binding 需 unsafe）；`Cargo.lock` 内 0 命中 |
| release 5 平台 | **恰 5 条** `x86_64-pc-windows-gnu` / linux gnu ×2 / apple-darwin ×2 | `.github/workflows/release.yml` |
| fuzz 8 target | `fuzz/Cargo.toml` `[[bin]]` = 8，与 `fuzz.yml:49-58` matrix 8 项一致，`-max_total_time=300` 在 `fuzz.yml:106` | 同左 |

**二、本波纠出并修正的 5 处编码位置错误（全在 `CODE_WIKI.md §13.4`，别处无）**

1. INV-7 方法名 `check_inv7_budget()` → 真名 **`check_inv7_context_budget()`**（`invariants.rs:275`），
   并补两条真实约束（单 Agent 驻留 ≤ `effective_capacity`；全局 `m_total ≤ m_budget × 0.9`）。
2. INV-7 动作列 "LRU 淘汰 Warm/Cold(保留 Hot)" → **检测型**：早退返回 `MasError::TokenBudgetExceeded`，
   不执行淘汰；层级淘汰属归档调度（`archive/mod.rs:28`）。
3. INV-8 动作列 `archive::upgrade_with_audit()` **全仓不存在** → 真实现是
   `archive/scheduler.rs:123` 的 `trigger` 内 `?` 早退返回 `MasError::ArchiveMonotonicityViolated`。
4. `MAX_AGENT_DEPTH` 位置 `chimera-mas/src/delegation.rs` → 该文件提及该常量 **0 次**；真位置见上表。
   动作列错误变体名（少一个 `Max` 前缀）→ 改为 enum 内真实名（实测 `MasError` 共 **40** 个变体）。
5. `forbid(unsafe_code)` 的 "第 1 行" → 改为 "41/41 的 `src/lib.rs` 顶部属性区，实测无一处在第 1 行"。

**三、`CODE_WIKI.md §13.5`「与三方权威源的一致性声明」整表是 v2.19.0 时点快照**（`2.19.0-omega` / 38 crates /
测试 8455 / 事件 129 / `chimera-mas` 24 内部依赖 / Critical 14），但**每行都打着 ✅** ⇒ 已加"仅对该时点成立"横幅，
表体数值作为历史记录保留不重算（政策：带日期证据只标注、不改写）。

**四、工具教训（两条，均已入用户级记忆）**

- **取常量值必须锚 `pub const` 声明行**：第一版判据用 `(NAME)[^=\n]*=\s*([\d.]+)` 全文扫，
  命中 `error.rs:223` 的**文档注释** `(130MB × 0.9 = 117MB)`，于是把正确的 130 报成 "MEMORY_BUDGET_MB = 117"。
  ⇒ 注释行里的等式是被解释的对象，不是定义。
- **`grep -rl` 的首个文件不是权威位置**：查 `fn publish_critical` 时首命中是
  `efficiency-monitor/src/monitor.rs`（那是 `publish_critical_alert`，另一回事），差点把 `bus.rs:854` 的
  正确主张"改正"成错的。⇒ 判"文档写的位置对不对"要用 `grep -rn "^\s*pub async fn 名"` 精确到定义行，
  并打印**全部**候选而不是第一个。

## §27 核心领域类型的归属真值表（波次 P95，2026-09-22 现取）

**背景**：指令面 §5.1 把 `UserIntent`/`Quest`/`Checkpoint` 的"权威源"写成 `nexus-core/src/types.rs`，
而该文件自己的模块文档（`crates/nexus-core/src/types.rs:1-10`）就写着它是「**纯 re-export 汇聚点**，
所有共享领域类型均下沉至 L0 `nexus-contracts`」。⇒ 权威源指针指向了 L1 的中转站，改字段的人会去 L1 找不存在定义。

**真值表（全部经字段级抽取器实测，行号 = `pub struct` 声明行）**

| 类型 | 归属（唯一 `pub struct` 定义点） | 实测字段（文档主张项全部命中） |
| --- | --- | --- |
| `UserIntent` | `crates/nexus-contracts/src/domain.rs:37` | intent_id · raw_text · multimodal_inputs · risk_level（另有 task_id/description/status/dependencies 等，共 15） |
| `MultimodalInput` | `crates/nexus-contracts/src/domain.rs:53` | **活变体只有 `Text(String)`**；`Image`/`Video`/`Audio` 在 `:57-59` 被注释 |
| `Quest` | `crates/nexus-contracts/src/domain.rs:103` | **quest_id**（不是 `id`）· title · tasks · thinking_mode · checkpoint_id · priority |
| `Checkpoint` | `crates/nexus-contracts/src/checkpoint.rs:36` | quest_id · checkpoint_id · memory_snapshot_hash · serialized_state · created_at · description |
| `OmniSparseMasks` | `crates/nexus-contracts/src/omni_masks.rs:46` | routing · context · memory · audit · budget（五维 ✓）；`osa-coordinator/src/coordinator.rs:54` 只是 `pub use` |
| `SemanticBlock` | `crates/kvbsr-router/src/types.rs:83` | 15 字段：block_id · block_vector · tools · block_coherence · temporal · clv · top_blocks · top_tools …（**无能力 id 字段**） |
| `CLV` | `crates/nexus-core/src/clv.rs:26`（newtype over `Array1<f32>`） | `pub const DIMENSION: usize = 512` 就在该文件内 |
| `cosine_similarity_slices` | 权威实现 `crates/nexus-contracts/src/util.rs` | 经 `crates/nexus-core/src/clv.rs` → `lib.rs:68` 再导出，`use nexus_core::cosine_similarity_slices` 调用面不变（`clv.rs:104` 自述委托 L0） |
| `NexusState` | `crates/nexus-core/src/state.rs:88` | ✓ L1 归属正确（唯一没错的一条） |

**`Checkpoint` 同名歧义**：`crates/gsoe-evolution/src/checkpoint_preserver.rs:32` 也有 `pub struct Checkpoint`
（字段 task_type/score/metadata/best_checkpoints），与 L0 那个**完全不同**。⇒ 裸名检索必须带 crate 前缀，
否则会把 GSOE 的检查点当成领域检查点（本仓 `types.rs` 有 37+ 个，同型歧义是常态）。

**多模态现状的正确说法（旧述把两件事混成一件）**：
- 感知器侧 **已实现**：`crates/nmc-encoder/src/fusion.rs:244-246` 按 `PerceptionInput::{Image,Video,Audio}` 分派；
- 领域侧 **待实现**：`MultimodalInput` 只有 `Text`，三变体注释掉 ⇒ 端到端"多模态意图"目前只有文本入口。

**工具教训（两条，本波各踩一次并自纠）**

1. **单目录层级 glob 会造出假缺失**：`Path("crates/nexus-core/src").glob("*.rs")` 不递归，
   于是判据看不见子目录里的定义，报出"5 处主张失败"（含正确的 `OmniSparseMasks`/`UserIntent`）。
   换 `rglob` 后同批主张全部成立 ⇒ 又是"整批红先怀疑仪器"。
2. **复制判据时丢 `re.M`**：为自证编辑结果，我把抽取器抄进第二个脚本，抄漏 `flags=re.M`，
   导致 `^\s{4}` 只在字符串首匹配 ⇒ 5/5 字段"缺失"的**假回归**。⇒ 自证脚本要**复用同一函数**
   （import 而非粘贴）；粘贴就必须拿一个已知为真的用例先跑一遍。

**复现口径**：`pub struct X\b[^{]*\{` 取花括号配平段，再 `^\s{4}(?:pub\s+)?(\w+)\s*:`（`re.M`）取字段；
变体用 `^\s{4}(\w+)\(` 取活变体、`^\s*//\s*(\w+)\(` 取注释待启用变体。

## §28 退役 crate 引用面 + README 可执行主张对拍（波次 P96，2026-09-22）

**一、`model-router`（已物理删除）在活文档里的引用面分三态**（复用"已删实体引用面三态分类"）

| 态 | 处数 | 位置与判定 |
| --- | --- | --- |
| 现势主张（把该 crate 当在盘实体索引，须改） | **0 行** | 逐行判定 20 行无一例外都是注记；`agents.md:142`、`.qoder/rules/chimera.md:173` 的 CAF 行早已改成 `mca-gateway` 并带"原 `model-router` 已物理删除，见 CODE_WIKI §3.12"注记 |
| 退役登记 / 删除注记内复述被删名（保留） | **20 行 / 23 次**，分布 `agents.md` 2、`.claude/CLAUDE.md` 2、`.trae/rules/nuxus规则.md` 2、`chimera.md` 3、`chimera2.md` 1、`CODE_WIKI.md` 10 | `CODE_WIKI.md:759` 退役登记表、`:763` 层图夹具曾借用生产名的说明、`:765` RK-P22 出处改标"待重定位"、`:766` 计划文档落点注记、`:1430` ADR-068 主题行（ADR 表按设计即历史）、`:9/:40/:718/:742/:1490` 减法批次"43 − auto-dpo/model-router"口径行；其余 5 文件的行是 CAF 行与 fuzz 阻断项注记 |
| 工件指向已失效（另案） | 1 | `fuzz/Cargo.toml:61` 仍 `path = "../crates/model-router"` ⇒ 属发布阻断项 **#18**，需改工件而非改文档 |

**二、`auto-dpo`（已物理删除）在数据流图里的收口状态**

- 已带退役注记：`agents.md:480`、`.qoder/rules/chimera.md:586`、`.qoder/rules/chimera2.md:213`；
- **缺注记**：`.claude/CLAUDE.md:240` 的流程图仍写 `→ Wiki 沉淀 / GSOE 进化 / Auto-DPO(L5)` 且无说明 ⇒
  本波补齐（措辞与前三处同源，不新造说法）。注记里"事件面未断"一句经实测成立：
  `DpoPairGenerated` 仍在 `crates/event-bus/src/registry.rs:149`（`=> Normal, Knowledge;`）与
  `crates/event-bus/src/types.rs:447`，故"不要再按 L5 crate 去找它，但事件契约仍有效"是准确的。

**三、`README.md` 的 8 条可执行主张逐条对拍 = 全部为真**（因此**不改写**，只登记为已验证，避免后侧重复劳动）

| 主张 | 证据 |
| --- | --- |
| `install.ps1 -SetupEnv` 一步注入 env + GNU channel | 脚本在盘，`SetupEnv` 相关分支 10 处 |
| `install.sh` 覆盖 Linux/macOS | 在盘（可执行位） |
| `cargo install chimera-cli` 不可用 | 结论正确但**机制此前写错**（2026-09-22 复核）：真因是本仓库从未发布到 crates.io；`publish = false` 只写在根 `[package]`（`Cargo.toml:11`，包名 `chimera-e2e-tests`），**未进 `[workspace.package]`、`chimera-cli` 自身也没有** ⇒ 它拦不住 `cargo publish -p chimera-cli`。详见 §54 与 `README.md:43` 更正 |
| 二进制名是 `chimera` 而非 `chimera-cli` | `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "chimera"` + `path = "src/main.rs"` |
| 子命令清单以代码为权威 | `crates/chimera-cli/src/cli.rs` 的 `Commands` enum（P89 已做源码×二进制对称差） |
| audit ignore 条数不登记、以 CI 为准 | `.github/workflows/audit.yml:65` |
| 逐包 fmt 聚合法替代 `cargo fmt --all` | `.claude/CLAUDE.md:322`（`os error 206` + `cargo fmt -p <pkg> -- --check`） |
| commit 需 HEREDOC + UTF-8 | `agents.md §10.3a` 存在 |

**四、新升格的治理冲突（不在文档面自行解决）**：`README.md:57` 自登记的"许可证口径不一致"经实测**成立**——
`LICENSE` 正文是 **Apache-2.0**（抬头 `Apache License / Version 2.0, January 2004`，全文 4 处 "Apache License"），
而 `Cargo.toml:85 workspace.package.license = "MIT"`，各 crate 经 `Cargo.toml:10 license.workspace = true` 继承。
⇒ 登记为待裁决项 **#22**（三个选项：改 `LICENSE` 为 MIT / 改 manifest 为 `Apache-2.0` / 双许可并同步两处），
文档不擅自择一，README 的立场句保持原样。

## §29 TUI 命令面与"W7~W11 能力"真值表（波次 P98，2026-09-22 现取）

**一、`SlashCommandRegistry` 的量纲（此前文档只写"53 命令"，未说构成）**

| 口径 | 数 | 取法 |
| --- | --- | --- |
| `BUILTIN_COMMANDS` 数组内 `name:` 字面量 | **53** | 只在 `const BUILTIN_COMMANDS` 到其 `];` 区间内取（数组外的 `name:` 是测试局部变量/形参，混进来会得 55） |
| 其中一级命令 | **41** | 名称不含空格 |
| 其中二级命令 | **12** | 形如 `quest list`/`agent spawn`（**带空格**，文档若按单词抽取会把它们当 12 个一级命令） |
| 别名 | **5** | `quit`,`q` / `?` / `find` / `settings` / `btw` |
| 自证 | `reg.len() == 53` | `crates/chimera-tui/src/actions/slash_registry.rs:654`（被检对象自己的断言 = 正对照） |

**二、`docs/tui/README.md`「Concord 重构 W7~W11 新增能力」逐条判定**（9 条里 **6 条与代码不符**）

| 主张 | 实测 | 证据 |
| --- | --- | --- |
| `/compact` ✅ | 已注册；ContextCurator 已编译 | `slash_registry.rs` 数组含 `compact`；`data/mod.rs:43 pub mod curator` |
| TuiHello 握手 ✅ | 已落地 | `app/event_loop.rs`、`data/snapshot.rs`、`data/sync.rs` |
| `/pace`(W8) | **未接线** | `data/pace_gate.rs` 在盘但全 crate 107 处 `mod` 声明无一提及 `pace_gate`；`PaceMode` 仅现于该文件 ⇒ 不参与编译；命令表无 `/pace` |
| `/context`(W9) | **未注册** | 41 条一级命令不含 `context`；策展入口只有 `/compact` |
| `/recap`、`/copy`(W11) | **无实现** | 全库 `recap`/`clipboard`/`copy_selection` 0 命中 |
| `/commands` + 用户自定义命令(W11/ADR-083) | **未接线** | `actions/user_commands.rs` 未出现在 `actions/mod.rs:18-25` 的 8 模块列表；`expand_template` 仅本文件 |
| `/notify` / NotifyBridge(W11) | **未接线** | `app/notify.rs` 未出现在 `app/mod.rs:35-44`；`NotifyBridge` 仅本文件；且它本就不该是 slash 命令 |
| AgentTree 面板(W10/ADR-082) | **无实现** | `AgentTree`/`agent_tree`/`DelegationTree` 全库 0 命中，`PanelId` 27 变体无此项 |

⇒ 手册里这 6 条已按实测改标 **未接线 / 无实现 / 待实现**，并各带一条 `path:line` 或 0 命中证据；
代码侧处置（接线 or 删档 + 加"孤儿源文件"门）= 待裁决项 **#24**。

**三、判据教训（两条）**

1. **`mod` 声明是编译闸门：文件在盘 ≠ 已编译 ≠ 测试跑过。** 这三个文件自带单测，但因未进模块树，
   `cargo test` 从不执行它们 ⇒ "有实现有测试"的叙述可以完全假。判"是否落地"的最小可靠组合 =
   ① 在**所有** `mod.rs`/`lib.rs` 里检索该文件名；② 检索该文件的公开类型是否出现在**别的**文件。
   （本波正对照：同一检索能命中 `newline_gate`、`curator` 的声明，证明判据不是空转。）
2. **整行替换优于片段替换**：改写 8 条 bullet 时用"行首唯一前缀 → 替换整行"，
   既避开长行截断留孤儿尾巴的老坑，也让"命中数 == 计划数"成为天然断言。
   另注：写"类型::方法"前必须回查 `impl` 归属（本波先误写 `App::toggle_companion_action`，
   实为 `impl TuiApp` @ `app/event_loop.rs:44`，随即改正）。

## §30 全仓孤儿源文件普查（波次 P99，2026-09-22 现取；判据可复跑）

**判据**（`tmp/p99_orphans.py`）：从每个 crate 的 `src/lib.rs`/`src/main.rs` 出发做 BFS，
按 `^\s*(pub(\(..\))?\s+)?(crate\s+)?mod\s+X\s*;` 解析到 `dir/X.rs` 与 `dir/X/mod.rs`（含 `#[path="…"]`），
收集"可达文件集"；`src/**/*.rs` 减去该集 = **孤儿（不参与编译）**。
**自检（正/负对照）**：`chimera-tui` 的 `pace_gate`/`user_commands`/`notify` 必须在孤儿集，
`curator`/`newline_gate`/`types` 必须不在 ⇒ 抓全 3、零误报后才出数。

**结果：9 个孤儿 / 768 个 `src/*.rs`（41 crate），共 2,864 行、内含 63 个 `#[test]` 从不执行**

| 孤儿文件 | 行数 | 内联 `#[test]` | 文档是否主张其存在 |
| --- | --- | --- | --- |
| `crates/chimera-tui/src/data/pace_gate.rs` | 575 | 16 | 是（ADR-080、TUI 手册 `/pace`）⇒ 已在 P98/本波校正 |
| `crates/parliament/src/event_pipeline.rs` | 341 | **0** | 否（`EventPipeline` 全库仅该文件） |
| `crates/quest-engine/src/wal.rs` | 466 | 10 | 否（`CheckpointWal` 仅该文件；WAL 文档指的都是 `scc-cache/src/wal.rs`，那是**已接线**的同名文件） |
| `crates/parliament/src/scheduler.rs` | 165 | 5 | 否（`WorkStealingScheduler` 仅该文件；文档提到的 scheduler 是 `mas-sched`/`chimera-mas/archive` 的已接线同名文件） |
| `crates/chimera-tui/src/actions/user_commands.rs` | 275 | 9 | 是（ADR-083、手册 `/commands`）⇒ 已校正 |
| `crates/parliament/src/delegation.rs` | 264 | 8 | 否（`DelegationOverheadPredictor` 仅该文件） |
| `crates/chimera-tui/src/app/notify.rs` | 183 | 7 | 是（ADR-083、手册 `/notify`）⇒ 已校正 |
| `crates/chimera-tui/src/components/prefix_hint.rs` | 136 | 4 | 是（ADR-083 §决策 8）⇒ 已校正 |
| `crates/chimera-tui/src/data/sanitize.rs` | 59 | 4 | 否（`sanitize_control_sequences` 仅现于 `data/sanitize.rs` 与**同为孤儿的** `app/notify.rs`——两者互相引用但对 crate 整体是死岛） |

**判定要点：同名文件要先分 crate 再下结论**
`scheduler.rs` 在 5 份文档里被引用、`delegation.rs` 3 次、`wal.rs` 1 次——逐条核对后**全部指向别的 crate 的已接线同名文件**
（`mas-sched`、`chimera-mas/archive`、`scc-cache`），不是对这 4 个孤儿的主张 ⇒ 本波**不因此改任何文档**。
（同型风险：`types.rs` 全仓 37+ 个、`Cargo.toml` 46 个，裸名匹配必然张冠李戴。）

**ADR 侧已落的校正（决策文本不改，只更正"已交付"叙述）**

- `ADR-080`：+6 行块，写明 `pace_gate.rs` 未被 `mod` 声明 ⇒ 不参与编译、无 `/pace`，现状"未接线/待实现"。
- `ADR-082`：+5 行块，写明 **TuiHello ✅ 落地**、**AgentTree 0 命中** ⇒ 该波次实为半交付。
- `ADR-083`：+7 行块，写明四命令均不在内建表（实测 53 = 41 一级 + 12 二级，自证 `slash_registry.rs:654`）、
  三个支撑文件均未接线，并登记其**内部矛盾**：正文"词表 55→59"vs 背景节"v1.0 词表 53 条"。

**建议的门（当前不存在，故此类缺陷可长期潜伏）**：把"未被任何 `mod` 声明引用的 `crates/**/src/**/*.rs`"
做成阻塞项（白名单放行 `build.rs`/`tests`/`benches` 等 cargo 自动根）。
在文档面，这等价于一条通则：**判"某能力是否已实现"，`mod` 声明 + 类型的外部引用是两个必要检查，缺一不可**。

## 31. 文档间（md→md）引用可解析性事实基座（波次 P100，2026-09-22 实测）

> **定位**：此前所有门禁只校验「文档→代码」锚点，**没有任何门校验「文档→文档」**。本节建立该面的单一事实基座，后续波次直接复用，不再重算。
> **仪器**：`tmp/p100_mdrefs.py`（解析器）+ `tmp/p100_ghosts.py`（去重分类）。植入式自测 5/5 PASS：已知存在→OK、已知搬移的中文名设计文档→MOVED（证 CJK 抽取器有效）、编造名→GHOST、根文件从子目录相对解析→OK。

### 31.1 语料口径（必须连口径一起引用）

| 量 | 值 | 口径 |
|----|-----|------|
| 全仓 md | **993** | 排除 `target*`/`.git`/`.toolchain`/`tmp*`/`node_modules`/`.cargo` |
| 本面扫描对象（活文档+指令面） | **231** | 再排除 `_archive/`、`_blueprints/`、镜像件（`.tower`/`.trae/specs`/`.zcode`/`.superpowers`/`.workbuddy`）、`docs/{reports,audit,research,optimization}`、`fuzz/` |
| 抽取手段（三路并集） | 反引号内联码 / Markdown 链接 / 裸路径正则 | 剔除含 `http`/`*`/`<`/`{` 与以 `-` 开头的片段型噪声 |

### 31.2 修正后分类计数（P100 落盘后复测）

| 类 | 站点数 | 唯一名 | 判读 |
|----|-------:|-------:|------|
| **MOVED**（首列裸名，实件在别处） | 449（修前 477，Δ28=本轮修正数） | 242 | **缺陷**：表格首列裸文件名=「在仓库根」的隐含断言 |
| **AMBIG**（同名多处） | 267 | 37 | 需人工择一，不得机械改 |
| **GHOST**（全仓无此名） | 242 | 88 | 见 31.3 六分类，**多数不是缺陷** |

### 31.3 GHOST 六分类（判定规则，供后续波次直接套用）

| 类 | 代表名（站点数） | 判定 | 依据 |
|----|----------------|------|------|
| **G1 仓库外记忆件** | `project_memory.md`(37)、`topics.md`(2) | **非缺陷（仪器盲区）** | 实测在 `c:\Users\30324\.trae-cn\memory\projects\-d-Chimera-CLI\project_memory.md`；解析器只索引仓内 |
| **G2 大小写变体** | `AGENTS.md`(58)、`Chimera2.md`(3)、`GHANGELOG.md`(1, ADR-085 笔误) | 活文档内宜小写归一；Linux/CI 大小写敏感 | 真件 `agents.md`/`chimera2.md`/`CHANGELOG.md` |
| **G3 已声明缺失** | `AETHER_NEXUS_OMEGA_ULTIMATE.md`(24+9+3) | **非缺陷** | 各表已写「❌ 不存在(已被取代,无需恢复)」 |
| **G4 示意/模板占位** | `file.md`、`snake_case.md`、`UPPER_SNAKE_CASE.md`、`_report.md`、`ADR-0xx.md`、`ARCHITECTURE.md`/`API_SPEC.md`/`SECURITY.md` | **非缺陷（但需读上下文）** | 前四类是 `docs/CONVENTIONS.md` 命名规则示例；后三类在被取代手册的目录树示意内 |
| **G5 真断链（本轮已修/已注）** | `tui-four-dimension-eval-2026-09-09.md`(1, CHANGELOG:33)、`Chimera_HCW-Sparse…`(1, 在 §32 受损带内)、`arch-batch2-new-directions-2026-09-03.md`(1, ADR-180:9)、`ADR-072-overwindow-tui-link.md`(2, v2.25 诊断报告：ADR-072 是 token-efficiency，overwindow 属 ADR-073)、`tmp/fnlen-scan.md`(2, 指向已消失的 gitignored 草稿) | **缺陷** | 前两条已就地改为实测存在名；ADR-180 加实测注；后三条属带日期快照件→按政策仅登记不改写 |
| **G6 历史 plan 未落地产物** | `.claude/plans/v1.5.7-omega-release.md` 内 `docs/release/v1.5.7-omega_*`（9 站点） | **非活文档**，不修 | 该目录为旧版本发布 plan 归档 |

### 31.4 4 份前瞻设计文档的真实落点（本轮最重要更正）

| 文档 | 规则面长期写法 | 实测 |
|------|--------------|------|
| `CHIMERA_NEXUS_OMEGA_完整对话摘要.md` | 根目录 ✅ | **仍在根**（48,888 B） |
| `CHIMERA_MULTI_AGENT_四象限协同工作系统_系统性设计文档.md` | 根目录 ✅ 存在 | `docs/architecture/_blueprints/mas-quadrant/`（82,863 B，2026-08-30） |
| `CHIMERA_全球大模型亲和系统_终极设计文档.md` | 根目录 ✅ 存在 | `docs/architecture/_blueprints/caf-router/`（24,221 B） |
| `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` | 根目录 ✅ 存在 | `docs/architecture/_blueprints/triple-paradox/`（33,828 B） |
| `三环循环_十层接口_元架构重组深度分析.md` | 根目录 ✅ 存在 | `docs/architecture/_blueprints/three-ring-reorg/`（53,128 B，2026-09-21） |

> **旁证（三份，互不依赖）**：① `DOCUMENT_LIFECYCLE_POLICY.md:108` 已登记「已移入 `_blueprints/` 4 项 + 元文档确在根」；② `doc-systematic-sync-2026-08-30.md:130-133` 记录了对这四份文件的横幅更新（即 08-30 时点它们已在 `_blueprints/`）；③ `doc-roles-matrix.md:169-174` 全程使用完整 `_blueprints/` 路径。⇒ **失效方只有指令面 4 份文件的 6 张表**，本轮已改。
> **注意区分**：`_blueprints/mas-quadrant/` 下**另有** `CHIMERA_MULTI_AGENT_协同工作系统_终极设计文档.md`（62,607 B，07-30）——同名近似但不同文档，勿混为一份。

### 31.5 门禁缺口（本轮新增裁决项 #25/#26 的依据）

- `scripts/t2_doc_paths_verify.py` **不是**全仓 md 链接门：它只读 `docs/reports/phaseR-t2-commit-batches.md` 一份文档，且其 TOK 正则 `[A-Za-z0-9_.\-/]+\.(?:…|md)` **字符类不含 CJK** ⇒ 对 4 份中文命名设计文档天然失明（该文件的注释自己记过「漏了 md 会得假 0」的同类教训，但漏的是 CJK 维度）。
- 结论：**「文档指向不存在文件」目前无任何自动检出能力**，且现有校验器结构上无法检出中文名件。

## 32. `CHANGELOG.md` 编码受损带（波次 P100，2026-09-22 实测；全仓唯一受损文件）

### 32.1 判据（原则式，非手抄产物字）

> 字符 `c` 满足 `c.encode('gbk')` 为 2 字节且 `b0∈[E0,EF] ∧ b1∈[80,BF]` ⇒ 它是「某个 UTF-8 三字节序列被按 GBK 解码后剩下的前二字节」；再要求**紧随 ASCII `?`**（被吞掉的第 3 字节无法成 GBK 首字节 ⇒ 落 `?`）。

- **正控（暴露仪器缺陷的两轮）**：首版手抄产物字符集只检出 72 行/124 处，漏 `鍖`(化)/`妗`(案)/`銆`(动) 等 ⇒ 计数系统性偏低。改原则式判据后，`README.md`/`agents.md`/`CODE_WIKI.md`/`docs/CONVENTIONS.md`/`adr_index.md`/`docs/tui/README.md` 命中 **0**，而 `悖`(悖论)/`浏`(浏览器)/`飙`(飙高)/`瑕`(瑕疵)/`姊`(姊妹) 5 个合法罕用字被暴露并从判据排除。
- **终判据全仓复测（993 份 md）**：**只有 `CHANGELOG.md` 命中**，共 **444 处**（其中 3 处是本轮横幅内作为示例引用的产物字 ⇒ 受损真值 **441 处 / 154 行**）。

### 32.2 受损范围与形态

| 项 | 实测 |
|----|------|
| 区间 | 登记前原行号 **888–1270**（插入 3 行横幅后 891–1273），覆盖 `[2.25.0-omega]` 至 `[2.11.0-omega]` **全部章节** |
| 区间外 | 行 1–887（v2.26.0 及更新批次）与尾部 "Earlier Versions" 干净 |
| 规模 | 154 行 / 441 处 / 104 个不同二字节前缀；top：`锛?`(98, 候选=U+FF00–U+FF3F 全角区 64 字，`，）：；！？`皆在内)、`銆?`(47)、`涓?`(30, 候选=U+4E00–U+4E3F 块 64 字：一/三/上/下/不…) |
| 附加形态 | 换行亦丢失：原行 891 内出现 `…）銆?> 版本裁…` ⇒ 两个引用行被并入一行（非纯字符级替换，结构亦受损） |
| 写入完整性自证 | 横幅落盘后 `CHANGELOG.md`：**严格 UTF-8 解码通过 / U+FFFD=0 / BOM=0 / 1,277 行全 CRLF、裸 LF=0**；产物字符计数 444 = 受损真值 441 + 横幅内示例 3 ⇒ 本轮写入未新增任何损坏 |

### 32.3 不可逆性论证（为何不自动修）

- 每处丢失的是 1 个 UTF-8 尾字节（`0x80–0xBF`，64 值）。实测每个二字节前缀的**已编码候选数：min 0 / 中位 64 / max 64** ⇒ 单处最多 64 个合法原字，算法无法定夺（例：`EF BC` 前缀对应 U+FF00–U+FF3F 全角区，`，）：；！？` 皆在其中）。
- **无干净副本可比对**：`git ls-files` 现况不含 CHANGELOG.md；`bb471f9`（2026-07-26「remove tracked .md files per .gitignore policy」）起停止跟踪，其父 blob 仅 **42,891 B**（本文件现 155,351 B）且**不含**该区间（`git show bb471f9^:CHANGELOG.md | grep "B-1: RL 共享类型补齐"` → No matches）。全仓亦无第二份项目 CHANGELOG（仅 `.toolchain` 内三方 crate 的同名文件）。
- **字节口径自纠**：补丁脚本自报「+2,708 字节」是**错的**——它拿 CRLF 复原后的文本与 LF 归一化文本相减，把全文 1,277 个 CR 重新计了一遍。实测净增 **1,431 字节 / 3 行**（153,917 → 155,351）。⇒ 复用该脚本时必须记住：**其字节差只在纯 LF 文件上可信**（ADR-180 那份自报 +156 B，纯 LF，可信）。
- ⇒ 本轮只加**受损带横幅**（禁照抄 + 改读 ADR/收口报告的指针），不猜字。修复路径两条，属治理裁决：**人工逐处依上下文重建**（441 处）或**以对应 ADR/`docs/reports/` 收口报告反向重写该区间**。

## 33. CLI/API 面与文档覆盖度矩阵事实基座（波次 P101，2026-09-22 实测）

### 33.1 CLI 权威集（两源互证，可直接引用的口径）

| 平面 | 权威源 | 实测 |
|------|-------|------|
| 一级子命令 | `crates/chimera-cli/src/cli.rs:154` `pub enum Commands`（18 变体）；`cli.rs` 内 `alias` 属性 **0** 处 | 第二源＝当日构建 `target/debug/chimera.exe --help` 的 `Commands:` 段，**18 项、顺序逐项一致** ⇒ 两源一致 |
| 二级子命令 | 同文件 `QuestAction:498`、`ConfigAction:520`、`McpAction:533`、`AgentAction:564`、`LlmAction:592` | `quest`(cancel/checkpoint/list/show) · `config`(init/list/path/show) · `mcp`(call/inspect/list/serve) · `agent`(cancel/inspect/list/spawn) · `llm`(channels/list/set-default/show/strategy/test) ＝ **22 项**，父命令由 `Commands` 变体块内 `字段: XxxAction` 绑定推出 |
| 全局 flag | `cli.rs` `#[arg(long, global=true)]` | `--config <FILE>` `--verbose/-v` `--json` `--yes` `--no-permission` `--no-color` `--no-banner` `--dry-run` `--version/-V` `--help/-h` |
| **平面区分** | TUI slash 面另有 12 个双词命令（含 `quest pause/resume/priority/vote`） | ⇒ 这 4 个是 **TUI-only**，写成 `chimera quest pause` 即跨平面污染，检查器必须区分 |

### 33.2 文档 CLI 面普查结果（仪器 `tmp/p101_cli2.py` / `p101_cli3.py`）

> 落地形式三型必须都覆盖：`chimera <cmd>`、`chimera-cli -- <cmd>`、`--config X <cmd>`。
> **首版只匹配 F1 便报出"0 违规"假干净**——植入正控（`generate` 应被抓）失败才暴露盲区。

| 项 | 结果 |
|----|------|
| 一级非法命令 | 993 份 md 中 **2 站点**：① `.qoder/rules/chimera2.md:74` `chimera-cli -- generate`（**真缺陷**，`cli.rs` 内 `generate` 零命中）→ 已按 `.claude/CLAUDE.md:76` 既有措辞就地改为 `run "…"` 并保留"无 generate 子命令"注；② `docs/release/week8_release_guide.md:234` `docker run -d --name chimera chimera:1.0.0-omega`（**假阳性**，`--name` 值被当命令） |
| 二级非法命令 | **0 站点**，且**正向覆盖非空**：活文档内合法二级调用 **18 处 / 9 种**（`config init`×9、`llm list`×2、`llm show/set-default/test/channels/strategy`、`agent spawn`、`config show` 各 1）⇒ 检查器确实被语料喂到，"0 违规"可读作真干净 |
| 植入自测 | 6 例全 PASS（2 个必抓的非法 + 3 个必放的合法 + 1 个"无二级面的命令不得报"）；其中 `wiki "查询词"` 一例最初被我误设成"必抓"，**自测失败暴露的是我的期望错**（`wiki` 收位置参数，无 `wikiAction`），修正期望后通过 |
| 反向覆盖（哪些命令文档写得薄） | `completions` 仅 **1** 份活文档提及；`serve`/`acp` 各 **5**、`exec` **8**、`doctor`/`llm` 各 13（口径：201 份活文档中出现过该命令名）。属**薄覆盖**而非错误，登记不硬改 |

### 33.3 `CODE_WIKI.md` chimera-cli 子命令行曾缺 6 项（已补）

- 原行（`:699`，`| **子命令** | …`，竖线数 3）只列 **12** 个一级命令：`run/tui/chat/quest/config/wiki/parliament/mcp/audit/agent/doctor/completions`，**缺 `grep` `llm` `exec` `serve` `acp` `help`** 六项；其括号内二级集与 33.1 实测一致（旧行本身二级无误）。
- 已补齐为 18 项（同一物理行、竖线数不变、附 `cli.rs:154` 指针与"旧行缺六项"注）。改后复跑：`CODE_WIKI.md` 仍 **1,745 行 / 全 CRLF / 裸 LF=0 / fence 平衡**，符号门 occurrences 435→**436**、gaps 仍 **7/7 registered PASS**，drift 与 consistency 门均绿。

### 33.4 代码侧文案滞后（文档面发现，改码需授权）

`cli.rs:59` 的 `--help` 顶部介绍仍写 **"基于 OMEGA 九定律(Ω₁…Ω₉)"**，而治理权威源已是**十一定律**（Ω₁₀-Card / Ω₁₁-Synthesize，`agents.md` §0 与 ADR-170；`CODE_WIKI.md §1.2`）。⇒ 用户手册/TUI 里用户实际看到的帮助文本落后于定律体系一个版本。本轮**未改码**（`crates/` 属仓库工件，需授权）；同文件 `:264` 的 `chimera wiki --limit 20 "OMEGA 九定律"` 是检索词示例，不算错，勿一并"修"。

### 33.5 文档覆盖度矩阵（把"逐一对比每份 MD"变成可核查存量）

| 口径 | 数 |
|------|----|
| 全仓 md（排除 target/.git/.toolchain/tmp*） | **993** |
| 审计口径内（再排除镜像/工具面 `_archive`/`_blueprints`/`.tower`/`.trae`/`.zcode`/`.superpowers`/`.workbuddy`/`fuzz`） | **437** |
| 已被账本/事实文件点名（basename 或 ≥10 字干命中） | **130** |
| 未点名 | **307**，合计 4,879,632 B |

未点名分桶（份数 / 字节）：**带日期证据 200 / 3,014,411**（政策：只加注不改写）、**ADR 与索引 83 / 910,536**（决策时点记录）、**根目录设计与报告 7 / 674,478**、**架构权威面 5 / 177,493**、**交付/运维流程面 7 / 87,421**、其他 docs 5 / 15,293。

⇒ 其中"开发指导性且 ≥8KB"的未点名候选只有 **14 份**，再按已知处置归类：**3 份是 #23 裁决对象**（v3.4.0/v4.0/九源 三份 superseded 手册）、**3 份是 `OMEGA_大模型架构魔改…` / `AETHER_NEXUS_GEN3_OMEGA` / `CHIMERA_NEXUS_GEN2_INNOVATIONS`（政策 §4.3 明列"已带已被取代横幅、未物理归档"）**、**5 份是带日期的验收/研究件**（`docs/security/week8_security_report`、`docs/acceptance/week8_limitations_*` ×2、`docs/mca/experience_parity_acceptance_report`、根目录 `DEEP_RESEARCH_NEXUS_OMEGA_architecture_review`）、余 3 份为根目录评估/诊断报告（`CHIMERA_TUI_全维度…`、`CHIMERA_全库级深度现状诊断报告_v2.25.0-omega`、`Chimera_全模型亲和适配体系设计文档_v1.0`）。
⇒ **结论**：活文档中"从未被看过"的开发指导性文档实际为 0；剩余未处置项全部落在"政策保护"或"待用户裁决(#23/#7)"两类里。此矩阵即本轮完整性交付物。

## 34. CLI 选项（flag）面事实基座与普查口径（波次 P102，2026-09-22 实测）

### 34.1 权威面（三层，两源互证：源码声明 + 当日构建二进制实跑）

| 层 | 内容 | 证据 |
|----|------|------|
| 全局透传 7 | `--config <FILE>` `--json` `--yes` `--no-permission` `--no-color` `--no-banner` `--dry-run` | `target/debug/chimera.exe --help` Options 段 + `cli.rs` `#[arg(long, global=true)]` |
| 源码声明的局部 flag（17） | 上列 7 项 + `--limit`(wiki) `--protocol` `--no-v3-engine` `--fix` `--parallel` `--quadrant` `--severity` `--task` `--legacy-exit-code` `--verbose` | 抽取器：显式 `long = "x"` 命中 5 项 + **裸 `#[arg(long)]` 由字段名 kebab 推导**补 12 项（首版只取显式式，`--limit` 正控当场失败） |
| `tui` 子命令局部 2 | `--no-v3-engine`、`--protocol` | `cli.rs:213-220` + `chimera tui --help` 实跑输出逐项一致 |

### 34.2 二进制实跑的三条硬结论（比正则推理更强）

| 命令 | 实测输出 |
|------|---------|
| `chimera tui --version` | `error: unexpected argument '--version' found`（并提示"相似项 --verbose"）⇒ **`tui` 层不接受 `--version`** |
| `chimera tui --bench-frame-time` | `error: unexpected argument` ⇒ 该 flag **从未存在**（全 `crates/` 无 `bench_frame_time`） |
| `chimera tui --lazy-config` | `error: unexpected argument` ⇒ 未实现（`lazy_config` 仅出现在测试文件名 `crates/chimera-cli/tests/config_lazy.rs`） |

### 34.3 普查口径（三级作用域归因，缺一条就会假干净或假报错）

1. **crate/包名不是调用点**：`\bchimera\b` 会误配 `chimera-tui`/`chimera-cli`/镜像名 `chimera-cli:rc` ⇒ 谓词须 `(?![\w-])` 且后随空白。
2. **cargo 行只认 ` -- ` 之后、且首 token 是合法子命令**：否则 `cargo fmt -p chimera-cli -- --check` 的 `--check`、`cargo clippy -p chimera-cli --all-targets --jobs 2` 的 `--all-targets/--jobs`、criterion 的 `--noplot/--save-baseline` 全会被误判成 chimera 幽灵 flag。
3. **`wiki` 一类无二级 enum 的命令收位置参数**，不得按"二级命令必须合法"去判（本波一条自测期望因此被判错并修正）。

### 34.4 普查结果（`tmp/p102_flags.py`，11 项植入自测全 PASS）

- 语料：活文档 201 份、其中 chimera 调用行 **116** 行。
- 可疑 flag：**6 站点 / 6 唯一** ⇒ **真缺陷 2**（`--bench-frame-time`@`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md:912`、`--lazy-config`@`tui-suite-tech-stack.md:216`），**已知误报 4**（`features`@`agents.md:359` 散文里挂到 `--features mca`；`mode`@九源手册:1247；`grep`@`CHIMERA_V3_专项优化方案:232`；`sysinfo-refresh-interval-`@`CHANGELOG_v1_8_0_omega_detailed.md:97`——该名对应的是 **TUI 配置键**（`crates/chimera-tui/src/config/mod.rs` 等）而非 CLI flag）。
- 处置：两处真缺陷**均为带横幅的时点/被取代文档 ⇒ 只加"就地校正"注，不重写正文**（见 §34.5）。

### 34.5 本波落盘的两处校正

1. `NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §7.4 代码块后 **+2 行注**：第 4、5 条命令不可执行（实跑报错）；第 1-2 条 criterion 基准**在盘可跑**（`crates/chimera-tui/benches/render_bench.rs`、`data_pipeline_bench.rs` 实测存在）；帧时间类测量由 `v3_frame_clone_cost_bench.rs` / `tui_update_cost_bench.rs` 承担，**CLI 侧无 P95 帧时间开关（待实现）**。fence 62→62 不变。
2. `tui-suite-tech-stack.md:216` 表格单元格内补「⚠ 2026-09-22 实跑：未实现」注（单物理行、**逐行竖线数全等**、fence 10→10、行数不变）。

## 35. cargo feature 面事实基座（波次 P103，2026-09-22 实测；含免编译校验法）

### 35.1 权威集（三源：正则 + tomllib 结构解析 + `cargo tree` 实跑）

全仓 **43** 份 manifest（根 + `fuzz/` + 41 crate）中**仅 7 份**声明 `[features]`，共 **14 条声明 / 10 个唯一名**：

| 包 | 特征 |
|----|------|
| `chimera-cli` | `mca` · `rl-client` · `r2_unfreeze` · `r2_island_repayment` |
| `chimera-e2e-tests`（根包） | `mca` |
| `chimera-tui` | `default` · `v3-engine` |
| `gsoe-evolution` | `default` · `r2_path` |
| `hcw-window` | `default` · `hcw-recall` |
| `pvl-layer` | `dynamic-blacklist` |
| `seccore` | `default` · `wasm-sandbox` |

> **规模闸门自纠**：首版把"特征总数应 >20"写进脚本，实测只有 10 ⇒ **闸门报红是我的假设错，不是数据缺**。
> 该假定的来源（"多 crate 仓库必有多特征"）被 43 份 manifest 的实际分布推翻（36 份根本无 features 段）。
> 保留的正确闸门是**两源集合相等**（正则 vs tomllib），它才是有效性证据。

### 35.2 免编译校验法（比读 manifest 更强，且秒级）

```
cargo tree -p chimera-cli --features mca -e features --depth 0     → rc=0
cargo tree -p chimera-cli --features frobnicate-xyz --depth 0      → rc=101
   error: the package 'chimera-cli' does not contain this feature: frobnicate-xyz
```
⇒ 判定"某文档命令里的 `--features X` 对 `-p <pkg>` 是否成立"，**无需编译**（不占 D 盘，当前 D 盘仅剩 4.0 GB / 99% 满）。
⚠ 该法同样能揭穿"名字对但包不对"：feature 是**按包**声明的，`--features wasm-sandbox` 只有配 `seccore` 才成立。

### 35.3 三级作用域判定与文档普查结果

判定分级：`PHANTOM`（名字不在 10 内）/ `SCOPE`（`-p` 选中的包未声明）/ `SCOPE?`（行内无 `-p`，需人工看默认成员）。

- 活文档 201 份 → **非 OK 站点 7**，其中 **PHANTOM 0**（正则修正后归零：首版把
  `--features rl-client --test rl_client_test` 里的 `--test`、`rl_client_test` 误当特征名，
  因捕获字符类含空格/连字符 ⇒ 越界吞并；加"截到下一个 `--` 前"边界后消除）。
- `SCOPE 1`：`CHIMERA_架构设计文档_v3.0_2026-09-21.md:704` 一格内含多命令（`cargo test -p <x>` 占位 + `--features r2_unfreeze`）
  ⇒ **仪器按行分析的固有局限**，非文档缺陷。
- `SCOPE? 6`：4 处为散文提及（`rl-client-protocol.md:86`、`ADR-034:158`、`ADR-167:18`、`r2_unfreeze_implementation_report.md:94`
  ——末者正文已自陈"原写法 `cargo build --features r2_unfreeze` 会在 feature 解析阶段直接失败"，是**正确的现状陈述**）；
  2 处为同表多命令。
- ⇒ **命令型文档（README / `.claude/CLAUDE.md` / `agents.md` / 两份 `.qoder` 副本）内无任何 feature 幽灵**；
  同时发现**指令面从不给可运行的 `--features` 命令形式**（只在散文里提 `--features mca`），属"缺失补充"面，
  补法须先实测（本波已备好免编译校验法）。

### 35.4 两项新登记的仓库工件事实

1. **4 份 Cargo.toml 带 UTF-8 BOM**：`crates/chimera-tui`、`crates/gqep-executor`、`crates/mcp-mesh`、`crates/nexus-app-server`
   （另 39 份无）。`tomllib` 严格解析在 line 1 col 1 直接失败；cargo 自身容忍 ⇒ 影响第三方工具/脚本，不阻断构建。
   同 4 份还是 CRLF 行尾（本仓 manifest 常态）。
2. **`release.yml` 无任何 feature 矩阵**：全文 `features`/`v3-engine` 0 命中；workflow 内 `--features` 仅
   `ci.yml:395-443` 的独立 `--features mca` job。⇒ `ADR-034` 「受影响面」第 3 项（双组合矩阵）**未落地**，
   已在该 ADR 状态区插「实施现状核对」块标为待实现（未改决策正文）。

## 36. 依赖版本面事实基座（波次 P104，2026-09-22 实测）

### 36.1 权威源与现行值

**依赖"解析后版本"的唯一权威源是 `Cargo.lock`**（623 个包 / 692 条 `name+version`；同一包多版本共存会被如实体现），
`Cargo.toml` 只是**要求范围**（如 `tokio = "1"`），二者不可混用为同一断言。本波起，3 份带
`[CONFIG-REFACTOR-20260920] 唯一权威源` 清单的文件（`agents.md`、`.qoder/rules/chimera.md`、`.qoder/rules/chimera2.md`）
已把 `Cargo.lock` 补进该清单。

| 关键依赖 | `Cargo.lock` 现行解析值 | 文档主张核对结果 |
|---------|------------------------|-----------------|
| `wasmtime` | **47.0.4** | 指令面曾存 47.0.3/47.0.4 混用 ⇒ 已按历史归位（见 §36.3） |
| `tract-onnx` | **0.22.3** | 与 `agents.md`/`CLAUDE.md` 一致 ✅ |
| `rusqlite` / `tokio` / `clap` / `proptest` | 0.32.1 / 1.52.3 / 4.6.1 / 1.11.0 | 文档中 `proptest 1.11+` 等前缀写法判为一致 |
| `sqlite-vec` | **不在锁内** | 与 ADR-005"永久禁用、改内存 KNN"一致 ✅ |
| 工具链 | 实测 `rustc 1.97.1` / `cargo 1.97.1` / active `stable-x86_64-pc-windows-gnu` | 与 CI 钉定 `dtolnay/rust-toolchain@1.97.1`（`ci.yml:54,344,408,467`）一致 ✅；文档里 `rustc 1.96.0 + GNU 的 ICE`（`CLAUDE.md:45`、`chimera2.md:43`）是**成因史**，非现状断言 |

### 36.2 普查分级（`tmp/p104_deps.py`，锁包数/真假版本三取自测全 PASS）

活文档 201 份 → 非 MATCH 主张 **114**：**MISMATCH 7** + **NOT-IN-LOCK 107**。

- **MISMATCH 才值得看，且须逐条读上下文**：7 条里 4 条是**正确的历史陈述**
  （`CHANGELOG:961` 记 v2.24.0 当时 `47.0.2 →47.0.3`、`CHANGELOG:271/284` 记 `47.0.3 → 47.0.4`、
  `adr_index:93` 记 ADR-035 当时的 47.0.2 PoC、`week8_stress_test_report` 记当年构建输出）⇒ 一律不改。
- **NOT-IN-LOCK 不可当缺陷用**：绝大多数不是依赖——工具链版本（`cargo 1.96.0/1.97.0`）、
  `--version` 期望输出（`chimera 1.0.0-omega` 等）、"workspace 版本 bump 2.22.0-omega"、
  以及英文词被误当包名（`rolled back v1.0.0` 里的 `back`）。⇒ 该面判定必须停在 MISMATCH 级。

### 36.3 本波确证并修掉的真实缺陷（同一条被误改的历史，存在于两处）

`agents.md` 的 v2.24.0 里程碑句在 **:104 与 :281 各出现一次**，两处都已被改成
「…修复(`tract-onnx 0.22.3` + `wasmtime 47.0.4`;2026-08-31 R1-T7 又按 RUSTSEC-2026-0268/0269 升至 47.0.4)」
⇒ **自相矛盾**（先称 v2.24.0 就用 47.0.4 修，后文又说"后来升到 47.0.4"＝一句空话），且与 `CHANGELOG:961` 的当年记录冲突。
已把两处还原为 `wasmtime 47.0.3` 并补「现 `Cargo.lock` 解析值即 47.0.4」。另两处：
`.qoder/rules/chimera.md:345`（派生副本缺后半句）补齐同款从句；`.claude/CLAUDE.md:147` 的 audit 注释加「现锁 47.0.4」。

> **断言纪律的一次实测价值**：补丁脚本对每条锚点断言"命中数 == 期望值"。首版写死 `==1`，
> 而 agents.md 实际命中 **2** ⇒ **整批不落盘**（文件零改动），才逼我去查第二处、并把期望改成 2。
> 若用"命中即替换"的写法，这一波就会只改一半、留下两份互相矛盾的同一里程碑。

### 36.4 刻意不做（避免假装同步彻底）

`.claude/CLAUDE.md` 与 `.trae/rules/nuxus规则.md` **没有**「唯一权威源」清单结构（前者是 `>` 项目指引块、
后者是"核心位置/历史溯源"块），强行塞入同一串只会制造排版异物 ⇒ 未加，并在此登记为**已知不对称**：
`Cargo.lock` 作为解析版本权威源，目前显式写在 `agents.md` + 两份 `.qoder` 副本；其余文档需现取时以本节 §36.1 为准。

## 37. 环境变量与配置优先级面事实基座（波次 P105，2026-09-22 实跑取证）

### 37.1 Figment 链的真实形状（三源，不是四源）

`crates/chimera-cli/src/config.rs:219-221`：

```
Figment::from(Serialized::defaults(ChimeraConfig::default()))   // L1 内置默认
    .merge(Yaml::file(&path))                                   // L2 配置文件（默认 ~/.chimera/omega.yaml）
    .merge(Env::prefixed("CHIMERA_").split("__"))               // L3 环境变量
```

⇒ **CLI 参数不在 Figment 链内**（`config.rs:216-218` 注释自陈；clap 解析后由各命令直接消费）。
所以"最终生效优先级"里 CLI 可以在第 4 位，但**不能说 Figment 有 4 个源**（`docs/architecture/data_flow.md` 旧图正是这么写的，已更正）。

### 37.2 合法 env 名的推导法（可直接复用的判据）

| 来源 | 数量 | 说明 |
|------|-----:|------|
| `chimera config show` 的 JSON **叶子路径** → `CHIMERA_段1__段2…` | 72 路径 | 与 `Env::prefixed("CHIMERA_").split("__")` 对称；比抄文档可靠 |
| 代码 `env::var("字面量")` 直读 | 8 个 `CHIMERA_*`（+`NO_COLOR`/`TERM_PROGRAM`/`TMUX`/`HOME`/`USERPROFILE`） | `CHIMERA_CHAT_CHUNK_DELAY_MS`、`CHIMERA_RUN_CHUNK_DELAY_MS`、`CHIMERA_TUI_CHUNK_DELAY_MS`、`CHIMERA_PANIC_TEST`、`CHIMERA_NO_PARALLEL_GSOE`、`CHIMERA_PARADOX_LOG`、`CHIMERA_PERF_SCALE`、`CHIMERA_NO_V3_ENGINE` |
| **`const`/前缀族间接读**（正则盲区） | ≥2 | `chimera-tui/src/engine/sync_probe.rs:40` `pub const NO_SYNC_ENV: &str = "CHIMERA_NO_SYNC_OUTPUT"`；`chimera-tui/src/config/tui_bible.rs:14-16` 前缀族 `CHIMERA_BIBLE_`（如 `CHIMERA_BIBLE_LAYOUT__MAIN_PANEL_RATIO`） |

### 37.3 三条实跑结论（`config show` 逐行 diff，非推理）

| 注入 | 结果 |
|------|------|
| `CHIMERA_QUEST__AUTO_DECOMPOSE=false` | 第 6 行 `auto_decompose: true → false` ⇒ **生效** |
| `CHIMERA_MODEL_ROUTER__STRATEGY=probeC` | 第 40 行 `strategy: "Auto" → "probeC"` ⇒ **生效**（段名含单下划线也可，分隔符只认 `__`） |
| `CHIMERA_MODEL__PRIMARY=probeA` | **逐行 diff 为空** ⇒ 静默忽略（顶层无 `model` 段）；`cli.rs`/`config.rs` 对该名 0 命中 |

### 37.4 活文档普查与两处更正

39 处可疑 `CHIMERA_*` 主张逐条判定 ⇒ **真缺陷 2 处**（均已就地更正），其余为四类噪声：
文件名型（`CHIMERA_NEXUS_GEN2_INNOVATIONS`、`CHIMERA_HCW`、`CHIMERA_V3` 等指 `.md` 名）、
前缀族型（`CHIMERA_BIBLE_*` 用 `*` 表前缀，实为真）、带日期快照件里的历史 env（`CHANGELOG_v1_8_0` 的
`CHIMERA_TUI__SYSINFO_REFRESH_INTERVAL_MS`）、以及"否定句里引用旧示例"（`CLAUDE.md:275` 已修过的说明）。

1. `docs/architecture/data_flow.md`：图中假示例换成实测生效的 `CHIMERA_QUEST__AUTO_DECOMPOSE=false`；
   「优先级 4,Clap 解析」补「—— 不进 Figment」；fence 后插 2 行更正注（含三源链 `path:line` 与实跑 diff 结论）。
   改后 205 行 / 全 CRLF / 裸 LF=0 / fence 平衡。
2. `docs/onnx-versioning.md`：`` ```rust `` 灰度示例所用 `CHIMERA_MODEL_VERSION` 在 `crates/` **0 命中**
   ⇒ fence 后加注标为**示意/未实现**（第 328 行），避免被当成可用灰度开关照抄。

### 37.5 代码侧滞后项（本审计未改，需授权）

`config.rs:255`（内嵌模板 / 默认值区）写 `version: "1.0.0-omega"`，而 `Cargo.toml` 的
`workspace.package.version = 2.28.2-omega` ⇒ `chimera config show` 与 `chimera config init` 产物都会报出一个
落后 2 个大版本的 `nexus.version`。属仓库工件（`crates/`）改动，登记为待裁决。

## 38. CLI 机器接口（API 规范面）事实基座（波次 P106，2026-09-22 实测）

### 38.1 为什么这是"缺失补充"而不是"过时"

四类特别关注里 **API 规范** 是覆盖最薄的一面：`--json` 输出契约（脚本消费的接口）在 201 份活文档中
**零处描述**，只存在于 `crates/chimera-cli/src/output.rs:14-32` 的 doc-comment，
而 `chimera --help` 还把读者指向"详见 `output.rs` schema 文档"（即指向源码而非文档）。
`ADR-060-chimera-cli-exit-code.md` 有决策矩阵，但活文档里 `exit_code`/`退出码` 的检索命中仅
`CODE_WIKI.md:618` 的一处无关文本 ⇒ 新开发者写 CI/管道脚本时无从查契约。

### 38.2 契约权威值（逐条对码核验，非抄 ADR）

| 项 | 现行实现 | 出处 |
|----|---------|------|
| 成功 envelope | `{"status":"ok","data":<payload>}` | `output.rs:14`, `output.rs:187` |
| 错误 envelope | `{"status":"error","error":{"kind","message"},"exit_code":<u8>}` | `output.rs:23-28`, `output.rs:198/203`, `print_json_error` `output.rs:218` |
| `kind` 稳定标识符（7） | `NotImplemented`/`ConfigError`/`EngineError`/`UserCancelled`/`PermissionDenied`/`Timeout`/`IoError` | `error.rs:103-112`（WHY 注释：`kind` 不含详情，供 `jq .error.kind` 匹配） |
| 退出码矩阵 | 0 success / 1 user_error(`ConfigError`) / 2 not_implemented / 3 system_error(`EngineError`+`IoError`) / 4 user_cancelled / 5 permission_denied / 6 timeout | `error.rs:84-94` **与 ADR-060 §1 逐条一致** ✅ |
| 回退开关 | `--legacy-exit-code`（置位后所有错误统一返回 1） | `cli.rs:94`, `main.rs:38/124-126` |

### 38.3 本波登记的一处"决策 vs 实现"矛盾（第 3 例，与 #9/#20 同族）

`ADR-060` 迁移计划写明「**v2.11.0-omega 移除 `--legacy-exit-code`**，矩阵成为唯一行为」，
但实测该 flag **仍在码内且仍生效分支**（`cli.rs:94` 声明 / `main.rs:125` `if cli.legacy_exit_code` /
`main.rs:38` 注释自陈保留）⇒ 文档化的"已移除"陈述不成立。已在 ADR-060 §1 前插「实施现状核对」块，
列明：矩阵默认生效，但显式传 `--legacy-exit-code` 会退回全 1；补做移除属 `crates/` 改动（需授权）→ **待裁决 #30**。

### 38.4 落盘（2 处，均为"补契约 + 标矛盾"，未新增文件）

1. `docs/architecture/CODE_WIKI.md:701`（chimera-cli 块）新增 **机器接口** 行：envelope 两形状 + `kind` 7 名 +
   退出码矩阵 + `output.rs:14-32`/`error.rs:84-94`/`error.rs:103-112` 三处 `path:line` + ADR-060 指针。
   单物理行、竖线数 3、fence 58→58、1,746 行 / 全 CRLF / 裸 LF=0。
2. `docs/architecture/ADR-060-…md:27` 插入 3 行「实施现状核对」块（不改写决策正文）。

> 选 CODE_WIKI 而非新建文档的理由：本审计的"单一落点 + 交叉引用"原则；`--json`/退出码属 chimera-cli 的
> 架构级契约，与同一表格里的 子命令/关键类型 行同级；新建 `docs/architecture/CLI_API.md` 会与
> `output.rs`、ADR-060、`--help` 形成第 4 份复制，反成漂移源。

## 39. 构建部署流程面事实基座（P107 波次，2026-09-22 现测）

**为什么单开一节**：目标把"构建部署流程"列为四个"特别关注"类别之一，判据是**能指导后续开发**。
此前所有波次都在改"文档说的值对不对"，而本面最大的缺陷是**覆盖面**——`agents.md §10.1`
与 `chimera.md §10.1` 的"CI/CD 准入门槛"表只列 **3 份** workflow，而 `.github/workflows/`
实测 **11 份**（`ls` 现取）。新开发者照 §10.1 理解门禁，会以为改 bench、改安装脚本、
提交 PR 到 main 都不触发任何门——这四种情形实际都有阻塞门。

**权威口径**：下表每一格来自本轮对对应文件的 `grep -n`/`sed -n` 现测，行号即证据。

### 39.1 11 份 workflow 全量清单（触发 × job × 准入）

| workflow | 触发行 | cron/事件 | job 清单 | 阻塞性 |
|---|---|---|---|---|
| `audit.yml` | `:7-14` | `0 2 * * *` + PR 改 Cargo.lock/Cargo.toml + dispatch | `audit` / `outdated` | audit 阻塞；outdated `continue-on-error` 非阻塞 |
| `bench_check.yml` | `:30-42` | PR(改 `crates/**/benches/**`、任一 `Cargo.toml`、`chimera-cli/src/**`、`scripts/check_cli_startup.*`、本文件) + tag | `bench-check` / `cli-startup` | 阻塞（逐项阈值断言，键与 `scripts/perf_thresholds.toml` 对齐） |
| `ci.yml` | `:13-18` | PR→main + push main + dispatch | `check` / `e2e` / `mca-feature` / `benchmark` | 阻塞；`check` 内 typos(`:261-265`) 与 taplo fmt(`:270-274`) |
| `coverage.yml` | `:22-26` | `0 4 * * *`(每晚) + dispatch | `coverage` | 阻塞；裁决唯一入口 `python3 scripts/coverage_gate.py --check coverage`(`:124`) |
| `deny.yml` | `:8-17` | `0 5 * * *` + PR 改锁/`deny.toml` + dispatch | `deny` | 阻塞；`command: check licenses bans`(`:37`) |
| `fuzz.yml` | `:23-32` | tag + `0 3 * * 1`(每周一) + dispatch | `fuzz`(matrix **8** target × 300s) | **非阻塞**（crash 上传，90 天留存） |
| `release.yml` | `:12-18` | tag `v*.*.*-omega` + dispatch | `build` / `dual-build` / `test` / `docker` / `release` | build+test+docker 全 pass 才 release |
| `slo.yml` | `:32-45` | tag + `0 4 * * *` + dispatch(scale 输入) | `slo-release-gate` / `slo-daily` | 发布门按 `CHIMERA_PERF_SCALE=1.0` |
| `stress.yml` | `:11-20` | `0 3 * * *` + dispatch(scale 输入) | `stress` | 超时按 `CHIMERA_TEST_TIMEOUT_SCALE`(默认 1.0) 缩放 |
| `test-install-scripts.yml` | `:26-35` | `0 4 * * *` + dispatch(可选 version) | `test-install` | 3 平台 matrix 跑 install.sh/ps1 全链路 |
| `zizmor.yml` | `:9-16` | `0 6 * * *` + PR 改 `.github/workflows/**` + dispatch | `zizmor` | 阻塞；`zizmor . --min-severity=high`(`:38`) |

### 39.2 本面逐项判定（哪些是缺陷、哪些不是）

- **`[profile.release]` 无缺陷**：`Cargo.toml` 实测 `strip = true` / `panic = "abort"` /
  `opt-level = "z"` / `lto = true` / `codegen-units = 1`，与 `agents.md:612`、`chimera.md:703`
  括号内五项**逐字一致**。
- **`Dockerfile` 6/6 与 §10.2 一致**：`:19` `FROM rust:1-slim-bookworm AS builder`、
  `:52` `gcr.io/distroless/cc-debian12`、`:58` `ENV RUST_BACKTRACE=1`、`:80` `COPY --chown=nonroot:nonroot`、
  `:86-87` `HEALTHCHECK CMD ["chimera","--version"]`、`:92` `USER nonroot:nonroot`、`:95` `ENTRYPOINT ["chimera"]`。
- **覆盖率阈值 85 一致**：`scripts/coverage_baseline.toml:22` `min_line_coverage = 85`，
  且该文件 `:10` 自述棘轮纪律"只升不降（下调须 ADR）"；文档记 85 ✓。
- **`audit.yml` 权威行号是 `:65` 不是 `:61`**：`sed -n '55,70p'` 定位 `run: cargo audit --deny unmaintained
  --deny unsound …` 落在 **`:65`**（6 条 `--ignore`）。而 `agents.md:581`、`chimera.md:804`、
  `CLAUDE.md:146`、`CLAUDE.md:325` 四处均写 `audit.yml:61`——**同一文档内还自相矛盾**
  （`agents.md:580` 写 `:65`、`:581` 写 `:61`）。本波次四处已改 `:65`。
  仍留 `:61` 的是带日期正文：`CHANGELOG.md:269/:324`、`ADR-167:5`——按处置规则**不改写历史正文**，
  登记给 #26 门落地后捕获。
- **`[[test]]` 计数：真值 37**（`grep -c '^\[\[test\]\]' Cargo.toml`）。`agents.md §8.1` 原写 **34**、
  `chimera.md §8.1` 原写 **35+**，两处已改 37 并绑定复算命令。
- **`CLAUDE.md §2` 的 14 条 `cargo test --test …` 不是缺陷**：与 `Cargo.toml` 37 个真名做
  `comm` 差集 ⇒ **幽灵 0 条**（14 条全部真实存在），且 `:105-106` 自述"下列为常用示例；
  全部 target 名以根 `Cargo.toml` 的 `[[test]]` 注册为唯一权威源，本文件不登记清单与计数"
  ⇒ 该清单**自我限定为非穷举**，不属"过时/冲突"类，未改动。
  ⇒ 教训：**清单类内容先读它的自述限定语再判是否穷举**，否则会把 27 条"缺失"误记成缺陷。
- **fuzz "8 target" 文档是对的，错在 workflow 内部注释**：`fuzz.yml:49-59` matrix 实列 **8** 项，
  `fuzz/Cargo.toml` `[[bin]]` 亦 **8** 个（`:94,100,106,112,118,124,132,140`），
  与 `CLAUDE.md:295`、§10.1 表格"8 target × 300s"一致；而 `fuzz.yml:45` 的 WHY 注释仍写
  "7 target × 300s"（该文件 `:8`/`:13` 又写 8）⇒ **workflow 自身注释陈旧**，属 `.github/workflows`
  工件，未单方改，登记为 **#31**。
- **#18 fuzz 发布阻断项复核仍成立**：`crates/model-router` 目录不存在，而 `fuzz/Cargo.toml:61`
  仍声明该 path 依赖 ⇒ `cargo check --manifest-path fuzz/Cargo.toml` 解析即失败（2026-09-22 复测）。
- **`--version` 断言有两处，只有一处会红**：`release.yml:164`（build job）是
  `… --version || echo "version check skipped"` ⇒ **非阻塞**；`release.yml:429-456`（docker job）
  在 `set -euo pipefail` 下 `exit 1` ⇒ **阻塞 release**。文档 §10.2 原只写"grep `^(aether|chimera)`"，
  未区分二者。
- **CI 正则含死分支 `chimela`**：`release.yml:451` 实为 `^(aether|chimera|chimela)`，
  而 `chimela` 在 `crates/` 与 `scripts/` 全库 **0 命中**（`grep -rn "chimela" crates/ scripts/ Cargo.toml`）
  ⇒ 无任何产出方，属历史拼写残留；真实可达集仍是 `aether`/`chimera`。文档口径不变，
  但补注"不得因 CI 宽松而放过 `chimela` 产物"。

### 39.3 本波次落盘（三文件，全部结构自检通过）

| 文件 | 改动 | 字节 |
|---|---|---|
| `agents.md` | §10.1 表 3 行 → **11 行**（含 job 名与阻塞性）；§10.2 加 2 条子项（双断言 / `chimela` 死分支）；`:581` `audit.yml:61`→`:65`；§8.1 test target `34`→`37`+复算命令 | +2,434 B（853 行） |
| `.qoder/rules/chimera.md` | 同上四处（§10.1 表保持本文件宽分隔线风格；`* ` 列表用 `  * ` 嵌套）；§8.1 `35+`→`37` | +2,287 B（963 行） |
| `.claude/CLAUDE.md` | `:146`/`:325` 两处 `audit.yml:61`→`:65`；§5 新增"完整门禁面（11 份 workflow）"一行，**指针指向 `agents.md §10.1` 而不复制表格**（去重口径） | +674 B（389 行） |

**结构自检**：两份 §10.1 表新增区各行竖线数**全部 = 5**（表头/分隔线/11 行 = 13 行，逐行 `gsub` 计数）；
三文件行首代码围栏计数均为 22（偶，未变）；CRLF 纯净（bareLF = 0）；行数 843→853 / 953→963 / 388→389。
**门禁复跑**：`check_doc_drift.sh` EXIT=0（146/10/41 对齐）｜`check_doc_symbol_claims.py` PASS
（436 occurrences / 7 gaps / 7 registered）｜`check_doc_consistency.sh` EXIT=0（三方对账 canonical 2.28.2-omega / 41 crates）。

**净篇幅影响披露**：本波次为"缺失补充"类，指令面 **+5,395 B**（D 口径相应上升）。
理由：目标硬约束之一是"构建部署流程必须能指导后续开发"，而 3/11 的门禁表客观不足以指导开发；
补偿是 `CLAUDE.md` 侧**只加指针不加表格**（避免第三份副本），以及把 4 处错误行号收敛为 1 处口径。

## 40. 命令面 / 层图 / 宿主协议面事实基座（P108 波次，2026-09-22 现测）

### 40.1 文档内命令的可执行性普查（可执行性标准的机检化）

**判据**：新开发者会照文档敲的每一条命令，其引用的 `scripts/` 路径、`-p <crate>`、`--bench <name>`、
`--test <name>` 都必须真实存在。量纲：439 份在盘 md（`docs/` 去掉 `_archive`/`_blueprints` + 根 11 份 +
四份规则/命令文档），指令面 15 份。

| 类别 | 命中 | 其中指令面 | 判定 |
|---|---|---|---|
| `scripts/<file>` 幽灵 | 2 | 0 真实缺陷 | 两处均在带日期正文：`CHANGELOG.md:8` 提 `cleanup_code_wiki.ps1`、`CHIMERA_全库级深度现状诊断报告_v2.25.0-omega.md:8` 提 `_audit_fnlen.py`（一次性诊断件，用完即删属正常生命周期） |
| `-p <crate>` 幽灵 | 1 | 0 | `CHANGELOG.md:283` 的 `wasmtime` 是 `-p wasmtime` 形式出现在依赖升级叙述里，非可执行指令 |
| `--bench` 幽灵 | 0 | 0 | — |
| `--test` 幽灵 | 1 | 0 | `CHANGELOG.md:1033` 落在 §32 编码受损带内 ⇒ **受损行使 token 级命令普查不可靠**，只能在指令面下结论 |

⇒ **结论：指令面与计划文档面的命令引用零幽灵**（脚本 107 个、bench 114 个、`[[test]]` 37 个为接受集）。
**植入正对照**：临时件 `zz_p108_control.md` 注入 4 类 bogus（`zz_bogus_gate.py` / `zz-bogus-crate` /
`zz_bogus_bench` / `zz_bogus_target`），扫描器 **4/4 全部命中且行号正确**，随后删除该件
（`ls` 复核不存在）⇒ 上面的"0 命中"是有牙的干净，不是空转。
**仪器自纠**：首版扫描器 `os.walk` 未排除 `.toolchain/`、`tmp_podman/`，把 vendored 第三方 README
（clippy/rustc/ crates.io registry）卷进来，量纲从 437 涨到 3,974 且报出 22 个"幽灵 crate"
（`getrandom`、`pest`…）——全是噪声。**规则：文档普查前必须先固定并核对范围量纲**，
两法对照（437 期望值 vs 3,974 实测）即暴露越界。

### 40.2 手写层图与 manifest 的一致性（架构面）

- **接受集**：`Cargo.toml` 经 `tomllib` 解析 `workspace.members` = **41**，与 `crates/` 目录数 **41** 完全一致
  （`dir-not-member = []`、`member-not-dir = []`）。
- **判定**：6 份文档（`agents.md`、`CODE_WIKI.md`、`CLAUDE.md`、`chimera.md`、`chimera2.md`、`nuxus规则.md`）
  的 11 行手写层图**幽灵 crate = 0**，且各列 39~41 个成员名。
- **退役 crate 的呈现方式已合规**（逐名 grep 复核，非仅层图）：`model-router` / `auto-dpo` / `acb-governor`
  （三者均已物理删除，`Cargo.toml:215` 明记"acb-governor 已按 ADR-182 退役删除"）在指令面只出现于
  **带否定语境的正文**——如 `agents.md:142`"（原 `model-router` 已物理删除，见 CODE_WIKI §3.12）"、
  `:371-372`"清单里的 `auto-dpo` 已物理删除"、`:435`"旧例 ACBGovernor 随 acb-governor 按 ADR-182 退役"。
  ⇒ 不属"与代码库不符的旧描述"，未改动。
- **仪器自纠（本波第二次）**：`[a-z][a-z0-9]*(-[a-z0-9]+)+` 强制要求含连字符 ⇒ 单词 crate
  `seccore`、`parliament` 被 tokenizer 漏掉，一度误报"层图未列 2 个"。
  修正判据：**层图完整性要按"成员名逐个 grep 行"二次确认**，不能只信正则集合差。

### 40.3 宿主协议（nexus-app-server / JSON-RPC v1）接口面

此前 API 面只覆盖了 **CLI**（§38）。本轮补齐**内部宿主协议**，全部为现测：

- **wire method 全集只有 2 个**：`app.op`（请求；`protocol.rs:171` 编码、`:225` 解码守卫）
  与 `app.event`（服务端推送；`:214` 编码、`:151` 通知帧声明）。
- **`AppOp` 6 变体**（语义入口，`crates/nexus-contracts/src/app.rs`）：
  `:331 ThreadStart` / `:333 TurnSubmit` / `:340 TurnInterrupt` / `:345 ApprovalRespond` /
  `:352 ThreadFork` / `:359 ModeSet`。
- **`AppEvent` 5 变体**：`:405 ThreadStarted` / `:410 ItemChanged` / `:415 ApprovalRequested` /
  `:420 TurnCompleted` / `:427 Error`。
- **JSON-RPC 错误码实际实现 3 个**：`-32700`（`:104`）、`-32600`（`:109`）、`-32601`（`:114`）；
  而 `JsonRpcError` 的文档注释 `:78` 另列 **`-32602` 无效参数**，全 crate 无对应构造点
  ⇒ 代码侧注释-实现偏差，登记 **#32**（文档只如实记"实际 3 个"）。
- **落点**：`CODE_WIKI.md` nexus-app-server 小节新增 **`协议线格式`** 一行（`:713`，1 物理行、3 竖线、+1,004 B）。
  刻意**只写一份**（CODE_WIKI 是架构权威源），`agents.md`/规则副本不复制该行 ⇒ 符合"通用背景移至一处 + 交叉引用"。
- **门的作用面**：`check_doc_symbol_claims.py` 的 table-row 扫描数从 436 → **440**（正是新行引入的 4 个符号），
  证明该行**确实受符号门管辖**（不是加了门看不见的内容）；distinct gaps 仍 7 / registered 7，PASS。

## 41. 过度声明与引用真实性普查（P109 波次，2026-09-22 现测）

**为何做这一面**：目标要求"删除所有与代码库不符的旧描述"且"未开始的任务须标注待实现"。
此前各波次主要抓"值漂移"（数字/版本/行号）与"幽灵符号"，本面抓**反向缺陷**：
文档把**不存在**的东西写成**已完成**（over-claim），以及**引用路径不在盘**。

### 41.1 方法与量纲

- **标识符接受集**：`crates/ tests/ fuzz/ scripts/ src/` 内 `.rs/.toml/.sh/.ps1/.py/.yml/.json`
  共 **1,451 文件 → 36,854 个 distinct 标识符**（Python 一次性索引）。
- **路径接受集**：全仓在盘文件+目录 **8,589 条**（排除 `.git/target/.toolchain/tmp*`）。
- **声称已完成的词**：`已实现|已落地|已交付|已完成|已接线|已生效|已注册|已同步|✅`；
  排除同行含 `待实现|草案|未落地|未实现|待接线|计划中|TODO|GATED|已删|退役|误载|零载体` 等否定/规划词。
- **范围**：776 份 md（排除镜像与 IDE 面 `.tower/.zcode/.superpowers/.workbuddy/_archive/_blueprints`
  及代码目录）。**路径形引用**（含 `/`）共 **6,009 条**。

### 41.2 结果：活权威文件里只剩 1 条真缺陷

按"是否在盘 + 全库是否有同名文件"分桶后，**指令/手册/计划面**的 699 条不在盘引用中，
绝大多数属两类噪声：① `docs/reports/*`、`docs/architecture/progress/*`、`_archive` 之类的**带日期正文**；
② `.trae/specs/**`（IDE 镜像，**审计范围外**，占 top-22 里的 12 席）。
逐名核到**活权威文件**（agents/CLAUDE/CODE_WIKI/INDEX/rules 四份/chimera2）只剩 8 条：

| 引用 | 位置 | 判定 |
|---|---|---|
| `docs/architecture/AETHER_NEXUS_OMEGA_ULTIMATE.md` | `agents.md:639`、`INDEX.md:44`、`chimera.md:730` | ✅ 合规——三处同行即写"❌ 不存在(已被取代,无需恢复)" |
| `crates/model-router` | `CLAUDE.md:328`、`chimera.md:449`、`:900`、`chimera2.md:273` | ✅ 合规——四处均在"`fuzz/Cargo.toml:61` 仍指向已删除 crate"这一**缺陷叙述**内（#18），是"指名报错"而非"当作存在" |
| `crates/seccore/src/formal_verifier.rs` | `CODE_WIKI.md:303` | ❌ **真缺陷**：见 §41.3，已注记 |

⇒ 附带结论：**目标要求的"每项声明要有 path:line 证据"这一硬标准，此前审计自身产出的引用面已高度自洽**
（699 → 8 → 1），这是可复核的收敛证据，不是主观判断。

### 41.3 ADR-047 决策 1 与实现相反（本轮唯一实质缺陷）

`CODE_WIKI.md:303`（seccore 小节）转述 ADR-047：形式化验证器"**嵌入 seccore 而非新建独立 crate**（决策 1）"、
"落地路径 `crates/seccore/src/formal_verifier.rs` + 4 子模块"。实测三件事：

1. `crates/seccore/src/` 现 **22 个文件，无任何 `formal*` 文件**（`ls` 现取）；
2. 全 `crates/` 内**唯一** `FormalVerifier*` 命名类型是 `FormalVerifierGate`
   （`crates/gsoe-evolution/src/formal_gate.rs:84`，定义 + `impl` 在 `:84/:95`）；
3. 验证器属性/门实际分散在 **6 个 crate**：`parliament/src/formal/`、`gsoe-evolution/src/formal_gate.rs`、
   `nexus-contracts/src/formal_props.rs`、`decay-engine/src/formal.rs`、`event-bus/src/formal.rs`、
   `omega-learner/src/formal.rs`（全部 `find` 现取，均在盘）。

同时，**能力主张本身成立**：`Cargo.toml` `[[test]]` 注册了 `formal_verifier_e2e` /
`formal_verifier_m1_e2e` / `formal_verifier_m2_e2e` / `formal_verifier_gate_e2e` 四个 target
⇒ 所以缺陷**不是**"M0/M1/M2 全部落地"这句过度声明，而是**落点与文件规划已被实现推翻**，
且 ADR-047 状态至今仍是 **Proposed**（`CODE_WIKI:303` 自述）。这正是"ADR 决策与实现要反向核对"的复发案例。

**已落动作**：`CODE_WIKI.md:303` 行尾追加实测核对（+943 B，行结构仍 3 竖线）；
`ADR-047` 在"决策 1"上方追加只加注不改写的实施现状核对（+727 B，该文件纯 LF、686 行）。
**待裁决 #33**：补 superseding ADR 记录落点改判，还是改写 ADR-047 决策 1。

### 41.4 两份根手册的"待实现"标注判定（否证）

`Chimera_CLI_v3.4.0_omega_统一架构…二十三篇论文融合权威版.md`（213,600 B / 4,094 行）与
`Chimera_CLI_九源交叉融合彻底重构工程手册V1.0.md`（92,927 B / 1,430 行）内含大量规划对象
（`crates/hils-attention`、`crates/nexus-acp`、`tests/cross_layer_integration.rs`、`docs/roadmap/openforge-proxy.md`、
占位号 `docs/architecture/ADR-0xx.md`），**全库零命中**。
但两文件首部均已带**「档案化权威基线核准横幅（2026-09-02 追加，2026-09-06 核验刷新，历史文档只加注不改写）」**
+ **「⚠ 2026-09-21 追加校正」** 双重限定 ⇒ 其规划对象整体处于"档案化设计"语义内，
**不按"未标注待实现"记缺陷**（若要逐条改写属重写档案件，与处置规则 1 冲突）。
⇒ 本面登记为**否证**：不新增标注，避免把档案件改成现状件。

## 42. 跨文档重复块事实基座（P110 波次，2026-09-22 现测）

**判据（可复算）**：语料 = 计划/手册/规则面 **228 份 md / 4,836,649 B**（在 425 份在盘非镜像 md 基础上
排除带日期证据面 `docs/reports|optimization|research` 与 `CHANGELOG.md`，并排除 `_archive`/`_blueprints`/
镜像目录/代码目录）。取每行 `strip()` 后长度 ≥60 字符者为"内容行"，MD5 归组；
组内**跨 ≥2 文件**记为重复簇，可回收字节 = 组内副本数 −1 的字符量之和。

| 量 | 现测 |
|---|---|
| 重复长行组 | **274** |
| 可回收字节 | **35,161 B**（占语料 **0.73%**） |
| 其中簇内全部为档案化手册 | 15,447 B（43.9%） |
| 其中至少一处为活文档 | 19,714 B（56.1%） |

### 42.1 档案化手册侧（可由 #23/#7 一次性消化）

| 共享长行组 | 文件对 |
|---|---|
| 115 | `docs/architecture/AETHER_NEXUS_OMEGA_从零搭建完全指南.md` ↔ `…_终极文档_v3.md` |
| 18 | `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md` ↔ `CHIMERA_NEXUS_GEN2_INNOVATIONS.md` |
| 9 + 7 + 4 | `AETHER_NEXUS_FULL_DOCUMENTATION.md` 与上两者的三元组合并簇 |
| 7 | `从零搭建完全指南` ↔ `终极文档_v3` ↔ `OMEGA_大模型架构魔改创新_AI_Agent项目套用设计.md` |

⇒ 这些正是 `DOCUMENT_LIFECYCLE_POLICY.md` §4.3 判"移 `_archive/2026-Q2/`"但**实测未执行**的 7 份；
其重复体量（约 15 KB 长行副本）**远小于**把它们整体移出活跃集所能释放的字节（§9 记 0.48 MB 量级）。
⇒ 结论：归档的价值在**分母/语义分类**，不在去重。

### 42.2 活文档侧（逐簇判定：全部刻意保留，不改写）

| 组 | 文件对 | 判定 |
|---|---|---|
| 15 | `agents.md` ↔ `CODE_WIKI.md` | 依赖铁律块 + 11 层图块。属"速查版/详细版"两层设计（`.trae/rules` 与 `agents.md` 的既有分工），删任一侧都会让该读者被迫跳文档 ⇒ **保留**，已在 §9/§10 口径下计入"证据面换篇幅" |
| 13 | `docs/audit/full-codebase-diagnostic-v2.25.0-omega.md` ↔ `quad-perspective-progress-audit-v2.25.0-omega.md` | 同日两份带日期审计件 ⇒ **带日期正文不改写** |
| 5 | `ADR-042` ↔ `ADR-043` | ADR 正文 ⇒ 处置规则明确"标注不改写" |
| 4 | `docs/audit/dimension_f_security.md` ↔ `docs/security/week8_security_report.md` | 均为带日期审计件 |
| 3 | v3.4.0 手册 ↔ v4.0 六编终版 ↔ 九源手册 | 三份档案化根手册（已带核准横幅，见 §41.4） |
| 11 | RL 开发闸门横幅句在 11 份文档中重复 | **刻意冗余**：治理横幅必须可 grep 且要在每份被治理文档内自证（记忆：治理标记须落可机检词） ⇒ 保留 |

⇒ **本面最终结论**：指令面/手册面的重复不是"未去重"，而是**分层设计 + 治理冗余 + 带日期正文**三类合理重复；
可机检的剩余去重空间 = 0.73% 量级，已在 §11 的三结论中钉死。**不再开压缩波次**，除非用户改口径或批 #23/#16。

## 43. 测试策略面事实基座（P111 波次，2026-09-22 现测）

目标把"测试策略"列为需与代码对拍的维度之一。本轮以**编译无关**的取证方式（读 CI 调用 + 门自报）建立基座。

### 43.1 CI 侧实际测试调用（逐行现取）

| 位置 | 调用 | 语义 |
|---|---|---|
| `.github/workflows/ci.yml:311`（`check` job） | `cargo nextest run --workspace --profile ci-fast --exclude chimera-e2e-tests -E 'not binary(/stress/)' --no-fail-fast` | **PR 快轨**；`ci-fast` profile 定义在 `.config/nextest.toml:44`，其目标"总耗时较 baseline-full 降 ≥30%"记于 `:31` |
| `.github/workflows/ci.yml:371`（`e2e` job） | `cargo nextest run -p chimera-e2e-tests -E 'not binary(/stress/)' --no-fail-fast` | E2E 独立并行承担（快轨排除的补偿门禁） |
| `.github/workflows/ci.yml:384` | `cargo nextest run -p chimera-e2e-tests -E 'binary(/formal_verifier/)' --no-fail-fast` | 形式化验证 E2E 单独成段 |
| `.github/workflows/ci.yml:435`（`mca-feature` job） | `cargo hack check --workspace --each-feature --no-dev-deps --exclude chimera-e2e-tests` | **全 feature 组合编译门**（cargo-hack） |
| `.github/workflows/ci.yml:443` | `cargo nextest run --workspace --features mca -E 'not binary(/stress/)' --no-fail-fast` | MCA 双轨运行门 |
| `.github/workflows/release.yml:296` | `cargo test --workspace` | clean-runner 完整验收（**含 doc-tests**，与快轨口径不同） |
| `.github/workflows/release.yml:231,251` | `cargo test --release -p nexus-core crossbuild_marker_output …` | 交叉编译产物 marker 校验 |

**两个易被误用的语义点**（文档此前没写清，已在 `CLAUDE.md` 补齐）：
1. `CHIMERA_TEST_TIMEOUT_SCALE=0.1` **不是 profile 设的**——`.config/nextest.toml:37` 自述"依赖：CI 端设"，
   ⇒ 本地跑 `--profile ci-fast` 而不注入该变量，等待仍按 1.0，**耗时与 CI 不可比**。
2. stress 类靠 `-E 'not binary(/stress/)'` **按 binary 名**排除，**与 test-group 无关**：
   该仓 `test-groups` 声明数为 0，`nextest.toml:7-11` 已把旧注释"由 test-group 收纳"判为
   "描述了一个不存在的机制"（2026-08-31 订正）。

**nextest profile 清单**（`[profile.*]` 现取）：`default`(`:22`，`retries = 0`)、`ci-fast`(`:44`)、
`full`(`:54`)、`stress`(`:66`)、`slo`(`:107`，`retries = 2` + `slow-timeout` `:115`)。
⇒ `retries` 非 0 只出现在 `slo` 档，且 `:85` 自陈语义约束"重试只吸收环境噪声，不吸收回退"。

### 43.2 `#[ignore]` 档归属现测（与指令面登记值对拍）

`bash scripts/check_perf_redlines.sh --static-only` 自报：**total ignored = 70**，
分布 `manual-only=10` / `slo-blocking=13` / `slo-daily=47`，`unignore-target 0 <= 0` **PASS**。
⇒ 与 `agents.md §1.2 ③` 现登记值（70 / 10 / 13 / 47 / 归零）**逐项一致，无漂移**；
该文的"76 条已登记 / unignore-target 7 条待解除"是批次时点值，已由门自报口径取代（此前已注）。

### 43.3 本波次落盘与新增待裁决

- **落盘**：`.claude/CLAUDE.md:87` 快速回归命令改为与 PR 门禁**逐字同口径**
  （补 `--profile ci-fast` 与 `-E 'not binary(/stress/)'`），并加 1 行三点说明（+866 B，fence 22 未变、bareLF 0）。
  ⇒ 修复的是"新开发者照本地命令跑，误以为等价于 PR 门禁"的可执行性缺口。
- **新裁决项 #34**：`ci.yml:289` 注释写"根 package **33** 个 E2E/压测 `[[test]]`"，
  而 `grep -c '^\[\[test\]\]' Cargo.toml` 现测 **37** ⇒ workflow 注释计数陈旧（同 #31 类，改 `.github` 需授权）。
- **否证**：`release.yml:296` 用 `cargo test --workspace`（含 doc-tests），与 §10.1 门禁表所记
  `test` job 一致；`agents.md §7.2` 第 2 步的 `--jobs 4` 是**本地** OOM 缓解而非 CI 口径，二者不冲突（文档已分别注明）。

## 44. 事实基座自身的复测与刷新（P112 波次，2026-09-22 现测）

**为什么单开一节**：本审计把"每项声明要有 `path:line`"立为硬标准，那么**充当权威源的事实基座本身**
也必须被复测——否则它就成了新的陈旧源（同一族教训：立"计数看权威源"规矩的那份文档，常是最大的陈旧源）。
本轮复测 §1 的两条"计数冲突事实"，**两条都已在 2026-09-20 → 09-22 之间被现实闭合**，原文按只加注不改写处理：

| §1 原主张（2026-09-20） | 2026-09-22 复测 | 处置 |
|---|---|---|
| 五份文件（`agents.md`/`CLAUDE.md`/`nuxus规则.md`/`CONVENTIONS.md`/`CODE_WIKI.md`）仍把 **43 crates** 当现势登记 | 四份已无"43 crate"字样；残留的"38→43"全是**版本增量史句式**（`nuxus规则.md:40/83/133`、`INDEX.md:8`、`CODE_WIKI.md:130/155`、`agents.md:171`）；现势 41 由 `CODE_WIKI.md:40`/`:720` 承担并被 `check_doc_drift.sh`（自报 `workspace_crates=41`）钉住 | 原句上方加**【已消解】**复测块，原文保留 |
| `agents.md §5.3` / `nuxus规则.md §6.2` 登记 Critical mpsc「**13 类**」，实际 14；事件数文档记 144/145 实际 146 | 权威清单实测 = `crates/event-bus/src/bus.rs:109 is_critical_mpsc_event()` 的 **14 个 match 臂**；`nuxus规则.md:169` 现写"14 个"✓；`chimera.md §5.3` 末尾亦写"14 个"且其 14 个名字与 bus.rs **逐名同序一致**（脚本对称差 = ∅、`顺序一致=True`）；`agents.md §5.3` 已改指 `registry.rs` 注册表 | 同上，原文保留；事件计数另见下行 |

**同轮唯一实修**：`.qoder/rules/chimera.md:589`（§5.3）的版本增量串以"v2.28 定稿 **144**"结尾，
读者极易把它当现势数 ⇒ 在该串内就地加限定："**此串为版本增量史，不作现势依据；2026-09-22 现测 146 个变体，
权威源 `types.rs`，`scripts/check_doc_drift.sh` 自报同值**"（+177 B，行结构未变）。
`agents.md`/`CLAUDE.md`/`nuxus规则.md` 经 `grep -c '定稿 144'` 实测 = **0**，无需动。

### 44.1 控制项（证明本轮判据有牙，而非空跑）

- **双清单同步控制**：不是"看起来一样"，而是脚本比对 `bus.rs` 的 14 个臂与 `chimera.md` 行内 14 个反引号名，
  **集合并对称差 ∅ + 顺序全等**两项同时打印；任一不满足即 `ABORT` 不落盘。
- **"计数与符号绑定"的反例（仪器自纠第 6 起，本轮）**：我用"同一行内第一个 `\d+ 个`"取计数，
  于是把 `chimera.md §5.3` 里的 **"v2.0.0 +7 个 Agent 事件"**（版本增量）读成"Critical mpsc = 7"，
  差点据此把正确的 14 改成 7。⇒ **同段落多个计数时，必须取与被测符号语义邻接的那个**
  （本次真值就在同一行末尾"权威源 `is_critical_mpsc_event()`,14 个"）。
  与本基座 §40.2 的"tokenizer 漏单词名"、§42 的"裸文件名不能判存在性"同族：**判据的形状决定它永远查不到哪一类问题**。
- **`grep -o "NexusEvent::[A-Za-z]*"` 会把 `R2FreezeViolation` 截成 `R`**（字符类漏了数字与 `_`）
  ⇒ 名字类抽取一律 `[A-Za-z0-9_]+`；这条与既有记忆里 `rstrip(".rs")` 的字符集合坑同族。

## 45. TUI 能力清单全量对拍与"收尾"语义校正（P113 波次，2026-09-22 现测）

### 45.1 命令计数：文档是对的，我的第一把尺是错的

`docs/tui/README.md:48` 主张"全表 **53** 条 = `BUILTIN_COMMANDS`，按域分布 Session 11 / ModelApproval 4 /
Orchestration 12 / CodeProject 6 / ViewConfig 14 / System 6（合计 53）"，`:128` 主张"内建表 **41** 条顶层命令"。

**正确的量纲**：按 `SlashCommandDesc { … }` 块解析（而非按 `name:` 行计数）⇒
**块数 53、域分布逐项 11/4/12/6/14/6 与文档逐字相同、基数闭合（11+4+12+6+14+6=53）**；
其中 `name` 含空格的**二级命令 12 条**（`quest list`/`quest show`/`quest pause`/`quest resume`/`quest cancel`/
`quest checkpoint` 等）⇒ **顶层 = 53 − 12 = 41** ✓ 两行主张全部成立，`nuxus规则.md:225` 的"53 命令注册"亦成立。

**两枚仪器自纠（同一探针先松后紧，方向相反）**：
1. 第一版正则 `name:\s*"([a-z0-9-]+)"` 的字符类**不含空格** ⇒ 结构性丢掉 12 条二级命令，
   得到"42 distinct"，据此会把**正确的** README:128"41 条顶层"判成缺陷。
   与既有记忆"`Variant {` 结构体变体被深度顺序丢掉"同族：**判据字符类/结构顺序决定它永远看不见哪一类**。
2. 第二版放宽为 `name:\s*"([^"]+)"` 全文扫 ⇒ 得到 **55**（把 `SlashCommandDesc` 之外的 `name:` 字面量也算进来），
   又足以让人把**正确的**"53"判成陈旧。
   ⇒ 定论：**计数必须锚在被计数对象的语法单元上**（这里＝结构体字面量块），且**用域分布做基数闭合校验**；
   只做"行数匹配"的两个版本都产生了假缺陷。

### 45.2 幽灵命令：已由前波标注，本轮否证

令牌级扫描在 `docs/tui/README.md` 报出 7 个"注册表里没有的命令"（`/quest` `/commands` `/pace` `/context`
`/recap` `/copy` `/notify`）。逐条读原句后判定 **全部合规**：`:126-131` 已逐项写明
"**未接线**（`pace_gate.rs` 在盘但全 crate 107 处 `mod` 声明里没有它）"、"**未注册** ⇒ 上下文策展唯一入口是 `/compact`"、
"**无任何代码足迹** ⇒ 待实现"、"`user_commands.rs`/`notify.rs` 在盘但未被 `mod` 声明"、
"`AgentTree` 全库 0 命中、`PanelId` 27 变体里无该面板 ⇒ 待实现"。
⇒ 令牌级"未登记"必须过**句子级状态词**这一道才允许记缺陷（本会话第 3 次靠它避免制造破坏）。
`PanelId` 枚举实测 **27 变体** ✓ 与文档一致。

### 45.3 本轮唯一实质缺陷：「W0~W11 全部收尾」的语义过宽

`agents.md:285`、`CODE_WIKI.md:1634`（版本表 v2.26.0 行）、`.trae/rules/nuxus规则.md:80` 三处均写
"Concord TUI 重构 **W0~W11 全部收尾**"，而 §45.2 的实测现状是 **W8~W11 的若干子项从未落码**
（PaceGate 接线、`/context`、`/recap`、`/copy`、`user_commands`、`notify`、AgentTree 面板）。
"波次收尾"与"每项交付"是两个命题，读者按前者会以为后者全部成立 ⇒ 属目标的"与代码库不符的描述"。

**处置**：三处就地插入同一限定句（各 +243~246 B，共 +734 B）——
「此处「全部收尾」指**波次工程收口，不等于清单内每项均已落码**；W8~W11 若干子项实测未接线/未注册，
逐项现状的**唯一权威源**是 `docs/tui/README.md` 的「Concord 重构 W7~W11 新增能力」节」。
⇒ 细节**不复制到第二处**（符合"通用背景移至一处 + 交叉引用"）；
CODE_WIKI 那份因符号门管辖，限定句刻意**不复述任何不存在的符号名**（只给指针），
复跑符号门 441/7/7 仍 PASS 即为证。

**结构自检**：CODE_WIKI:1634 行竖线 = 4，与同表块其它行一致；nuxus规则:80 = 3 与同表块一致；
agents.md:285 为项目符号行（非表格）；三文件行首围栏计数不变（22/58/8）、CRLF 纯净 bareLF=0。
**门禁**：`check_doc_drift.sh` EXIT=0 ｜ `check_doc_symbol_claims.py` PASS ｜ `check_doc_consistency.sh` EXIT=0。

## 46. 数据库 schema 维度的表级闭合与锚点精度（P114 波次，2026-09-22 现测）

四个"特别关注"类别里，**数据库 schema** 此前只做过"引用面/重复 DDL"检查（§85、§31 族），
本轮做**表集合闭合 + DDL 指针精度**两级对拍，结论是**该维度干净**，并把复算式写进被检文档本身。

### 46.1 表集合双向差集为空

- **代码侧**：对 `crates/**/*.rs` 抽 `CREATE [VIRTUAL] TABLE [IF NOT EXISTS] <name>` 去重 = **17 名**；
  其中 `if`（大小写混排的续行造成的假名）与 `test_kv`（`cmt-tiering/src/pool.rs:207` 的
  `#[cfg(test)] mod tests` 夹具，`SQLITE_SCHEMA.md §1` 末注已排除）⇒ **生产 15 表**。
- **文档侧**：`SQLITE_SCHEMA.md §1` 登记 16 行 / 15 个不同表名（含 `entries` 两次：WikiStore 主库与
  `agent_grep.rs` 的一次性索引库同名表；`entries_fts` 以虚拟名单列）。
- **差集**：`文档有代码缺 = ∅`；`代码有文档缺 = {if, test_kv}`（均为已登记的噪声，非缺失）。
  ⇒ **基数闭合 15 = 15**。

### 46.2 DDL 锚点精度：17 处全部命中 CREATE 行

按 **crate 限定路径**解析 §1 的每个 `path:line` 并打印实际打开的文件后逐条回读：
`samples→chimera-tui/src/data/metrics_history.rs:132`、`warm_capabilities→cmt-tiering/src/warm.rs:96,128`、
`cold_capabilities→cold.rs:129`、`experience_cards→experience_card_storage.rs:92`、
`token_evidence→:144`、`session_turns→mca-gateway/src/session.rs:132`、
`procedural_memory→mlc-engine/src/l3_procedural.rs:79`、`entries/anchors/entry_relations/distilled_insights→repo-wiki/src/store.rs:997/1018/1039/1062`、
`wal_entries→scc-cache/src/wal.rs:315`、`segments/events→session-store/src/tree.rs:210/220`
⇒ **17 处 DDL 指针全落在 `CREATE TABLE` 行上，零错位**。
另有 2 处非 DDL 指针（`cold.rs:53` = `const ATTACH_ALIAS`、`l3_procedural.rs:11` = `//!` JSON 存列的 WHY 注）
**按行内文字被有意引用**，不属锚点失效。

### 46.3 两枚仪器自纠（都在"审计器自己"这一侧）

1. **basename 碰撞制造假失效**：首版用 `crates/*/src/<file>` 解析，本仓库有**两个** `src/wal.rs`
   （`scc-cache` 与 `quest-engine`），`c[0]` 取到后者 ⇒ 把**正确**的 `wal.rs:315` 报成
   "落点是 `assert!(…)`"。改为**用行内 crate 列限定路径 + 打印实际打开的完整路径**后假阳性归零。
   ⇒ 与既有记忆"审计器自己的路径解析就是假阳性来源，必须打印它打开了哪个文件"完全同族，本次是第 N 次复发。
2. **行尾守卫写反造成假中止**：守卫写成 `if cr != lf: ABORT`，对**纯 LF** 文件（`cr=0, lf=70`）必然成立
   ⇒ 正确逻辑是 `if cr and cr != lf`（只有既有 CR 又不等于 LF 总数才叫混合）。
   本次幸而 abort 方向保守（未写坏文件），但要记住：**假中止会让人误以为"文档被并发改动"而绕道**。

### 46.4 落盘

`docs/architecture/SQLITE_SCHEMA.md` §1 末（插入后位于 `:32`）新增**表级闭合自检**引用块（+743 B，纯 LF 保持、72 行）：
给出可复算式、15/15 闭合结论、17 处 DDL 指针精度结论，以及"改表后复跑 + 按 crate 限定解析"的两条纪律。
⇒ 该文档是活手册，**新增内容是复算方法而非第二份 DDL 复制**，与其"反漂移原则"（列级定义不复制、只给 `path:line`）一致。

## 47. 端到端可执行性实跑：配置落盘链路（P115 波次，2026-09-22）

目标的"可执行性"标准是**新开发者能纯靠文档搭环境**。此前各波都在**静态**对拍命令是否存在、参数是否合法；
本轮做一次**动态实跑**，对象是文档反复承诺的配置落盘链路（`agents.md §10.5` / `chimera.md §10.5` /
`CLAUDE.md §4` 三处都写"可用样例由 `config.rs:253` 内嵌模板经 `chimera config init` 或 `chimera doctor` 落盘，
不会在首次运行自动写"）。

### 47.1 实跑步骤与观测（可复算）

| 步 | 命令 | 观测 |
|---|---|---|
| 1 | `rm -rf tmp/p115_home && mkdir -p tmp/p115_home`（沙箱 HOME） | 干净空目录 |
| 2 | `HOME=$PWD/tmp/p115_home ./target/debug/chimera.exe config init` | **rc=0**；打印 `✓ 已生成默认配置:<沙箱路径>\.chimera\omega.yaml`；落盘文件 **4,312 B**，含 `nexus/quest/thinking_toggle/…` 真实配置项（**非 `examples/config.sample.yaml` 那种 9 字节壳**） |
| 3 | 同环境 `chimera config show` | 输出含 `"auto_decompose": true`、`"max_tasks_per_quest": 20`、`"version": "1.0.0-omega"` ⇒ **读的就是步骤 2 落的那份文件** |
| 4 | 把文件里 `max_tasks_per_quest: 20` 改成 `7` 后再 `config show` | 变为 `"max_tasks_per_quest": 7` ⇒ **YAML 确实被消费，不存在"改了静默不生效"** |
| 5 | 故意写坏 YAML（`nexus: [this is : not valid yaml`）后 `config show` | **rc=1** + `ConfigError: 加载配置失败:\\?\D:\…\.chimera\omega.yaml`（`ERROR` 日志同样点明文件） |
| 6 | 清理 | 删除沙箱与两份日志，`ls` 复核不存在 |

### 47.2 三条结论（都写进了手册）

1. **文档承诺成立**：`config init` / `config doctor` 落盘、首次运行不自动写 —— 动态证实，不是只有代码注释自证。
   `default_config_path()`（`config.rs:40-44`）取 `$HOME` 再退 `%USERPROFILE%` ⇒ **Git Bash 下只需改 `HOME` 即可隔离**，
   这也是本轮能做沙箱实跑的前提（PowerShell 侧要改 `USERPROFILE`）。
2. **错误形态符合文档化的退出码矩阵**：坏 YAML → `ConfigError` → **rc=1**，与 §5/`error.rs:84-94` 登记的
   「1 = ConfigError」一致 ⇒ **失败可诊断**（消息里带绝对路径），不会伪装成成功。
   ⚠ Windows 下该路径带 `\\?\` verbatim 前缀 —— 属显示层现象非缺陷，但**照抄去 grep 需按字面匹配**。
3. **顺带坐实 #29**：生成模板里 `nexus.version` 的值是 `1.0.0-omega`，与工作区 `2.28.2-omega` 脱节 ⇒
   新开发者落盘即拿到一个陈旧版本字段；已在实跑块第 ④ 条写明"该行不得当版本权威源"，工件修法等 #29 裁决。

### 47.3 落盘（单一真本 + 两处指针）

- `.claude/CLAUDE.md §4`（`:285-289`）新增 **5 行"端到端实跑验证"块**（+967 B）：命令、观测、`\\?\` 显示层提示、#29 关联。
- `agents.md:786` 与 `.qoder/rules/chimera.md:895` 的 §10.5 figment 行**第 2 列尾**各加一句指针式小结
  （各 +209 B；竖线数不变、状态列未被污染）。
- ⚠ 过程自纠：指针**首次落在末列（状态列）**——因为我按"行尾前插入"处理，而表格行的最后一格是状态列。
  已用列切分脚本把它移回描述列（净字节 0）。⇒ 通则：**往表格行里加内容要先确定落在第几列**，
  "行尾"默认落在**状态列**，而状态列只应放状态词。

## 48. 跨文档"章节号指针"对拍（P116 波次，2026-09-22 现测）

**为什么查这一面**：`§N` 型指针是"改 X 先看 Y"这类配方的寻址方式；目标要求文档"可指导后续开发"，
而小节号漂移不会让任何现有门变红（符号门查标识符、t2 查文件路径，**都不查 `文档 §小节`**）。

**判据**：取被引文档的全部 `^#{2,4} <数字>[.数字]` 标题号集合，再从 10 份活文档（`agents.md`、`CLAUDE.md`、
两份 `.qoder/rules`、`nuxus规则.md`、`README.md`、`docs/tui/README.md`、`docs/tui/…plan`、`INDEX.md`、
`docs/architecture/README.md`、`SQLITE_SCHEMA.md`）抽 `CODE_WIKI|INDEX|adr_index|CONVENTIONS|SQLITE_SCHEMA` + `§N(.M)` 引用，
判 N 是否在被引文档的标题集合内。被引集合大小：`CODE_WIKI.md` **81**、`INDEX.md` 4、`SQLITE_SCHEMA.md` 5、
`adr_index.md` 0、`CONVENTIONS.md` 0。

### 48.1 抓到并修掉的真实缺陷

| 缺陷 | 处数 | 位置 | 修法 |
|---|---|---|---|
| `` `CODE_WIKI.md §2.3` 为权威源 `` —— CODE_WIKI **无 §2.3**（其 ADR 全量索引在 §11） | **5** | `agents.md:253`、`chimera.md:108/225/297`、`INDEX.md:99` | 改为 `` `CODE_WIKI.md §11` + `docs/architecture/adr_index.md` ``。这些位置都在**"当权威源用"**的语气里，不是否定式引用（逐条读原句确认） |
| `chimera.md` 仍写 **OMEGA 九定律**（权威为十一，Ω₁₀/Ω₁₁ 由 ADR-170 收录） | 4 | `chimera.md:108/225` 区 + 两处检查清单 | 改词 + 在**该行原分隔符**下补 `Ω₁₀-Card / Ω₁₁-Synthesize`；另 `chimera2.md`、`CLAUDE.md` 各 1 处改词 |
| 自身改出的两处衍生问题（回读即暴露，未流到下一轮） | 5 + 1 | 见 48.2 | 已修 |

修后复扫：**跨文档章节指针零悬挂**（10 份活文档 × 5 个被引目标）。
残留 `CODE_WIKI.md §2.3` 仅存在于带日期正文（`rule-docs-cross-reference-report.md` ×4、
`docs/optimization/*` ×2、`docs/release/*` ×1）⇒ 按处置规则只标注不改写。

### 48.2 两枚新仪器自纠（都属于"把正确内容改成错误内容"的高危方向）

1. **反引号嵌套**：把指针换成 `` `CODE_WIKI.md §11 + `docs/architecture/adr_index.md`` `` 时，
   新串里带了一对**内层反引号**，落在原 code-span 内部 ⇒ markdown 渲染提前闭合、留下游离反引号（5 处）。
   是我回读文件时被"…§11 + `adr_index.md``…"的怪相发现，随即改成
   `` `CODE_WIKI.md §11` + `docs/architecture/adr_index.md` ``（两个独立 code-span）。
   ⇒ 规则：**替换文本若要插进已有 code-span，只能整体重写该 span，不能塞进内层反引号**。
2. **改词与补枚举必须分两步校验**：把"九定律"改成"十一定律"后，同行枚举仍止于 `Ω₉-Preserve` ⇒
   **词说十一、列只九个**，制造了新的内部矛盾；且不同文件分隔符不同（表体用 `·`、括号列表用 `/`），
   统一插 `/` 会破坏表格行的排版。终版做法：按该行实际出现的
   `Ω₈-Assess ·/÷ Ω₉-Preserve` 反推分隔符再补两项，并断言"含十一定律 且 含 Ω₉ 且 不含 Ω₁₀"的行数归零。
3. **门禁盲区（并入 #26/#35 的论证）**：这类 `文档 §小节` 悬挂**任何现有门都看不见**——
   符号门只认标识符、t2 只认文件路径。本次是人工脚本一次性查出 5 处，说明该维度值得进门。

### 48.3 检查器的适用边界（诚实标注）

`adr_index.md` 与 `docs/CONVENTIONS.md` 用**无编号标题** ⇒ 其小节集合为空，
对它们的 `§N` 引用在本判据下"要么全部判悬挂、要么无人引用"（本轮实测为后者）。
⇒ 若要把它俩纳管，需先给这两份文档引入编号标题或改用"按标题文本匹配"，属 #11/#26 落地时的设计约束。

## 49. 运维手册里的镜像坐标与人工清单精度（P117 波次，2026-09-22 现测）

**这一面的性质**：`docs/release/` 与人工验证清单属于"出事时被人逐条照抄"的文档，
**一处复制错的坐标在事故现场就是二次故障**。本轮从两个方向查：勾选框语义 + 命令可抄性。

### 49.1 勾选框普查（否证为主）

活文档（排除带日期证据面）含勾选框的 **36 份 / 未勾 363 / 已勾 323**。逐档读语义后判定：
- `agents.md` 16 个 `- [ ]` = §3.3.4 / §3.4.4 **开发检查清单模板**（本来就该留空）⇒ 非陈旧。
- `TUI_MANUAL_VERIFICATION_CHECKLIST.md` 23 个 `- [ ]` = **待人工在真实终端逐项核验**的协议项
  （文件头 `:5-6` 自陈"每项记录 ✅/❌"），语义正确；且该清单已在正文里带实测校正
  （`input/router.rs:248-249` 键位实况、`types.rs:263` 的 `ALL == REGISTERED_FOCUS_ORDER ∪ {InjectionStrategy}`、
  Timeline 于 Concord T1.4 接线、`R` 刷新实为 6 面板等）⇒ **未勾不等于未做，也不等于已完成**，无过度声明。
- `docs/architecture/audit/phase2/*`（91 勾 / 47 勾…）属 v1.x 阶段审计留痕 ⇒ 带日期正文，不改写。
- `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE.md`（149 未勾 / 69 已勾）是已判 superseded 的 v2.0 建造指南
  （带横幅，待 #7/#23 物理归档）⇒ 不动其勾选。
⇒ **结论：勾选框面没有"已做未勾 / 未做已勾"的系统性问题**；这一格关闭，避免后续波次再逐档重看。

### 49.2 真缺陷：GHCR 镜像名被误抄成仓库名（13 处，其中 runbook 11 处）

权威源 `.github/workflows/release.yml:34` `IMAGE_NAME: ghcr.io/${{ github.repository_owner }}/chimera-cli`
（owner 归一化小写的理由见 `release.yml:317-320`），而**仓库名是 `Chimera-CLI-`（带尾字符，`git remote -v` 实证）**。
文档把仓库名的尾巴一起抄进了镜像坐标：

| 文件 | 处数 | 面 | 处置 |
|---|---|---|---|
| `docs/release/rollback_runbook.md` | **11**（`:216,219,220,247,252,266,271,367,368,370,395`） | 活运维手册（回滚时逐条照抄） | **全量更正为 `chimera-cli:<tag>`** + 文件头加"镜像名校正"注（+485 B）；修后残留错误名 = 0 |
| `docs/release/post_ga_monitoring_checklist.md` | 1（`:17`） | 活监控清单 | 就地更正并附权威源指针（+325 B） |
| `docs/release/ga_sprint_ci_verification_report.md` | 2（`:39,121`） | 带日期 GA 验证报告 | **只加注不改写正文**（其后各插一行校正注，+494 B） |

**为什么严重**：`docker pull ghcr.io/yoloccyt/chimera-cli-:v1.0.4-omega` 不是"风格问题"，而是
`manifest unknown`——回滚 runbook 恰好在事故压力下被逐字执行，属于目标"可执行性"直接失败的面。

### 49.3 顺带精度修复

`TUI_MANUAL_VERIFICATION_CHECKLIST.md` 里 `` `view.rs:37` ``：行号**正确**（该行正是 `default_key: Some("]")`），
但同节 §4 有"路径前缀 `crates/chimera-tui/src/panels/`"的说明，而 `view.rs` 实际在
`src/actions/domains/view.rs` ⇒ 属**可解析性歧义**（同名文件多、前缀声明会误导）。改为完整路径并注明动作名在 `:39`（+77 B）。
⚠ 自纠：我第一版用全角括号做锚点导致命中 0 而 abort（未落盘）——**锚点须取自当轮 `Read` 的原文，不能凭语义重写标点后当它是原文**。

## 50. "未开工规划"落点可行性与现势计数的最后一片（P118 波次，2026-09-22 现测）

目标要求：**对未来未开始的任务，只保留经当前代码库验证可行的方向并标注"待实现"**。
本轮把这条做成两个机检问题：① 含未来标记的行是否引用**不存在的 crate/落点**；② 活文档里
"**现行/当前** + 数字"型的现势计数是否还成立。

### 50.1 未来引用不存在 crate：12 个命中，逐条判为"非缺陷"（该面已闭合）

对 `docs/architecture/**` + `docs/*`（排除带日期证据面与归档）166 份活文档扫描
"含未来标记行内的 `crates/<name>` 或 kebab 名"，得 12 种不存在名，**逐条读原句后全部合规**：

| 名字 | 出现处 | 判定 |
|---|---|---|
| `model-router`、`auto-dpo` | `ADR-172-…retirement….md:33`、`adr_index.md:16` | 标题即"退役"，是**指名退役对象**（否定语境） |
| `nexus-moe-router` | `fusion-concept-map.md:60`、`ADR-135-144:12`、`ADR-145-152:10/33` | **已有专章裁决**：`fusion-concept-map.md §2「未落地 / 缺失项清单」`逐条写"缺失（合入既有 crate）"+真实落点（`faae-router/src/tsr_moe.rs`、`sesa-router/src/sparsity.rs`），ADR-145/151 记**否决新建** |
| `tool_plan`、`on_demand_synthesizer`、`error_signature_collector`、`experience_card_bus` | 同上 §2 | 该表把它判为"**已落地**（非缺失）"并给 `path` ⇒ 反向防误删，正确 |
| `tract-onnx`、`blas-src`、`arc-swap`、`tokio-console` | `onnx-models.md:90`、`COMPLETE_BUILD_GUIDE:3208`、`CODE_WIKI:1411`、`DEEP_RESEARCH…:789` | 都是 **crates.io 依赖/可引入的第三方**，不是 workspace 成员 ⇒ 判据噪声，非缺陷 |
| `add-chimera-mas-subsystem`、`systematic-deep-analysis-optimization`、`audit-redundancy-deadcode`、`how-to`、`v3-engine` | ADR/INDEX/审计件 | 分别是 **spec 目录名 / 文档角色 / feature 名** ⇒ 判据噪声 |

⇒ **"未开工规划挂在死落点上"这一缺陷类在活文档中为空**；`fusion-concept-map.md §2` 就是这一面的单一真本
（它同时防两种错：把已落地项当缺失删掉、把缺失项当现状引用）。

### 50.2 现势计数：4 处真陈旧已修

| 位置 | 旧（现势声称） | 现测/改后 |
|---|---|---|
| `fusion-concept-map.md:56` | "现行 **43-crate** 代码库" | 改为"现行代码库（2026-09-22 实测 **41**，权威源 `Cargo.toml` `workspace.members`；43 为 v2.28.0 时点值）"（+94 B） |
| `DEEP_RESEARCH_34CRATE_OPTIMIZATION.md:14` | "**当前基线为 37 crate**(v2.4.0-omega)" | 改"现势基线不在此登记：以 `workspace.members` 为准（实测 41）；37 为 v2.4.0 时点值"（+93 B） |
| `DEEP_RESEARCH_LLM_ARCHITECTURE_MAPPING.md:17` | 同上 | 同上 |
| `DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md:14` | 同上 | 同上 |

**保留不动的两类**（附理由，防后续误"修"）：
- `DEEP_RESEARCH_OPTIMIZATION_ALGORITHM.md:848`"当前 1024 缓冲区在 34 crate 全部实现后是否足够"——
  属该报告 **2026-06 研究正文里的当时提问**，是带日期内容而非现势声称。
- `agents.md:662/663` 的 `<!-- v2.26.0-omega 同步: 2026-08-11 … 38 crate -->` 等同步标记——
  每行自带版本与日期前缀，读作历史锚点；按"计数只在权威源现取"的政策，这类注释里的数字
  不构成功能性声称（若要彻底清，属 #16 规则副本塌缩时一并处理）。

### 50.3 门禁

`check_doc_drift.sh` EXIT=0（`workspace_crates=41`）｜`check_doc_consistency.sh` EXIT=0｜
`check_doc_symbol_claims.py` PASS。四份被改文件行尾形态保持（`fusion-concept-map.md` 纯 LF、
三份 DEEP_RESEARCH 纯 CRLF，bareLF=0）。

## 51. 语义锚点全库化普查与「书写方言」修正（P120，2026-09-22）

**范围与量纲**：把 P119 只作用于 `CODE_WIKI.md` §3 卡片的「`path:line` 是否还指着它声称的东西」探针，扩到 5 份活文档、卡片外的章节级表格行。语料 = `docs/architecture/CODE_WIKI.md`(60) + `agents.md`(30) + `.claude/CLAUDE.md`(18) + `docs/tui/README.md`(14) + `docs/release/rollback_runbook.md`(2) = **124 条 `path:line` 断言**（分母随每轮打印，不待出「0」再补）。

**结论：真缺陷 0 条**，但**首版报出 33 条「硬缺陷」全部为仪器方言盲区**——即上一批写入 `scan-needs-planted-positive-selftest` 的「假阳性驱动你把正确文档改错」在本波以最大规模复现。四类必须支持的方言（逐类给出实证）：

1. **同行左侧 crate 续写裸名**：`CODE_WIKI.md:762` 一格内先写 `event-bus/src/types.rs:447`、再写「定级 `registry.rs:149`」⇒ 裸名属**同格前缀那 crate**。实测 `crates/event-bus/src/registry.rs:149` = `DpoPairGenerated => Normal, Knowledge;`（`:151`=AuditLogged、`:227`=RouterStatsReported 同理命中）。若按 basename 兜底会打开 `crates/chimera-tui/src/actions/registry.rs` ⇒ 把三条正确锚点判红。
2. **crate 内相对路径**：`CODE_WIKI.md:1690` 的 `archive/scheduler.rs:123` 实指 `crates/chimera-mas/src/archive/scheduler.rs`（第 123 行 = `InvariantChecker::check_inv8_archive_monotonicity(...)?;`，且该调用在 `pub fn trigger(`（`:113`）体内 ⇒ 文档「调用点 :123 的 `trigger`」成立）；`:15` 是该模块 `//!` 自述行，同样命中。
3. **裸仓库工件名（本波唯一实际改动）**：`ci.yml:311` / `audit.yml:65` / `release.yml:164,429,451` / `nextest.toml:37` 全库无同名文件在 `crates/` 下，新开发者按字面路径找不到落点 ⇒ 违反「可执行性」。已把 **17 处**补全为 `.github/workflows/<f>` 与 `.config/nextest.toml`：`agents.md` 7 + `.qoder/rules/chimera.md` 6 + `.claude/CLAUDE.md` 3 + `docs/release/rollback_runbook.md` 1。改后重跑同一探针，「文件缺失」桶 8→**0**。
4. **否定式引用**：`CODE_WIKI.md:1409` 引用 `sandbox.rs:474` 正是为了说它是**幽灵引用**（注释已移除）；`:711` 引用 `server.rs:97`/`backend.rs:23` 是标出旧设计名 `CoreOp`/`CoreEvent` 仅存的注释位置。豁免沿用仓库既有词（幽灵/不存在/旧引/旧名/已退役/零命中…），未新造词。

**符号绑定判据的两处必要放宽**（否则同样造出假缺陷）：① impl 块名不与方法同行 ⇒ `app/mod.rs:217` 落在 `make_panel` 的 match 臂、`cli.rs:154` 落在 `pub enum Commands`；② 同行多单元格 ⇒ 邻近符号取自**别的格子**（`omni_masks.rs:48` 邻近 `budget` 而该行是 `pub routing`，实测 `:48-56` 恰为五维字段全集，`OmniSparseMasks` 在 `:46`、`SparseMask` 在 `masks.rs:53`，全部与文档一致）。定型形 = 窗口 ±(8,2) 且接受 `::` 任一分段。

**抽样实证（9 条直接对拍代码，8 命中 1 需读上下文，全部与文档一致）**：`omni_masks.rs:46/48-56`、`masks.rs:53`、`quota.rs:85`+`:88`（镜像常量 MEMORY_BUDGET_MB / MEMORY_BUDGET_UTILIZATION）、`invariants.rs:87/95/275`、`event-bus/registry.rs:149/151/227`、`omega-learner/src/lib.rs:249`（`pub use s9_route::{... S9RouteLearner ...}`）、`chimera-cli/cli.rs:154`、`archive/scheduler.rs:15/123`。另 `nmc-encoder/src/fusion.rs:244-246` = `PerceptionInput::{Image,Video,Audio}`，与 `agents.md:467` 明写的「两套枚举不要混谈：领域面 `MultimodalInput`(`nexus-contracts/src/domain.rs:53`) 只有 Text」完全吻合。

**D 面口径复测**：`tmp/doc_char_baseline.py` 现值 **471,724 字**（P116 471,456 → +268，恰为 17 处路径补全的净增），比值仍 **98.9%**（上界 476,890 冻结不变）。⚠ 本波一度用**记忆里的 12 份清单**量出 418,137 字符 / 568,022 字节两个都对不上登记值——D 面的文件集合与单位口径**只认该脚本**，不得凭记忆重列（登记惯例：整数只写 §9 D 行、取整到百位）。

**仪器自纠（4 版）**：v1 卡片外行不解析（漏 27 条）→ v2 `cur` 不随非卡片标题复位，把 §5–§11 全部锚点记到 `nexus-app-server` 名下（假「文件缺失」25 条）→ v3 裸 basename 无全局阶梯（假 AMBIG）→ v4 加 5 级阶梯 + hint 方言 + 否定式豁免，才产出可信的 0。**教训固化**：单桶命中率 43.9% 时中止出数是对的方向，但「修到 0 缺陷」的过程本身要逐条人判留证，否则下一次只是把同一个仪器 bug 换个比例再犯。

**新增待裁决**：无（本波无代码/CI 工件改动，17 处均为文档锚点补全）。**既有 #35 追加一条实现要求**：若把锚点探针纳入门禁，判据必须内置上述四类方言 + 否定式豁免，且**多义一律 NEEDS_HUMAN 不出红**，否则门禁自己就是最大的假阳性来源。

## 52. 计划文档的"✅ 交付"与代码实态反向核对（P122，2026-09-22）

**触发**：核对目标里"未开始任务须标注待实现"这一条时，扫 `docs/tui/tui-refactor-implementation-plan.md` 发现其状态标记只有 6 个 `✅` + 2 个"已完成"、**零个** `- [ ]`/`待实现`/`未开始`，而该文件 25 处提及 W8~W11 ⇒ 形状本身可疑（一个"全部完成"的收官表最容易藏未落地项）。

**逐项对拍（判据 = 文件名在盘 + 符号命中数，两手段互证）**：

| 计划子项 | 文档声称 | 代码实测 | 判定 |
|---|---|---|---|
| T8.1 PaceGate | ✅ | `crates/chimera-tui/src/data/pace_gate.rs` 在盘 | 已交付 |
| T9.1-3 策展 | ✅ | `data/curator.rs` 在盘（`ContextCurator` 命中 1 文件） | 已交付 |
| T10.1/2 TuiHello 握手 | ✅ | `TuiHello` 14 处 + `TuiHelloAck` 10 处命中 | 已交付 |
| **T10.4 AgentTree 面板** | ✅（"第 26 注册面板"） | `panels/` 目录 31 文件内**无** `agent_tree.rs`；`AgentTree`/`agent_tree` 在 `crates/chimera-tui/src` 各 **0 命中**；`PanelId` 枚举 27 变体内无该项 | **未交付 = 待实现** |
| T10.5 INV-F/INV-K 守护 | ✅ | 以 `INV-K-A`/`INV-K-B` 形态在 `actions/codegen.rs:64,105`、`actions/descriptor.rs:80`、`actions/domains/export.rs:18` 落地 | 已交付（名字带后缀，勿按 `INV_F` 字面判无） |
| T11.1/3/4 生态 | ✅ | `actions/user_commands.rs`、`app/notify.rs`、`components/prefix_hint.rs` 均在盘 | 已交付 |

⇒ **W10 是半交付**，其余三波名副其实。`ADR-082`（文件名即 `ADR-082-tui-hello-agent-tree.md`）顶部**已有** 2026-09-22「实施状态校正」块写明"本波次实为半交付，委托树可视化 = 待实现" ⇒ **ADR 正确、计划文档陈旧**，二者此前互相矛盾，且 `agents.md §1.2/§8.1` 把该计划文档列为 TUI 重构参照，读者按它会把 AgentTree 当成已上线能力去调用。

**改动（1 份文档 3 处，均为把"已完成"改写为实态 + 指向权威校正块）**：`§10 收官表` W10 行 `✅ 交付`→`⚠ 半交付`（含四子项分别判定与 0 命中证据）；`M-Orch` 门禁行把 `AgentTree 第 26 面板` 划除并注明未达成，`INV-F/INV-K` 更正为实际形态 `INV-K-A/INV-K-B`；`§10 尾注` "面板 25 → 26（AgentTree）" 改为实测 `PanelId` 27 变体（末位 `InjectionStrategy`，不在焦点环）且**无 AgentTree** ⇒ 待实现。行尾 CRLF 保持、表格管道数逐处断言不变、反引号偶数。

**仪器自纠（本波 2 处，都是我自己差点制造假缺陷）**：
① 首版"存在性检查"用 `grep -rl "AgentTree"`，若我只凭它下结论会漏判近似名 ⇒ 补 `panels/` **目录列举** + `PanelId` 枚举解析两路交叉，三路同为 0 才允许写"未实现"（与 `subagent-audit-claims-need-retest` 第 3 类假阳性同型：命名相似 ≠ 同一实体，反向也成立）。
② 查"哪些 `W0~W11` 声称缺限定语"时，我的限定词表写成 `半交付|未在盘|待实现|W10.*未|AgentTree`，而盘上 `agents.md:285` 与 `CODE_WIKI.md:1634` 用的是「不等于清单内每项均已落码」⇒ 两条**已正确限定**的行被判成"★无限定"。这是"词表未从文档实际用词反查"的第 N 次复发；本轮因人工读原文而未产生错误改动，但该判据若进门禁即会造出假阳性。

**未动项与理由**：`agents.md:635`（`docs/tui/README.md` 行）与 `:662`（版本同步注释）仍写"W0~W11 收尾"不带限定 ⇒ 与同文 `:285` 的限定版**同文件共存**，但此两处是索引/元数据短指针，且同一文档内已有权威限定句；按"一个权威限定 + 其余指针"的既有惯例不再复制限定语（复制 4 处即成为新的漂移面）。**C 面比值未刷新**：本波净增约 1.1K 字 / 计划面 4,836,649 字 = +0.02%，在 0.1pp 登记精度之下，留待下次批量刷新（不为抖动而改数）。

## 53. 子代理指令面（`.qoder/agents/*.md`）首次审计 + 计数冲突扫描的判据失败（P124，2026-09-22）

**为什么这一面之前漏了**：本审计的指令面清单一直是 `agents.md` / `.claude/CLAUDE.md` / `.trae/rules/*` / `.qoder/rules/*`，而 `.qoder/agents/*.md`（6 份角色文件：architect / auditor / perf / rl / rust / security）**每次派子代理都会被注入**，却从未进入过任何一轮扫描。⇒ 教训：审计范围要按"**谁会读它**"划，而不是按目录惯例划。

**4 处现势计数缺陷（全部改为"指针 + 现测值 + 旧值标注"，不再写死数字）**：

| 位置 | 旧述 | 代码实测 | 处置 |
|---|---|---|---|
| `chimera-architect.md:30` | 「保持 **38 crates / 134 NexusEvent** 三方一致」 | `Cargo.toml` members=**41**、`types.rs` `NexusEvent`=**146**（与 `check_doc_drift.sh` 自报一致） | 改指针（3 个权威源）+ 标旧值失效；原句会让架构子代理去"维护"一个错的不变量 |
| `chimera-auditor.md:41` | 「**22 面板** FocusManager …四处断言」 | `PanelId` 枚举 **27** 变体（22 为 W0~W6 时点） | 改指针 + 现测值；四处同步点规则本身正确，保留 |
| `chimera-perf-engineer.md:28` | SLO 表「**22 面板** 循环 O(N)/O(22)」 | 同上 27 | 单元格改「O(面板数)」+ 现测值 |
| `chimera-rl-engineer.md:13` | 「**八接缝**（S1-S9）」+ 手列 8 个文件 | `omega-learner/src/lib.rs:101-138` 声明 **9 个** `pub mod s*`；手列**漏 `s9_integration.rs`** | 改指针 + 写明两个反直觉事实：**S7 无独立 `s7_*.rs`**（`lib.rs:23,154` 记其在 `r1_recall_quota`），**S9 有两个模块** ⇒ 按编号连续性猜文件必漏 |

**顺带排除的两个"假冲突"（先验证再改，避免把对的改成错的）**：
① `agents.md:169` / `chimera.md:200` 的「**六接缝**异步下发 `SelectorPolicy::Learned`」与上面的"9 个模块"**不矛盾**——`SelectorPolicy` 只在 `s4_selector.rs`(15 处) 与 `s9_route.rs`(7 处) 被引用，"六"是 v2.4.0 该变更条目的**当时接缝数**（版本化 changelog 语义），不是现势清单 ⇒ 不动。
② `chimera-security-engineer.md:21`「M0/M1/M2 共 9 属性」与 `agents.md` 的 7+1+1=9 一致 ⇒ 不动。

**计数冲突全库扫描：判据过宽，规模闸门触发 exit 3（未产出任何清单）**：语料 521 份 .md（排除镜像面/工具链/归档），5 个维度对权威值比对，命中 **1125 条** ⇒ 触发我预设的「>200 即怀疑仪器」闸门。根因两条，都属老错误的复现：
① **单位形状多义**：`(\d+)\s*crate` 会把「内环 **9** crate」「第 **35** 个 crate」「Week 7 **4** crate」「靠其他 **9** 个 crate」全当成"总 crate 数"——它把**子集计数/序数/事件波及数**混进了全集计数桶；分布表也印证：crates 维度出现 0~232 共 40 个不同取值，真实"总数"只可能是少数几个 ⇒ **一个维度出现几十个不同数值本身就是判据失能的证据**。
② **历史限定语正则写错**：`v2\.[0-2][0-9]` 想匹配 "v2.0.0"，但 `[0-9]` 吃不到第二个 `.` ⇒ 大量"v2.0.0-omega 变更"这类明确带版本时点的行未被豁免。
⇒ 修正方向（若要复用此判据）：只对**紧跟"共/合计/全库/全部"等总量词**且**位于同一小句**的数字判冲突，且豁免词表用 `v\d+\.\d+\.\d+` 重新写。**本次未据 1125 条改任何文档**——那就是制造破坏。

**新增待裁决**：无（4 处均为文档自身更正，未触碰代码/CI）。

## 54. 根 README 逐条实证 + `publish = false` 覆盖面错判（P125，2026-09-22）

**为何做这一面**：按 P124 教训（审计范围按"谁会读它"划），`README.md` 是新开发者读的**第一份**文件，其 4 条可证伪主张直接对应目标里的「可执行性」。**逐条实测结果**：

| README 主张 | 实测 | 判定 |
|---|---|---|
| `.\install.ps1 -SetupEnv` 可注入 env + GNU channel | `install.ps1` 含 `[switch]$SetupEnv` 形参；`install.sh` 在盘 | ✅ 真 |
| 产物名是 `chimera` 不是 `chimera-cli` | `crates/chimera-cli/Cargo.toml` 的 `[[bin]] name = "chimera"` | ✅ 真 |
| 子命令权威源是 `cli.rs` 的 `Commands` enum | `crates/chimera-cli/src/cli.rs:154 pub enum Commands`（本会话锚点探针已判 OK） | ✅ 真 |
| `cargo install chimera-cli` 不可用，**原因写作「`Cargo.toml` 设了 `publish = false`」** | 结论真、**机制错** | ❌ 已更正 `README.md:43` |

**机制错判全貌**：`publish = false` 实际只出现在根 `[package]`（`Cargo.toml:11`，包名 `chimera-e2e-tests`），**未进 `[workspace.package]`**，`crates/chimera-cli/Cargo.toml` 也没有该键 ⇒ 它**不覆盖** `chimera-cli`，`cargo publish -p chimera-cli` 不会被它拦下。真因只是"从未发布到 crates.io"。根因线索在 `Cargo.toml:5` 注释「`publish = false` 防止误发布」——**注释把自己作用的包范围说宽了**，我此前 4 处账本/事实表（`DOC-AUDIT-FACTS:711`、账本 `:215`/`:2108`/`:3302`/`:3874`）照抄了这个因果，属"把被解释对象当成被证明对象"。本波更正 `README.md:43` 与 `DOC-AUDIT-FACTS:711`；账本内 4 处为历史波次记录，按证据面规矩不回写，**以本节为准**。

**未做的验证（如实登记）**：没有实跑 `cargo publish -p chimera-cli --dry-run` 去证明"确实拦不住"——该命令会触发全量打包校验（本机 D 盘约 99% 满，`target/` 已 20GB+ 级），代价与收益不成比例。本波结论建立在 **manifest 层面直接证据**（`tomllib` 解析出的三个 `[package]` 表中 publish 键分布）之上，足以支撑"该键不覆盖 chimera-cli"这一否定式主张；但"未来某人真的误发布"属概率事件，不在可证范围。

**★ 新增待裁决（已入任务清单）**：把 `publish = false` 上提到根 `[workspace.package]`，使 41 个成员全部继承（一行改动、语义上正是 `Cargo.toml:5` 注释声称的效果）。属仓库工件改动故待授权；与既有 #22（LICENSE Apache-2.0 vs `license = "MIT"`）、#28（4 份 Cargo.toml 带 BOM）同属"发布面 manifest 治理"簇，可并批裁决。

**口径连带**：README 属 D 面文件 ⇒ 本波净增 268 字，`tmp/doc_char_baseline.py` 复测 **471,992 字 = 上界的 99.0%**（比值由 98.9% 进位到 99.0%，登记惯例不变：整数只写覆盖率报告 §9 的 D 行）。

## 55. 残余小面补审：`research/` · `tests/` · `crates/`（P126，2026-09-22）

**发现方式**：把 P124 的「按谁读它划范围」再执行一遍，按顶层目录清点 md 分布（排除工具链/镜像/会话产物后 `docs` 448 份 / 根 11 / `.qoder` 8 / `.claude` 2 …），暴露出 **3 个既不在审计语料、也不在用户排除清单里的面**：`research/cross_verification.md`(5.5K)、`tests/e2e/reports/rhi_cg_audit.md`(20.7K)、`crates/chimera-cli/README.md`(241B)。⇒ 范围表要按目录清点**反向核对**，不能只按「我记得有哪些面」列。

**逐面结论**：

1. `crates/chimera-cli/README.md`：唯一实质主张是「需 `cargo build --features mca` 启用 L10 `mca-gateway`」⇒ `tomllib` 解析其 `[features]` 得 `mca = ["dep:mca-gateway"]`，**主张为真**，未改。
2. `research/cross_verification.md`（2026-08-23 快照）：把 `classification.rs` 同时写进 §1 基线行与 **D-8 的证据位置**（`classification.rs:46-91`），而 `find crates -name classification.rs` = **0 命中** ⇒ 该裁决的复跑路径断裂。已加注现势指针：定级 = `crates/event-bus/src/registry.rs`（宏展开 `severity()` 见 `:59`）；mpsc 旁路 = `bus.rs:109 is_critical_mpsc_event()` 实 **14** 变体，规模由 `bus.rs:48 pub const CRITICAL_MPSC_VARIANTS: usize = 14` 守护（`FormalVerificationFailed` 于 L4 P1-1 使 13→14）；故 D-8 的「17 = 13 + 4」现势口径为 **「18 = 14 + 4」**（`bus.rs:50-52` 注释自陈）。
3. `tests/e2e/reports/rhi_cg_audit.md`（2026-07-26 快照）：「权威源」行按 `docs/architecture/<名>` 字面引两份文档，实测**均已迁移**——`NEXUS_OMEGA_v5_P5_实施计划文档.md` 在 `docs/architecture/_archive/2026-Q4/architecture/`（**归档件仍被称作权威源**），`NEXUS-OMEGA_v5.0_系统性完整设计文档.md` 在 `_blueprints/nexus-v5.0-design/`。已加注新路径与「归档不作现行权威」提示。

**顺带证实为干净的一面（负面结果留证）**：Critical/mpsc 指令面**无缺陷**——`.claude/CLAUDE.md:376` 写「等 **14 个**」、`.trae/rules/nuxus规则.md:169` 写「**14 个真实 NexusEvent**，规模由 `bus.rs:48 CRITICAL_MPSC_VARIANTS` 常量守护」，两者与代码实测 14 完全一致，且被引用的守护常量确在 `bus.rs:48`。我一度怀疑「某处仍写 13 个」，来源又是**开局上下文里的旧正文**（`agents.md:483` 现势已不写计数）⇒ 「上下文快照不是证据」第二次被证实。

**口径**：本波 3 份文件**均不在 D 面集合**（`tmp/doc_char_baseline.py` 的 12 份清单不含 `research/`、`tests/`、`crates/*/README.md`）⇒ D 不刷新，属如实而非漏刷。两处加注共约 +1.5K 字，落在证据面（按准据表只加注不改写），C/A 面计数不动。

**新增待裁决**：无。

## 56. 废弃改名计划的溯源、CODE_WIKI 目录树体检，以及一次自我证伪（P127，2026-09-22）

### 56.1 `.claude/plans/v1.5.7-omega-release.md`（P126 目录清点新暴露的面）

内容是一份**把产品品牌由 `chimera` 全局改名为 `chimela`** 的实施方案（Task 1 逐文件列改名点：README / Dockerfile / `install.sh` / `install.ps1` / `packaging/apt/*` / `release.yml` / 规则文件 / CLAUDE.md，基线版本 `1.5.7-omega`）。

**我第一版判词是错的，且被 git 证伪**：我据"现势 5 处证据"（`[[bin]] name="chimera"`、`install.ps1:226 BinName='chimera'`、`install.sh:42 BIN_NAME="chimera"`、`Dockerfile:6,42,44`、`packaging/` 不存在）写成「**从未执行**」，并把 `release.yml:451` 的 `|chimela` 分支归因于"本计划留下的死分支、待清理"。`git log -S"chimela"` 给出的时序推翻了两点：

- `9e3301b`(2026-07-13)「fix(install): 统一品牌名为 **chimela** 消除 Release 下载 404」⇒ 计划**被执行过**；
- `98c3bdf`(2026-07-15)「refactor(cli): 统一 CLI 命令入口为 **chimera**(aether->chimera)」⇒ 两天后**被回退**，且提交信息自陈「Compatibility: **chimela/aether 作为别名保留**，不破坏现有脚本」。

⇒ 正解：**执行后回退**（不是"从未执行"）；`release.yml:451` 的 `chimela` 分支是**刻意保留的兼容别名**（不是死分支，不该删）。教训：只凭"现势状态与文档不符"就写因果（"某计划留下的残留"）是**未经检验的归因**；文件状态只给"是什么"，**git 时序才给"为什么"**。凡是把现状解释成某历史动作的结果，必须先 `git log -S` 找引入提交。

**连带修正我自己此前的两处错判**（同一 `chimela` 事实，此前写作"属历史拼写残留"）：`agents.md:735`、`.qoder/rules/chimera.md:832` 已改为"commit `98c3bdf` 刻意保留的兼容别名…不得当残留删除"。**未新增待裁决项**（原判"删除 chimela 分支"随证伪一并取消）。

**该计划文件的处置**：`.claude/` 整目录在 `.gitignore:55`，文件**不受 git 跟踪也无历史**；且 `.claude/plans/` 语义上是"当前计划"目录，若被 agent 当作待执行计划读取会真的去改品牌 ⇒ 风险类别是**破坏性指令**而非陈旧计数，故在其顶部加 `⛔ 已回退——不得按本文操作` 横幅，逐条给 5 项现势证据 + git 时序 + 别名说明 + 版本口径以 `Cargo.toml` 为准 + 现行权威源清单（保留原位不删，符合生命周期政策）。

### 56.2 CODE_WIKI 仓库结构树：此前所有探针都看不见它

我的路径存在性探针**刻意跳过 ``` 围栏**（早期为免把模板示例目录树当路径断言，见 `scan-needs-planted-positive-selftest`）。代价是 `CODE_WIKI.md` 的**真实仓库结构树**（`:1484-1573`）从未被校验。本波专门对它做"顶层条目存在性"判据（只认行首即 `├`/`└` 的条目；⚠ 首版我把前缀 `│` 先 `replace` 掉再算深度，导致 44 条嵌套项被误报为"顶层缺失"——**深度信息就编码在被我删掉的字符里**）：21 个真顶层条目中 **2 个不存在**，另 2 处注释陈旧：

| 树内声明 | 实测 | 处置 |
|---|---|---|
| `Formula/chimela.rb`（Homebrew formula） | `Formula/` 不在盘 | 删除该行，替换为真实存在的 `.config/`（`nextest.toml`） |
| `bucket/chimela.json`（Scoop manifest） | `bucket/` 不在盘 | 删除该行，替换为真实存在的 `agents.md` |
| `tests/e2e/ # 11个E2E测试` | 实有 **32** 个 `.rs`（注册为根 `[[test]]`，现势 37 target） | 注释去计数，改指 manifest |
| `fuzz/fuzz_targets/` 列 6 个 target | 实有 **8**（缺 `harness_spec_parse.rs`、`sse_parse.rs`） | 补齐 2 行 + 标注"与 `fuzz/Cargo.toml` `[[bin]]` 及 `fuzz.yml` matrix 同步" |

顺带：`docs/configuration/token_efficiency_vendor_config.md` 的权威源 `crates/scc-cache/src/vendor_profiles.rs` 与 ADR-069 均在盘（无需改）；`docs/planning/ps2-4-*`、`docs/roadmap/omni-message.md`、`docs/adr/ADR-006-*` 均已带 2026-09 复测状态位（无需改）。

### 56.3 判据自纠：反引号全局奇偶在有围栏的文档里必然失真

改完 CODE_WIKI 后全局反引号计数 = 3057（奇）⇒ 乍看像我把代码跨度改坏了。按"逐行、且跳过围栏行"重判：**非围栏行奇数行数 = 0** ⇒ 内联代码跨度全部配对完好；奇偶失真是 ``` 围栏标记自身（每处 3 个反引号，部分围栏行还内嵌额外反引号）造成的。**定型**：校验 md 内联代码完整性只能用"非围栏行逐行奇偶"，不能用全文 `count("```")` 或全局反引号奇偶（此条已回写记忆 [[md-edit-fence-discipline]]）。门禁复验：`check_doc_drift.sh` / `check_doc_symbol_claims.py` 均 rc=0（441  occurrences / 7 gaps / 7 registered 不变）。

**口径**：CODE_WIKI 与 agents.md、chimera.md 均在 D 面清单 ⇒ `tmp/doc_char_baseline.py` 现值 **472,234 字 = 上界 99.0%**（P125 471,992 → 本波 +242；整数只登记在覆盖率报告 §9 D 行）。

## 57. 围栏内目录树全库体检（P128，2026-09-22）——此前所有探针的结构性盲区

**为什么这是盲区**：本审计的路径存在性探针为免误报，**一律跳过 ``` 围栏**（早期为防把"模板示例目录树"当路径断言，见 `scan-needs-planted-positive-selftest`）。代价是：**凡是画在围栏里的目录树，从未被校验过**——而 CODE_WIKI 的仓库结构树、CONVENTIONS 的布局树正是新开发者按图索骥的对象。

**做法**（先给验证器本身做植桩自检）：按 `行首 ├/└ 之前的 4 字符组数` 计算深度，用栈重建每条目的完整路径再判存在性。合成语料三断言：嵌套项不得判为顶层、兄弟项归属正确、假路径必须被抓出。
⚠ 验证器首版两次被自己的植桩拦下：① 树首行常是无连接符的裸 `crates/`，旧正则整行不匹配 ⇒ 根不入栈，其后所有路径挂到错误父级；② 裸名分支未 `rstrip("/")` ⇒ 产出 `crates//event-bus` 假缺失。**两次都在出数前 abort，未产出任何错误清单**。

**语料与分桶**（活文档 64 份含树、93 个树块、677 个"看似仓库绝对路径"的条目）：首版报 **196 缺失**，触发"整批可疑先怀疑仪器"判据，分桶后人判：

| 桶 | 量 | 性质 | 处置 |
|---|---|---|---|
| `.toolchain/**` 第三方 README 的 `examples/…`、`.tower/worktrees/**` 镜像副本 | 多数 | **语料泄漏**：我的排除式写 `"/.toolchain/"` 带前导斜杠，而 `as_posix()` 产出的路径是 `.toolchain/…` 无斜杠开头 ⇒ 根锚定路径全部逃过过滤（老错复发） | 修过滤器，不计缺陷 |
| `CONVENTIONS.md` / `CODE_WIKI.md` 的 `<crate>/tests/*.rs`、`src/*.rs` | 8 | **模板树（crate 标准布局）**，非仓库绝对路径 ⇒ 类别错误 | 不判存在性 |
| `九源手册 §10.1 工作区目录（终版）` 的 `crates/L0/…`、`Cargo.toml # 48 members` | 11 | **目标态设计**，且文档顶部档案横幅第 26 行已明写「48/53 crate 目标态重命名体系与现行命名/分层错位，**不可直接照单执行**」⇒ 已被治理 | 无需再标 |
| `CHIMERA_NEXUS_COMPLETE_BUILD_GUIDE §6.3` 等历史搭建指南 | 其余 | 带日期历史件，同类横幅承担 | 不改写 |
| **真缺陷** | 4 | 仅 `CODE_WIKI.md` 仓库结构树（P127 已修：`Formula/chimela.rb`、`bucket/chimela.json` 不存在 + e2e 计数陈旧 + fuzz target 少列 2 个） | 已修 |

**修后终态**：`CODE_WIKI.md` 仓库结构树 92 行 / **90 条目全部在盘**；`docs/CONVENTIONS.md:88` 那张自称「**实际示例**（`crates/nexus-core/`）」的树逐文件核对 **12/12 全在盘**（含 `benches/clv_bench.rs`、`tests/proptest.rs`、`tests/types.rs`）⇒ 该树可信。

**新增可复用判据（入记忆）**：目录树必须先分三类（模板 / 目标态 / 现势）再决定用什么判据；对"现势树"才跑存在性，且**排除式过滤器要同时匹配带斜杠与不带斜杠两种路径形态**（`as_posix()` 的顶层目录没有前导斜杠）。

**口径**：本波**未改动任何文件**（纯验证 + 一次语料过滤器修正），故 C/D/A 三面计数不变；账本 §133 记录过程与负面结果。

## 58. 文档门"有牙吗"实测：`check_doc_consistency.sh` 只做存在性检查（P130，2026-09-22）

**动机**：交付封口要求三门在终态绿。运行 `--selftest` 时 `cons_selftest=0` 看似通过，但该脚本**没有任何参数处理**（`grep -n 'selftest\|case\|getopts\|\$1'` 零命中）⇒ 未知参数被静默忽略，rc=0 只是普通运行 ⇒ **不能当作"有牙"的证据**。于是改用反证法实测。

**实验（植入-复跑-还原，单进程内完成）**：把 `docs/architecture/CODE_WIKI.md` 中第一处 `2.28.2-omega` 改成 `2.28.9-omega`（只改 1 处，其余正确串保留），跑门，随后按内存原字节还原并以 sha256 校验一致（还原 True，原文长 195,211 B）。

| 观测 | 结果 |
|---|---|
| 植入版本漂移后 | 门 **rc=0**，`[OK] three-way reconciliation all pass` |
| 还原后 | 门 rc=0 |
| 判定 | **该门对"文档侧版本漂移"无牙** |

**根因（机制级，不是猜）**：`scripts/check_doc_consistency.sh:138` 的判据是 `if ! grep -qF "${current_version}" "$f"` ⇒ 只要求**该文档至少出现一次正确串**；它从不检查文档里是否**另有不等于 canonical 的版本串**。自报覆盖面也印证：`[B2] 5/5 docs carry baseline string`、`[B1] 4/4 docs carry current crate count (41)`——在 413 份 `docs/` 活 md + 11 份根 md 的语料里，它检查的是 4–5 份的"含不含"，不是"有没有冲突"。

**为什么这条重要**：本审计花最多力气的缺陷类恰恰是**共存型陈旧**——同一文档里既有正确的 41 又有旧的「43 crate」「37 crate 现势基线」，以及 `2.28.2` 与 `v2.26.0 收尾` 并存。这类问题**门永远看不见**，因此三门的"绿"不能作为该缺陷类已收口的证据（此前我在 §12 完成度审计里把精确性判给 symbol 门与 drift 门，那条仍然成立：drift 门只管它登记的三个计数；本条是补充说明**未被任何门覆盖的缺陷类**）。

**对照组（证明另两门确有牙，非一律虚绿）**：
- `check_doc_drift.sh --selftest` = PASS，且其自检明写「re-locked baseline -> **stale doc declaration fails again**」「missing code target -> **exit 2 (cannot decide)**」⇒ 该门能抓"声明陈旧"并能区分"不可判定"。
- `check_doc_symbol_claims.py --selftest` = `RESULT: PASS (11/11 detectors hold)`，含 `t10-missing-crates-undecidable ok exit=2`。

**★ 新增待裁决 #44**：给 `check_doc_consistency.sh` 加**负向（排他）检查**——凡活文档出现 `[0-9]+\.[0-9]+\.[0-9]+-omega` 且 ≠ canonical 且该行无历史限定词 ⇒ GAP；crate 数同理（仅认"共/全部/members"邻接的总量词数字）。判据形状可直接复用 P124 的教训：**只判总量词同小句内的数字**，并内置否定式/历史豁免词表，否则门自己会产 1000+ 假阳性。属 `scripts/` 改动故待授权；落地前，**三门的 rc=0 不应被引用为"文档侧版本/计数已全局一致"的证明**。

## 59. 手工补齐 consistency 门缺的"排他检查"：共存型陈旧全库净判定（P131，2026-09-22）

**动机**：P130 证明 `check_doc_consistency.sh` 只做存在性判断（`:138` 的 `grep -qF`），看不见"同一文档里既有正确值又有旧值"。在 #44 获批前，这类缺陷**没有任何自动防线** ⇒ 本波把该判据当一次性探针手工跑一遍，给出当前净判定。

**判据（含 7 条植桩自检，全部通过）**：
- 版本：`\b\d\.\d{1,2}\.\d{1,2}-omega\b`，出现 ≠ `2.28.2-omega` 且该行无历史限定词 ⇒ 报；
- crate 总数：**只认总量词同小句内**的数字（`共|全部|所有|合计|members … N crate` 或 `N crate … 全覆盖|全部实现`），≠41 且无历史限定词 ⇒ 报；
- 语料 208 份（排除证据面 `docs/reports|audit|optimization|research|acceptance|performance|security|release` 与 `_archive`/`_blueprints`/镜像/会话产物）；规模闸门 >120 才 abort（本次 62，未触发）。

**结果：62 命中 → 经角色分诊后确认缺陷 = 0。**

| 命中类 | 数量级 | 为何不是缺陷 |
|---|---|---|
| `CHANGELOG.md` 的 2.20.0/2.24.0/2.27.1… 版本串 | 版本类多数 | 该文件**就是版本史**，列全部历史版本是其职能；把它判红等于要求版本史删掉历史 |
| `.claude/plans/v1.5.7-omega-release.md` 的 `1.5.7-omega` | 5 | P127 已在其顶部加 `⛔ 已回退` 横幅并逐条给证据，横幅本身即"历史限定" |
| `ADR-026:27`「34 crate」、`adr_index.md:207`「34/34」、`ADR-030:155`「35 crate」、`ADR-048:39`「37 crate」 | 4 | **ADR 正文记录决策时点事实**，按政策只加注不改写；改成现值反而破坏决策可追溯性 |
| `docs/architecture/audit/*`、`模块级优化分析报告`、`phase3-cross-layer-analysis` | 8 | 带日期证据面（同第一类） |
| `DEEP_RESEARCH_NEXUS_OMEGA_architecture_review.md:331`「全部 43 crate」 | 1 | 该文档顶部有 **2026-09-21 时效校正横幅第 2 条**：「本文按「43 crate」通篇计数，现 41」⇒ 已显式治理，且第 7 条说明其价值保留故**不归档** |
| `CLAUDE.md:289` 提到模板里的 `1.0.0-omega` | 1 | 该行自身就写「该行**不得**当版本权威」（我的限定词表缺 `不得` ⇒ 假阳性） |
| `tui-suite-architecture.md:488`「L1-L9 共 17 个 crate」、`layer-7-3-8…:475`「共 10 crate」 | 2 | **子集计数**：`共 N crate` 形态天然歧义（既可是全库也可是本报告覆盖的层子集）⇒ 总量词判据在这一形上失效 |

**给 #44 的三条实现约束（否则门自己会产 62 条噪声）**：
1. **文件角色豁免优先于词表**：`CHANGELOG.md`、`docs/reports/**`、`docs/audit/**`、ADR 正文属"按设计含历史值"的角色，应整体排除出排他检查，而不是靠行内关键词猜；
2. **`共 N crate` 不认定为全库总数**——只有 `workspace\.members`、「全库/整个 workspace N 个 crate」这类**闭合形**才判；且需带 `43→41` 一类的"已知曾为 X"豁免；
3. 历史限定词表必须含 `不得|勿|以…为准|以现取|现测|时效校正|横幅`（本波两处假阳性都是缺 `不得` 与"横幅已在别处"这一事实）。

**净结论**：本审计的"共存型陈旧"缺陷类，在**活指令面 + 架构面**范围内已清零；剩余含旧值的位置全部由**角色（版本史/ADR/证据面）或已落地的时效横幅**承担。#44 落地时若不做上述三条豁免，会把这 62 条全报成缺陷——那将是"门越努力、工件越错"的重演。

**本波未改任何文件**（纯验证）。
