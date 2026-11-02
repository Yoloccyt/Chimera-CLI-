# ADR-196：计算派发面承接扩展（TaskKind/HtsTable 登记位扩容 + 借用与共享表项解耦 + NMC 模型资产前置）

- **状态**：✅ **Accepted（2026-10-07，所有者认证）**；原 🔶 Proposed / 已决-未实施（储备）（2026-09-30）经所有者路径完成认证（§8 认证登记）。注：D2/D3（Phase B2）已按 §6 于 2026-10-02 授权落地；D5 空档已由 G-90 闭合；D4（NMC 资产前置）由 W4 批 C WI-C2 承接
- **决策者**：E01（架构）+ E03（Rust 工程）+ E08（度量）
- **来源**：`docs/architecture/ADR-195-heterogeneous-gpu-rejection-affirmation.md` D6（残余价值转接）；证据见 `docs/reports/hetero-compute-v6-assessment-2026-09-30.md` §4/§5
- **承袭**：ADR-095~099（双运行时与 ComputeBridge）、ADR-101/102/106（确定性与禁手写 SIMD）、ADR-103/128（HTS-CPU 阈值三重来源 + arc-swap RCU）、ADR-120/127（L-f 三态派发为唯一调度入口）、ADR-160（可达性棘轮）、ADR-190（反幽灵门）、ADR-192（CPU 算法分层先例与 env 降级形态）
- **不触碰**：`NexusEvent` 变体集（151 锁定）、crate 数（39 锁定）、R2 冻结面（ADR-042/052/053）、`perf_redlines.toml` 现有阈值（不放宽、不新增未实测项）

---

## 1. 背景与判据

v6.0.0 总案的 GPU 主张被 ADR-195 否决，但其**问题意识**在三处指向了真实缺口（现测，报告 §4）：

1. **批量卸载已完成，但 `TaskKind` 被语义借用**：P1-T14（WI-34「并行化首批注入」）为 7 个点位提供 `ComputeBridge::spawn_compute_batch` 卸载并**默认开启**（`osa-coordinator/src/config.rs:222 parallel_masks: true`）：`hcw-window/src/parallel.rs`、`nmc-encoder/src/parallel.rs`、`faae-router/src/parallel.rs`、`osa-coordinator/src/parallel.rs`、`decay-engine/src/parallel.rs`、`repo-wiki/src/vector/parallel.rs`、`gsoe-evolution/src/fitness_parallel.rs`；另有 `mlc-engine/src/memory_graph.rs:233` 走 `hnsw.parallel_search`（**内建 rayon，不经 `ComputeBridge`，是另一种机制**）。**⚠ 2026-10-02 覆盖面更正（本行原文记「8 个点位…提供 `spawn_compute_batch` 卸载并默认开启」，该表述有两处不成立）**：① `mlc-engine` 走 hnsw 内建 rayon，从未进入 `spawn_compute_batch` 路径，不应并入该计数；② 7 个 `spawn_compute_batch` 点位中，**只有 5 个存在真实生产调用链**——`nmc-encoder` 与 `gsoe-evolution` 两个仅有公开 API 而仓内零生产调用方（`nmc-encoder/src/lib.rs:66` 的 `pub use parallel::perceive_batch;` 是**重导出而非调用**；`fitness_parallel::select_elite` 全库无外部调用方，`gsoe-evolution/src/engine.rs:381` 的 `Self::select_elite` 是引擎私有的**另一个同名符号**、其 `engine.rs:153` 调的才是它）。判定方法：排除各点位 `parallel.rs` 的 `#[cfg(test)]` 测试尾与 `benches/`、`tests/` 后反查生产调用点。有效生产调用链 5 条 = hcw（`compressor.rs:202`）/ faae（`router.rs:200`）/ decay（`engine.rs:532`）/ osa（`coordinator.rs:518`）/ repo-wiki（`vector.rs:223`）。更正后的现测生产消费映射：正名使用 = `OsaMask`（osa）/ `KnnSearch`（repo-wiki）；**语义借用** = hcw 借用 `CscCollapseScore`（`hcw-window/src/parallel.rs:25-28`）、nmc 借用 `ClvSimilarity`（`nmc-encoder/src/parallel.rs:22-26`，**该点位无生产消费**）；**共享表项** = faae 与 decay 同挂 `Generic`（`faae-router/src/parallel.rs:24-28`、`decay-engine/src/parallel.rs:30-34`）。注：两处借用与共享均**注释自陈为刻意的波次约束**（「任务约束不新增 TaskKind 变体」）且阈值选取有理由（faae「10000 与批量专家场景吻合」、decay「低于阈值时串行更优——调度开销反超计算量」）——**本 ADR 不把它们定性为缺陷**。**更正的下游后果**：`TaskKind::ClvSimilarity` 与 `GsoeEvaluate` 两个表项对应的负载在生产中不发生，ADR-198 的 SPRT 校准采不到样本；D3 计划为这两个点位做的表项解耦，动机从「可独立校准」降级为「本就不发生」。另现测唯一未挂桥的生产相似点：`mlc-engine/src/memory_graph.rs:315-325` `recall_with_graph` 种子选取（全量 `map(...).max_by`，注释自陈「规模 <10K 可接受」）。
>
> **⚠ 2026-10-05 覆盖面再核（第三轮收口 T0.1/T0.4 追加；只加注，决策不变）**：「有效生产调用链」自本日起以**组合根口径**为唯一机器基线——G-87（`scripts/check_compute_offload_reachability.py` + `scripts/compute_offload_reachability_freeze.txt`）：WIRED_COMPOSITION=0；CRATE_API_LIVE=2（faae scoring 链←`router.rs::route`、repo-wiki `search_batch` 链）；PUB_API_DIAGNOSTIC_ONLY=1（hcw←sentinel 诊断）；DEAD=5。此前流传的 5 条/7 个/uses=8/2-4 条均为不同口径（API 语义/调用点数/链可达性）并存，非事实矛盾；任何覆盖率陈述须引用 G-87 基线，棘轮禁止静默升降级。
2. **借用/共享的真实损害是「表项不可独立校准」**：`HtsTable` 的写入语义是**改快照副本后全表 RCU store**（`hts.rs:71-72`），且 `ThresholdSource::SequentialTest` 的历史与升级决策是**按表项**记录的。因此：① `Generic` 一项同时服务 faae 专家评分与 decay 批量衰减两类语义不同的负载，SPRT 一旦判定该表项应降阈值，会**连带把另一类不相关的负载一起推上 rayon**（反之亦然）；② `dispatch.rs:38-49` 的枚举 doc 与真实消费者不一致（名为「CSC 四级压缩评分」，实为 HCW 条目重要性评分；名为「CLV 相似度批量计算」，实为多模态编码），表项的离线测定与历史无法归旧到具体负载。这两项才是 D2 要消除的可证损害，而不是「并行未开启」。
3. **NMC 推理不可度量**：全仓 **0 个 `.onnx` 文件**，四个感知器在 `model_dir` 为空时一律 `backend = None` 走占位回退（`perceptors/{image,video,audio}.rs:30-36`、`text.rs:64-68`），`benches/encoding_benchmark.rs` 只覆盖文本/桌面档。任何「推理加速比」论断在当前状态下**既不可证也不可谓**——缺的是资产与基准。

---

## 2. 决策

- **D1 派发面扩展走既有抽象，不新建平行系统**：新增后端/类型一律经 `TaskKind` + `HtsTable` + `ComputeBridge::route()`（ADR-120/127 已裁决 L-f 为唯一调度入口）。**明确否决** v6.0.0 总案的 `UnifiedComputeLayer::select_backend()` 与 `HeterogeneousTaskScheduler` 两套新系统——后者与在码 `compute/hts.rs` 的「HTS-CPU 混合阈值调度」**缩写撞名**（总案 GPU 阈值数字 `1000/100/5000/500` 逐字抄自 `dispatch.rs:88-93`，即同一张表）。
- **D2 `TaskKind` 扩容：只增四个登记位（实施清单，本轮不落地）**：`HcwEntryScore`（替 hcw 借用）/ `NmcEncode`（替 nmc 借用）/ `ExpertScore`（从 `Generic` 拆出 faae）/ `DecayBatch`（从 `Generic` 拆出 decay）。同批必须原子同步的站点集合（**2026-10-01 穷举现测**，详 `docs/superpowers/specs/2026-10-01-compute-dispatch-kind-alignment-design.md` §3；本节初稿只列六处，属不完整清单，已更正）：
  1. `dispatch.rs:37-50` 枚举 + doc 注释阈值标注；
  2. `dispatch.rs:54-61` `TaskKind::ALL` 数组长度与 `const ALL: [TaskKind; 6]` 的 `6`；
  3. `dispatch.rs:86-95` `threshold()` 静态参照表（`match self` @ `:87`）+ `:152-166` `thresholds_match_manual_s8_4` 期望数组；
  4. `hts.rs:83-85` `HtsTable` `entries: [Entry; 6]` → `[Entry; 10]` 与其 `Clv=0..Gen=5` 索引注释 + `:95-132` `Default` 表体；
  5. **`hts.rs:175-176` `const fn kind_index` 的 `match kind` 穷举**（新增变体的编译期强制点，即漏改探测器）；
  6. **`hts.rs:1049-1053` `kind_index_bijective` 含硬编码 `assert!(idx < 6)`** → `idx < 10`；
  7. `hts::tests::default_table_matches_dispatch_static`（动态表 ↔ 静态表等价性锁，`hts.rs:660-664`）；
  8. `dispatch.rs:104-142` 三态路由测试的遍历断言（Inline/Rayon 边界随新阈值移动）；
  9. **`TaskKind::ALL` 共 13 处消费点**：`dispatch.rs:107/121/138/147`、`hts.rs:664/762/779/1049`、`bridge.rs:465/626`、`benches/hts_bench.rs:32/49`（含 `:27` 「穷举六类」注释）、`benches/bridge_bench.rs:33`；两个 bench 不致编译错，需人工核注释与数组同步。
  阈值来源必须逐条标 `ThresholdSource`（`OfflineMeasured` / `SequentialTest` / `ConservativeDefault`）；未经 W 复测的初值须在注释保留「预填初值，不作已校准结论」措辞（`dispatch.rs:10` 诚实数据红线）。
- **D2-bis 提案变体的批判性收窄（哪些登、哪些不登、哪些条件登）**：提案 `TaskType` 九变体中只收上叙四个。**不登记**两个：`ParliamentVote` 与 `EventFilter`——① 归约规模 = 3-5 个角色（`parliament/src/debate/mod.rs:752 SIMPLIFIED_ROLES: [Role; 3]`），任何高于此的阈值永不触发；② 事件匹配已是命名空间前缀 trie + 字面量哈希 O(1) 并有 Arc 零拷贝缓存（`event-bus/src/pattern_index.rs:7-11,49-54`），码内自陈「145 事件量级 broadcast 无瓶颈」；③ 两者均落在 ADR-195 D2 四面永久排除区（确定性归约 / Critical 发布不阻塞）内——登记即制造**永不使用的表项**，违背 YAGNI 与本仓「有病灶才建分发面」先例（ADR-194 §3.1 #4）。**条件登记**一个：`MlcRecall` 仅当 D3-bis 量实测得种子扫描确实需要入桥时才新增，否则不给表项留空位。
- **D2-ter `Entry::chunk` 的诚实处置**：现测除 `bridge.rs:161-166` 的 `update(kind, threshold, chunk, source)` 写入外，**无任何消费方读 `Entry::chunk`**（四点位均用各自 `parallel.rs` 的本地 `CHUNK = 64` 常量），而 `hts.rs:33-34` 注释却称「T14 并行化接线消费」。本 ADR 要求：拆位**不得宣称带来 chunk 可调收益**；要么仅修正该注释（零行为变更，默认选择），要么另立波次让点位改读表内 chunk（属行为变更，需拐点实测）。
- **D3 借用与共享表项解耦（本 ADR 主治理项，走 CPU）**：把 D2 的四个新位接回各点位（hcw 评分→`HcwEntryScore`、nmc 编码→`NmcEncode`、faae→`ExpertScore`、decay→`DecayBatch`），使每个语义负载拥有**可独立校准**的 `HtsTable` 表项；`Generic` 回归其本义（未登记任务的保守默认，10_000）。**阈值初值不得自创**：新位直接沿用各点位现行实际生效阈值（hcw = 200、nmc = 1000、faae = 10000、decay = 10000）标 `ThresholdSource::OfflineMeasured{measured_at}` 或 `SequentialTest`，保证解耦不改变任何现有路由行为（纯命名重构）。约束：
  - 不新建调度器、不改卸载入口：继续走现有 `bridge().route(kind, n)` + `spawn_compute_batch`（`bridge.rs:147/230`）；
  - Top-K 继续走 `select_nth_unstable_by` / L0 `xts_top_k_by`，**不新增 `sort_by` 站点**（红线 R8）；
  - rayon 闭包内禁 `.await` / IO / 持锁跨闭包边界（ADR-099 + `compute/mod.rs:17-19`）；faae 已经做过的「快照分离」（`faae-router/src/parallel.rs:9-13`）是同类改造的标准范本；
  - `hcw-window` 侧候选含 `Option<CLV>`，须在卸载前 filter-map 而非闭包内 `unwrap`（E-5 已 opt-in `unwrap_used = "warn"`，根 `Cargo.toml:98-100`）；
  - **逐位一致性不得回退**：每个改阈值点位必须保留/新增「并行与串行逐元素一致」断测（P1-T14 已有先例，如 `osa-coordinator/src/parallel.rs:17` 与 `hcw-window/src/parallel.rs:15` 的「Top-K 仍串行、与注入前逐位一致」）；归约/排序继续走 DetReduce 与确定性决胜（`osa-coordinator/src/parallel.rs:294-296` 的 `mask_hash` 一致性论证为同型判据）。
- **D3-bis 残余串行面清点（次优先级）**：`mlc-engine/src/memory_graph.rs:315-325` `recall_with_graph` 种子扫描是全库唯一未挂桥的生产相似度循环。本 ADR **不授权直接入池**：先按 G-B 同款方法量实生产常驻规模，若确实 ≤ 10K 且 `decay`/`knn` 各档无违例，则**保留串行现状并删除误论断**；仅当实测跨过 `MlcRecall` 阈值才接 `spawn_compute_batch`。
- **D4 NMC 模型资产前置项（度量前置，非性能项）**：闭合四件事后才允许为多模态编码登记任何阈值——(1) 模型获取路径与版本/SHA-256 清单；(2) 加载失败降级保持现有 `backend = None` 占位语义（不得把「无模型」变成运行期错误）；(3) image/video/audio 三档真实 `run()` 测试（当前测试仅覆盖 `backend=None` 分支）；(4) `benches/encoding_benchmark.rs` 扩三档 p50/p95 基线并按 **P-6 三源**登记（`perf_redlines.toml` + `perf_thresholds.toml` + `bench_check.yml`）。**明确排除**引入 `candle`/`ort` 等新推理栈：`nmc-encoder/Cargo.toml:22-34` 已登记「`ort` 仅支持 MSVC 工具链，tract 为纯 Rust 且跨平台」的选型理由，重开须 supersede 该条。
- **D5 治理空档登记（不在本 ADR 内建门）**：「nexus-core 最小依赖」目前只有文档硬条款（`agents.md §2.2`），无机器门可拦截外部版本依赖（`check_dependency_rules.sh:184-202` 只解析 `workspace = true` 声明）。本 ADR 只登记该空档并把建门决策移交委员会（ADR-195 Q2，下一可用 G-号 G-85）；**不得**以「先建门后补 spec_ref」方式绕过 `run_gate_manifest.py` 的 spec_ref 强制。
- **D6 命名与文档纪律**：本仓「HTS」唯一指代 `compute/hts.rs` 的 HTS-CPU 混合阈值调度；任何外部文档/后续实现不得复用该缩写指代其他系统。派生文档引用 ADR-103 时须保持「HTS-CPU 阈值三重来源法」语义（`ADR-095-134:19`）。

---

## 3. 后果与代价（诚实登记）

- D2 属 **L1 公开枚举扩容**（九个提案变体只收四个，两个明确不收，一个条件登记，详 D2-bis），是向后兼容的非破坏性变更（新增变体不删不改既有），但会移动 `TaskKind::ALL` 穷举守卫与 `HtsTable` 数组维度（`[Entry; 6]`→`[Entry; 10]`），触及面为 nexus-core 单 crate + 其测试；不改任何调用方签名。
- D3 改阈值不会提升已有吞吐（并行已存在），其收益是**让 `HtsTable` 的逐类型调优（含 SPRT 序贯检验升级）能真的作用到这些负载上**；`Generic` 借用状态下改表对它们无效。已卸点位冷路径仍付池调度开销（`bridge.rs:38/41` 下限 2 线程、栈 2 MB，池 = num_cpus−2）；小输入不回退纪律参照 ADR-192 D2（`HNSW_MIN_ENTRIES=4096` 以下走精确线性），故新登记名必须给出「低于阈值不卸载」的实测拐点。
- D4 不产生任何性能收益，其价值是**把「不可度量」变为「可度量」**；若委员会认为模型资产不应由本项目持有（体积/许可），D4 应降级为「外部资产 + 缓存目录」方案，届时本 ADR 相应改注。
- 不减二进制、不加快启动、零新增依赖（D3/D4 只用已在 `Cargo.toml` 的 `rayon` / `tract-onnx` / `criterion`）。
- **实施细案已外提**：本 ADR 只承载决策；逐站点清单、阈值迁移表、T1-T9 任务与验收命令见 `docs/superpowers/specs/2026-10-01-compute-dispatch-kind-alignment-design.md`。该 spec 的硬判据是**行为零改变**（路由决策逐条相同），而非性能主张。
- 若 D2/D3 实施后仍无 SLO 违例，则 ADR-195 的 G-A 门继续不满足，GPU 议题在本项目内**按事实关闭**（而非按意见关闭）。

---

## 4. 验收（实施波次适用）

| # | 判据 | 验证方式 |
|---|---|---|
| V1 | 派发面扩容原子同步六处 | `cargo check --workspace --all-targets` rc=0；`cargo test -p nexus-core` 0 failed（含 `thresholds_match_manual_s8_4` 与 `default_table_matches_dispatch_static`） |
| V2 | 并行化不改数值结论 | 每处改造点新增「串行 vs 卸载」逐位/容差对照测（分数序列 `to_bits()` 一致或 ≤ 既有 1e-6 容差）；确定性排序键不变 |
| V3 | 红线不新增违例 | `check_topk_registry` / `topk_sortby_freeze` 无新增站点；`check_await_across_guard`、`check_lock_await.sh`、`check_sqlite_blocking.py` rc=0；`clippy --workspace --all-targets -D warnings` rc=0 |
| V4 | SLO 不放宽 | `check_perf_redlines.py slo invariants` rc=0，且 `perf_redlines.toml` 现有六条 `[[slo]]` 数值零改动（diff 为空） |
| V5 | 性能改善可测量 | 改造点位前后 criterion 同批对照，收益须 ≥ 报告登记的实测拐点；未达拐点则该点位回滚 |
| V6 | NMC 资产闭环 | `chimera doctor` 可报告模型缺失为「未启用」而非错误；三档基准有 p50/p95 落盘；`.onnx` 不进仓库（许可/体积），仅进清单 |
| V7 | 覆盖率不下降 | `coverage_gate.py` 单点裁决通过（min_line = 85，现测 92.97%），新增分支均有测试覆盖 |
| V8 | 零 GPU 依赖 | 复核 `Cargo.lock` 仍无 `wgpu`/`naga`/`candle`/`burn` 家族（ADR-195 V1 同判据） |

---

## 5. 与 v6.0.0 总案的映射（哪些主张被转接、哪些被丢弃）

| 总案主张 | 本 ADR 处置 |
|---|---|
| §3.2 UCL 自动后端选择 | **转接**为 D1/D2（扩 `TaskKind` + `HtsTable`，入口仍是 `ComputeBridge::route()`）；其中 `ParliamentVote`/`EventFilter` 两个变体按 D2-bis **不登记** |
| §3.3 HTS 异构任务调度器 | **丢弃**（与 `compute/hts.rs` 撞名同功能位，ADR-127 已裁决唯一入口） |
| §3.4 HeterogeneousBuffer 零拷贝 | **丢弃**（统一内存卖点仅 Apple Silicon 成立；`Pod` 约束与 forbid 共存性无先例，ADR-195 D5 E1） |
| §3.5 GpuComputePool / 18 WGSL 内核 | **丢弃**（ADR-195 D1） |
| §4.x 十四点位 GPU 化 | **丢弃**；其中批量并行诉求已由 P1-T14 在码内实现，本 ADR 以 D3 清偿其 TaskKind 语义借用 |
| §6.3 十四条加速比 | **丢弃**（未实测；D7 反幽灵门） |
| §4.1 NMC 推理「不可测」隐含缺口 | **转接**为 D4（资产 + 校验 + 基准闭环） |
| §6.2 新增 ADR-103~110 | **作废**（撞号）；本议题以 ADR-195/196 承接 |

---

## 6. 实施状态（2026-10-02 追加；只加注，不改动上方决策文本）
经用户 2026-10-02 明确指令授权，本 ADR 的 Phase B2（D2/D3）实施已落地并全绿：nexus-core 计算派发面 TaskKind/HtsTable 由 6 扩至 10（新登记位阈值原值迁移，路由决策逐条不变），hcw/nmc/faae/decay 四点位换名为专用登记位（faae 与 decay 解耦，附回归测试），T7 量实结案（mlc 种子扫描生产 n_items=0，降账 bench-only）。同批据 ADR-195 V1 落地加速器排除门 G-86 并接入 CI。证据与命令见 `docs/reports/cpu-plane-phase0-c0-2026-10-02.md` §5 与 `docs/reports/cpu-plane-c1c2-scale-bounding-2026-10-02.md`（`cargo check --workspace --all-targets` rc=0、nexus-core 208/0、四 crate 全绿、clippy -D rc=0）。
> 认证语义：上方各 D 段仍按 ADR-194 D1 保留 Proposed 文本；本节仅登记「实施已由所有者授权并落地」的事实，不替代 `certify_if_reviewed` 正式认证程序。

---

## 7. osa 单次五维路径例外登记（2026-10-05 追加；登记型加注，不改写上方决策）

**事实**：`crates/osa-coordinator/src/coordinator.rs` 的 `PARALLEL_MIN_ACTIVE_ITEMS = 30_000` 驱动 `should_use_parallel_masks`（尺寸纯函数），后端为 `std::thread::scope` 五 OS 线程，**不经** `HtsTable`/`ComputeBridge`。阈值有实测依据（≈293µs 五线程 spawn/join 固定成本、2210 活跃项并行慢 3× 反例、交点推算 ≈13.6×2210≈30k；见该常量文档注释、`docs/reports/perf-slo-2026-09-25.md §1.2`、`cpu-hetero-four-dimension-audit` U-5）。

**为何不迁 HtsTable（四维审计 U-5 / spec Phase C2 的处置结论）**——三条独立理由，任一即足以否决：
1. **后端语义错配**：`DispatchPlan::Rayon` 的既定语义是「进 L-a rayon 池」（`dispatch.rs` 枚举文档）；本路径后端是五 OS 线程，与池预算（num_cpus−2）、池饱和三态（ADR-197）、校准双桶（ADR-198）均无对应物。借用 `Rayon` 表达 `thread::scope` = 重犯本 ADR D3 刚清偿的「TaskKind 语义借用」病。
2. **量纲错配**：HtsTable 表项 threshold 语义 =「条目数」；`OsaMask` 已承载「批量 profiles 数（阈值 100 → 入池）」。本路径输入是 `active_items`（五列表总项数，阈值 30_000 → scope），同一 kind 不能双量纲，新增近亲 kind 则制造第二个语义歧义位。
3. **校准器污染**：ADR-198 双桶（Pool vs Inline）语义对本路径不成立——若入表，SPRT 会把 scope 路径样本当池路径统计，`calibration_eligible` 需为此再加特判，复杂度外溢大于收益。

**约束（自本登记生效）**：
- `HtsTable`/`SequentialTest`/`calibration` 的任何阈值变更**不得被解释为**影响本路径；本路径并行开关仅由 `PARALLEL_MIN_ACTIVE_ITEMS` + `config.parallel_masks` + env `CHIMERA_NO_PARALLEL_OSA` 三者决定（现状不变，本节仅定性）。
- 代码侧注释已引本节（`coordinator.rs` 常量文档），满足「有病灶才建分发面、有例外必登记」。

**复评触发条件**（满足其一即重评迁表可行性）：① `DispatchPlan` 扩展第四后端（如 `Scope`）；② 五维路径出现真实生产流量且校准需求被提出（当前单 profile 路径的调用面以 `Inline/Rayon` 池语义为准，见 `cpu-plane-phase0-c0` C0 终判）；③ thread::scope 固定成本显著下降（换 tokio tasks 或池内再分块），使「后端=池」重新成立。

**一致性断测现状（登记时核点，2026-10-05）**：阈值边界（`coordinator.rs` 内测：under→串行 / at→并行「含等号」）、分布无关性、`tests/parallel_tests.rs` 的 scope-vs-serial 逐维一致与「不同调度下相同五维掩码」测**均已在位**——本例外不新增测试义务。

---

## 8. 认证登记（2026-10-07；W4-0 T0-4 认证推进；只加注）

经所有者 2026-10-07 明确授权（L0-L3 融合打磨方案 W4-0 T0-4），本 ADR 完成正式认证，头部状态由 Proposed 翻为 ✅ Accepted。认证证据链：

1. **D2/D3（Phase B2）已落地**（§6）：TaskKind 6→10 + `HtsTable [Entry; 10]`，阈值原值迁移、路由决策逐条不变；hcw/nmc/faae/decay 四点位换名专用登记位（faae/decay 解耦）；证据 `docs/reports/cpu-plane-phase0-c0-2026-10-02.md`、`docs/reports/cpu-plane-c1c2-scale-bounding-2026-10-02.md`（workspace check rc=0 / nexus-core 208 项 0 failed / 四 crate 全绿 / clippy -D rc=0）。
2. **D5 空档闭合**：`nexus-core` 最小依赖机器门 G-90（提交 `898ab00`；G-90 note 明记 spec_ref = ADR-196 D5）。
3. **D4 实施状态**：NMC 资产前置四件事未全落（全仓 0 `.onnx`）——非本认证阻塞项，由 W4 批 C WI-C2（资产管线骨架，模型不入库）承接；真实资产到位时按 D4 登记基准。
4. **认证不改变任何决策文本**；历史注记保留。

---

## 9. N-4 批量失败语义绑定转正（2026-10-07；W4-2 WI-G6b；append-only）

**背景**：`cpu-plane-c1c2-scale-bounding-2026-10-02.md` §3 裁定「N-4（7 点位批量失败语义统一改造）应绑定到『接线到真实流量』的功能决策之后，而非现在铺开」；`cpu-plane-closeout-2026-10-04.md` §7 以 7 点位姿态盘点（fail-fast×5 + item-wise/ nmc + in-place-fallback/gsoe）+ 显式登记结案。本 ADR 认证（§8）后，该绑定由「报告散文」转正为**登记册 + 机器钩**（WI-G6b 转正）。

**转正物**（零生产代码面 / 零默认行为变更 / 零新增依赖边）：

1. `scripts/n4_binding_registry.txt` —— 绑定行 `n4_batch_failure_semantics|DEFERRED_UNTIL_WIRING|wired_composition>=1` + 7 点位姿态基线锚行（逐字引自 closeout-2026-10-04 §7）；
2. `crates/nexus-core/tests/n4_binding_test.rs` —— 接线测试三项：①绑定状态 ⇔ G-87 `compute_offload_reachability_freeze.txt` 的 WIRED_COMPOSITION 行数（零接线 ⇒ 维持 DEFERRED；**接线出现 ⇒ 翻红强制重新裁决 N-4**，牙齿探针已实测双向）；②7 锚点文件存在且仍调 `spawn_compute_batch`（盘点与代码不失联）；③登记册 schema 守卫。

**绑定语义**：状态机仅两态转移——当 G-87 组合根口径出现首个 `WIRED_COMPOSITION` 行时，「接线到真实流量」条件成立，N-4 改造义务启动（可由未来批次实施或重新裁决），不得继续静默 DEFERRED。

**回滚（断开绑定）**：删除上述两文件 + 本节（单提交 revert，零生产面）。

## 10. overwindow dense 挂桥与 §9 绑定首次触发（2026-10-08；W5-2 R-4；append-only）

**本节只加注，不改动上方决策文本**（沿用 §6/§7/§8/§9 的登记型先例）。

**事实**：本轮把 `crates/chimera-cli/src/overwindow_bridge.rs` 的 dense 段（逐块 CLV
余弦 → `select_nth_unstable_by` → truncate → `sort_by`）接进 L-a 计算池，使
`scripts/compute_offload_reachability_freeze.txt` 自建立以来**首次**出现
`WIRED_COMPOSITION` 行（键 = 实际调用点所在函数 `dense_scores_pooled`）。
接线前该链在任何规模均串行执行（池从未被调用），所以 W4-2 的 n\*≈287 拐点无法兑现。

**等价前置闸（§4 V2 口径，实测非推定）**：池化分批后的分数向量与 inline **逐项逐位相等**
（含故意并列表与 chunk 边界 n），已做牙齿探针（拼接改逆序 → 两测立即翻红）。
任一池段 `Err`（饱和拒绝/任务 panic）→ 整批回落 inline + warn，召回率不因计算面故障下降。
worker 让出按 faae 先例（`Handle::try_current` + flavor → `block_in_place`），G-88 现判 GUARDED。

**实测（本机中位数，dense 段隔离，非端到端）**：n=1024 349.58µs→119.26µs（2.93×）；
n=4096 1504.0µs→301.35µs（4.99×）；n=16384 6967.7µs→1794.4µs（3.88×）。
残余：offload 在 n=256/512 已更优（1.26×/2.02×），但 `ClvSimilarity` 阈值 1000 使
n∈[287,1000) 不启用；关闭它需 §3 的 ADR-198 静默窗口重标定，不属本批权限。

**§9 绑定义务的实际转移（按 §9「可由未来批次实施或重新裁决」执行）**：
- 绑定行状态 `DEFERRED_UNTIL_WIRING` → `ADJUDICATED_IN_PLACE_FALLBACK_2026_10_08`；
- 第 8 点位入册（`crates/chimera-cli/src/overwindow_bridge.rs|in-place-fallback`），
  **历史 7 点位逐字保留**（扩展而非覆写），盘点计数 7→8 / 行数 8→9 同步；
- API 级 8 点位失败语义**统一改造未做**：性质由「被接线阻塞」转为「已解锁的独立
  工作项」（W6 候选，需另立 ADR 裁决），本轮只做了姿态登记与钩子实证（改回
  DEFERRED 即 FAILED）。

**回滚**：提交 `28a88e3` 单提交 revert 即断开（代码 + 三张登记面同批，无中间态红）。

---

## §11 拐点读数的可复现性订正（2026-10-08；v3.5 W6-1；只加注，上方决策 1-5 与 §10 文本逐字保留）

§10 末句“`n∈[287,1000)` 不启用”里的左端点 **n\*≈287 不可由盘上工件复现**，现按同一批 criterion 工件用同一算法复算并登记：

- **取数方法**（零构建、只读）：`tmp/m3_knee_recount.py` 解析 `target/criterion/overwindow_dense_knee/{serial,offload}/<n>/base/estimates.json` 的 `median.point_estimate`（criterion 3.x 形状：`median` 是 `{point_estimate, confidence_interval, standard_error}` 而非标量）。
- **同轮配对读数**（serial/offload 属同一次运行，比值可用；跳轮绝对值按 W5-C 结论不可比）：

| n | serial (ns) | offload (ns) | speedup |
|---|---|---|---|
| 256 | 147,017.7 | 113,526.0 | **1.295× offload 已更快** |
| 512 | 295,316.6 | 129,894.4 | 2.274× |
| 1024 | 635,988.4 | 192,628.2 | 3.302× |
| 2048 | 1,316,234.5 | 288,459.1 | 4.563× |
| 4096 | 2,685,071.1 | 532,768.1 | 5.040× |
| 8192 | 5,356,950.0 | 1,073,002.2 | 4.992× |
| 16384 | 11,785,420.0 | 2,397,881.2 | 4.915× |

- **结论**：n=256 已是 offload 更快，与给出 n\*≈287 那一轮“n=256 慢 10%”的断言**符号相反**。因此拐点要么在最小采样点左侧，要么落在噪声带内；**仅凭单轮 7 点无法插值定拐点**。§10/P-2 中的“落差区间 [287,1000)”应改述为“拐点目前不可辨识（≤256 或落噪声带），而生产带明确在右侧”。
- **结论不变且更强**：挂桥值得做、`ClvSimilarity` 阈值 1000 偏保守——但**不得据此直接改阈值**。改阈仍须 ADR-198 静默窗口下的多点×≥5 轮**交替随机序**实测（判据：每点 ≥4/5 轮 median 同号才算拐点存在），因为本轮已经证明“同号”本身在现有采样密度下不可靠。
- **阈值表的校准态（C-38）**：`crates/nexus-core/src/compute/hts.rs` 各表项均为 `ThresholdSource::OfflineMeasured { measured_at: 0 }`，且本仓测试**主动断言 `measured_at == 0`**（防“未复测却声称已校准”）⇒ “`HtsTable` 阈值已校准”是 **C 级主张**，不得作为任何性能结论的前提。
- **不新增幽灵门**：本轮未把任何未实测阈值写入 `perf_redlines.toml` / `perf_thresholds.toml` / `bench_check.yml`（ADR-195 D7 反模式禁令）。

---

## §12 端到端 A/B 靶标产出与**本节自我否定**（2026-10-09；v3.5 W6-1b M6；只加注）

新增 bench 组 `overwindow_e2e_dense_ab`（`crates/chimera-cli/benches/overwindow_bridge.rs`）：三档语料
× 四臂，经 `ComputeBridge::update_threshold` 在 `usize::MAX`（`route()` 恒判 Inline，即挂桥前的形状）、
`1000`（生产档）、`256` 之间切换；chunk 固定生产值 64，其余变量逐字相同 ⇒ 配对可比。本批为该组加了两件
**取证装备**，它们本身就是结论的一部分：

- **自报真值**：每臂在计时前 `eprintln!` 出 `actual_blocks`（来自本批新增的只读访问器
  `OverWindowBridge::block_count()`）与 `route(TaskKind::ClvSimilarity, actual_blocks)`，让“这条读数走的是
  哪条路”由被测对象自己说，而不是由读者拿估算值去推断。
- **同路质控臂** `inline_recheck_last`：与首臂 `inline` 参数完全相同、只是排在最后 ⇒ 位置效应/热漂移的
  直接测量，给出本机读数的精度下限。

实测（`tmp/v35-m6f.txt`，`CHIMERA_COMPUTE_CALIBRATION=off`，100 samples/臂；括号内为该臂 `route()` 自报）：

| 实际块数 | inline（MAX） | pooled_prod_1000（1000） | pooled_if_thr_256（256） | inline_recheck_last | 同路质控漂幅 |
|---|---|---|---|---|---|
| 681 | 452.50 µs (Inline) | 466.80 µs (**Inline**) | **329.22 µs** (Rayon) = **1.375×** | 495.15 µs (Inline) | +9.4% |
| 1100 | 671.38 µs (Inline) | **407.59 µs** (Rayon) = **1.65×** | 411.36 µs (Rayon) | 781.44 µs (Inline) | +16.4% |
| 3684 | 2.3632 ms (Inline) | **921.33 µs** (Rayon) = **2.57×** | 918.22 µs (Rayon) | 2.3704 ms (Inline) | +0.3% |

**★ 本节此前版本（同日上午）写下过一条错误结论，现作废并留痕**：旧表把语料规模标为
“≈703–844 块 / 2344 块”，那是**按 256 token/块做的除法估算**。据此出现“两臂 `route()` 均判 Inline
却仍差 2.04×”，于是旧文写出“阈值的影响面超出 dense 段（尚未归因，禁止当已知）”。加了
`block_count()` 自报后：真实块数是 **681 / 1100 / 3684**，两臂在 681 上确实同路且只差 **3.2%**
（小于本机 +9.4% 的质控漂幅），“矛盾”随之消失。**成因是我的估算块数错误，不是阈值影响了 dense 段
以外的阶段。** 反向印证：凡自报同路的臂对（1100 与 3684 上的 1000/256）端到端差 ≤0.9%。
⇒ “阈值影响面超出 dense 段”在本仓**没有证据**；旧文指向的下一波次工作项（对 `run` 全链路逐阶段
记录 `route()` 做归因）**取消**，改为下面的结论 2 所需的多点重标定。

**四条结论**：

1. **量具可信**（这是前提而非结果）：`route()` 自报与阈值设置逐臂一致，无臂间串扰；同路两臂差 ≤3.2%
   落在质控漂幅内 ⇒ 组内配对可读。若无 `inline_recheck_last` 这一臂，+16.4% 的热漂移会被误读成真实收益。
2. **端到端收益真实，但段测不可外推**：n=681 → **1.375×**，n=1100 → **1.65×**，n=3684 → **2.57×**；而 §11
   同轮 dense 段隔离读数为 n=512 → 2.274×、n=1024 → 3.302×、n=2048 → 4.563×、n=4096 → 5.040×。把段测
   曲线按 n 内插到端到端等效规模，外推差稳定在 **≈49%（[推断]，内插本身是估计）**，远超计划预设的 20%
   判据 ⇒ **段测 speedup 不得用于阈值决策**，只能支撑“该不该挂桥”的定性判断。
3. **收益只来自“越阈/未越阈”这一次跳变**：在已越阈的规模（1100/3684）上，`thr=256` 与 `thr=1000` 同路、
   读数差 ≤0.9% —— 阈值跨过之后继续下调**没有二级收益**；全部收益都在“从未越阈变为越阈”这一步（681 块
   处 1.375×）。这同时否证了“阈值越激进越好”的直觉。
4. **新增账实漂移项**：既有 bench 标识 `overwindow_provider_select_2344` 及其注释“≈2344 块（256 token/块）”
   是估算而非实测——600k token 语料聚类后实测 **3684 块**。重命名会同时触 `bench_inventory_freeze.txt`、
   `perf_thresholds.toml`、`bench_check.yml` 三源登记，本批**只把注释口径改为实测**、保留名称作历史标识，
   是否重命名交 W6-6 裁决（不在本批权限内）。

**对 P-2 的含义**：阈值**仍不改**（维持 1000），但**理由已被本节更新**——
- 旧理由“收益成因尚未定清”**不成立**（成因就是 dense 段派发路径切换，见结论 3）；
- 现理由三条：① `ClvSimilarity` 阈值属生产行为变更，须走 ADR-198 静默窗口的多点 ×≥5 轮**交替随机序**
  （判据：每点 ≥4/5 轮 median 同号才算拐点存在），本机单轮不满足；② 本机质控漂幅最高 **16.4%**，精度下限
  只够支撑“越阈/未越阈”的二分，不足以标定“256 还是别的更优点”；③ 结论 2 表明端到端收益显著低于段测，
  用段测值标定阈值会**系统性高估收益**约一倍。
- 可交付的判断（[事实]）：`[513,1000)` 这一段**确有收益**（n=681 降到 256 → 1.375×），因此“阈值 1000 偏保守”
  的主张由 C 级升为 **B 级**；但它仍不构成“应改为 256”的依据——那需要 clean-runner 重标定与 ADR。
