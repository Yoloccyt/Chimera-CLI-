# ADR-239 -- 工具链性能阈值基线与棘轮登记（四环链分层归因 + 批量并行 + CLI 启动分段）

- 状态：Accepted（2026-11-06）
- 决策者：工具链并行化重构第一波（W1）
- 关联：ADR-237（权限三态 fail-closed）、ADR-238（强制沙箱派发链四环）、ADR-225（阈值双源对拍与切流）、ADR-228（govd 调度治理七态判决）
- 影响面：`scripts/perf_thresholds.toml`、`scripts/bench_inventory_freeze.txt`、`scripts/ignored_test_inventory_freeze.txt`、`.github/workflows/bench_check.yml`、`crates/gqep-executor/benches/tool_dispatch_layers.rs`、`crates/gqep-executor/benches/tool_runtime_batch.rs`、`crates/chimera-cli/benches/cli_parse.rs`、`crates/mcp-mesh/benches/registry_lookup.rs`、`crates/chimera-tui/benches/tui_tick_bench.rs`

## 1. 背景与问题

ADR-238 建立了强制沙箱派发链（policy → sandbox → route → transport），并在同批引入了分层归因基准与批量吞吐对照基准。但**基准存在不等于判据存在**：

| 缺口 | 实测证据 | 后果 |
|------|----------|------|
| bench 三态清单未登记 | `python scripts/check_perf_redlines.py inventory` → `counts: total=124 gated=11 registered=45 dev-only=62 unknown=6`，`RESULT: FAIL`，**退出码 1** | 新增 bench 被门禁判 `unknown` ⇒ 性能红线门禁**当前为红**，阻断提交 |
| 归因基准无阈值 | `perf_thresholds.toml` 的 `[thresholds_ns]` 键集不含任何 `dispatch_layers/*` / `dispatch_chain/*` 键；`perf_redlines.toml` 的 `[[slo]]` 段无 `tool_dispatch_layers` 条目 | 分层归因测点**从未被任何阈值评估过** —— 无法回答「改完是否变快」，也无法回答「是否变慢」 |
| 启动优化无端到端验收 | `main.rs` 已有 4 段计时（`parse_and_init_ms` / `config_ms` / `runtime_ms`），但目标值未登记；`cli_parse` bench 仅覆盖 clap 解析 | D1/D4/D5 三项启动优化的收益无门禁守护，后续重构可静默回退 |
| 新增 `#[ignore]` 测试未登记 | `check_perf_redlines.py all` → `ignored` 子模式 FAIL：`unregistered ignored test: gqep-executor crates/gqep-executor/tests/tool_runtime_batch_latency.rs::serial_vs_parallel_latency_percentiles` | 「写了不跑」未被登记为决策，门禁判红 |

根因是同一个：**性能观测已建，判据未建**。本 ADR 补齐判据侧。

## 2. 决策

### 2.1 D1：bench 三态登记 —— 升格 `registered` 而非 `dev-only`

`scripts/bench_inventory_freeze.txt` 的四态语义：`gated`（被 `perf_redlines.toml` 引用）/ `registered`（被 `perf_thresholds.toml` 的 `[[run_bench]]` 引用）/ `dev-only`（显式登记为无 SLO）/ `unknown`（未落入任何一态 ⇒ FAIL）。

本波新增的 6 个 bench 全部升格为 `registered`（写入 `[[run_bench]]`），**不**进 `dev-only`。

**WHY 不进 dev-only**：`dev-only` 的语义是「显式声明的无 SLO 开发/实验基准」——它让 bench 可跑但**不被任何阈值评估**，正是本次要消除的缺口。这 6 个 bench 是并行化重构的**判据来源**，必须进 CI 的 Assert thresholds 步骤。

**登记纪律**：清单头注释要求「新增一行须附理由 + ADR 号」。本波以 `[[run_bench]]` 形式登记（该形式由 `perf_thresholds.toml` 承载，`bench_inventory_freeze.txt` 无需新增行），ADR 号由本文件提供。

### 2.2 D2：阈值一律先实测后登记，禁止拍脑袋

阈值来源：本机 Windows GNU 全量实测 median（2026-11-06，criterion）。余量按 `perf_thresholds.toml` 既有分级原则 —— **ns 级 ×10、us 级 ×3-8、ms 级 ×2-2.5**（与既有 18 键校准同原则，见 `docs/reports/c6-kpi-threshold-calibration-report-2026-09-04.md`）。

**WHY 沿用分级余量而非严格取 median**：criterion median 是单一机器、单一负载下的中位数；CI runner 与开发机的 CPU 频率、后台负载均不同。分级余量是该仓库既有的抗抖动约定，本波不新造口径。

### 2.3 D3：阈值键名必须互不为子串（不变式 I2）

`check_perf_redlines.py` 的 `keyword_overlaps` 断言**任意方向**的真子串关系，基线为 `max_keyword_overlaps = 1`（只减不增棘轮）。匹配循环是 first-match-wins —— 一个路径同时命中两键时归给先出现者，键名互为子串即造成「长路径被短键窃取」的串台（`cache_hit` 污染是既有先例）。

故本波采用**四个互不包含的前缀族**：

| 族 | 覆盖测点 |
|----|---------|
| `dispatch_` | 分层归因单点（传输地板 / 策略层 / 沙箱层） |
| `dchain_` | 链级端到端（allow / deny / route miss + legacy 对照） |
| `tbatch_` | 批量派发（串行 vs 并行，零延迟 / 慢速） |
| `trchain_` | 工具运行时链（独立 bench） |

外加 `cli_` 族（CLI 启动分段）与 `mcp_registry_lookup`（MCP registry）。

### 2.4 D4：拒绝路径也必须有阈值

`dchain_policy_deny` / `dchain_sandbox_deny` / `dchain_route_miss` 三键锁定的是 **fail-closed 的性能特征**：拒绝路径必须**快于**放行路径（否则拒绝将成为攻击面 —— 用大量拒绝请求耗尽 worker）。

实测：policy deny 73ns < allowall 539ns < seccore 1698ns。

### 2.5 D5：`legacy` 对照测点必须保留在门禁内

`dispatch_sandbox_uncached`（3094ns）与 `dchain_seccore_legacy`（3694ns）是**优化前实现的同轮对照组**。保留它们在门禁内有两个作用：

1. 若沙箱缓存被静默移除，链级端到端会跳到 legacy 量级 ⇒ `dchain_seccore_hit` 阈值（40µs）会红；
2. 保留对照数据，使「缓存收益」这一结论可持续复核，而非一次性快照。

**代价**：legacy 路径在每次 CI bench 中多跑一组。接受 —— 这是让性能结论可证伪的必要成本。

### 2.6 D6：零延迟下并行略慢是预期，不得据此判红

实测 `tbatch_serial_zero`（146µs）**小于** `tbatch_parallel_zero`（165µs）。

**WHY 这是预期**：零延迟场景下每条请求立即完成，并行化无收益可摊，只有调度开销（`FuturesUnordered` 入流 + 槽位回填）增量 ≈ 19µs。因此：

- `tbatch_parallel_zero` 的阈值锁定的是「**并行开销不失控**」，不是「并行更快」；
- **核心收益判据**是 `tbatch_serial_slow`（493ms）vs `tbatch_parallel_slow`（112ms）—— 1ms 模拟延迟下的 **4.4× 加速**，这才是真实 I/O 场景的收益形态。

若未来有人把零延迟场景的「并行更慢」当成 bug 去「优化」，本 ADR 的这一条即是反驳依据。

### 2.7 D7：归因口径由两组独立测点交叉印证

分层归因的两组测点（单点微基准 vs 链级端到端残差）互相印证：

| 环 | 单点实测 | 链级残差 | 偏差 |
|----|---------|---------|------|
| 传输地板 | 58ns | （基准） | — |
| 策略层 | 465ns | 539 − 58 = 481ns | 3.4% |
| 沙箱层 | 1179ns | 1698 − 539 = 1159ns | 1.7% |

**WHY 要交叉印证**：单点微基准可能测到编译器未内联掉的代码路径；链级残差可能混入其他环的噪声。两组独立测点吻合到 3% 内 ⇒ 归因结论可信。

### 2.8 D8：新增 `#[ignore]` 测试必须登记为决策

`crates/gqep-executor/tests/tool_runtime_batch_latency.rs::serial_vs_parallel_latency_percentiles` 归入 `slo-daily`（比值类、需真实墙钟、不阻塞 PR）。

**WHY 是 slo-daily 而非 slo-blocking**：它断言的是「串行 vs 并行的延迟分位数对比」，是**比值类**而非绝对 ms 级红线；且需真实墙钟，release 阻塞门要求静默态单跑，不适合该形态。

### 2.9 D9：CLI 启动分段以「等价路径」测量，不复制 `main.rs` 逻辑

`main.rs` 的 `build_runtime` / `use_current_thread` 是二进制 crate 的私有函数，bench 无法引用。故 `cli_parse` bench 以**等价路径**复现三段测量：

| 段 | `main.rs` 实现 | bench 等价测量 |
|---|---|---|
| parse+init | `Cli::parse()` + 颜色模式 + tracing 初始化 | `Cli::try_parse_from`（help/run/doctor 三档） |
| config | `spec::needs_full_config(cmd)` 决定是否 `config::load()` | `spec::needs_full_config` **判定**（不含 14 段 Figment 解析） |
| runtime | `build_runtime(spec::classify(cmd))` | 按 `spec::classify` 选 flavor 后构建 tokio runtime |

**WHY config 段只测「判定」而非「真实加载」**：真实加载读盘 + 解析 14 段，受文件系统缓存影响，噪声远大于信号；判定是纯函数，是 D5 优化的**实际决策点**（决定是否付那笔税），测判定即可锁定决策路径未退化。

**WHY 不抽 `main.rs` 逻辑到 lib**：那是与本次优化目标无关的重构（且会扩大改动面）。等价测量 + 注释说明口径差异，是本仓「最小必要改动」原则下的正解。

## 3. 后果

### 3.1 正向

- 性能红线门禁从 `unknown=6` / EXIT=1 转为 **PASS / EXIT=0**，解除对提交的阻断。
- 22 个新阈值键**全部命中真实 criterion 值组**（零幽灵键）—— 经 `tmp/check_new_thresholds.py` 核验：`matched=22/22 ghost=0 over_threshold=0`。
- 四环链各环成本首次有了**客观、可回归的数字**；后续任何改动都能回答「是否变慢」。
- 不变式全绿：I1 键集双向对等（75/75）、I2 重叠数未增（1 ≤ 1）、I7 CI 与登记表 53 对相等、I9 heredoc 键集 75 相等。

### 3.2 负向与代价

- CI bench 时长增加：6 个新 bench 进入 `bench_check.yml` 的 run_bench 列表。
- `legacy` 对照组每轮多测一组（接受，见 D5）。
- 阈值一旦登记即成棘轮，后续**只能收紧不能放宽**（放宽需单独 ADR 说明理由）。

### 3.3 关键量化结论（本 ADR 的核心产出）

| 结论 | 数值 | 依据 |
|------|------|------|
| 沙箱缓存收益（单点） | 3094ns → 1179ns，**降低 61.9%** | `dispatch_sandbox_uncached` vs `dispatch_sandbox_cached` |
| 沙箱缓存收益（链级） | 3694ns → 1698ns，**降低 54.0%** | `dchain_seccore_legacy` vs `dchain_seccore_hit` |
| 有界并发收益（1ms I/O，64 请求） | 493ms → 112ms，**4.4× 加速** | `tbatch_serial_slow` vs `tbatch_parallel_slow` |
| 策略层成本 | 465ns（占 allowall 链 86%） | `dispatch_policy_eval` + 链级残差交叉印证 |
| 沙箱层成本 | 1179ns（占 seccore 链 69%） | `dispatch_sandbox_cached` + 链级残差交叉印证 |
| multi_thread runtime 构建 | 783µs（one-shot 命令启动税主项） | `cli_startup_runtime` |
| 启动端到端（parse+分类+runtime） | 982µs | `cli_startup_e2e` |

## 4. 验收证据（2026-11-06 实测）

```
python scripts/check_perf_redlines.py inventory
  counts: total=124 gated=11 registered=51 dev-only=62 unknown=0
  RESULT: PASS (every bench explicitly guarded)          EXIT=0

python scripts/check_perf_redlines.py invariants
  [PASS] I1 thresholds_ns keys == path_keywords keys (75/75)
  [PASS] I2 keyword substring overlaps 1 <= 1
  [PASS] I7 bench_check.yml run_bench lines == [[run_bench]] table (53 pairs)
  [PASS] I9 bench_check.yml heredoc keys == [[thresholds_ns]] keys (75 keys)
  RESULT: PASS (all invariants hold)                     EXIT=0

python tmp/check_new_thresholds.py
  SUMMARY: matched=22/22 ghost=0 over_threshold=0       EXIT=0

python scripts/check_perf_redlines.py all
  VERDICT: PASS (rc=0, failing_submodes=none)           EXIT=0
```

## 5. 失效条件

1. 若 `perf_thresholds.toml` 的 `[thresholds_ns]` 与 `[path_keywords]` 键集不再对等，I1 会先红。
2. 若新增阈值键的关键词与既有键构成真子串关系，I2 棘轮会先红（`max_keyword_overlaps` 只减不增）。
3. 若 `bench_check.yml` 的 run_bench 行与 `[[run_bench]]` 表不等，I7 会先红。
4. 若 `bench_check.yml` 的 heredoc `THRESHOLDS_NS` 键集与 `[thresholds_ns]` 不等，I9 会先红。
5. 若沙箱派生策略缓存被移除，`dchain_seccore_hit`（阈值 40µs）会因链级耗时跳到 legacy 量级而红。
6. 若批量派发的并行路径退化为串行，`tbatch_parallel_slow`（阈值 400ms）会红。
7. 若 `cli_startup_*` 任一段退化，`cli_startup_e2e`（阈值 5ms）会红。

## 6. 后续

本 ADR 只覆盖**判据侧**。四环链生产接线、批量派发接入生产、借用化契约与无锁读分别由后续 ADR 覆盖（见工具链并行化重构 Wave 2 / Wave 3）。
