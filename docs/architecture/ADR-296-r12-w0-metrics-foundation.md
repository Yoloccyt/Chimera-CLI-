# ADR-296: R12 W0 —— 度量地基与口径收口（两个新基准 + 三处口径订正 + 两条长期红测试的实质收口）

- 状态: Accepted
- 日期: 2026-11-14
- 批次: R12 W0（八波次计划的第一波；设计依据见 `docs/architecture/_blueprints/toolchain-r12/`）
- 关联: ADR-295(R11 三域收敛) / ADR-239(工具链性能阈值基线与棘轮) / ADR-281(混层写屏障分段) / ADR-291(HCV v3 P0 能力档案) / ADR-234(性能/并行化验证判据四层——原典在未合并分支 `feat/toolplane-execution`，主线物理缺档)

---

## 1. 背景：R11 收口留下三类"读不清"

R11（ADR-295）把三域收敛批次做完了，但收口报告与既有基准表留下三类会让后续决策失准的问题：

| # | 问题 | 实测证据 |
|---|---|---|
| 1 | **判据结构性不足**：既有 `toolplan_layers` 只测**单层**（`readonly8_ms2` / `undeclared8_ms2` / `mixed7r1w_ms2`），单层没有层间 barrier，**结构上测不出**「一层一个慢节点把后续所有层按住」的代价；跨执行面的过订阅（六面各 8）无任何基准 | `crates/gqep-executor/benches/toolplan_layers.rs:1-21`；未合并分支 ADR-233 追加节一实测：均匀 16 节点改「就绪即派发」仅 31.39ms→31.25ms（≈0.4%），收益全在偏斜场景而偏斜无基准 |
| 2 | **口径漂移**：workspace crate 数被记成 47（实测 46）；未合并分支改动规模被记成「118 文件」（那是 `crates/` 子集，全仓三点 diff 为 153 文件 / +21710−740） | `crates/*/Cargo.toml` 实测 46；`git diff --shortstat HEAD...feat/toolplane-execution` 与 `-- crates` 两个口径 |
| 3 | **基线污染**：`target/criterion/<group>/<bench>/new/estimates.json` **跨会话持久**，跨会话 `change:` 百分比会把机器负载当代码回退 | 本轮只跑 `dispatch_*`，目录里同时存在 `toolplan_layers`(11-12 21:14)、`l2_knn_slo`(11-12 08:08) 等旧组；`chain_*` 显示 +39%~+131% 假回退 |

另有一条长期红：`l0_exception_registry` 的两条不变量在 HEAD 态即红（R11 报告 §5 已归因为并行会话提交 `2430d44c` 的登记错位），使全量回归基线永远带 2 failed，**任何"failed 不增"的判据都被噪声吃掉**。

---

## 2. 决策

### D1 —— 新增两个基准，登记态 `dev-only`，不设 CI 阈值

| 基准 | 测什么 | 为什么必须是它 |
|---|---|---|
| `crates/gqep-executor/benches/plan_dag_skew.rs` | `depth × width` 多层 DAG，每层第 0 列服务时间放大 `skew` 倍；组内对照 `uniform_w8_d4` / `skew2x` / `skew4x` / `uniform_w32_d4` / `skew4x_w32` | 补 `toolplan_layers` 的单层盲区；它是「层间 barrier 损失随偏斜单调上升」这一预测的唯一可判形状（W4 落地后同文件复用） |
| `crates/gqep-executor/benches/sched_plane.rs` | 四面各 8 路（现状拓扑）vs 全部共享 8 路（目标拓扑）的**在飞峰值**与 makespan | 跨面过订阅此前无量化面；用 bench 内最小复刻取基线，**不引用 W6 才落地的 `ResourceGovernor`**——先有判据、后有实现（ADR-234 A2-2） |

**为什么登记 `dev-only` 而不是 `registered`**：`registered` 要求进 `perf_thresholds.toml` 的 `[[run_bench]]` 并由 CI 断言阈值；而阈值的合法性前提是**先在本机实测再有后验余量**（ADR-239 D2「阈值一律先实测后登记，禁止拍脑袋」）。W0 阶段只有首跑量级锚，直接登记 `registered` 等于把噪声锁成门禁。升格条件已写进 `scripts/bench_inventory_freeze.txt` 对应行。

### D2 —— 三处口径订正（一次改到底，不留双真值源）

1. **crate 数**：以 `crates/*/Cargo.toml` 现取为准 = **46**（旧记 47 作废）。派生文档同步订正：`00-R12-总纲.md`（A3/A4/D1 三处）与 `01-现状证据-…md` 抬头。
2. **未合并分支规模**：统一用**三点 diff 双口径**并同时写明——全仓 **153 文件 / +21710 / −740**，其中 `crates/` 下 **118 文件 / +18099 / −494**。只写一个数会被另一方误读成"同一件事"。
3. **ADR 编号缺口**：主线 `docs/architecture/` 在 `ADR-228` 后直接跳 `ADR-237`，缺 **ADR-229~236**（8 个号，实体在未合并分支）与 **ADR-250**（全库缺）。本批一切引用若指向这 9 个号，必须标注"来源分支"，不得让读者按号回查主线。

### D3 —— 两条长期红测试的收口（**本 ADR 的核心裁决**）

**现象**（实测复现）：

```
registry_equals_substantive_logic_set            FAILED
  多登记=["context_capability"]
every_registered_exception_is_a_crate_root_declared_module  FAILED
  ADR-033 例外登记名 `context_capability` 在 crate 根无对应 `pub use <group>::context_capability;`
```

成因：`context_capability` 的**真实落点是 sibling crate**（`crates/nexus-contracts-mca/src/mca/context_capability.rs`，`mca/mod.rs:7` 声明），但它被登记进了 `crates/nexus-contracts/src/lib.rs` 的 `ADR_033_EXCEPTION_MODULES`，而当时漏了两件同批动作——crate 根的 facade 声明、以及测试侧 `SUBSTANTIVE_LOGIC_MODULES` 的同步。

**试过并否决的修法**：从 `ADR_033_EXCEPTION_MODULES` 摘掉该行。
否决理由：摘掉后两条测试确实转绿，但 `scripts/check_l0_logic_ledger.py`（G-119）立刻报
`[REGISTRY-GAP] 1 module(s) carry derived logic but are absent from lib.rs ADR_033_EXCEPTION_MODULES: mca/context_capability`
—— 那等于**把真模块藏起来换取测试变绿**，是把治理漂移从测试面搬到台账面。

**采纳的修法（三处同批，缺一即回红）**：

1. `crates/nexus-contracts/src/lib.rs` 补根 facade：`pub use mca::context_capability;`（与既有 `pub use mca::affinity;` 同组，保持旧路径可达）；
2. 保留 `ADR_033_EXCEPTION_MODULES` 中的登记行，并把订正来历写进行内注释（防止后人再次"顺手摘掉"）；
3. `crates/nexus-contracts/tests/l0_exception_registry.rs` 的 `SUBSTANTIVE_LOGIC_MODULES` 补 `"context_capability"`。

**为什么三处必须同批**：`ADR_033_EXCEPTION_MODULES`（lib.rs）与 `SUBSTANTIVE_LOGIC_MODULES`（测试）是一对双向锁——只改一侧，测试的不变量 2（恰好相等）必红；而只补测试不补根声明，不变量 1 必红；只补根声明不补测试，不变量 2 仍红。G-119 又是从 lib.rs 现读登记名做 `REG=YES|NO` 与 `REGISTRY-GAP` 判定的，因此"摘登记"这条捷径在机器层被直接堵死。

### D4 —— 度量规程（写进 W0 报告，作为后续每波的取数纪律）

1. 取 `target/criterion` 结果**先看 mtime**；跨会话存档标 `STALE`，不得用于回退判定。
2. criterion 的 `change:` 是**跨会话比对**，不是同轮对照；只有同进程同轮的 A/B 差值才作因果证据。
3. 冷启动首轮只作量级锚（本仓实测首轮比后三轮高 29–43%）。
4. 编译或其他重活进行中不采集（本轮实测漂移 +39%~+131%）。
5. 报告句式遵循 ADR-234 A3 模板：L1 结构判据 + L2 方向性 + 「绝对阈值待 CI 首跑校准」。

---

## 3. 验收（全部机证，逐条可复跑）

| # | 判据 | 命令 | 实测结果 |
|---|---|---|---|
| 1 | 新基准全部落入四态、无 `unknown` | `py scripts/check_perf_redlines.py inventory` | `counts: total=131 gated=11 registered=54 dev-only=66 unknown=0` → `RESULT: PASS` |
| 2 | 两条长期红测试转绿 | `cargo test -p nexus-contracts --test l0_exception_registry --jobs 2` | `5 passed; 0 failed` |
| 3 | G-119 不因收口而开口 | `py scripts/check_l0_logic_ledger.py` | `[REGISTRY-GAP] 0 module(s) …`，rc=0 |
| 4 | 两个新基准可编译并产出基线 | `cargo bench -p gqep-executor --bench plan_dag_skew` / `--bench sched_plane` | 见 §4 |
| 5 | ADR 索引：新增编号已入表，遗留缺口**不被洗白** | `py scripts/adr_index_sync.py` | `UNINDEXED = [291, 292, 293, 294, 295]`（296 已消除）；exit=1 与开工前基线同类，缺的是 291–295｜

---

## 4. 基线读数与解读（W0 产出）

> 完整数值、理论模型与复跑命令见 `docs/reports/toolchain-r12-w0-baseline-2026-11-14.md`。
> 本机 Windows 11 + GNU，**绝对值不可与 CI Linux runner 直接比**；可用的是同轮比值与结构判据。

### 4.1 `plan_dag_skew`（base 20ms、c=8、depth 4）

| 形状 | 中位数 | 相对同宽均匀 |
|---|---|---|
| `uniform_w8_d4` | 124.62 ms | 1.00× |
| `skew2x_w8_d4` | 187.75 ms | 1.51× |
| `skew4x_w8_d4` | **374.37 ms** | **3.00×** |
| `uniform_w32_d4` | 498.60 ms | — |
| `skew4x_w32_d4` | 623.46 ms | 1.25×（对同宽均匀） |

**读法**：三类形状在 0.3% 内命中 `makespan = Σ_layers max(service)` 的解析式，说明量到的是调度形状而非噪声。
**核心结论**：barrier 的伤害不是"有长尾"，而是**层宽刚好等于并发上界、长尾无处重叠**——w8（宽=上界）下 4× 长尾线性放大成 3.00×，
w32（4 波）下同样长尾只放大 1.25×。W4 就绪派发应把 w8 形状拉回接近均匀档，而 w32 形状允许几乎不动。

### 4.2 `sched_plane`（4 面 × 8 任务，单面许可 8，任务 20ms）

| 拓扑 | 中位数 | 在飞峰值上界 |
|---|---|---|
| `per_plane_4x8`（现状） | 31.06 ms | ≤ 32 |
| `shared_budget_8`（目标） | 124.57 ms | ≤ 8 |
| 比值 | **4.01×** | 4× 峰值收敛 |

**读法（必须成对）**：现状买到墙钟、共享预算买到可预测性，本基准不主张谁更好。
W6 的判据因此**不是"跑得更快"**，而是 `oversubscription_bp ≤ 1000` + **分车道**峰值可分解
（ADR-234 A2-1：不得用合并高水位证某条车道没被钉住；本基准的单值 `peak` 只够做 W0 基线，不够做 W6 验收）。

### 4.4 覆盖率在本宿主**不可测**（实测，非推断）

R12 一度准备把"新增功能覆盖率 ≥85%"按"CI 才可测"登记。为避免把"没试过"写成"不可用"，本批实测了一次：

```powershell
cargo llvm-cov --package mca-gateway --package session-store --summary-only --jobs 2
```

结果 `rc=101`：

```
error[E0463]: can't find crate for `profiler_builtins`
  = note: the compiler may have been built without the profiler runtime
```

环境是 `stable-x86_64-pc-windows-gnu` + `cargo-llvm-cov 0.9.0` + 已安装 `llvm-tools-x86_64-pc-windows-gnu`。
失败点在 **build script 编译期**（`proc-macro2` 的 build.rs 即已红），说明不是"某个 crate 有问题"，而是
**该工具链发行版未随附 profiler runtime** ⇒ 与本机装不装 `llvm-tools` 无关。

**处置**：覆盖率是 **CI 侧门禁**，本地不宣称。

- 权威源：`scripts/coverage_baseline.toml`（`min_line_coverage = 85`、`target_line_coverage = 85`、`buffer_pp = 5`）+ `scripts/coverage_gate.py` + `.github/workflows/coverage.yml`（夜间 04:00 + dispatch，`cargo llvm-cov --workspace --no-cfg-coverage --json` 后 `coverage_gate.py --check`）。
- 最近一次 CI 实测（文件头注记载）：聚合行覆盖率 **92.97%**（43 成员 / 118 文件，逐 crate floor 全过），已越 `target 85`，故 `min` 由占位 50 提到 85。
- **本地替代判据**（本批实际使用）：新增分支必须被**具名用例**覆盖，且"守卫/阳性对照"成对出现（如 `threshold_point_uses_hnsw` / `threshold_minus_one_stays_exact`、`wire_on_never_dedups_conversation_turns`）。这不等价于覆盖率数字，但可复核、且不依赖不可用工具。

> ⚠ 同族风险：`cargo fuzz`（libFuzzer 仅适配 MSVC 目标）与 `--release` 压测档在本宿主同样受限，处置见 ADR-234 A4；本批未尝试，**不声称可用或不可用**。

### 4.5 一个会污染全仓 sleep 类基准的发现（决定级）

**Windows 默认定时器粒度 ≈ 15.6 ms**：`sleep(n)` 对 `n < 15.6ms` 一律落地为 ~15.6ms。
首跑实证：`base=2ms` 时三个形状读数 62.236 / 62.305 / 62.439 ms（**最大差 0.33%**），因为 2/4/8ms 全落进同一跳；
改 `base=20ms` 后立刻变成 124.62 / 187.75 / 374.37（1.00 : 1.51 : 3.00）。

**处置**：本批**不改**既有基准（`toolplan_layers` 的 `*_ms2`、`tool_batch` 的 `*_ms1` 等）——它们的**结构判据（峰值/顺序）仍然有效**，
但**墙钟数值不得用于 µs/ms 级推断**。后续新增基准一律 `base ≥ 20ms`，或用 `tokio::time::pause()` 的虚拟时钟。
本条同时解释了为什么本 ADR 的 D1 不给新基准设 CI 阈值：在粒度未解决前锁阈值，等于把定时器抖动锁成门禁。

---

## 5. 回滚

| 改动 | 回滚方式 |
|---|---|
| 两个新基准 | 删两个 `.rs` + 撤 `crates/gqep-executor/Cargo.toml` 两段 `[[bench]]` + 撤 `scripts/bench_inventory_freeze.txt` 两行 |
| 红测试收口 | **三处同退**（lib.rs facade 行、lib.rs 登记行注释、测试期望集行）；任何单退都会重新变红或变 REGISTRY-GAP |

---

## 6. 减法（本波明确不做）

- **不设 CI 阈值**：阈值须先有 CI 首跑校准（ADR-239 D2），本波只给量级锚。
- **不清理 `target/` / `tmp/`**：清理时机属用户（ADR-234 A4 同款纪律）。本波只记录 `tmp/` 现为 128 MB / 18,118 文件、`target/` 约 8.3 GB 这一事实，供后续判断快照/磁盘前置。
- **不修 ADR 编号缺口本身**（把 229~236 补进主线或登记为外部档）：那需要合流裁决（R12 Q1），属 W0 决策项而非 W0 代码项。
