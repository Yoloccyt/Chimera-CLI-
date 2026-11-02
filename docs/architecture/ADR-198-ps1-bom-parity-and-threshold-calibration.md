# ADR-198: PowerShell 5.1 parity 守门与计算面阈值校准闭环（Phase B1）

> **状态**: ✅ Accepted（2026-10-02，依据 = 用户批准《PS51 parity 与 SPRT 校准》实施计划）
> **层级**: 治理工具（`scripts/` + `.github/workflows/ci.yml`）+ L1 Core（`nexus-core::compute`）
> **关联**: ADR-166/175/187（`.sh`/`.ps1` 双源同构与 BOM 乱码面）、ADR-197（计算面 Phase A，本 ADR 承接其 Phase B1）、ADR-103/128（HTS-CPU 三重来源与 SPRT）、ADR-196（TaskKind 扩容，**是 B2 前置而非 B1 前置**）
> **验证正文**: `docs/reports/ps1-parity-phaseB1-progress-2026-10-02.md`

---

## 1. 背景与问题（两条独立线索，同一根因家族：**声称的守护没有牙**）

### 1.1 Windows PowerShell 5.1 侧从未跑成

| 实测事实（2026-10-02） | 数值 |
|---|---|
| `scripts/*.ps1` 总数（均为 git 跟踪，`ls-files -v` 全为 `H`） | **24** |
| 工作树副本：无 UTF-8 BOM 且含非 ASCII 字节 | **17** |
| 其中在 Windows PowerShell 5.1 下解析失败（`[Parser]::ParseFile`） | **11 个文件、合计 55 个 parse error** |
| 被 Rust 测试真实调用的 | `check_dependency_rules.ps1`（根 e2e `tests/dependency_rules_inner_ring_test.rs` 用 `powershell -File`） |

根因不是语法错：`check_dependency_rules.ps1` 在 `pwsh 7` 侧 **rc=0**，5.1 侧报 `386:1 意外的标记 "}"`；工作树副本首字节为 `23 20 3D`（无 `EF BB BF`）且含 1034 个非 ASCII 字节 ⇒ 5.1 按 ANSI(GBK) 解码，多字节序列吞掉引号/反引号使 tokenizer 错位（`generate_changelog.ps1` 的报错直接露出乱码 `琛?`）。

#### 1.1a 更正：归属是本机工作树漂移，不是仓库内容缺陷（本批完成审计发现）

本 ADR 初稿把根因写成“仓库里的脚本没带 BOM”，该结论**错了**，现根据仓取数更正：

| 取证 | 读数 |
|---|---|
| `git show HEAD:<file>` 逐文件取 blob 首 3 字节（24 个） | **20 个带 BOM**，4 个不带 |
| 那 4 个不带 BOM 的 | `check_crate_reachability.ps1`、`check_perf_redlines.ps1`、`count_lines.ps1`、`test_benchmark_parser.ps1` —— 均为**纯 ASCII**（非 ASCII 字节=0），在本 ADR D1 规则下本来就合法 |
| 补 BOM 后 `git diff --numstat HEAD -- scripts/` | 对 17 个 `.ps1` **无任何输出** ⇒ 工作树与提交态逐字节一致（blob 实测 1037 个非 ASCII = 工作树 1034 + 3 个 BOM 字节） |

⇒ 两个直接推论：
1. **“补 BOM”不是一个可提交变更**，而是**把本地副本还原为提交态**；因此本批对 Windows 5.1 的修复作用域是**本机**，其他克隆与 CI（走 `pwsh`）从未因此受损。
2. **真正需要守的是“不可见性”而非“仓库脏数据”**：`git status` 在漂移存在期间报告该文件无改动（当时实测），而仓内又无任何机器判据去校 BOM —— 一旦某个工具/编辑器以“UTF-8 无 BOM”重写 `.ps1`，Windows 侧就静默不可用，且没人能从 `git status` 看出来。另：未来新提交一个“含非 ASCII 而无 BOM”的 `.ps1`，仓库层目前也没任何东西拦得住。

因此 G-85 的价值重新表述为：**把“BOM 存在性”从依赖 git 可见性（已证不可靠）改为依赖逐字节的机器判据**，既防本机漂移复发（`RESULT: OK (24/24)` 当场可见），也防仓库侧未来回归（已接 `ci.yml` 阻断位）。

**为什么长期无人发现**：CI 与 `gate_manifest.toml` 一律用 `pwsh`（默认 UTF-8），唯一的 5.1 消费者是那个根 e2e 目标；而 ADR-166/175 把 `.ps1` 定位为"Windows 权威执行体"，该定位从未被任何门守住。上一批（ADR-197）做全量回归时，该缺陷表现为"3 项失败"并被正确归属为既存红，但**未被修掉**——本 ADR 就是那次归属的续账。

### 1.2 守门脚本本身不存在（账面漂移）

ADR-187/`adr_index` 文本记有"G-60 `check_ps1_bom_gate.py` 判无 BOM"。实测：
- `scripts/check_ps1_bom_gate.py` **文件不存在**（`python` 报 `No such file`）；
- `scripts/gate_manifest.toml` **无 `ps1_bom` 条目**，84 条中最大号为 **G-84**，即"G-60 = BOM 门"从未登记过。

⇒ 结论：这是一处**幽灵门**（文档声称存在的治理工具实物缺失），且它恰好是本缺陷能长期存活的直接原因。

### 1.3 计算面阈值"可校准"但无人校准（ADR-197 续）

`HtsTable` 的三重来源里，SPRT 序贯检验与 cgroup 核数校正**实物早已落地**（`compute/hts.rs:242-420, 543-619`），但唯一写入口 `ComputeBridge::update_threshold` 的生产调用者为 **0** ⇒ 运行期阈值表永远等于 S9 预填初值，"动态阈值"名不副实（ADR-197 台账已登记为 DEFERRED）。

## 2. 决策

### D1 `.ps1` BOM 规则：含非 ASCII 必须带 UTF-8 BOM；纯 ASCII 不强制

WHY 只强制一半：5.1 只在**遇到非 ASCII 字节**时才会因缺 BOM 而误码；纯 ASCII 文件两种解码结果逐字节相同，强制补 BOM 只会制造无意义 diff。纯 ASCII 无 BOM 者以 INFO 输出，保持可见但不阻断。

落地：`scripts/check_ps1_bom_gate.py`（**本 ADR 落盘**，含 `--selftest` 四夹具双向验证：非 ASCII 无 BOM 必红、非 ASCII 有 BOM 必绿、纯 ASCII 无 BOM 只 INFO、空文件不炸——防"正则偷空即永远绿"的假守护），输出保持纯 ASCII。

修复面：17 个违规文件**字节级前置 BOM**（每文件 +3 字节，正文零改动、逐文件可独立 revert），修后 5.1 全量解析 `PARSE_FAIL=0/24`，`pwsh` 侧行为与修复前一致。

### D2 登记 G-85 并**同批接入 CI 阻断位**

`gate_manifest.toml` 追加 G-85（`spec_ref = "ADR-198"`，`kind = "light"`，`expect = 0`）；`.github/workflows/ci.yml` 在层图对拍门之后新增阻断步骤（先 `--selftest` 再裁决，沿 `check_layer_map_parity` 先例）。

WHY 必须同批接 CI：`audit_gate_parity.py` 自身的判据就是"登记却未接入 CI 的门 = 保护不了任何东西"。只登记不接线，`audit_gate_parity` 立刻转红——这正是本 ADR 要消灭的那类"账面有门、实物无守护"的复发形态。
（实测：登记后未接线时 `audit_gate_parity` rc=1，接线后 rc=0，80 个门脚本全部阻断位齐备。）

### D3 Phase B1：SPRT 校准闭环接入，**默认 `observe` 零行为变更**

新增 `nexus-core/src/compute/calibration.rs`，`CHIMERA_COMPUTE_CALIBRATION` 三态：

| 值 | 行为 |
|---|---|
| `off` | 完全不采样（基准测量与 CI 固定用该值，保证可复现） |
| `observe`（**默认**） | 按 1/64 采样 `inline_us`/`rayon_us` 成对样本并喂 `SequentialTest`，**不写表** |
| `apply` | 在 `observe` 基础上，仅当 `decision == Promote` 才 `update_threshold(kind, candidate, chunk, SequentialTest{promoted_at, samples})` |

> **⚠ 2026-10-02 现测勘误（只加注，不改动上方决策文本）**：代码实际默认档为 `off`（`crates/nexus-core/src/compute/calibration.rs` 标注「`off`（默认）」，`parse_calibration_env(None) -> Off`），非本表所载的 `observe`。收紧理据见 `docs/reports/ps1-parity-phaseB1-progress-2026-10-02.md` §2.1（探测=执行路径变更，真实负载验证前先不采样）。默认态以 `calibration.rs` 为权威源；本 ADR 决策文本依「历史只加注不改写」原则原样保留。

WHY 默认不写表：阈值变化 = 分支变化 = 行为变化。ADR-197 的 A1 教训（首轮把"默认降级"当成默认态，后被自己的风险缓解条款纠正）直接适用到这里；且 `dispatch.rs:10` 的诚实数据红线要求"未经复测的初值不得冒充已校准"，反过来也一样——**未经真实样本验证的校准动作不得默认生效**。

约束：① 全部判定委托既有 `hts.rs` SPRT，**不改 Wald 常数 / `s_min` / `SequentialTestConfig`**；② `route()` 与 HTS 读路径零改动（`hts_table_get` 实测须仍为 2.2 ns 量级）；③ 写表必须带 `promoted_at` 与真实 `samples` 计数（可审计，防伪造"已校准"）；④ 严格模式不产样本（闭包未执行即记账属虚增）；⑤ 采样与写表均在同步段完成，不引入持锁跨 await。

### D4 B1 与 ADR-196 解耦（防误等）

TaskKind 四登记位扩容（ADR-196）是 **B2** 的前置；B1 只给**现有 6 个表项**接校准驱动，二者无依赖。此判断写入本 ADR，避免"等 ADR-196 批准"成为推迟 B1 的理由。

### D5 顺带查明的既存红不改（归属并登记）

`audit_gate_selfreport.py` rc=1（经 `pwsh` 调 `check_doc_consistency.ps1`，报 GAP-B1：`docs/architecture/README.md` 等三处未记当前 crate 数 39）。该门调用的是 `pwsh` 而非 5.1 ⇒ 与本批 BOM 改动无因果；相关三份文档 `git status` 干净、处于提交态 ⇒ **既存红**，本批只归属不代改（避免与并发批次抢文档行）。

## 3. 被否 / 推迟的方案

| 方案 | 判定 | 理由 |
|---|---|---|
| 只改根 e2e 测试为调用 `pwsh` | 否 | 掩盖而非修复：11 个脚本在真实 Windows 用户手里仍不可用，且 BOM 缺失会继续潜伏 |
| 把 17 个文件的中文注释全部 ASCII 化 | 推迟 | 对齐 ADR-187 字面要求，但 churn 大、易误删注释；补 BOM 已消除解析风险。ASCII 化列为后续独立小批 |
| 只修被测试调用的那 1 个 `.ps1` | 否 | 实测爆炸半径是 11 个文件/55 个错误，只修一个等于留下 10 个静默不可用的"权威执行体" |
| 登记 G-85 但不接 CI | 否 | 正是本 ADR 要消灭的形态；且 `audit_gate_parity` 会立刻红 |
| B1 默认 `apply` | 否 | 见 D3；行为变更需显式授权 |
| 用 `check_gates_provenance.py` 补"门存在性"审计 | 推迟 | 该脚本同样在盘上不存在（AGENTS.md 提及），落盘它需另立范围；本批以"G-85 实物 + CI 接线"消除当下风险，并把"门脚本实物缺失审计"作为遗留项写入进度报告 |

## 4. 后果

**正面**：Windows 侧 5.1 权威执行体从"从未跑成"变为 24/24 可解析，根 e2e 5/5 转绿，全量 `cargo test --workspace` 失去最后一个非本仓可控的阻力；守门门实物化并接入阻断位，同类回归（新增非 ASCII 的 `.ps1` 忘带 BOM）从此必红；阈值表首次具备"用真实样本改写自己"的生产路径，且默认零风险。

**代价与遗留**：① 17 个文件各 +3 字节的 diff 噪声（无逻辑变化）；② 采样在 1/64 窗口引入 `Instant` 调用，须以 bench 同轮对照证明落在噪声带内（本仓同机实测方差 ±40%，故不主张小于该量级的差异）；③ `apply` 模式尚未在真实负载下验证，`update_threshold` 的 WIRED 升级以生产调用点存在为准（B1 落地后由 `check_dead_contract_apis.py` 复判）；④ 既存红 GAP-B1（crate 数未记入三份文档）与本批无关，留待文档批次处理。
