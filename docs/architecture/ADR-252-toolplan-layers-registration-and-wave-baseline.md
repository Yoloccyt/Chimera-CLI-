# ADR-252 -- 工具链优化波次口径与 `toolplan_layers` 升格 registered（X1 红门收口）

- 状态：Accepted（2026-11-06）
- 决策者：工具链深度优化统一批次 X1（总方案 T 序第 2 波）
- 关联：ADR-239（阈值三态语义与棘轮纪律）、ADR-234（先实测后登记 / 四层证明力 / 测量态与提交态同一棵树）、ADR-241（并行化裁决先例：D5 否决 DashMap、D6 诚实记录净收益 0）、ADR-244（单一装配点）、ADR-246~251（已批准 W7 续作方案预留号段，未落盘）
- 影响面：`scripts/perf_thresholds.toml`、`.github/workflows/bench_check.yml`、`crates/gqep-executor/benches/toolplan_layers.rs`

## 1. 背景与问题

`95376f8 perf(gqep): 计划执行器分层串/并发化与不变量守护` 新增了 bench
`crates/gqep-executor/benches/toolplan_layers.rs` 与其 `Cargo.toml:67-69` 的 `[[bench]]`
声明，但**未登记任何性能台账**（既无 `[[run_bench]]` 行，也无
`bench_inventory_freeze.txt` 的 dev-only 行）。门的判定序是
`gated → registered → dev-only → unknown`（`check_perf_redlines.py:144-167`），
因此该 bench 落入 `unknown` ⇒ `inventory` 判红，并连带使 `all` 子模式判红。

同日收官报告 `docs/reports/toolchain-parallel-optimization-closure-2026-11-06.md §3.1`
自述 `inventory unknown=0 / VERDICT PASS`——**与 HEAD 实况不符**，因为该读数早于 `95376f8`。
W0 进度报告 `§3.1` 已独立复跑并取得真实读数（`unknown=1`），把它登记为基线债务与 Q-D。

本批同时确定「完整统一总方案」的波次口径：已批准的 W7 续作（W0~W6，ADR-246~251）
原样承接不重复施工，本轮勘察新增的增量波次编为 X1~X8 并 interleaved 进同一张总表。

## 2. 决策

### 2.1 D1：`toolplan_layers` 升格为 `registered`（否决 `dev-only`）

`dev-only` 路径同样能让 `inventory` 转绿，但它等于把一个**有真实并行性能语义**的基准
声明为「无 SLO」——这属降低门强度，与 ADR-234「不得为过门而弱化判据」、
ADR-239 §2.1 的三态语义（`registered` = 进 CI 阈值断言）相悖。
该 bench 度量的是「同一形状的只读层，声明 `side_effect` 与否，并发是否真生效」，
正是本分支的核心命题，必须有阈值守护。

### 2.2 D2：关键词取 criterion **叶子全路径**，不取组名

四个键的 `path_keywords` 用 `toolplan_layers/readonly8_ms2/1` 这类**全路径**，
而不是 `readonly8_ms2` 组名。理由：CI 侧匹配是
`if any(kw in rel_path for kw in keywords)` 后 `break`（`bench_check.yml` 的
Path 关键词唯一性原则注释同源），宽关键词会把并发档（`/8`）窃取进串行阈值，
或反之——即「阈值断言的对象与被测对象不是同一件事」。

I2（关键词双向互不为子串，`max_keyword_overlaps = 1` 已顶格）用**门自身的**
`keyword_overlaps()` 函数验证，而非另写一份判据：植入已知重叠对作正控，
读数由 1→2 被抓（证明探针有牙）；本批 4 键读数维持 1。
键数 75→79，`registered` 51→52。

### 2.3 D3：`undeclared` 两键登记的是「改前上界」，X3 负责收紧

本机实测 median（20 samples / criterion 默认，`cargo bench -p gqep-executor --bench toolplan_layers`）：

| 值组 | `concurrency=1` | `concurrency=8` | 并发是否生效 |
|---|---|---|---|
| `readonly8_ms2`（显式声明 read_only） | 69.086 ms | **14.901 ms** | 生效（4.6×） |
| `undeclared8_ms2`（`side_effect` 缺省 `None`） | 69.814 ms | **63.804 ms** | **不生效**（≈串行） |

⇒ 「未声明 ⇒ 整层串行」（`toolplan_runner.rs:373-377`，`SideEffectDecl` 缺省 `None`
见 `nexus-contracts/src/spec/tool_plan.rs:56`）的代价被量化为
**63.8 ms vs 14.9 ms ≈ 4.3×  slowdown**，不再停留在论述层面。

因此 `tplan_undeclared_parallel = 160_000_000`（median × 2.5，ms 档余量分级同
ADR-239/C6 先例）是**故意的宽松上界**：ADR-254 的执行器能力表落地后本键收紧到 ≈38 ms，
「只收紧不放宽」由此获得可机检的执行路径。若一开始就把 undeclared 写成 38 ms，
本批自身会立即判红——那是伪造验收，不采纳。

### 2.4 D4：bench 会轮转 `estimates.json`，取证基线须先备份

`cargo bench` 把 `target/criterion/**/new/estimates.json` 轮转为 `base/`，
即破坏 ADR-234「测量态与提交态必须同一棵树」所依赖的读数面。
W0 报告据此判定「W0 无源码改动则不跑 bench」——该推理在 W0 语境成立，但 X1 的
**前置条件恰恰是必须取得该 bench 的首个 median**（无读数即无法登记阈值）。

裁决：**先备份后测量**，并把备份路径写入本 ADR 以便复现改前态：
`tmp/x1_criterion_backup/criterion_6475027.tar.gz`（1,026,504 B，含 133 份
`estimates.json`，其中 `new/` 53 份 —— 与 W0 报告 `§0` 自述的 53 份逐字一致，
构成对备份完整性的交叉对照）。`tmp/` 受 `.gitignore:17` 忽略，不污染工作树。

### 2.5 D5：订正 `bench_check.yml` 内一处过期自述（反腐化工件自身腐化）

原注释（R2 M44 批次留下）称「真正的键集对拍不变式尚未实现，已登记为待落单」。
实测该不变式**已实现为 I9**（`ci_threshold_key_sync`），且现测
`I9 bench_check.yml heredoc keys == [[thresholds_ns]] keys (79 keys; dict found=True) PASS`。
保留根因叙述（它解释了为什么需要 I9），只订正结论句并改为**指向门自报行**，
不把键数写死进注释 —— 注释里的整数是会漂的（同一测量整数只能有一处权威源）。

本条的价值不在文字：若下一个改动本 heredoc 的人读到「对拍尚未实现」，
他会认为只改 TOML 就够了，于是再次制造 M44 那类静默差集。

## 3. 后果

**正面**
- `check_perf_redlines.py inventory` 由 `unknown=1 FAIL` → `unknown=0 PASS`；
  `all` → `VERDICT PASS (rc=0, failing_submodes=none)`；
  I1 79/79、I2 1≤1、I3 54、I7 54 pairs、I9 79 keys 全 PASS。
- F-A5（并行分支在生产形同不存在）有了可对比的数字与一把后续会收紧的尺子。
- 本分支不再存在「收官报告说全绿而工作树是红」的口径分裂。

**负面 / 代价**
- CI bench job 增加 4 个值组 ≈ 25 s 墙钟（每组 warmup 3 s + 采样 ≈ 5.5 s）。
- `tplan_declared_serial` 与 `tplan_undeclared_serial` 阈值同为 175 ms，
  信息量重叠——保留两条是为了让 X3 只动 `undeclared_parallel` 一条即可证明收口，
  属刻意冗余而非疏漏。
- 本机 `D:` 仅剩 40 GB（86% 用满），`cargo bench` 走 release profile（`lto=true`,
  `codegen-units=1`）会向 `target/` 写入大量工件；后续波次若需再测，先确认空间。

**不可判定项（不伪绿）**
- 阈值最终裁决权在 CI：本机 `run_gate_manifest.py` 因 `wsl.exe` 在程序黑名单而不可跑，
  本 ADR 只声称「本地单门读数」，不声称全门对拍绿。
- Windows timer granularity 使 `sleep(2ms)` 实测远超 2 ms（并发档 14.9 ms ≈ 一个
  OS tick），故本 bench 的绝对值不可读作「工具真实延迟」；只有**同轮两档之比**有意义。
  这正是 ADR-234「比值判据优于绝对值判据」在本批的具体体现。

## 4. 回滚

单提交 revert 即回原状，牵连面为零（无 Rust 逻辑改动）：
`scripts/perf_thresholds.toml` 三处增行（`[thresholds_ns]` / `[path_keywords]` /
`[[run_bench]]`）+ `bench_check.yml` 四处增行 + bench 文件新增 `undeclared` 值组。revert 后 `inventory` 回到 `unknown=1`（已知红，不阻塞其它波次）。
注意：若先落 ADR-254 再回滚本批，`tplan_undeclared_parallel` 的收紧值会失去登记处 ⇒
回滚必须逆序（先撤 ADR-254 的阈值收紧，再撤本批）。

## 5. 未采纳的替代

| 替代 | 否决理由 |
|---|---|
| 走 `dev-only` 让门转绿 | 见 D1：把有 SLO 语义的基准声明为无 SLO ＝ 降低门强度 |
| 关键词用组名 `readonly8_ms2` | 见 D2：宽词使 `/1`、`/8` 互相窃取，阈值断言对象错位 |
| `undeclared_parallel` 直接登记 38 ms | 伪造验收：登记时该值尚未达成，本批自身即红 |
| 自写一个 overlap 检查器 | 判据漂移风险；改用门自身的 `keyword_overlaps()` 并植入正控 |
| 不备份直接跑 bench | 破坏 ADR-234 的取证基线，且改前态不可复现 |
