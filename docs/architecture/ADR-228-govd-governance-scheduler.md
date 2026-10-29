# ADR-228 -- govd 调度治理系统（判断/执行/证据/权限/呈现五面收敛）

- 状态：Accepted（2026-10-29）
- 决策者：架构组（本轮由只读审计 + 主会话复测共同定稿）
- 关联：ADR-033（L0 恒允许）、ADR-048（gqep→qeep 特批）、ADR-049（0 新增 crate）、ADR-197/198（门禁注册与幽灵门）、ADR-219/221/227（L0 轮次审计基线）
- 影响面：`scripts/`（调度面）、`.github/workflows/ci.yml`、`docs/reports/govd-governance-scheduler-2026-10-29/`

## 1. 背景与问题

`gate_manifest.toml` 已是门清单 SSOT，但它只回答“有哪些门”，不回答“谁在什么提交上跑过、跑出什么”。本轮复测（2026-10-29）确认三处分裂：

| 面 | 现状读数 | 证据 |
|---|---|---|
| 执行面 | CI 138 处脚本引用 / 69 个步骤 vs manifest 140 门，二者无对拍 | `.github/workflows/ci.yml`；`scripts/check_ci_manifest_parity.py` 输出 `ci_refs=138 ci_steps=69 manifest_gates=140 matched=68` |
| 证据面 | `tmp/gate_logs/` 按门 id 覆盖写，无 run_id/HEAD/时长/重试 | `scripts/run_gate_manifest.py` 日志写入段 |
| 生命周期面 | 64 个脚本既不在 manifest 也无豁免，仅 4 条豁免，无 active/diagnostic/evidence/retired 区分 | `scripts/check_gates_provenance.py` 方向 C：`scanned=158 registry=64 unaccounted=0` |

## 2. 决策

### 2.1 五面收敛，不新写 runner 之外的东西

判断（`gate_manifest.toml` + `tasks.toml`）、执行（`scripts/govd/executor.py`）、证据（`store.py` JSONL 哈希链 + SQLite 索引）、权限（`policy.py` + `policy.toml`）、呈现（`report.py`）收敛为一个 stdlib-only 包 `scripts/govd/`。

**落点**：`scripts/govd/` 是既有 `scripts/` 目录下的子包，不是新 crate（ADR-049 决策 1 延续）。零第三方依赖（默认决策 8.1）。

### 2.2 单一清单 + 可选字段，v1 零改动

`gate_manifest.toml` 只**新增可选字段**：`timeout_s`、`retry`、`mutating`、`needs`、`profiles`、`owner`、`source`、`disk_gb`。v1 条目（140 门）无需改动即可被 v2 加载器消费。

**拒绝项 R1**：拒绝把 rust/external 任务塞进 `gate_manifest.toml`。理由：`[[gate]]` 的计数被 `check_manifest_schema`、`check_ci_manifest_parity`、`check_py_tooling_health` 三方读作“门数”，混入 `cargo check` 会让“140 门”这个口径失去意义。因此新增 `scripts/govd/tasks.toml` 的 `[[task]]` 族，且**只有 release profile 合并**（`cli.load_all` 仅在 `profile=="release"` 时 `load_tasks`）。

### 2.3 退出码与七态判决

对外的 rc 仍是 0/1/2（与 ADR-060 一致）；对内引入七态 `ok/mismatch/env/timeout/launch/unknown/skipped`（+ `cancelled`）。

**顺序即判据（F33）**：`env` 必须先于 `rc == expect` 判定。`expect=1` 的门若在解释器层崩掉，rc 恰等于期望值，会被记成“符合期望”——这是静默假绿。`classify.py` 移植 `env_failure_reason` 的全部正负样本（含 `real-finding-nosuchfile-midbody` 这类“正文里出现 No such file”的真红）。

**拒绝项 R2**：拒绝把 `mismatch` 纳入重试。红判定重试一次只是把同一条红推迟 30 秒，且会让台账出现“第 2 次才红”的噪声。重试仅限 `env/timeout/launch`，最多 1 次。

### 2.4 证据不可覆盖

每次 run 落 `tmp/govd/runs/<run_id>/`（`events.jsonl` 带 `prev_sha256` 哈希链、`logs/`、`report.json`、`junit.xml`），另有 `index.sqlite3`（可重建）与 `latest.json`。`verify-journal` 重算哈希链。

**拒绝项 R3**：拒绝继续用 `tmp/gate_logs/<gate_id>.log` 作为权威证据。按 id 覆盖写在多 run 并发下必然互相覆盖；它保留为 legacy 引擎的兼容输出，不再是 SSOT。

### 2.5 权限与 mutating 守卫

`mutating = true` 的任务必须同时满足：`--allow-mutating`、工作树干净、跑前跑后 `git status --porcelain` 对比后白名单（`tmp/`、`target/`）外无新增改动。违反判 `policy_violation`（rc 2）。

**现状**：140 门里 `mutating` 为 0——这不是“没有变更型脚本”，而是“变更型脚本尚未进门清单”。64 个 registry 行中 `diagnostic/evidence` 类脚本（如 `cleanup_disk_space.ps1`、`generate_changelog.ps1`）一旦提门，必须先打 `mutating=true`。

### 2.6 profile 与并发

| profile | 选择面 | 并发 | 用途 |
|---|---|---|---|
| `pr` | 133 light（140 - 7 heavy） | `--jobs 4` | 本地/CI 影子 |
| `nightly` | light + heavy | `--jobs 2` | 夜间全量 |
| `release` | 140 门 + 6 任务（R-01..R-03/E-01..E-03） | `--jobs 1` | 发版前 |
| `local` | 同 pr | 1 | 串行调试 |

### 2.7 CI 影子先行

`.github/workflows/ci.yml` 新增 `governance-shadow` job（`continue-on-error: true`）。**3 次影子绿之后**才允许把 108 步硬编码分批退役；在此之前 legacy 步骤仍是阻塞面。

## 3. 回滚

| 层 | 回滚动作 | 影响 |
|---|---|---|
| 执行面 | `run_gate_manifest.py` 默认 engine 仍是 `legacy`；删 `--engine` 分支即回到 2026-10-28 行为 | 零 |
| 清单 | `tasks.toml` 仅在 release 合并；删除该文件后 `load_all` 只读 manifest | release 少 6 项 |
| 证据 | `tmp/govd/` 可整目录删除（gitignored）；不影响任何门 | 零 |
| 门 | G-137..G-140 为新增门；从 manifest + ci.yml 同批删除即可，无下游依赖 | CI 少 4 步 |
| Aegis | `.gitmodules` 单文件新增；删除后 gitlink 恢复为“无映射”原状 | 干净克隆不可 init（原病） |

## 4. 失效条件（何时应重审本 ADR）

1. `check_ci_manifest_parity.py` 的 `gate_missing` 或 `unmatched` 连续 2 次非 0 —— 说明双执行面重新分裂。
2. `governance-shadow` 连续 3 次红 —— 说明 govd 判决与 legacy 不一致，必须先对拍再谈翻转。
3. 出现新的 `expect=1` 门且 `classify` 夹具未覆盖其日志形态 —— 假绿风险回升。
4. `tool_registry.toml` 的 `retired` 行重新出现在磁盘 —— 说明“只登记不删除”的纪律被绕过。
5. 需要引入第三方依赖（pytest/ruff/pydantic）—— 违反默认决策 8.1，须另开 ADR。

## 5. 与既有 ADR 的关系

- **ADR-049**：本决策不新增 crate，`scripts/govd/` 是脚本层子包。
- **ADR-197 D5 / ADR-198 §3.4**：新增门仍遵守“脚本 + manifest 两条 + workflow 一步”的四件同批；G-137..G-140 即按此登记。
- **ADR-060**：rc 0/1/2 语义不变，七态只在内部记账。
- **ADR-033**：`scripts/` 不参与十层依赖，故本决策不产生跨层边。
