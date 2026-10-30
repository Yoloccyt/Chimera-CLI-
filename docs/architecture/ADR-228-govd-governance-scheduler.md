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

**拒绝项 R1**：拒绝把 rust/external 任务塞进 `gate_manifest.toml`。理由：`[[gate]]` 的计数被 `check_manifest_schema`、`check_ci_manifest_parity`、`check_py_tooling_health` 三方读作“门数”，混入 `cargo check` 会让“140 门”这个口径失去意义。因此新增 `scripts/govd/tasks.toml` 的 `[[task]]` 族：`release` 合并全族，其余档只合并 `profiles` 显式点名它的 `external` 行（本句为 2026-10-29 追加节 D-1 对原文“只有 release 合并”的订正）。

### 2.3 退出码与七态判决

对外的 rc 仍是 0/1/2（与 ADR-060 一致）；对内引入七态 `ok/mismatch/env/timeout/launch/unknown/skipped`（+ `cancelled`）。

**顺序即判据（F33）**：`env` 必须先于 `rc == expect` 判定。`expect=1` 的门若在解释器层崩掉，rc 恰等于期望值，会被记成“符合期望”——这是静默假绿。`classify.py` 移植 `env_failure_reason` 的全部正负样本（含 `real-finding-nosuchfile-midbody` 这类“正文里出现 No such file”的真红）。

**拒绝项 R2**：拒绝把 `mismatch` 纳入重试。红判定重试一次只是把同一条红推迟 30 秒，且会让台账出现“第 2 次才红”的噪声。重试仅限 `env/timeout/launch`，最多 1 次。

### 2.4 证据不可覆盖

每次 run 落 `tmp/govd/runs/<run_id>/`（`events.jsonl` 带 `prev_sha256` 哈希链、`logs/`、`report.json`、`junit.xml`），另有 `index.sqlite3`（可重建）与 `latest.json`。`verify-journal` 重算哈希链。

**追加（2026-10-29 实施期，见 §6 D-8）**：`run_start` 事件携带 §2.5 的全部审计字段（整份 `RunMeta`），因为本文声明 journal 是权威——provenance 只活在派生产物 `report.json` 里，则删掉派生产物就无人能回答“谁在哪个 HEAD 上跑了什么”。机证：实测 `rebuild_index` 在把 `index.sqlite3` 整个移出后仅凭 journal 重建，得 2992 行 / 25 run 与移出前全等；`AuditFieldTests` 钉住字段集与 digest 一致性。

**拒绝项 R3**：拒绝继续用 `tmp/gate_logs/<gate_id>.log` 作为权威证据。按 id 覆盖写在多 run 并发下必然互相覆盖；它保留为 legacy 引擎的兼容输出，不再是 SSOT。

### 2.5 权限与 mutating 守卫

`mutating = true` 的任务必须同时满足：`--allow-mutating`、工作树干净、跑前跑后 `git status --porcelain` 对比后白名单（`tmp/`、`target/`）外无新增改动。违反判 `policy_violation`（rc 2）。

**现状**：140 门里 `mutating` 为 0——这不是“没有变更型脚本”，而是“变更型脚本尚未进门清单”。64 个 registry 行中 `diagnostic/evidence` 类脚本（如 `cleanup_disk_space.ps1`、`generate_changelog.ps1`）一旦提门，必须先打 `mutating=true`。

### 2.6 profile 与并发

| profile | 选择面 | 并发 | 用途 |
|---|---|---|---|
| `pr` | 133 light（140 - 7 heavy）——不含任何 task 族 | `--jobs 4` | 本地/CI 影子 |
| `nightly` | 140 门 + `external` 三行（E-01..E-03）= 143 | `--jobs 2` | 夜间全量（含子仓回归） |
| `release` | 140 门 + 6 任务（R-01..R-03/E-01..E-03）= 146 | `--jobs 1` | 发版前 |
| `local` | 同 pr | 1 | 串行调试 |

> 面大小以 `govd list --profile <p>` 实报为准（2026-10-29 实测：pr `selected=133 total=140 skipped=7`，nightly `143/143`，release `146/146`）。

### 2.7 CI 影子先行

`.github/workflows/ci.yml` 新增 `governance-shadow` job（`continue-on-error: true`）。**3 次影子绿之后**才允许把 108 步硬编码分批退役；在此之前 legacy 步骤仍是阻塞面。

## 3. 回滚

| 层 | 回滚动作 | 影响 |
|---|---|---|
| 执行面 | 默认 engine 已翻为 `govd`（A/B 对拍后）；`--engine legacy` 保留为回滚面，也可改 `engine_of()` 默认值 | 一步即可退回 legacy 行为（其夹具与 `[SUM]` 文本仍在） |
| 清单 | `tasks.toml` 行靠 `profiles` 声明归属；删除该文件后 `load_all` 只读 manifest | nightly 少 3 项、release 少 6 项 |
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

## 6. 追加节（实施期修订，2026-10-29）

> 本节是 P3 分批实施与接管验收期间的**已执行修订**。代码与本节一致；§2.2/§2.6/§3 中被本节订正的旧句子已就地更新，不留双口径。

| 编号 | 修订 | 为何（含被否的方案） | 机证 |
|---|---|---|---|
| **D-1** | `nightly` 合并 `external` 三行（E-01..E-03），`release` 仍合并全族；非 gate 行靠 `profiles` 字段声明归属 | 原设计只在 release 合并，子仓回归最晚一次/版本才暴露，而 nightly 的存在意义就是抓 release 之间的漂移。**否**：“全族无条件合并”——会把 pr 账本的 `skip=7` 撑成 13，使运维唯一要看的那个数不再等于“磁盘 gated 的 heavy” | `list pr` 仍 `133/140 skip=7`；`nightly=143/143`；实测 `E-01/E-02/E-03 ok rc=0`（run `20261029T142854Z-9979e60-45500`，`ok=137 unknown=6`，journal `events=145 bad_line=0`） |
| **D-2** | G-138 开始校验 `govd/policy.toml`：`reuse_ok` 不得为 true、mutating 仅限 release、白名单非空且合法、role 必须指向存在的 profile | profiles/白名单/两个开关**就是**权限模型本体，此前无人校验——一行无人复核的 TOML 编辑就能静默削弱它（尤其 release 继承旧判决） | schema 自检 22→33 条；真树 rc=0 打印 `policy_profiles=4` |
| **D-3** | 慢门显式 `timeout_s`：G-07=900s、G-08=600s | 默认 300s 对实测 225-242s / 152-205s 只剩几秒余量，2026-10-29 真发生一次 timeout——把环境噪声报成仓库缺陷的 flake 发生器 | 清单内注释记录测量值；`list` 显示 `timeout=900s/600s` |
| **D-4** | “无域不得交判定”：扫描根/语料缺失时**拒绝交出判定**（rc 2），不得返回空账本或 `continue` | `check_dead_contract_apis` 缺 `crates/` 曾把全部 WIRED 报成死契约；`audit_phaseR_artifacts` 对缺报告 `continue` ⇒ **删证据即变绿的单调形状** | 三门各加夹具（+4）；真实运行 rc=0 不变；`verify_fmt_all_packages` 解析失败打 `[UNKNOWN]` 后退 2 |
| **D-5** | H9 STDIODUP 只减不增登记表（`scripts/stdio_dup_baseline.txt`）+ `gate_rc.utf8_stdio()` 公开入口 | 多份重复的 UTF-8 stdio 代码属客观冗余（现数以门自报的 `stdio_dups` 为准，本行不抄数），但为风格一次性改完全部在工作门，回归风险大于收益——改为**只防增长**并鼓励逐个收敛 | py_tooling_health 自检逐次增加且全绿，打印 `stdio_dups=<现取>`；登记表行必须带理由，缺表 / malformed 均退 2；后续 D-9 把检测改为 AST 调用节点并删掉 6 处必然冗余块 |
| **D-6** | 逐文件清单由工具生成（`audit_python_surface.py --markdown-out`）；并修正 `_third_party` 把仓内模块误报为第三方依赖 | “stdlib-only”这条声明此前由一个坏探针（只看同目录 stem）背书；`gate_rc`/`scripts` 被报成第三方依赖——不查就是仪表谎报安全 | 清单 344 行（116 py + 46 sh/ps1 + Aegis 182）；`third_party_importers=0`（沿脚本目录/`scripts/`/仓根三处解析） |
| **D-7** | 默认 engine 翻为 `govd`，`--engine legacy` 保留为回滚面；legacy **不** import govd 的分类器 | 回滚面若依赖被测物本身就不是回滚面；两份 env 分类实现由同一批夹具钉住（legacy 26 条 + govd 分类测试） | A/B 对拍 `union=140 matched=133`；翻转后全量 pr rc=0 |
| **D-8** | `run_start` 事件携带完整 `RunMeta`（provenance 入权威日志，不只入派生 `report.json`） | 本文 §2.4 声明 journal 是权威，§2.5 却只存在于派生产物——删掉派生文件后，权威日志答不出“谁在哪个 HEAD 上跑了什么”。同时实测“删索引仅凭 journal 重建”与移动前行数全等，坐实“可重建” | `AuditFieldTests`（1 例）；真库实测 2992 行 / 25 run 重建全等；shard 3/40 实跑 run_start 含 actor/host/python/role/digest（run `20261029T161052Z-c156494-31880`） |
| **D-9** | H9 检测从子串改为 **AST 调用节点**，并删除入口已早于块的 6 处 stdio 代码 | 子串扫描把散文算作代码：本门自己的 docstring 与夹具命中后，它把自己登记成了重复项——一个会抓句子的登记表教人的是“不必信它”。其余行分两类：入口在块之后（块就是生效的钉子，收敛需动入口位置=行为变更）与无 gate_rc 入口的一次性工具 | py_tooling_health 自检含“docstring 提及不算重复”负样本；`stdio_dups` 由 31 降到当前自报值；六门各自达声明期望（G-16 rc=1==expect=1） |
| **D-10** | `run --report <kind>` 从"被解析却无人读"的装饰旗标变成**后置条件**：所请求产物必须存在且非空，否则 `UNDECIDABLE` + rc=2；写盘仍无条件 | journal 目录是证据（§2.4），一个 CI 旗标不该获得"删掉证据"的能力；但旗标也不能只是装饰——否则"我要 junit"这句话没人兑现。被否方案：`--report junit` 只写 junit（证据会按调用方残缺）；把整份 XML 打到 stdout（日志噪声，artifact 已上传） | 打印 `REPORT junit.xml=<path> (<n> bytes)`；HEAD 上 CI 形状全量 `--report junit` / `--report all` rc=0（产物带字节数）；`ReportSurfaceTests` 3 例 |
| **D-11** | **方言外退出码归环境**：`rc ∉ {0,1,2}` 或 `rc==2 且 expect!=2` 判 `env`（`abnormal-exit:rc=…` / `gate-declared-undecidable`），两侧分类器各实现一份且由跨引擎测试比对**三分判决本身** | 实测一次被污染的全量 run 把 `rc=127`（shell 没启动命令）、`3221225773`（fastfail）、`2147516553`（控制台/RPC）、门自宣不可判定的 `rc=2` 全部记成 `mismatch`＝假红；而 `expect=1` 的门遇同形状会是**静默假绿**。G-138 已强制 `expect ∈ {0,1,2}`，故方言外码必非判决。刻意不共享 import：回滚面不得依赖被回滚物 | 九条被误记的门单独逐门重跑全达期望；legacy 自检 26→28→32；修后 HEAD 全量 `ok=133 env-fail=0` 作**反证**（新规则不吞在册真红）；`test_legacy_and_govd_classifiers_agree_on_every_env_fixture` |
| **D-12** | mutating 守卫的判断**上移到规划之前**并点名被挡任务；原拒跑分支不可达 | `cmd_run` 读的是 `planner.plan()` 之后的 `ordered`，而 `selected_by()` 早已丢弃 mutating 行 ⇒ 条件永假，守卫退化成静默跳过——不变红也不告警，没有外力能揭穿。只统计"本 profile 真会选到"的行，故限定 release 的 mutating 行不会挡住 pr 档 | `MutatingGuardTests` 5 例（数执行器被调用次数）：无旗标 rc=2 且零调用、脏树 rc=2 且零调用、有旗标且干净**确实执行**、改动落在 `tmp/`、`target/` 之外 ⇒ `POLICY-VIOLATION` + rc=2、pr 档不误拒 |
| **D-13** | 删除 `RunMeta.extra`：从未赋值、从未读取，却出现在每次 `run_start` 与 `report.json` 里恒为 `{}` | 可达性清扫（89 个模块级定义 + dataclass 字段）的产物。"留个扩展点"若无代码填、无消费者读，就是装饰性接口，与 D-10 的旗标同族；它出现在**证据结构**里更糟——读报告会把它当信息。顺带记录：删后遗留的未用导入被 G-121/H2 当场判红（`[GAP][DEADIMPORT] model.py`），修完转绿——顺序如实入册而非粉饰 | 新产物 `run keys` 已无 `extra`、`run_start` payload 无 `extra`；`imports 590→589`；79 tests OK |
