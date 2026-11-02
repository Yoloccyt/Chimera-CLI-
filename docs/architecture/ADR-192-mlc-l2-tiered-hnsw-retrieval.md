# ADR-192:mlc L2 语义检索分层(精确层 + HNSW 近似层)

- **状态**:✅ Accepted(2026-09-26)
- **决策者**:E03(rust-engineer)+ E08(perf-engineer);四维深审 P2 立项 E-2/HNSW 承接
- **影响面**:`crates/mlc-engine/src/l2_semantic.rs`(L2 语义检索实现)、`crates/mlc-engine/benches/hnsw_tiered.rs`(新增)、`scripts/perf_thresholds.toml` + `perf_redlines.toml` + `.github/workflows/bench_check.yml`(新 bench 三源登记)、`docs/architecture/CODE_WIKI.md` §3.2/§11
- **承袭**:ADR-005(sqlite-vec 违反 `forbid(unsafe_code)` → L2 降级内存 KNN)、ADR-014(向量检索纯 Rust)、ADR-049 决策 6(memory_graph 规模路由先例)、ADR-190(P-6 三源登记纪律)

## 1. 背景与判据(性能可证伪,不取数不立项)

`docs/reports/perf-slo-2026-09-25.md` §1.1/§3 静默态 criterion 实测(唯一判据源):

| 场景 | median | 判定 |
|---|---|---|
| `l2_knn_slo/top10_recall/1000_entries` | 0.362 ms | 富余 |
| `l2_knn_slo/top10_recall/10000_entries` | **4.117 ms** | 已越 4ms redline 入 5ms SLO 警示带(6 条 `[[slo]]` 中**唯一**警示项) |
| `l2_knn_slo/top10_recall/100000_entries` | **42.136 ms** | O(n) 线性 10× 特征坐实(4.1→42 ms ≈ 10 倍条目 → 10 倍耗时) |

`l2_semantic.rs` 现为首表登记的**线性扫描 KNN**(ADR-005 降级产物)。既有红线条款**不放宽**:模块头「4096 条目 <200ms」与 `mlc_l2_knn` SLO 条目(5ms/4ms)保持原值。

## 2. 候选方案对比

| 方案 | 量级 | 依赖 | 工程量 | 判定 |
|---|---|---|---|---|
| **A 复用 `hnsw_rs` 分层** | O(log n) 近似 | 0 新增(workspace 已有 0.3.4,`memory_graph` Phase 8.3 + repo-wiki `HnswStore` 两条生产路径已证可行,100K p95=168µs,OOM 已排除) | 1× | **采纳** |
| B 自研 IVF 粗量化 | O(√n) 近似 | 0 新增 | ~2×(需自证收敛性与聚类质量) | 否决:收益不优于 A 而风险更高 |
| C 仅分片 + 结果缓存 | 仍 O(n) | 0 新增 | 0.5× | 否决:只收常数项,100k 场景退不出警示带,不满足立项判据 |

## 3. 决策

### D1 分层语义契约(接口零破坏)

`SemanticMemory::recall_by_clv(&self, query: &CLV, top_k: usize) -> Result<Vec<(MemoryId, f32)>, MlcError>` 签名与**输出契约**(相似度降序、长度 = `min(top_k, 存活条目数)`、分数为精确余弦相似度 clamp 到 `[0,1]`)完全不变。内部按条目数路由:

- **精确层(`len < HNSW_MIN_ENTRIES`)**:走原线性扫描代码路径,**逐位不变**(小输入不回退纪律,与 osa `PARALLEL_MIN_ACTIVE_ITEMS` 尺寸自适应同哲学,见 perf-slo §6)。
- **近似层(`len ≥ HNSW_MIN_ENTRIES`)**:HNSW 图取候选集 → **对候选逐条重算精确余弦并 clamp** → Top-K 选择。
  WHY 重算而非直接用 `1 - distance`:① 保持分数契约与精确层同源(hcw-window `BlockScore{score, hnsw_score}` 消费面依赖"score 是精确值"这一语义);② 消化 `DistCosine` 的 f32 漂移与负相似度 clamp 差异(clamp 后并列 0.0 的元素在近似层不应被优先返回)。
  近似性只体现在**候选集选取**，故质量判据按分数制定形（见 D2b，§5 验收）。
- **补口**:近似层存活候选 < `top_k` 时(重度墓碑/刚过阈值),对剩余向量做精确扫描补足,保证"长度 = `min(top_k, 存活数)`"不变量在全规模成立。

### D2 阈值常量 `HNSW_MIN_ENTRIES = 4096` 与图参数定档（实测驱动）

取现红线条目守护的规模（4096）为界：界以下保持精确且本就达标，界以上才付近似成本。该数字可在评审点改判。
另：`MlcConfig::l2_capacity` 默认即 4096 ⇒ 默认配置下近似层只在 L2 满容量饱和时接管，10k/100k 档对应调用方显式放大容量（即本立项的 SLO 场景）。

**图参数定档（由本批临时测量夹具在 4096 条 64 簇语料上扫参得出，非沿用先例）**：

| 旋钮 | 先例值 | 本层取值 | 实测根据 |
|---|---|---|---|
| M（`HNSW_MAX_NB_CONNECTION`） | 16（memory_graph / repo-wiki） | **32** | M=16 时 id-recall 在 0.96-0.99 波动（最低一次 0.95）；M=32 为 0.97-1.00，质量比恒 ≥ 0.9999。代价：建图 +15%、搜索 +30% |
| `ef_construction` | 200 | **100** | 200→400 对 recall 无统计意义提升（图的质量主要由 M 决定），而建图成本落在读路径 ⇒ 取论文区间下限 |
| `ef_search` | 100 | **50 / 100 / 200 阶梯**（`HNSW_EF_SEARCH=50` 用于 <10k；<100k 取 100；≥100k 取 200，继承 repo-wiki `adaptive_ef_search` 的档位边界 10k/100k） | ef 从 50→200 时召回几乎不变而单 query 搜索时间近似线性增长（debug 7.6ms→32ms）⇒ ef 不是质量旋钮，不能拿延迟换 |

### D2b 召回质量判据的**分数制定形**（原措词修正）

实测发现：图层级由 OS 随机种子生成（repo-wiki 已记录同型坑），**id 制 recall@10 在单轮之间会于 0.95-1.00 波动**，拿它做 ≥95% 硬断言会造出假红（本批首版测试实复现一次 flake）。
因此将契约表述定为三档（均在测试与 bench 里机检）：

1. **质量比** Σgot_score / Σgt_score **≥ 0.99**（实测 0.9997-1.0000，稳定）——“召回到的东西有多好”的本意；
2. **分数容差 recall@10 ≥ 95%**（返回项分数 ≥ 精确第 10 名分数，容 1e-4）——实测基线 0.97-1.00；
3. **id 制 recall@10 ≥ 90% 仅作回归哨兵**（参数/接线错位时会大幅塌缩，正常值 0.95-1.00）。
另：返回项**分数本身逐位等于精确重算值**（硬断言，结果分数不含任何近似值），以及长度/降序/成员存活三项契约不变。

### D3 eviction/删除语义:tombstone + 周期重建(hnsw_rs 不支持删点)

- `insert`(同 ID 更新)/`remove`/FIFO 驱逐三条路径一律对旧 dataid **打墓碑**,新条目分配**单调递增、永不回收**的新 dataid(与 repo-wiki `HnswStore::next_dataid` 同纪律,防墓碑 dataid 复用误命中)。
- dataid ↔ MemoryId 双向表随主表同事务维护(同一 `RwLock` 写锁内),不变量:`图侧存活 dataid 集合 == 主表 entries.keys()`。
- **over-fetch**:`fetch_k = top_k + min(tombstone_count, top_k × 2)`,再过滤墓碑(沿用 repo-wiki `TOMBSTONE_OVERFETCH_FACTOR = 2` 口径)。
  与 repo-wiki 不同处:墓碑量在 over-fetch 中取 **min** 而非直加 —— 重度墓碑时直加会退化成「图上做一次全量扫描」(反而比精确层更慢),
  此处改为钳住 over-fetch 并由下述精确补口接住缺口。候选不足时另需去重(图不保证不重复返回同一 dataid)。
- **重建触发（实测后由双规则收为单规则）**：死点占比 `墓碑数 × 4 ≥ 存活点数`（达 25%）→ 图标记 stale，下一次召回在 `try_write` 内从 `vectors` 全量重建并清空墓碑。
  原设计的「墓碑数 ≥ 4096 绝对上限」已撤销：建图成本随规模超线性（release 实测 10k 档 ≈3.6s / 100k 档 ≈32.6s，`ef_construction=200` 时），
  若 100k 图每 4%（=4096）变更重建一次则重建频率与规模不匹配；比例规则自身已保证 over-fetch（封顶 2×top_k）能凑齐 top_k 存活候选。
- **重建窗口回退精确扫描**:图缺失/处于 stale 且本轮拿不到写锁(`try_write` 失败,并发召回满载)、
  或图侧存活集与主表不一致(不变量被破坏的防御分支)时,该次召回直接走精确路径 —— 语义仍正确,只是暂时无加速。
  此为默认降级方向,**近似层任何异常都收敛到"变慢",不收敛到"变错"**。
- **建图/重建落在读路径而非写路径**:`insert` 只对已在场的图做增量(O(log n));首次建图与重建均由达阈后的
  召回触发。WHY:否则插入热路径(含填充型测试/迁移回放)会突然背上 O(n log n) 一次性成本。

### D4 Top-K 红线 R8 与并发

近似层候选/补口段合并后的截断使用 L0 `nexus_contracts::util::xts_top_k_by`(`select_nth_unstable_by` O(n) + 前 k 段二次排序),禁新增 `sort_by` 全排站点;精确层代码路径不改动(避免 tie-break 语义漂移)。全部逻辑在既有 `RwLock<SemanticInner>` 同步锁内完成,无持锁 await(§4.4);`#![forbid(unsafe_code)]` 保持(crate 自身零 unsafe,`hnsw_rs` 内部 unsafe 不传播,同 ADR-030 口径)。

### D5 逃生开关(运行时 opt-out)

`CHIMERA_NO_HNSW_MLC` 存在即**全盘关闭近似层 = 回到 ADR-005 降级后的纯线性行为**。
WHY env 而非 feature:与 `CHIMERA_NO_PARALLEL_GSOE`(fitness_parallel.rs)同构,`OnceLock` 启动期一次读取,不入热路径;回退无需重编译,便于线上止血与 A/B 取证。默认**不设置 = 分层开启**。

## 4. 否决项与代价(诚实登记)

- 否决"全量近似"(不分层):小输入 recall 抖动 + 建图成本 > 线性扫描收益,且破坏既有 213 测的逐位期望。
- 否决"L2 只存 Arc 引用、查询期反查主表向量":近似层需按 dataid 取向量,反查主表引入二次哈希;改为候选重算时经 `entries[id].clv` 取(单次查找,k ≤ ef 次)。
- **代价 1 内存（实测，区分两个口径）**：
  - **库侧真实增量**（分层带来的额外驻留）= 图内向量副本 + dataid 双向侧表，100k 存活点实测结构值 **≈ 195.3 MB + 19.2 MB ≈ 214 MB**
    （`hnsw_rs` `insert_slice` 内部 `to_vec` 自持副本，故与 CLV 池共约 2× 向量驻留，与立项时“~×1.5”的估计同量级）；
    10k 档 ≈ 19.5 + 0.9 ≈ 20 MB；CI/默认路径只跑 10k 量级，**不构成 OOM 风险面**。
  - **基准进程峰值**（包含对照实例与测试侧语料/ground-truth 缓冲，**不是生产数值**）：
    2026-09-27 静默单跑采样（PowerShell `Get-Process` 每 2s，覆盖建图+采样全程）：
    **peak WorkingSet ≈ 1161.5 MB、peak PrivateBytes ≈ 1541.2 MB**（同进程依次持有 10k exact+tiered、100k exact+tiered、4096 exact
    与 `Vec<Vec<f32>>` 语料副本，drop 后归还给分配器而非 OS）。
  - **结论：不需 M 降档**（库侧真实增量 214 MB @100k 已在可接受范围，且 100k 档为 dev-only 手动取数不入 CI 日跑）；
    若生产要将 L2 开到 100k+，需同时记得图侧副本使总驻留约为向量大小的 2×，可用 `CHIMERA_NO_HNSW_MLC` 或 `with_hnsw_threshold(cap,0)` 回到单副本形态。
- **代价 2 建图成本（实测，比立项时的估计严重；降 `ef_construction` 后已减半）**：一次性全量建图落在**读路径**
  （`release` 静默实测：`efc=200` 时 10k 档 3.56s / 100k 档 32.6s；改 `efc=100` 后 **10k 档 0.99-1.35s、100k 档 16.1-21.0s**；debug 再乘 4-8 倍，4096 档≈8.5s），
  稳态 `insert` 仅一次图内增量 O(log n)。缓解：（a）`ef_construction` 取区间下限 100（实测质量无损失，见 D2）；（b）重建仅在死点占比达 25% 时发生，成本均摊到数十次以上召回；
  （c）重建拿不到写锁则本轮精确回退不阻塞。**残留风险（诚实登记）**：单线程交互式会话中触发重建的那一次召回仍会卡住秒级；
  若生产上不可接受，下一步是后台重建（需 async 接缝，没有列入本 ADR）或按规模关闭近似层。
  另：hnsw_rs 在建图超 5 万点时会向 **stdout** `println!(" setting number of points …")`（库内部行为，非本仓），属可观测噪声而非缺陷。
- **代价 3 结果集成员**:`len ≥ 4096` 时 Top-K **成员**是近似值(分数仍精确),并列分数的 tie 选择不再与全排序逐位一致 —— 这是本 ADR 唯一的行为变更面,由 §5 recall 判据约束。

## 5. 验收(本批实测结果，可复跑)

0. Phase 0 登记面：`python scripts/adr_index_sync.py` **rc=0**（登记前为 rc=1）。
   但该红不是账面缺陷而是**探针自身的读数缺陷**：它只识逐号行与行内自述豁免，不识索引自己声明的合并段落
   （`ADR-001~006`/`ADR-007~025`/`ADR-095~160` 等），因而稳定报 26 BROKEN + 70 UNINDEXED 假阳；
   而**同一状态下孪生实现 `check_doc_consistency.ps1` D3（已含 range 解析）报 `unrowed 0` 绿**——双源不同读。
   修法定位为**双源同构**（ADR-190 纪律）而非补账面：把 D3 的语义（range 双向适用 + Released/历史 行免档 + 同编号取最强豁免信号）
   移植进本探针，并抽 `reconcile()` 纯函数加 `--selftest` **9 枚夹具**（T1/T5 守真缺陷仍被抓、T6 守段外不得越界免放、
   T7 守 `~`/`-` 两种写法与端点），顺手消除该文件的 `\d` 非 raw 文档串 SyntaxWarning。全程**未改一行索引/治理文本**。
   牙齿实测（真实树，内存反事实）：抹掉 ADR-192 索引行→ 192 立即出现在 UNINDEXED；恢复后双源均绿（101 行 / 146 磁盘编号 / 声明段覆盖 173）。

1. `cargo test -p mlc-engine --all-targets` → **0 failed**（lib 226 passed = 基线 213 + 新增 13；`--test hnsw_tiered` 日常 5/0（+1 ignored）且单 target 静默态 21-25s，未触发 120s 时长红线；`--doc` 16/0）。既有 213 测零改动零回退。
   **Phase 2 字面项的 100k 档已补实**：新增 `test_tiered_recall_quality_at_100k_manual_scale`（与 4096/10k 共用同一份 `quality_stats` 判据），
   带 `#[ignore]` 并已在 `scripts/ignored_test_inventory_freeze.txt` 登记为 **manual-only**（理由：100k 建图 release 16.1-21.0s + 图侧 ≈214MB
   与 CLV 池 ≈205MB，接日常会击穿时长/内存预算）——取数纪律与 bench 的 `CHIMERA_HNSW_BENCH_FULL=1` 同口径。
   实跑（2026-09-27 静默，`cargo test -p mlc-engine --release --test hnsw_tiered -- --ignored`）：**1 passed / 0 failed，22.72s，
   id-recall = 1.0000 / 分数容差 recall = 1.0000 / 质量比 = 1.000000**（均 ≥ 契约值）；`check_perf_redlines.py ignored` 因新行 total 70→71、rc=0。
2. 新增 unit 13（边界 4095/4096、图-主表一致性、tombstone 计数、重建后 stale 解除与计数、空输入/`top_k=0`/超界钳制、多次调用确定性、`clear` 作侧表、`threshold=0` 与 env 等价、**重建窗口精确回退**、**`TierRoute` 路由轴穷举**）
   + integration 6（`tests/hnsw_tiered.rs`：5 项日常 + 1 项 ignored 手动档——精确层逐位不变对照、达阈建图与分数制三档质量（4096）、**10k 真实 SLO 档质量**、**100k 真实规模质量（`#[ignore]`，manual-only 已登记）**、多次召回确定性、驱逐洪流下图一致与结果存活；后三者中质门共用单份 `quality_stats` 实现）
   + proptest 4（`tests/hnsw_tiered_proptest.rs`：结果集契约、插入序列置换下图/主表 size 一致（含删除后墓碑计数）、**被墓碑化条目绝不回到结果**、`top_k` 超界钳制）。
   **质量判据形态已机检化到分数制**（D2b）：原 id 制“≥95%”写法因图层级随机种子会在 0.95-1.00 波动（首版测试实地复现一次 flake：3 跑出 1 红），改后同一测试集 **5/5 稳定**。
3. `cargo bench -p mlc-engine --bench hnsw_tiered` 静默态单跑（release，同批对照，Windows GNU）：
   - `l2_knn_tiered/top10_recall/10000_tiered` = **0.446 ms**（同批精确层 4.386 ms，**9.8×**）→ **≤ 2.0 ms 达成**，退出 4ms redline 警示带；
   - 100k 档 = **2.110 ms**（同批对照 47.174 ms，**22.4×**）→ **≤ 8 ms 达成**；此档需 `CHIMERA_HNSW_BENCH_FULL=1` 手动取数（不入 CI 日跑，缘由见 bench `bench_sizes` 注）；
     第二轮独立静默单跑复测：**1.680 ms**（区间 1.56-1.81），质量 id-recall 1.0000 / 质量比 1.000000（10k 档同轮 tol-recall 0.98，仍 ≥ 0.95 契约），
     建图同轮实测 10k 0.99s / 100k 16.06s（`efc=100` 已较 200 时减半）；
   - `4096_exact` = 1.536 ms，既有「4096 < 200ms」红线不回退；`mlc_l2_knn` SLO 条目（5ms/4ms）**未放宽未改动**；
   - 质量采样与 bench 内置硬断言（D2b）：10k 质量比 1.0000 / tol-recall 1.00；100k 质量比 0.9999 / tol-recall 0.98；现值同步登记在 `docs/reports/perf-slo-2026-09-25.md` §1.1.1。
4. 新 bench 目标按 P-6 三源纪律登记（`[[run_bench]]` + `[thresholds_ns]` + `[path_keywords]` + bench_check.yml run_bench 行）：
   `invariants` rc=0（I1 50/50、I2、I3 45、I7 45 对、I5 11rl/6slo）；`thresholds` 对新键 **`[PASS] hnsw_tiered: max median = 453.02 us (threshold < 2.00 ms)`**；
   **`slo` 模式分层后现场复跑 rc=0、failed=0、skipped=0，六条 `[[slo]]`（含 `mlc_l2_knn`）均 `[PASS] below redline`**
   ——即原“唯一警示带”（10k 4.177ms 越 4ms redline）已由本分层实现退回 redline 以下，而 `mlc_l2_knn` 条目本身未动。
   **两条存量 FAIL 的处置（经用户授权，已全部闭环）**：
   - `cache_hit` 串台假红 → **已消除**：dev-only bench `hcw-window/probe_select` 按 P-6 三源纪律升为 registered
     （新键 `probe_select` = 500µs，实测最重组 338.51µs；关键词 `["probe_window_select/", "probe_score_throughput/", "hit_vs_recompute"]`，
     **必须排在 `cache_hit` 之前**，因匹配循环文档序 = 判定序），`bench_inventory_freeze.txt` 对应行同批删除（只减纪律）。
     结果：`cache_hit` 现在 worst group 回到真路径 `cache_hit/new/estimates.json` = **404 ns PASS**；`inventory` 报
     `registered hcw-window/probe_select`、unknown=0；`invariants` 全绿（I1 51/51、I2、I3 46 pair-unique、I7 46 对、I6 62）。
   - `scheduler_dequeue/1000` → **仍判红，但本批把它从“不可解释”变“可解释”**：同机同会话 A/B 实测——
     HEAD 原实现（u128 除法）83.34µs vs 本批新实现（至多 3 次比较 + 去边界检查）**79.78µs**（单样本 4%），
     但复跑同版本得到 103-105µs，**跑间漂移幅度 31% 远大于 50µs 门距**；另 09-25 存档的 53.34µs 与今日同一 HEAD 代码的
     83µs 差 1.56×——即本项目的既有红线“µs 级门槛须静默态单跑取样”的又一实例，本地值不足以判定回归与否。
     热路径微优化（语义由 300+ 边界探针逐点比对旧公式锁定，`test_aged_rank_matches_division_formula_at_boundaries`）已保留，
     但**它不声称能过门**：要把 1000 项稳当压进 50µs 需 `best_index` 的 O(n) 扫描改为可惰重键的堆结构（代码内已注
     “O(n) 扫描不可避免”），属 chimera-mas 调度器结构决策，仍待您定调。本批未改该键阈值（不走“改判据凑绿”）。
     **后续闭环（同批，2026-09-27）——定性为测量口径缺陷而非性能回归**：`bench_scheduler_dequeue` 原用
     `iter_batched`，该 API 只把 setup 排除在计时窗外而**闭包内 `sched` 的 drop 仍在计时窗**，1000 条各含
     String 任务体的 `ScheduleEntry` 析构即数十 µs——故读数长期由「拆解成本」主导并随分配器状态漂移。
     改 `iter_custom` 显式 setup→start→dequeue→stop→drop 后，关键词 `/1000/` 与 50µs 阈值一字未动而
     `/1000` 实测 = **5.32µs**（四档 147ns / 647ns / 5.32µs / 113.9µs，线性 ≈5.3ns/条目且不漂移），
     `thresholds` 整模式转 **rc=0**。堆结构改造（为 `/10000` 档）仍为待定决策，但已不是任何门的阻塞项；
     详见报告 §5.54。
   本批自身目标项全绿：`hnsw_tiered` PASS 453.02µs（<2ms）、`l2_recall`（4096 精确层）PASS 1.50ms（<5ms）、`memory_graph_recall` PASS 797.92µs。
5. 门禁面：`cargo check --workspace --all-targets` rc=0；`cargo clippy --workspace --all-targets -- -D warnings` **rc=0**（本批连同前序批次遗留的一处重复锁逻辑做了一行去重：`nexus-app-server/tests/mock_client_e2e.rs` 的 `next_notification` 改调已存在的 `recv_frame()`，同时消除 dead_code，锁纪律不变，该 target 5/0）；`cargo clippy -p mlc-engine --all-targets -- -D warnings` rc=0；`cargo fmt -p mlc-engine -- --check` rc=0；
   `check_topk_registry.py` rc=0（本批测试/bench 侧的 ground-truth 截断改用 L0 `xts_top_k_by`，**不新增 sort-then-truncate 站点**，首版曾触 V-newsite 判红已收）；
   全量 `run_gate_manifest.py` 总红 **4**（G-04/07/18/33 设计可见；入仓波后 G-07 转 rc=0 → 现总红 **3**：G-04/18/33，见报告 §5.53）；perf 门静态面 `--static-only`/`--selftest`（38 断言）/`inventory`（unknown=0，`registered mlc-engine/hnsw_tiered`）均 rc=0。
   **未达项收敛说明**：全集 clippy 已转 rc=0（见上）；`thresholds` 整模式的两条存量 FAIL 已全部闭环（`cache_hit` 串台假红消除 + `scheduler_dequeue` 量具口径修正），本 spec Phase 3 的字面验收「`check_perf_redlines thresholds/slo` 双模式 rc=0」至此成立（`thresholds` 模式仍有 4 枚因本地无 criterion 估计文件而 SKIP 的存量键，该模式不因 SKIP 退红，CI 侧由 `bench_check` 现跑消除）。

## 5b. 新增逻辑分支覆盖清单（Phase 2.3 人工核对，本地 llvm-cov 不可行的既定约束下）

| 新增分支 | 覆盖测 | 形态 |
|---|---|---|
| `hnsw_tier_enabled_by_env` 未设 env（启用侧） | 全部默认路径单测/集成测 | 自动 |
| `hnsw_tier_enabled_by_env` 已设 env（关闭侧） | `CHIMERA_NO_HNSW_MLC=1 cargo test -p mlc-engine --lib l2_semantic::tests::test_tier*` → 达阈建图断言转红（=先红可证伪证据，兼逃生开关实测） | 手动实测 |
| `approx_requested`：`threshold=0` | `test_threshold_zero_disables_tier_entirely` | 自动 |
| `approx_requested`：`len < threshold` | `test_tier_not_built_below_threshold` + integration `test_below_threshold_is_bitwise_identical_to_disabled_tier`（逐位） | 自动 |
| `approx_requested`：`len ≥ threshold` 且图新鲜 | `test_tier_engages_at_threshold_and_stays_consistent` 等 | 自动 |
| `route()` 四轴（关闭 / 低于阈值 / 达阈未建图 / 新鲜图 / stale） | `test_route_decision_axes`（穷举四轴，包含“stale 后不得继续用旧图服务”） | 自动 |
| `TierRoute::Approx` 分支下侧表缺失的防御性回退分支 | **不可达**（`route` 仅在侧表在场且新鲜时判 Approx），写成精确回退而非 expect/panic；无专用测 | 标注不测 |
| 被墓碑化条目不得回到结果（含图已在场时的真墓碑） | proptest `test_tombstoned_entries_never_returned`（先建图再随机删除，断言存活集一致 + 已删 id 不出现） | 自动 |
| `HnswTier::build` 首次建图 / 墓碑归零重建 | 同上 + `test_stale_then_rebuild_clears_tombstones` | 自动 |
| `remember`/`forget` 与 `refresh_stale` 双向（达阈置位、占比解除） | `test_update_and_remove_mark_tombstone`、`test_eviction_marks_tombstone`、`test_heavy_eviction_keeps_graph_consistent_and_results_live` | 自动 |
| `rebuild_and_recall` 的 `try_write` 失败分支（重建窗口回退） | `test_rebuild_window_falls_back_to_exact_when_write_lock_busy`（crate 内单测直接持有读守卫确定触发，并逐位对拍关闭侧） | 自动 |
| `recall_approx` 候选逐条精确重算 + `xts_top_k_by` 截断与降序 | `test_approx_scores_bitwise_match_exact_for_same_id` + integration 质量测 + proptest 不变量 1 | 自动 |
| `recall_approx` 存活候选不足时的**精确补口** | proptest `test_tiered_result_set_contract` 的长度契约（图欠返回时必须补口才能成立）；该分支是否触发由图连通性决定，两态均安全 | 自动（机会式） |
| 防御分支Ⅰ：`rebuild_and_recall` 取到写锁后条目数已跌回阈值以下 | **不可单线程确定触发**（释锁与取锁间无插入者）；语义与Ⅱ同型且同一回退方向（精确扫描） | 标注不测 |
| 防御分支Ⅱ：`recall_approx` 图侧存活点 ≠ 主表条目数 | 同上（不变量由同锁维护，外部无法在不走回退路径时打破） | 标注不测 |
| `clear()` 作废侧表后重新惰性建图 | `test_clear_drops_tier`（含二次建图） | 自动 |

两条防御分支均已标注释为“回退精确扫描、不返回残缺结果集”，与 D3 降级方向同构；
若后续引入测试专用的锁句柄装口（或后台重建接缝）可补测。其余新写逻辑分支均有至少一测。

## 6. 回退路径

`CHIMERA_NO_HNSW_MLC=1` 一键关闭(零代码回退);代码级回退 = revert 本 ADR 对应单 commit(l2_semantic.rs 分层 + bench + 三源登记同 commit)。两档回退都不改公共签名,故消费方(event-bus/repo-wiki/hcw-window 的 recall 语义)零改动。

## 7. 收益数字的同机方差注记（2026-10-09 追加；v3.5 W6-1b M2；只加注，上方决策与 §1-§6 文本逐字保留）

本 ADR §3 记录的 **10k 档 9.8×**（0.446 ms vs 4.386 ms）**不得当作可复现的契约值引用**：
同一 `l2_knn_tiered/top10_recall/10000_*` 基准在三次测量中的分层收益比为
**6.84×（2026-10-08 盘上 `estimates.json` 复算）/ 9.8×（本 ADR 留档）/ 13.07×（2026-10-09 复测：3.83 ms ÷ 293 µs）**，
跳幅 >2×；且两条曲线的**绝对值**同方向漂移（tiered 0.446 → 0.904 → 0.293 ms）。

含义与边界：
1. **结论不变**——“近似层显著优于精确层、退出 4 ms redline 入安全区”在本机三次测量中均成立（本轮 10k tiered 293 µs vs 2 ms SLO 余量 6.8×）；
2. **可变的是幅度**——单轮同机 speedup 属 `[Directional]`；要拿它做对外宣称或阈值依据，需 CI clean-runner 读数（本轮未推 tag，无 CI 数据）；
3. 同时修正一条旧归因：v3.4 W5-C 把 `hot_lru`/`hnsw_tiered` 的 2.8×/2.9× 变慢归给“可用内存压力（高置信推断）”，
   本轮在**同档内存压力**（FreeMem 2.5–2.8 GB vs 当时 2.33 GB）下两条均已恢复到第一轮量级（hot_lru 26–34 µs vs 第一轮 32.44 µs；
   hnsw 293 µs vs 第一轮 315 µs）⇒ 该归因降为**未证实**，详 `docs/reports/v3.4-w5-closeout-2026-10-08.md` §4.2。
   对本 ADR 无操作含义（不回退、不改阈值、不改三源登记）；仅防止后人拿单一 speedup 当不变事实。
