# W0 基线固化与可回溯性补齐 — 进度报告（最终版）

> **批次**：W7 续作 / W0，依据已批准方案 `C:/Users/30324/.workbuddy/plans/radiant-thunder-darwin-xenXoOpR.md`（13 节）§5 W0。
> **执行模式**：Agent / Craft + Unmetered + Staged；4 人精英专家团队并行分工，写操作由 team-lead 统一串行执行。
> **报告状态**：本轮 W0 阶段性收口报告。全文经一次重写，消除前几版因误读框架完成通知而产生的归属矛盾。
> **审计基准**：当前主检出工作树现态，排除 `.qoder/worktrees/`、`.tower/`、`tmp/`、`target/`（K12 / `AGENTS.md §2.8`）。
> **证据纪律**：所有数值标注「实测 / 报告自述 / 静态文本 / 不可判定」；判 `.md` 存在性一律用 `ls` / `test -f` / `git ls-files`（`Glob` 遵循 `.gitignore:68` 的 `*.md` 排除，会漏报）；依 ADR-234「证据先于断言、低报与高报同属错误」。

---

## 0. 结论摘要（先看这里）

1. **当前工作树并非全绿**。独立复跑发现 `check_perf_redlines.py inventory` 与 `all` 均 `EXIT=1`，唯一红项是 `gqep-executor/toolplan_layers` 未登记（`unknown=1`）。这与收官报告 `§3.1` 自述的「`unknown=0` / VERDICT PASS」**不符**。
2. **该红非本轮引入**。根因是 HEAD 之前的提交 `95376f8`（2026-11-06 20:08:29）新增了该 bench 与 `Cargo.toml:68-69` 声明，却未登记任何性能台账。
3. **本轮实际落码 2 处，均为纯注释**（R5）：`Cargo.toml:652` 文档指针日期笔误、`composition.rs:165-168` 过时的 `delegate = None` 回滚注释。行数守恒、零 Rust 逻辑改动。
4. **落码后三重验证全部通过**：门禁回归 7/8 全绿且 `inventory` 读数与落码前逐字一致；`cargo check -p chimera-cli --lib` EXIT=0（45.17s）；`cargo fmt --check` EXIT=0；手工逐行 diff 复核通过。
5. **未执行任何 git 写操作**（不 `add` / 不 `commit`），不擅自改 `.github/workflows`（ADR-234 A4），不编造覆盖率数值填表。5 项用户在途改动原样保留。
6. **W0 不需要跑新性能基准**：W0 无 Rust 源码改动，全部结论落在 L1 结构判据；跑 criterion 会把现有 53 份 `new/estimates.json` 轮转成 `base`，直接破坏取证基线（ADR-234 A5「测量态与提交态必须同一棵树」）。
7. **需用户裁定 4 项**（Q-A ~ Q-D，见 §8），在此之前 W1 代码改动不启动——这是「严格优先级、杜绝短期行为」的要求。

---

## 1. 基线与证据链（实测）

| 项 | 实测值 | 取证命令 |
|---|---|---|
| 分支 | `feat/toolchain-parallel-optimization` | `git branch --show-current` |
| HEAD | `64750279f4a3a564c2efa29c853ccf2a2a1d459e`（`6475027`，ADR-244/245 收口） | `git rev-parse HEAD` / `git log -1 --oneline` |
| 版本 / edition | `2.29.0-omega`（`Cargo.toml:93`）/ 2021（`:94`） | Read |
| cargo / rustc | `1.97.1 (c980f4866 2026-06-30)` / `1.97.1 (8bab26f4f 2026-07-14)` | `cargo --version` / `rustc --version` |
| Python | 3.13.14 | `python --version` |
| linker | `gcc`（`.cargo/config.toml` 固化为名称）；`D:/msys64/mingw64/bin/gcc.exe` 存在、在 PATH | `test -f` / `command -v gcc` |
| 落码前工作树 | 5 项 `M`、0 未跟踪；`5 files changed, 35 insertions(+), 3 deletions(-)` | `git status --short` / `git diff --stat` |
| 落码后工作树 | 7 项 `M`（原 5 项 + `Cargo.toml` + `composition.rs`）、0 未跟踪 | `git status --short` |
| `Aegis` 子模块 | `-fb05c618…`（前缀 `-`）；子模块自身 HEAD `e5172dd` ≠ 记录指针 `fb05c61` | `git submodule status` / `git -C Aegis log` |
| 快照时间 | 2026-11-06 21:09:16 +0800 | `date` |

### 1.1 基线快照（W0.2 交付物）

- **唯一索引**：`tmp/w7_baseline_6475027.md`（方案 §5 W0.2 指定载体）。
- **原始 diff 附件**：`tmp/w0-inflight-2026-11-06.diff`，6840 字节、5 段 `diff --git`、SHA256 `69dcebd12825c6c565f3c161116299fa4c893663ce80cdc2b4f16328e5a0b255`（源 `/tmp/w0_inflight.diff` → 首次落盘 → 移入 `tmp/` 三处哈希一致）。
- 受 `.gitignore:17:tmp/` 忽略 ⇒ 不产生 `??` 噪声。初版曾落在 `docs/reports/baselines/`，由 team-lead 自查发现会污染工作树且造成双载体分裂，用 `mv`（非删除，数据保全）合并；`docs/reports/baselines/` 已恢复原有单文件状态。
- 5 段内容：`.dockerignore`（新增 `.mimosa/`/`.tower/`/`tmp_podman/` 忽略）、`.github/workflows/ci.yml:292-295`（新增 `Workflow YAML lexical guard (G-141, G-142)`）、`.gitignore`（新增 `coverage/` + 5 个本地工具状态目录）、`.trae/rules/nuxus规则.md`（4 行）、`Aegis`（子模块指针）。

### 1.2 在途改动的 CI 噪声排除（交叉核验）

| 检查项 | 实测 | 判定 |
|---|---|---|
| `ci.yml:292-295` 引用的 `check_workflow_yaml_lexical.py` | 存在，8074 B，`git ls-files --error-unmatch` = TRACKED | 无断裂 |
| `check_gates_provenance.py` | 存在，17857 B | 无断裂 |
| `Dockerfile:38-40` 的 COPY 契约 | 仅 `Cargo.toml` / `Cargo.lock` / `.cargo/config.toml` / `crates/`；新增忽略项均不在 COPY 面 | 无冲突 |
| `coverage/` 忽略 | `git check-ignore -v` 命中 `.gitignore:11:coverage/` | 生效 |

⇒ 5 项在途改动**不会**成为 W0 门禁复跑的干扰源，基线可归因。

---

## 2. 团队组织与优先级跟踪

| 优先级 | 任务 | 负责角色 | 结果 |
|---|---|---|---|
| P0 | 基线固化与门禁独立复跑 | `gate-verifier`（12y 构建/质量保障） | 完成；门禁实测由 team-lead 亲跑复核 |
| P0 | 性能基准与证明力分级 | `perf-analyst`（12y 性能工程/测量学） | 完成；53 份 `estimates.json` 由 team-lead 独立重采 |
| P1 | 可回溯性与台账漂移修复 | `governance-auditor`（12y 治理审计）→ team-lead 串行落码 | **部分完成**：R5 已落码，R1–R4 因治理约束推迟（§7） |
| P2 | 质量审查与架构合规两阶段复核 | `arch-reviewer`（15y 首席架构师） | 完成；两阶段清单见 §6.3 |
| P3 | 回归验证与进度报告落盘 | team-lead | 完成（本报告） |

**证据口径说明**：本会话收到的子代理消息以框架完成通知（任务名 + 耗时）为主；**唯一可逐字核对的专家正文**是 `gate-verifier` 的「W0.5 待确认点关闭」收口同步（内容见 §7.2）。因此本报告的**全部数值、门禁读数、静态核对、落码与裁决，均以 team-lead 实际执行的命令输出为第一证据源**；专家正文仅在明确标注处作为补充。此前几版曾把完成通知误读为报告正文并产生虚构引用，已通过本次全文重写彻底清除，不作保留、不引用。

---

## 3. P0 门禁独立复跑 —— 真实读数（team-lead 亲跑）

### 3.1 性能红线三模式

| 命令 | EXIT | 关键输出 |
|---|---:|---|
| `check_perf_redlines.py inventory` | **1** | `counts: total=125 gated=11 registered=51 dev-only=62 unknown=1`；`RESULT: FAIL (1 finding(s))` |
| `check_perf_redlines.py invariants` | 0 | I1 75/75、I2 1≤1、I3 53、I7 53 对、I9 75 键、I4a/I4b/I5/I6/I8/I8b/I8c 全 PASS；`RESULT: PASS (all invariants hold)` |
| `check_perf_redlines.py all` | **1** | `VERDICT: FAIL (rc=1, failing_submodes=['inventory'])`；`#[ignore]` ownership gate PASS（79 ignored，manual-only 12 / slo-blocking 13 / slo-daily 54） |

红项精确定位（第 55 行）：`[FAIL] inventory unknown    gqep-executor/toolplan_layers - register it as run_bench or add to bench_inventory_freeze.txt`。

### 3.2 治理门禁集（全绿）

| 命令 | EXIT | 关键输出 |
|---|---:|---|
| `check_ci_manifest_parity.py` | 0 | `workflows=4 ci_refs=141 ci_steps=70 manifest_gates=142 matched=69 allowlisted=1 unmatched=0 gate_missing=0` |
| `check_config_sample_parity.py` | 0 | 模板 165 行 / 14 根级键；`ChimeraConfig` 15 字段；样例逐字等值 |
| `check_composition_root.py` | 0 | canonical assembly points 7；baseline-covered bypasses 0 |
| `check_crate_reachability.sh` | 0 | 46 members / 生产可达 38 / unreachable 0 / GATED 8 |
| `check_topk_registry.py` | 0 | 11 active entries，4 live shapes within baseline |
| `check_lint_ratchet.py` | 0 | 46 crates scanned / opted in 46 / ledger 46 / 0 offenders |
| `check_doc_drift.sh` | 0 | `nexus_event_variants=154, event_topic_variants=10, workspace_crates=46; 0 registered drift(s)` |
| `audit_commit_coupling.py` | 0 | 「退 0 —— 无 REF_UNTRACKED，0 条同批提示」 |
| `check_event_ownership.py` | 0 | variants=154 / 有生产态构造=136 / 无=18 / 账本=18 |
| `coverage_gate.py --validate-config` | 0 | `min ratchet monotonic over 1 committed values (last=85)` |
| `check_dependency_rules.sh` | 0 | 依赖铁律通过 |
| `check_workflow_yaml_lexical.py`（在途改动新增门） | 0 | 通过 |

### 3.3 不可判定项（如实记录，不伪造绿）

| 项 | 状态 | 原因 |
|---|---|---|
| `run_gate_manifest.py` | **不可判定** | 两次独立执行均被沙箱拦截：`PROGRAM BLOCKED BY SECURITY POLICY - wsl.exe` 在程序黑名单。该脚本内部调用 `wsl.exe`，本环境禁止且**不可申请绕过**；命令以 `SIGTERM` 结束、无任何输出。⇒ 不记录 `ok/mismatch/env-fail` 数值，不宣称绿也不宣称红 |
| `coverage_gate.py --check` | **不可判定** | Windows-GNU 宿主缺 profiler runtime（E0463），ADR-234 A4 `:61` 明确归 CI Linux 裁决 |
| `cargo llvm-cov` / fuzz | 不可判定 | 同上；libFuzzer 仅适配 MSVC |
| `pwsh scripts/check_dependency_rules.ps1` / `check_doc_consistency.ps1` | 未执行 | 本轮用 `.sh` 版覆盖；PowerShell 版待 CI |

### 3.4 与收官报告自述的对拍

| 项 | 收官报告 §3.1 自述 | 本轮实测 | 结论 |
|---|---|---|---|
| `inventory` | `unknown=0` EXIT=0 | `unknown=1` EXIT=1 | **不符** |
| `all` | `VERDICT PASS` | `VERDICT FAIL` | **不符** |
| `invariants` | I1 75/75、I2 1≤1、I7 53、I9 75 | 完全一致 | 符合 |
| `run_gate_manifest` | `ok=135 mismatch=0 env-fail=0 unknown=0 skip=7` | 沙箱拦截，不可判定 | 无法对拍 |
| `check_crate_reachability` | EXIT=0 | EXIT=0，46/38/0/8 | 符合 |

⇒ 收官报告的性能判据读数**早于 `95376f8` 提交**或漏计该项，属已过时的时点值；本轮以实测为准，并把它登记为 W0 基线债务（§4）。

---

## 4. 关键发现 D-1：`toolplan_layers` 未登记（基线自带红）

### 4.1 根因（完整证据链）

- 文件：`crates/gqep-executor/benches/toolplan_layers.rs`，2848 B，2026-11-06 17:31，`git ls-files --error-unmatch` = **TRACKED**。
- 声明：`crates/gqep-executor/Cargo.toml:68-69` 有 `name = "toolplan_layers"` / `harness = false`。
- 提交：`git log -1 --format=... -- <file>` → `95376f8 2026-11-06 20:08:29 +0800 perf(gqep): 计划执行器分层串/并发化与不变量守护`；`git show --stat` 显示该提交改 5 文件（含 `Cargo.lock`、`Cargo.toml`、bench、`toolplan_runner.rs`、proptest），**未触及任何性能台账**。
- 四态判定（`check_perf_redlines.py:145-170`）：`gated`←`perf_redlines.toml` 引用；`registered`←`[[run_bench]]`；`dev-only`←`bench_inventory_freeze.txt`；否则 `unknown → FAIL`。实测四个目标文件 `grep toolplan_layers` **全部无登记** ⇒ 落入 `unknown`。
- bench 内容（头部 28 行）：测「PlanRunner 只读层 8 个 `ToolCall` 有界并发，`concurrency=1` vs `8`，mock executor 每次 `sleep(2ms)`」，理论墙钟 16ms → 2ms。**是有真实性能语义的基准，不是实验性探针**。

### 4.2 两条合法修复路径及代价（不擅自裁决）

| 路径 | 动作 | 代价 / 风险 |
|---|---|---|
| A：登记 `dev-only` | `bench_inventory_freeze.txt` 增一行（`crate\|bench\|dev-only\|理由 + ADR 号`） | 该台账头注释「只减不增：新增一行须附理由 + ADR 号」；把一个有 SLO 意义的并行基准声明为「无 SLO 开发基准」**等于降低门强度**，与 ADR-234 A4 精神冲突 |
| B：升格 `registered` | `perf_thresholds.toml` 加 `[[run_bench]]` + `[thresholds_ns]` + `[path_keywords]`，`bench_check.yml` heredoc 双写 + `run_bench` 行 | 属已批准方案 **W5.0 阈值双源治理**范畴；改 `.github/workflows` 须先贴片段征求确认（ADR-234 A4 `:58-60`）；新键名须避 I2 子串冲突（当前重叠 1 对、cap=1 已顶格）；I7/I9 须同步 |

⇒ **本轮不修**，登记为 **W5 待办 + 待确认项 Q-D**。W0 越过基线阶段改 CI 数值面属短期行为。该红在 R5 落码前后读数逐字一致，已证明与 W0 改动无关。

---

## 5. 关键发现 D-2：`hnsw_tiered` I9 幽灵门（W5 范畴，本轮不碰）

静态文本核对（team-lead 独立 python 解析）：

| 判据 | 实测 |
|---|---|
| `perf_thresholds.toml [thresholds_ns]` / `[path_keywords]` | 各 **75** 键，双向差集空 ⇒ **I1 绿** |
| `[[run_bench]]` / `bench_check.yml` `run_bench` 行 | 53 / 53 ⇒ **I7 绿** |
| heredoc `THRESHOLDS_NS` | 75 键，与 TOML 差集空 ⇒ **I9 绿** |
| heredoc `PATH_KEYWORDS` | **74 键，缺 `hnsw_tiered`** |
| I2 关键字重叠 | **1 对**（`diff_engine_bench`/`writer_ansi`，因 `diff_incremental_5pct` 含 `incremental_5pct`），cap=1 已顶格 |
| `dev-only` 数据行 | 62 条（`bench_inventory_freeze.txt`），floor 60 ⇒ 余量 2 |
| G-27 跨 await 豁免 | `lock_await_freeze.txt` / `await_guard_allowlist.txt` 各 9 条，本轮不得新增第 10 条 |

`bench_check.yml:295` 有 `"hnsw_tiered": 2_000_000`，但 heredoc `PATH_KEYWORDS`（`:389-501`）无该键；断言循环按 `PATH_KEYWORDS.items()` 做 first-match-wins 归类 ⇒ `bench_results["hnsw_tiered"]` 恒空 ⇒ 打印 `⚠ no estimates found (skipped)` 并 `continue` ⇒ **该 2 ms 阈值在 CI 永不被断言**。`perf_thresholds.toml:14` 注释自陈此事（「R2 M44 实例：本表有 hnsw_tiered 而 heredoc 无」），说明是 M44 从 `THRESHOLDS_NS` 缺键迁移成 `PATH_KEYWORDS` 缺键的残留。

- **性质**：静态代码逻辑推演（非运行时观测）；I1/I9 实现只抓 `THRESHOLDS_NS` dict，故两不变式覆盖不到——这是「绿但覆盖不全」，不是「红」。
- **裁决**：本轮不修，归 W5.0 + 待确认项 Q-C。该缺陷**非 W0 引入**（`bench_check.yml:270-277` 的 2026-10-26 R2 订正注释与 `perf_thresholds.toml:6-14` 的 R1-T6 切流未完成登记均早于 W0）。

---

## 6. 性能基准与证明力分级

### 6.1 实测数据（53 份真实 `new/estimates.json`，team-lead 独立 python 只读解析）

读取 `median.point_estimate`（ns），与 `bench_check.yml` heredoc 断言口径一致。关键组：

| 键组 | 路径 | 实测 (ns) | 报告宣称 | 对拍 |
|---|---|---:|---:|---|
| dispatch_floor_echo | `dispatch_layers/floor_echo_transport` | 58.364 | 58 ns | ✓ |
| dispatch_policy_eval | `dispatch_layers/policy_evaluate_allow` | 464.521 | 465 ns | ✓ |
| dispatch_sandbox_cached | `dispatch_layers/sandbox_check_seccore` | 1178.836 | 1179 ns | ✓ |
| dispatch_sandbox_uncached | `dispatch_layers/sandbox_check_seccore_legacy` | 3094.394 | 3094 ns | ✓ |
| dchain_allowall_hit | `dispatch_chain/chain_allowall_route_hit` | 538.783 | 539 ns | ✓ |
| dchain_seccore_hit | `dispatch_chain/chain_seccore_route_hit` | 1698.094 | 1698 ns | ✓ |
| dchain_seccore_legacy | `dispatch_chain/chain_seccore_route_hit_legacy` | 3694.059 | 3694 ns | ✓ |
| dchain_policy_deny | `dispatch_chain/chain_policy_deny` | 73.287 | 73 ns | ✓ |
| tbatch_serial_zero | `tool_batch/serial_ms0/64` | 145824.253 | 146 µs | ✓ |
| tbatch_parallel_zero | `tool_batch/parallel_ms0_bound8/64` | 165227.437 | 165 µs | ✓ |
| tbatch_serial_slow | `tool_batch/serial_ms1/64` | 493366850 | 493 ms | ✓ |
| tbatch_parallel_slow | `tool_batch/parallel_ms1_bound8/64` | 111888700 | 112 ms | ✓ |
| cli_startup_e2e | `cli_startup_e2e/parse_classify_runtime` | 961329.870 | 982 µs | 差 2.1% |
| cli_startup_runtime (multi) | `cli_startup_runtime/build_multi_thread` | 765579.774 | 783 µs | 差 2.2% |
| cli_parse_run | `cli_parse/run` | 106329.868 | 110 µs | 差 3.3% |
| mcp_registry_lookup | `mcp_registry/list_alive_100`（最大组） | 10618.588 | 10.6 µs | ✓ |
| bus_bench | — | **无数据** | 55 µs | 仅文档注释，无 `new/estimates.json` 载体 |

**算术复核（非新测量）**：沙箱缓存 `(3094.394−1178.836)/3094.394 = 61.91%`；链级 `(3694.059−1698.094)/3694.059 = 54.03%`；批量 `493366850/111888700 = 4.409×` ⇒ 与 −61.9% / −54.0% / 4.4× 全部吻合。

**测量学处置**：4 个宣称值与快照差 2.1–3.4%，远小于 ADR-234 记载的**冷启动首轮 29–33%** 与组内带宽 2.1–8.0% ⇒ 该量级**不构成性能回归或改进证据**，属「台账口径与快照的溯源差异」（W0 文档治理项），不得据此宣称性能退化，也不得据此改数值。

### 6.2 证明力分级（ADR-234 A1）

| W0 结论类别 | 载体 | 最高层 |
|---|---|---|
| I1/I9/I7 键集对等、`run_bench` 53 行、heredoc `PATH_KEYWORDS` 缺键、门禁 EXIT | 静态文本 + 命令输出 | **L1 结构判据** |
| 棘轮余量、`slo.yml` 观察期状态、CLI 脚本口径 | 静态文本 / 环境探测 | **L1** |
| 四环链 58/465/1179 ns、−61.9%、4.4×、CLI e2e | 53 份真实 `new/estimates.json` | **L4 criterion**（既有改前快照） |
| ADR-234 四层判据定义本身 | 主检出无载体 | **不可判定**（原文仅在未并入 main 的 worktree） |

### 6.3 「W0 是否需要跑性能基准」——结论：**不需要**

理由（依 ADR-234 与已批准方案）：
1. W0 定义即「无 Rust 源码改动」，被测二进制未变；新 bench 只产生运行间噪声，不改变任何 W0 结论真值。
2. W0 全部结论落在 **L1 结构判据**，静态文本即可完全判定；ADR-234 禁止用「测试全绿」顶替 L1，反之 L1 结论也无需 L2/L3/L4 背书。
3. 唯一涉及性能数值者已有 53 份真实 **L4** 证据可直接取证。
4. 在 ADR-234 噪声下限下，2.1–3.4% 差异即便重跑也不构成证据；为形式跑全量反而制造不可归因差值。
5. **跑 criterion 会写 `target/criterion/**/new/*.json` 并把现有 53 个 `new` 轮转成 `base`，直接破坏本报告取证基线**（违背 ADR-234 A5「测量态与提交态必须同一棵树」）。
6. 性能基准只在后续真正改 Rust 源码的波次（W2/W4/W5）才需重跑，且应在 CI 侧按 `bench_check.yml` 的 53 个 `run_bench` 执行。

⇒ 用户要求的「每个步骤完成后进行性能基准测试」，在 W0 无性能敏感改动的语境下，正确落实方式是 **L1 结构判据 + 既有 L4 快照对拍 + 明确记录不适用理由**，而非强行跑 bench。强行套 criterion 属「用 L4 冒充 L1」的误用。

### 6.4 两阶段质量审查清单（team-lead 依 `AGENTS.md` / `CONTRIBUTING.md` 自建）

**阶段① 规格合规**（Critical 项任一不满足即阻断）
- [x] 所有改动严格落在 §5 W0 的 8 个工作项内，未触碰 W1+ 的 Rust 源码
- [x] 未新增 feature flag、未新增运行时安全开关（`ADR-242 §3.5`）
- [x] 未改 `.github/workflows`（ADR-234 A4 要求先征求确认）
- [x] 未改 `perf_thresholds.toml` / `slo.yml` 数值面（归 W5）
- [x] 每项改动附 `文件:行号` 与验证命令
- [x] 文档与代码同批，无悬挂引用（R5 的日期笔误已修，指针可解析）
- [x] 未触碰 `scripts/doc_count_freeze.txt`（G-37 已绿，且 W0 无新增 `NexusEvent` 变体）

**阶段② 代码/文档质量**
- [x] 注释仅在 WHY 不明显处添加，无冗余解释（`AGENTS.md:23`）
- [x] 无重复代码、无兼容垫片、无不可能场景的防御代码（`:24-26`）
- [x] 命名 / 错误模式 / 分层与既有约定一致
- [x] 依赖方向 `L(N)→L(N-1)` 未 inversed（`:182-197`）
- [x] OMEGA 对齐：主 **Ω₉-Preserve**（棘轮/回滚/双源对拍）、**Ω₈-Assess**（证明力分级），次 Ω₄-Event、Ω₇-Locate
- [x] 测试守恒：既有测试零删除（`:231/:304`）；本轮 W0 无新增功能代码，故不新增测试用例

**审查结论**：Critical 0 / Major 0 / Minor 0。唯一待办是 Q-A~Q-D 的用户裁定，不构成代码缺陷。

---

## 7. P1 落码内容与未落码项

### 7.1 已落码：R5 两处纯注释（精确 diff）

**改动 1 — `Cargo.toml:652`**：修复引用文件名与盘上不符（`docs/reports/L0-contracts-residual-debt_2026-10-14.md` MISSING，盘上实为 `_2026-10-13.md`，53120 B）。`:651` 同行自述退役日期即 2026-10-13，属注释日期笔误，**非报告丢失**。
```diff
-# 退役理由见 docs/reports/L0-contracts-residual-debt_2026-10-14.md §2:该 target 4/5 用例
+# 退役理由见 docs/reports/L0-contracts-residual-debt_2026-10-13.md §2:该 target 4/5 用例
```

**改动 2 — `crates/chimera-cli/src/composition.rs:165-168`**：原注释写「传输环当前是未接线状态（`delegate = None`）」，与同文件 `:277` 的 `Some(Arc::new(BuiltinToolExecutor))` 矛盾（ADR-244 D1 已收敛）。按 `CONTRIBUTING.md:90` 两账分开原则，这不算功能缺口，属文档漂移。
```diff
 /// # 回滚
-/// 传输环当前是未接线状态（`delegate = None`），派发会如实返回
-/// `transport: tool executor not wired: ...`。要真正执行工具，需上层注入
-/// 生产 `ToolExecutor`（见 ADR-240 的后续批次）。
+/// 传输环已在组合根唯一装配为 `Some(BuiltinToolExecutor)`（ADR-244 D1），
+/// 四环 policy→sandbox→route→transport 真实生效；回退该 delegate 即回到
+/// ADR-240 D3 的 fail-closed 恒拒绝态，不涉及任何安全层降级。
```

**落码纪律**：保持 4 行 → 4 行、1 行 → 1 行（`-3/+3`、`-1/+1`），避免 G-97 附录 4A 的 `composition.rs:NN` 证据行号漂移；`Cargo.toml` 总行数 761、`composition.rs` 总行数 698，落码前后守恒；仅改注释，零 Rust 逻辑、零数值面、零 CI 文件。

### 7.2 未落码项及理由（严格按优先级与治理约束）

| ID | 原计划动作 | 本轮裁决 | 依据 |
|---|---|---|---|
| **W0.1** | 逐文件 `git add` + commit 固化 5 项在途改动 | **不做**，改用 diff 快照达成同等「可归因」 | 改变 git 历史属不可逆；5 项是用户在途工作；`Aegis` 记录指针 `-fb05c61` 与子模块 HEAD `e5172dd` 不一致，未经确认推进有风险 → **Q-A** |
| **R1 / W0.4** | `.gitignore` 加精确豁免 + CI `git ls-files --error-unmatch` 断言 | **不做** | 单独加 `!` 而不原子提交会让 `audit_commit_coupling.py` 从 `IGNORED_TARGET` 升级为 `REF_UNTRACKED`（退 1）；且改 `.github/workflows` 须先贴片段征求确认（ADR-234 A4 `:58-60`）→ **Q-B**。**豁免清单精确为 3 个 UNTRACKED 文件**（team-lead `git ls-files --error-unmatch` 逐个实测）：`docs/architecture/adr_index.md`、`docs/release/release_guide.md`、`docs/reports/toolchain-parallel-optimization-closure-2026-11-06.md`（上一波报告，13671 B，在盘）；`ADR-234` 正文由既有 `:57` 白名单自动放行，但需 `git add` 才入索引。**注意方案 §9 的 `toolchain-parallel-optimization-w7-closure-<date>.md` 是 W6 才生成、含 `<date>` 占位符的文件，当前 `test -f` = MISSING**；若按占位符文件名写豁免或 CI 断言，断言必然红。⇒ Q-B 必须明确采用「上一波报告」而非占位符名，且 CI 断言需覆盖 4 个路径（3 个 .md + ADR-234），方案 §6.1 只列 1 个路径断言，覆盖不足 |
| **R2 / W0.5** | 补建 `docs/architecture/ADR-234-*.md` 正文 | **不做** | ADR-229~236 共 **8 个编号**整段缺失，只补 1 个 = 1/8；单文件搬运会引入指向 ADR-233/232/228 等缺失 ADR 的**散文式引用**（`相关：` 行），属治理/可追溯性风险、**机器盲区型文档漂移，不触发 CI 红**（性质订正见下）；补建后 `reconcile():123-126` 的三分支（`:94` / `:97` / `:100`）均不落入 ⇒ 运行时自动移出 exempt、**无需手工改脚本豁免列表**，但**必须同步改写 `adr_index.md:423` 补记**（其「八行物理文件仅在途分支」对 234 不再成立，且「七份」实为 8 个编号的笔误），而脚本看不见这个反向漂移。⇒ 应随在途分支整批合并 |
| **R3 / W0.6** | `event_ownership_baseline.txt` 末行 `total 152` 对齐 154 | **不做（简单改数字）** | 该末行是纯注释，`check_event_ownership.py` 实测 EXIT=0、variants=154 / 账本=18，**机器无门读取该数字**；`152` 与 `134` 两个数字均过时，手工改数字仍是人工台账。正确做法是用 `--emit` 机器重生成或删数字改指针，归 W3 七点位同批 |
| **R4 / W0.7** | `coverage_per_crate_floor.toml` 补 `gqep-executor` 行 | **不做** | 文件 `:13-19` 明确写「不得用估算或编造值填表（那正是本批在消除的假账形状）」、「本地不可代跑，出数地点为 coverage.yml nightly」。`coverage_gate.py --validate-config` 实测 EXIT=0 且确认 `coverage_baseline.toml` 只有 `min_line_coverage` 棘轮值、**无 locked 键**（已批准方案 §12 的「同步 locked 键」表述有误，本报告订正）。**`scripts/doc_count_freeze.txt` 亦不得改动**：`check_doc_drift.sh` 实测 EXIT=0、`nexus_event_variants=154` 已绿（G-37），该台账与 `coverage_baseline` 无任何联动门。补行需 nightly 逐 crate 实测值，本轮无据 |
| **D-1** | 登记 `toolplan_layers` | **不做** | 两条路径代价见 §4.2 → **Q-D** |
| **D-2** | 补 heredoc `PATH_KEYWORDS` 的 `hnsw_tiered` | **不做** | 归 W5.0 → **Q-C** |

**其余文档修订**（ADR-241 §3.3、ADR-242 §5.2、`adr_index.md`、收官报告 §5.1）：归 W6 收尾批次同批修订，W0 不夹带，避免治理面 diff 与代码 diff 混淆。

---

## 8. 待用户裁定项（W1 启动前置）

| # | 问题 | 推荐默认 | 若改选的影响 |
|---|---|---|---|
| **Q-A** | 是否代为逐文件提交 5 项在途改动？若需要，`Aegis` 子模块指针是否推进？ | **否**；保留工作树原样，用 `tmp/w7_baseline_6475027.md` 快照归因 | 若代提交，需先明确子模块指针语义；会改变 git 历史 |
| **Q-B** | 是否授权 `.github/workflows/ci.yml` 增加 `git ls-files --error-unmatch` 断言？ | **否**（本轮）；须由团队先贴出待加 YAML 片段 | 授权后 R1 须与 `.gitignore` 白名单 + **4 个路径**（3 个 UNTRACKED `.md` + 补建的 ADR-234）的 `git add` **原子同批**落地，否则 `audit_commit_coupling.py` 会红。**必须用上一波报告的真实文件名** `toolchain-parallel-optimization-closure-2026-11-06.md`，不可用方案 §9 的 W6 占位符 `...-w7-closure-<date>.md`（当前 MISSING）；CI 断言需覆盖全部 4 路径，方案 §6.1 仅列 1 个，须补足 |
| **Q-C** | `hnsw_tiered` 幽灵门何时修？ | **W5.0**（阈值双源治理波次） | 提前修需改 `bench_check.yml` heredoc，与 Q-B 合并授权 |
| **Q-D** | `toolplan_layers` 走哪条路径？ | **路径 B（升格 `registered`）在 W5 处理**，因该 bench 有真实并行性能语义 | 若选路径 A（`dev-only`），需附理由 + ADR 号，且等于把一个有 SLO 意义的基准声明为「无 SLO」，降低门强度 |

---

## 9. 三重验证汇总（R5 落码后）

| 验证类型 | 动作 | 实测结果 | 判定 |
|---|---|---|---|
| **自动化 · 门禁** | 复跑 8 条门禁 | `composition_root` / `doc_drift` / `config_sample_parity` / `ci_manifest_parity` / `audit_commit_coupling` / `event_ownership` / `invariants` 全 EXIT=0；`inventory` EXIT=1 `unknown=1`，**与落码前逐字一致** | PASS（证明 R5 未引入新红） |
| **自动化 · 编译** | `cargo check -p chimera-cli --lib` | `Finished dev profile in 45.17s`，EXIT=0 | PASS |
| **自动化 · 格式** | `cargo fmt -p chimera-cli -- --check` | EXIT=0，无输出 | PASS |
| **自动化 · 覆盖率** | `coverage_gate.py --validate-config` | EXIT=0（`min ratchet last=85`） | PASS（配置面）；`--check` 本机 E0463 不可判定，归 CI |
| **手动 · diff 复核** | `git diff -- Cargo.toml composition.rs` | 恰 2 处、均为注释；`-1/+1` 与 `-3/+3`；行数守恒；指针 `test -f` 解析成功 | PASS |
| **手动 · 工作树** | `git status --short` | 7 项 `M`（5 在途 + 2 R5）、0 未跟踪 | PASS |
| **性能基准** | 适用性判定 + 既有 L4 快照对拍 | **W0 不适用新 bench**（§6.3 六条理由）；53 份 `estimates.json` 与宣称值在 2.1–3.4% 内吻合，小于冷启动噪声 | N/A（有依据） |

**测试覆盖说明**：W0 无新增功能代码，故无新增单元/集成/proptest 用例（符合 YAGNI，且 `AGENTS.md:231/:304` 只要求「既有测试零删除」）。W1 起新增 Rust 代码时，按用户要求执行「单元 + 集成 + proptest 三层 + ≥85% 覆盖率 + criterion 基准」，验收命令已在已批准方案 §6.1 逐波列出。

---

## 10. 风险、证明力上限与不确定性

1. **基线非全绿**：`inventory` / `all` 红项在 W0 未消除（Q-D 待裁定）。这不是 W0 回归，但意味着 **`cargo test --workspace --no-fail-fast` 与性能红线门不能宣称「全绿」**；本轮未跑全量测试（W0 无 Rust 逻辑改动，且 12065 用例全量跑在 Windows-GNU 有内存约束），不得据此宣称「测试全绿」。
2. **不可判定门 3 类**：`run_gate_manifest.py`（沙箱 wsl.exe 黑名单）、`coverage_gate.py --check`（E0463）、fuzz（libFuzzer 仅 MSVC）。按 ADR-234 A3，必须写明「门在清单里、CI 不跑」的中间态风险，不得含糊带过。
3. **证明力上限**：本轮结论最高到 **L1 结构判据**（门禁/静态文本），性能数值属既有 **L4 快照**；`slo.yml:68/:170` 的 `continue-on-error: true` 使 A1/A2 观察期红线**不阻塞合入**，证明力封顶 L3 观测档，不得当作 L4 强制门。
4. **ADR-234 载体缺口**：主检出 `test -f` = MISSING、`git ls-files | grep -ci ADR-234` = 0；原文仅存于 `.qoder/worktrees/toolplane-execution/docs/architecture/ADR-234-perf-parallelism-verification-limits.md`（13234 B，2026-11-04 08:40，未并 main、不随 git 分发）；`adr_index.md:245` 有摘要但该文件自身也不入库 ⇒ CI 干净克隆连摘要行都无载体。本报告引用 A1/A2/A3/A4/A5 条款时以 worktree 原文为准，并标注此局限。
5. **md 分层事实**：HEAD `.gitignore:54 *.md`、`:57 !docs/architecture/ADR-*.md`、`:61`/`:62` 例外；实测 133 份 `ADR-*.md` 已入库（ADR-240~245 全部 TRACKED），`adr_index.md` / `docs/release/*.md` / 多数 `docs/reports/*.md` 未跟踪。**ADR-242 §5.2 与收官报告 §5.1 第 5 项「ADR 不随 git 分发」是过时陈述**，本轮未修订（归 W6）。本进度报告本身亦受 `*.md` 排除，不入库，属已知治理缺口。
6. **多 worktree 分叉**（`AGENTS.md §2.8`）：`feat/phase9-*` / `feat/quest-timeline*` / `toolplane-execution` 等在途；本轮审计基准固定为当前主检出，合并前须复核。
7. **棘轮余量极紧**：I2 重叠 1 对（cap=1，静态余量 0）、`dev-only` 62 条（floor 60，余量 2）、G-27 豁免 9/9、`calibration_pending` 1/1 顶格。W3 新增事件变体与 W5 新增阈值键前须先跑 `invariants` 取余量读数。
8. **Git/PR 硬约束**（本轮全部遵守）：逐文件 `git add` 禁 `-A`（`AGENTS.md:34`）、禁 `--no-verify`（`:36`）、提交信息 HEREDOC（`:38`）、每波 ≤8 工作项且逐 commit 可 revert（`CONTRIBUTING.md:29`）、增删 `NexusEvent` 变体六锁同批（`:143`）、全量回归 `--no-fail-fast`（`AGENTS.md:588-595`）。

---

## 11. 交付物清单

| 路径 | 性质 | 说明 |
|---|---|---|
| `tmp/w7_baseline_6475027.md` | 新建（受忽略） | W0.2 唯一基线快照索引：版本/工具链/工作树/性能/判据载体/硬约束 |
| `tmp/w0-inflight-2026-11-06.diff` | 新建（受忽略） | 5 项在途改动原始 diff，6840 B，SHA256 三处一致 |
| `docs/reports/toolchain-w0-progress-2026-11-06.md` | 新建 | 本进度报告（受 `*.md` 排除，不入库） |
| `Cargo.toml:652` | 修改 | R5 文档指针日期笔误 |
| `crates/chimera-cli/src/composition.rs:165-168` | 修改 | R5 过时回滚注释 |
| `C:/Users/30324/.workbuddy/plans/radiant-thunder-darwin-xenXoOpR.md` | 已存在 | 已批准的 13 节重构方案（W0–W6），本轮唯一未改动的上游权威文档 |

**未触碰**：任何 `.github/workflows/*`、`scripts/perf_thresholds.toml`、`scripts/*_freeze.txt`、`slo.yml`、`bench_check.yml`、`coverage_*.toml`、`event-bus`/`gqep-executor`/`seccore`/`nexus-core` 的 Rust 源码、以及 W1–W5 的全部目标文件。

---

## 12. 下一步

W0 的本轮可安全执行项已收口。W1（策略规则声明式配置源，含 TDD 三层测试与 ≥85% 覆盖）**需先获得 Q-A ~ Q-D 的裁定**；在此之前不启动，避免越过基线阶段造成不可归因的 diff。若用户裁定继续，首步按已批准方案 §5 W1 执行：`crates/nexus-core/src/config.rs` + `crates/chimera-cli/src/config.rs` 双文件同批 → `check_config_sample_parity.py --write` → `composition.rs` 签名改透传 → 三层测试 → 门禁复跑。

---

*本报告依 ADR-234「证据先于断言」与 Superpowers「Evidence over Claims」编写。所有数值来自本轮实际执行的只读命令或已批准方案；不可判定项明示原因，未独立复跑的既有报告自述值一律标注为「报告自述」。*
