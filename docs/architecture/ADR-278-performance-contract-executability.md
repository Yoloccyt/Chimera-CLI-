# ADR-278 -- 性能契约可执行性：I11 + `[[redline_exec]]` / `[[excluded]]` + `mode_slo` 语义修复

- 状态：Accepted（2026-11-10）
- 决策者：W0 波次（承接 ADR-239「收益不显著即否决」、ADR-249「门先行」、ADR-277「防重提」）
- 关联：`scripts/perf_redlines.toml`、`scripts/check_perf_redlines.py`、`.github/workflows/perf_redline.yml`、`docs/architecture/adr_index.md`
- 影响面：门禁脚本与表体、新增一条 workflow；**零 Rust 生产代码**
- 行为影响：`mode_slo` 在「redline < measured <= SLO」时由 PASS 改为 **FAIL**；多 `time:` 行由「读第一条」改为 **rc=2 不可判定**；新增 `redline` 模式。其余模式逐字不变。

## 1. 背景：一半的性能契约是空转的

2026-11-10 对 `scripts/perf_redlines.toml` / `perf_thresholds.toml` / `check_perf_redlines.py` / `.github/workflows/` 逐项复测，得到以下事实（命令随行）：

| # | 事实 | 证据 |
|---|---|---|
| 1 | `[[redline]]` 按设计只是**静态 lint**（文件存在 / fn 存在 / 阈值标记存在），不断言运行期数值 | `perf_redlines.toml` 头部「字段语义」段 |
| 2 | 11 条 redline 中 **9 条**的指标既无 `thresholds_ns` 键、其 bench 也不在 `run_bench` | 逐键 grep `perf_thresholds.toml` 全 0 命中；`run_bench` 56 条中 `decay-engine`/`omega-learner`/`mca-gateway` 零条目，`repo-wiki` 只有 `hybrid_search_bench`，`event-bus` 只有 `cross_layer_bench`/`bus_bench` |
| 3 | 6 条 `[[slo]]` 中 **5 条**的 bench 不在 `run_bench` | `[[slo]]` 与 `[[run_bench]]` 集合差：`window_select`/`mlc_l2_knn`/`decay_compute`/`wiki_knn_100k`/`linucb_40arm` 缺席；仅 `immune_probe` 在册 |
| 4 | `mode_slo` 会真跑 criterion 并断言，但**没有任何 workflow 调用它** | `grep -rn check_perf_redlines .github/workflows/` 只出现 `compare` / `emit-slo-filter` / `--selftest` / `--static-only` |
| 5 | `mode_slo` 的「80% 红线」是标签不是门：`<= redline_sec` 与 `<= slo_sec` **都打印 PASS** | `check_perf_redlines.py` 旧 `mode_slo` 尾段 |
| 6 | `mode_slo` 只取**首个** `time:` 行；`window_select/` 实际命中 4 个档位（L0_4K…L3_1M），旧实现断言的是**最轻档** | `crates/hcw-window/benches/window_select.rs` 的 4 个 `BenchmarkId::from_parameter` |
| 7 | I3 用 `(crate, bench)` 集合、I7 用 `(crate, bench) -> alias` 字典 ⇒ 同一 bench 的多个 filter 分组会被**静默折叠** | `check_perf_redlines.py` 的 `mode_invariants` I3 与 `ci_runbench_sync` |

**结论**：14 条契约（9 redline + 5 SLO）无运行期断言；`mode_slo` 即使被调用也不会在 80% 处判红。这是「可验证」要求的根缺陷，也是后续任何性能优化无法被机器证明的原因。

## 2. 决策

### D1 — 不变式 I11：声明了就必须能执行

在 `mode_invariants` 增加 **I11**（I10 已被 PATH_KEYWORDS 键集对拍占用）：

> 每条 `[[redline]]` 必须恰好满足以下之一：映射到 `[[redline_exec]]`，或登记 `[[excluded]]`。
> 每条 `[[slo]]` 必须由 `perf_redline.yml` 真实调用 `check_perf_redlines.py slo`（或被 excluded）。
> `[[excluded]]` 必须带 `owner` + `expiry` + `reason`，过期即红，只减不增。

I11 放在现有 `mode_invariants` 内 ⇒ 被现有 G-02/CI 的 `--static-only` 覆盖，**不新增门、不动 govd 普查（140/133/7）**。

### D2 — 新增两张表：`[[redline_exec]]` 与 `[[excluded]]`

`[[redline_exec]]` 字段：

| 字段 | 语义 |
|---|---|
| `id` | redline id |
| `surface` | `bench` / `slo` / `nextest` |
| `crate` / `bench` / `filter` | `surface=bench` 的执行目标；filter 必须**恰好命中 1 条** criterion id |
| `assert_ns` | `surface=bench` 的运行期阈值（纳秒） |
| `slo` | `surface=slo` 引用的 SLO 名（同一 bench/filter，避免重复执行） |
| `test` / `workflow` | `surface=nextest` 的测试名与归属 workflow |

`[[excluded]]` 字段：`id` / `owner` / `expiry` / `reason`。

### D3 — 新增 `mode_redline`：红线的唯一实跑面

`mode_redline` 逐条执行 `surface=bench` 的映射，调用 `cargo bench ... --quick <filter>`，
解析 criterion `time:` 行并与 `assert_ns` 比较。**恰好 1 条 time 行**才判定；
0 或多于 1 条 ⇒ rc=2（不可判定），不把 filter 失配冒充成绿。

辅助纯函数：`single_criterion_time(out)` 与 `slo_verdict(est, redline, slo)`，
使「多 time 行不可判定」与「80% 红线是门」可被 selftest 零 I/O 断言。

### D4 — `mode_slo` 语义修复

- 「redline < measured <= SLO」由 PASS 改为 **FAIL（fail_redline）**，与文件头部「80% 宽松门」及函数 docstring「对 80% 红线断言」一致；
- 若某条在 80% 处不稳定，正确做法是登记 `[[excluded]]`（owner+expiry），**不得**把阈值改成 100%；
- `window_select` 的 filter 由 `window_select/` 改为 `window_select/L3_1M`（最重档，保守判据），与 `decay_compute` 取 `aggressive` 同一原则。

### D5 — I3 / I7 身份改为三元组

I3 的 `run_bench` 唯一性键由 `(crate, bench)` 改为 `(crate, bench, alias)`；
I7 的 CI↔表对拍按 `(crate, bench)` 分组比较 alias 集合。这样同一 bench 可以有多个 filter 分组，
同时保留原有的 `alias drift` 诊断（selftest-10d 仍为 1 条）。

### D6 — CI 执行面：`perf_redline.yml`

- `perf-contract-static`（阻塞）：`check_perf_redlines.py --static-only`（I11 在内）；
- `perf-contract-exec`（观察期 `continue-on-error: true`）：依次跑 `slo` 与 `redline`。
- 切换条件：连续 2 晚全绿且无 SKIP 后改为阻塞，并在本 ADR 追记切换提交。
- 新增 workflow 引用的 `check_perf_redlines.py` 是既有 G-01/G-02 注册脚本，
  G-137 方向 A（workflow 引用必须已登记）与方向 B（未跑的门）实测均 rc=0。

### D7 — RL-08 显式豁免

RL-08 是「50 agent 内存峰值 ≤130MB」，不是时间阈值，无法用 criterion time 断言；
其真实构造轨阻塞于 MAS 多 Agent 常驻进生产（ADR-263 D3）。
登记 `[[excluded]]`：`owner=nexus-omega`、`expiry=2026-12-31`，到期必须重新裁决（接线或续期附新证据）。

## 3. 11 条 redline 的执行面映射

| redline | 指标 | surface | 目标 / 阈值 |
|---|---|---|---|
| RL-01 | window_select <1ms | bench | `chimera-mas/mas_benchmark` filter `window_select/l3_ultra_complex`，1ms |
| RL-02 | mlc_l2_knn <5ms | bench | `mas_benchmark` filter `mlc_l2_knn_top10_4096/top10`，5ms |
| RL-03 | decay <1us | bench | `decay-engine/decay_bench` filter `single_decay_step_latency/time_decay`，1µs |
| RL-04 | wiki_knn @1000 <10ms | bench | `repo-wiki/vector_bench` filter `single_thread_knn_latency/1000`，10ms |
| RL-05 | @10K p95<50ms | nextest | `slo.yml` 的 `test_hnsw_10k_p95_below_50ms` |
| RL-06 | @100K p95<50ms | nextest | `slo.yml` 的 `test_hnsw_100k_p95_below_50ms` |
| RL-07 | membrane p95<10ms | bench | `event-bus/membrane_delivery_bench` filter `membrane_e2e_delivery/publish_to_recv`，10ms |
| RL-08 | 50agent mem ≤130MB | **excluded** | 非时间指标；ADR-263 D3 阻塞（到 2026-12-31） |
| RL-09 | linucb 40arm <50us | slo | 复用 `[[slo]] linucb_40arm`（同 bench 同 filter） |
| RL-10 | cost_estimate <1us | bench | `mca-gateway/mca_hot_paths` filter `cost_estimate_1m_tokens_peak`，1µs |
| RL-11 | sse normalize <5us | bench | `mca-gateway/mca_hot_paths` filter `sse_normalize_100_text_deltas`，5µs |

6 条 SLO（window_select / mlc_l2_knn / decay_compute / wiki_knn_100k / immune_probe / linucb_40arm）
由 `mode_slo` 统一执行，`perf_redline.yml` 是其 CI 调用者。

## 4. 验收（现跑）

### 4.1 门禁自证

```
py -3 scripts/check_perf_redlines.py --selftest
  RESULT: PASS (all 71 assertions held)          # 含 23a-23i（I11）与 24a-24f（SLO 语义/解析）

py -3 scripts/check_perf_redlines.py --static-only
  I11 every redline/slo has an execution path or exclusion (exec=10 excluded=1)   [PASS]
  VERDICT: PASS (rc=0, failing_submodes=none)

py -3 scripts/check_py_tooling_health.py       [OK] eol_rows=218 imports=599 py_files=118 python_jobs=7 stdio_dups=24 workflows=12
py -3 scripts/check_workflow_yaml_lexical.py   scanned=12 files lines=3377 findings=0
py -3 scripts/check_ci_manifest_parity.py      workflows=5 ci_refs=148 ci_steps=70 manifest_gates=142 matched=69 unmatched=0 gate_missing=0
```

### 4.2 实跑（本机 Windows GNU；CI 为权威口径）

### 4.2 实跑（本机 Windows GNU，`--quick` 档，criterion 点估计/median；CI 为权威口径）

| 契约 | 目标 | 实测 | 余量 | 判定 |
|---|---|---|---|---|
| RL-01 `window_select/l3_ultra_complex` | < 1ms | **1.2656 ns** | ~79 万× | PASS |
| RL-02 `mlc_l2_knn_top10_4096/top10` | < 5ms | **136.94 µs** | ~36× | PASS |
| RL-03 `single_decay_step_latency/time_decay` | < 1µs | **66.125 ns** | ~15× | PASS |
| RL-04 `single_thread_knn_latency/1000` | < 10ms | **399.56 µs** | ~25× | PASS |
| RL-07 `membrane_e2e_delivery/publish_to_recv` | < 10ms | **14.954 µs** | ~669× | PASS |
| RL-10 `cost_estimate_1m_tokens_peak` | < 1µs | **5.6172 ns** | ~178× | PASS |
| RL-11 `sse_normalize_100_text_deltas` | < 5µs/**event** | **108.9 µs / 100 = 1.089 µs/event** | ~4.6× | PASS |
| SLO `window_select` | < 0.8ms | **1.4758 ns** | 极大 | PASS |
| SLO `mlc_l2_knn` | < 4ms | **207.63 µs** | ~19× | PASS |
| SLO `decay_compute` | < 0.8µs | **70.087 ns** | ~11× | PASS |
| SLO `wiki_knn_100k` | < 40ms | **161.38 µs** | ~248× | PASS |
| SLO `immune_probe` | < 80ms | **566.28 ns** | 极大 | PASS |
| SLO `linucb_40arm` | < 40µs | **7.2338 µs** | ~5.5× | PASS |

`mode_redline`：`failed=0 skipped=0`（7/7）；`mode_slo`：`failed=0 skipped=0`（6/6）。
RL-05/06 由 `slo.yml` 的 13 条 `#[ignore]` p95 测试执行（CI 权威，本轮未跑）；RL-08 已豁免。

### 4.3 实跑发现的两处量纲/解析陷阱（本轮由真实执行捕获）

1. **RL-11 的阈值是 per-event，criterion 迭代是 100 events**：`sse_normalize_100_text_deltas`
   每次 `b.iter` 喂 100 帧；首版把 `assert_ns` 写成 5000（5µs）⇒ 实测 107.29µs 被误判
   为超限 21×。正确写法 = `5µs × 100 = 500_000 ns`，实测 1.089µs/event 才是真值。
   **教训**：声明「每事件」的 redline 接入 criterion 时必须换算到「每次迭代」。
2. **criterion 有 baseline 时会多打一个 `change:` 块**，其内也含 `time: [-21.9% …]`（单位 %）。
   首版 `single_criterion_time` 按「恰好 1 条 time 行」判定 ⇒ 带 baseline 的正常运行被
   误判为不可判定（RL-07 实测 `cargo exit=0` 却 SKIP）。修正为「恰好 1 条**可解析为时长**
   的 time 行」：`%` 单位不在 `UNIT_TO_SEC`，自然被排除。selftest-24f 锁住该形态。

## 5. 回滚

**单 commit revert（实际落地形态）**。ADR 初稿设想拆成三个独立 commit
（语义 / 表+I11 / mode_redline+workflow），落地时改为单 commit：I11、`[[redline_exec]]`
表与 `perf_redline.yml` 三者**互锁**（新门四件同批纪律）——拆开会让中间态 I11 判红，
且「门 + 被检对象」必须同批进出。

单 commit 回滚后 `mode_redline`/I11 一并消失，`--static-only` 回到旧口径；
`mode_slo` 回到旧的「80% 不判红」语义（那是缺陷回来了，不是误报）。
若需保留 I11 而只撤执行面，正确做法是给对应条目登记 `[[excluded]]`，不是放宽 I11。

## 6. 未采纳

| 项 | 结论 |
|---|---|
| 把 9 条 redline 的指标全部塞进 `thresholds_ns` + `path_keywords` + `run_bench` + heredoc | 否决：会再制造一处四表同步（I1/I7/I9/I10），且 `[[redline]]` 的 `threshold` 多为符号（`LARGE_SIZE` 等），数值真值源反而更模糊。改为独立的 `redline_exec.assert_ns` + `mode_redline`，与现有四表解耦 |
| 把 80% 红线降级为观察线（保持旧 PASS 语义） | 否决：与文件头与 docstring 的「断言」措辞直接矛盾；「文档说断言、代码不判红」正是本轮要消灭的类别 |
| 多 `time:` 行时取中位数 / 取最大 | 否决：filter 失配时「取最大」仍可能读错对象；rc=2 让判据未运行可见 |
| 为 RL-08 新增内存采样断言 | 暂缓：需要 MAS 50-agent 真实构造轨（ADR-263 D3），当前为公式估算；先豁免并设到期日 |
| 新增独立 gate id 承载 I11 | 否决：I11 放进现有 `mode_invariants` 即被 G-02 覆盖，避免新增门触发 govd 普查与四件同批成本 |

## 7. 证据边界

- 本 ADR **不声称**「14 条契约全部在本机通过」：CI 是实跑权威口径（`perf_redline.yml`），
  本机结果见 §4.2；观察期内若某条 SKIP（rc=2），必须修正 filter 或登记 excluded，不得放宽阈值。
- `mode_slo` / `mode_redline` 的断言对象是 criterion 的**点估计**（`--quick` 档），
  不是 p95；p95 红线由 `slo.yml` 的 13 条 `#[ignore]` 测试承担（RL-05/06 在其中）。
