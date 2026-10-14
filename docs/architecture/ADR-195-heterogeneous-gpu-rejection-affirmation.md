# ADR-195：异构计算（GPU 卸载）维持否决 + 重启三门判据（v6.0.0 总案裁决）

- **状态**：✅ **Accepted（2026-10-07，所有者认证）**；原 🔶 Proposed / 待架构委员会批准（2026-09-30）依 ADR-194 D1 由评审侧发起 Claim，`certify_if_reviewed` 程序经所有者路径闭环（认证证据链见文末 §7 认证登记）
- **决策者**：E01（架构）提案，E08（度量）供判据，E02（安全）供 Critical/事件面红线，E09（RL）供 R2 冻结约束
- **评审对象**：`Chimera_CLI_CPU_GPU_异构计算极致融合总案_v6.0.0.md`（自称 `v6.0.0-heterogeneous-fusion`，1977 行）
- **证据正文**：`docs/reports/hetero-compute-v6-assessment-2026-09-30.md`（本 ADR 不重复其证据表，只承载决策）
- **承袭**：ADR-095/096/097/098/099/100/101/102/103/106/122/124/126/127/128（Phase 1 双运行时与确定性归约体系，`ADR-095-134-phase1-consolidation.md:11-46`）、ADR-190（反「门失效化」范式）、ADR-192（CPU 算法分层达成性能目标的先例）、ADR-193 D3（crate 基线锚定 39）、ADR-194 D1（Claim/Certify 分离）
- **不触碰**：`crates/`、`scripts/`、`.github/`；不新增 `NexusEvent` 变体；不新增 crate；不改三计数锁（151 / 10 / 39）

---

## 1. 背景与判据

v6.0.0 总案主张以 `wgpu` 计算着色器对 14 个 crate 做 GPU 化，并在 §2.3 自称「符合全部工程红线」。经全库现测取证（报告 §3/§4/§5），其主张与本仓既决否决、已锁定契约与实测性能包络三层面冲突：

1. **既决否决被静默重开**：ADR-107「ComputeKernel trait 预留（GPU 钩子不实现）… C14 GPU 否决维持」、ADR-123「否决 GPU 卸载 ✅已决-维持」、ADR-124「否决 crossbeam 作为第三运行时」、ADR-126「S8 Triple-Runtime 否决登记…双运行时唯一（tokio+rayon）」（均见 `ADR-095-134-phase1-consolidation.md:23,39,40,42`）。总案核心主张即「Tokio + Rayon + wgpu 三运行时」。
2. **性能前提被实测否证**：六条 `[[slo]]` 全绿（`check_perf_redlines.py slo` rc=0）；总案为 MLC KNN 设定的 GPU 目标（100K 条目 < 5 ms）已被 ADR-192 CPU 分层实现反超（现测 10k = 0.446 ms / 100k = 2.110 ms，独立复测 1.680 ms）；且提案所依据的「CPU 并行度不足」**在码内已被解决**——P1-T14（WI-34）已为 8 个 crate 默认启用 `ComputeBridge::spawn_compute_batch` 卸载（hcw/nmc/faae/osa/decay/repo-wiki/gsoe/mlc，带 env 回退与逐元素一致断测），利用率终验已做过并登记偏差（ADR-155 combined 55.2% → ADR-157 三条件联合判定 0.999）。**⚠ 2026-10-02 覆盖面更正（不影响本 ADR 结论，但削弱本条的部分论据）**：上句「8 个 crate 默认启用 `spawn_compute_batch`」有两处不成立——① `mlc` 走 `hnsw.parallel_search` 内建 rayon，从未经 `spawn_compute_batch`；② 7 个 `spawn_compute_batch` 点位中**只有 5 个有真实生产调用链**（hcw/faae/decay/osa/repo-wiki），nmc 与 gsoe 仅有公开 API 而仓内零生产调用方（判定法：排除 `parallel.rs` 的 `#[cfg(test)]` 尾与 `benches/`、`tests/` 后反查生产调用点）。**对本 ADR 结论的影响评估**：本条结论建立在「SLO 全绿 + ADR-192 CPU 分层反超 GPU 目标」之上，这两项**不依赖**卸载点位计数，故 D1 维持否决与 D3 三门现况均不变；但「CPU 并行度已被卸载解决」这一支撑论据的覆盖面从 8 降为 5，**属论据弱化而非结论动摇**。委员会若需重新评估该论据强度，应以 5 条有效链的实测吞吐为准，而非 8 个点位的账面。
>
> **⚠ 2026-10-05 覆盖面再复核（第三轮收口 T0.1/T0.4 追加；只加注，D1/D3 不变）**：覆盖面按**组合根口径**由 G-87（`scripts/check_compute_offload_reachability.py` + 基线冻结文件）机器判定，初值：WIRED_COMPOSITION=0 / CRATE_API_LIVE=2 / 诊断限定=1 / DEAD=5。历史上流传的 5/7/8/2-4/0 系口径混用非事实矛盾；今后任何「CPU 并行已解决」论据（含本 ADR G-A 支撑句）引用时必须标注 G-87 基线口径。结论不变：G-A 仍未满足。
3. **计算负载前提缺失**：14 个 A/B 级点位中 11 个的输入数据在码内不存在或规模不可达（OSA 无 `5×512×512` 权重矩阵；全仓 0 个 `.onnx`；Parliament 归约规模 = 3-5 角色；路由向量维度 = 64 非 512；事件匹配为 O(1) 精确索引且码内自陈「145 事件量级 broadcast 无瓶颈」；GSOE 打分开销在外部 rollout/LLM；PVL 验证为子串黑名单）。
4. **契约级互斥**：GPU 浮点 `atomicAdd` 击穿 `compute/reduce.rs` 跨构建逐位一致契约（ADR-101/102/106 + `deterministic_bitwise_repeatable` / `crossbuild_marker_output`）；`device.poll` 阻塞等待违反 `critical_sink.rs:41-43` 热路径不阻塞红线并危及 Critical 必达与 `recv == sent` 事件相等性；`event-bus` 调 `nexus-core` GPU 池构成编译级环（`crates/nexus-core/Cargo.toml:20-23`）。
5. **可运维性缺失**：`release.yml:46-73` 五平台矩阵全为 hosted runner（无 GPU）+ `cross`；binary < 50 MB、Docker < 100 MB、coverage min_line = 85 均为硬门；`Cargo.lock` 现测 690 包而 wgpu 家族零命中；无 CI 启用位的可选后端会被 `check_gated_wiring_substance.py:23-28` 记为 PARTIAL/DECORATIVE 入只减台账。
6. **编号治理违规**：总案拟立 ADR-103~110 八个编号 100% 撞号（`crates/nexus-core/Cargo.toml:38` 与 `compute/hts.rs` 已实际引用 ADR-103），违反 `adr_index.md:20-24` 取号前全域查重纪律。

---

## 2. 决策

- **D1 维持否决（不 supersede 任何既决 ADR）**：不引入 `wgpu`、`candle-core`、`burn` 或任何 GPU 计算依赖；不新增第三运行时；`nexus-core` 保持最小依赖形态。既有 ADR-107/123/124/126 状态不变，**本 ADR 是 affirm（重申）而非 supersedes**。
- **D2 永久排除面（Hard Exclusion，不随重启门放开）**：以下四类契约面**任何时刻**不得接受 GPU/非确定后端改造——
  1. 确定性归约（`compute/reduce.rs` DetReduce 双模式、`mask_hash` 一致性路径）；
  2. Critical 事件发布与旁路送达（`event-bus` publish 热路径、mpsc 旁路、`LANE_FORBIDDEN_SHARD` 车道）；
  3. 事件相等性（`recv == sent` / trace-id 传播契约）；
  4. R2 冻结面（ADR-042/052/053，omega-learner / gsoe 学习通道）。
  判据理由：这四面是「结果可复现 + 事件必达」的根，属正确性契约而非性能参数，容差与退化都无法补偿。
  >
  > **覆盖面注（2026-10-05 第三轮追加）**：四面中①确定性归约的实际保护对象 DetReduce 当前**零生产消费者**（全仓仅再导出与自身测试，第三轮审计 F-A4）——这不改变政策（确定性归约仍应永久排除 GPU 改造），但 V4 验收判据实际守护的是无消费者契约；守卫保留成本极低且接线即生效，故判据不撤、证据权重据实登记。
- **D3 重启三门（G-A / G-B / G-C，须同时满足才可另立 ADR 申请）**：申请形式硬约束——新 ADR **必须**以 supersession 形式逐条引用被推翻的 ADR-107/123/124/126 并写明推翻理由与新增证据；**不得复用 10x 号段**（现测最大主编号 = ADR-196，续号自 ADR-197 起），且取号前须按 `adr_index.md:20-24` 纪律全域 grep 查重并在预留登记表立案。
  - **G-A 违例门**：`scripts/check_perf_redlines.py slo` 出现真实判红，且该点位**已完成算法级与并行级 CPU 优化并留档**（对照先例：算法层 ADR-192 拿到 9.8×/22.4×；并行层 P1-T14 已默认开启八 crate 卸载。注：本前置的并行一路**现测已满足**，即未来只剩「算法层仍可优化」或「出现真判红」两条实际入口）。外推加速比、dev-only 档、未复测初值不构成立项证据（`dispatch.rs:10` 诚实数据红线）。
  - **G-B 载荷门**：目标算子在**生产常驻**规模下 ≥ 现 `compute/dispatch.rs`/`HtsTable` 阈值 × 10，且算术密度（FLOP/byte）通过 roofline 判定，且 CPU 路径已无可行的算法级改进空间。
  - **G-C 豁免门**：改造点不属于 D2 四面；且能提供「CPU 参照实现逐位对照」的可测判据（不得以 GPU 结果自证正确）。
- **D4 落点预先改判（若未来通过三门）**：GPU/加速后端**禁止落 L1**；合规形态二选一——(a) L2 可选后端 crate，走 GATED 可选依赖记账（先例 ADR-177 `mca-gateway`）；(b) 运行时能力探测 + 配置/env 降级（先例 `CHIMERA_NO_HNSW_MLC`，ADR-192 D5）。**明确不采用**「feature 标志作降级开关」——ADR-122 已决「降级路径 = 子命令 + 配置（禁 feature 标志）」且全 Phase 1 依此落地，`compute/mod.rs:19`、`reduce.rs:21` 亦自陈无 feature 标志。
- **D5 Phase 2 前置实验（准入门槛，非承诺）**：三项实验全部通过才允许进入 spike——E1 `#![forbid(unsafe_code)]` × `bytemuck::derive(Pod)` 生成 `unsafe impl` 的共存性（本仓 39/39 forbid、零 Pod 用例，**无先例**）；E2 `wgpu` 在 `x86_64-pc-windows-gnu` 的可链接性（先例教训：`nmc-encoder/Cargo.toml:22-34` 因 `ort` 仅支持 MSVC 而弃用）；E3 headless runner/Docker 上 `request_adapter` 行为与退化路径覆盖性（含对 coverage 门的实际影响）。实验一律在 scratch crate，不动主仓。
- **D6 残余价值转接**：总案中真实有效的三项诉求转交 **ADR-196（CPU 侧承接）**——(1) 批量相似度/评分的并行化诉求（真实瓶颈位是串行 map，非算力）；(2) 任务类型登记扩展（`TaskKind` 现有 6 类不含 Hcw/Mlc/Nmc/Vote/Event）；(3) NMC 多模态推理「无法度量」的缺口（性质是模型资产为零，而非 CPU 太慢）。
- **D7 反模式禁令**：总案 §6.3 的 14 条加速比**一律不得登记**入 `perf_redlines.toml`/`perf_thresholds.toml`/`bench_check.yml`；未实测阈值入表即制造幽灵门，违 ADR-190「门 `expect=0` 却 `rc≠0` = 待清债务，不得常态化豁免」范式。

---

## 3. 后果与代价（诚实登记）

- **放弃了什么**：若未来出现真实的大规模纯算术负载（例如常驻 ≥ 100K 向量的检索、真实 ONNX 模型常驻推理），本 ADR 会延缓其 GPU 化路径。缓解：D3 三门把「延缓」变为「可测量、可申请、有判据」，而非永久关闭。
- **不减二进制、不加快启动、不改测试基线**：本轮零代码变更，账面与现状同构。
- **新增文档负担**：本 ADR + ADR-196 + 评审报告三份，须在 `adr_index.md` 预留登记表与 `CODE_WIKI.md §11` 双处登记（治理纪律）。
- **既存债务不受本 ADR 影响**：`check_doc_drift.sh` 与 `check_doc_symbol_claims.py` 开工前既存红（146 vs 151 两处；CODE_WIKI 5 个未注册符号主张），登记于报告 §10.1，留待下一文档批次闭合。
- **治理空档显式化**：外部版本依赖（非 `workspace = true`）不经 `check_dependency_rules.sh:184-202` 拦截，「nexus-core 最小依赖」目前**只是文档硬条款、无机器门**。是否升格为机器门属独立裁决（报告 §7 Phase 1.4），本 ADR 只登记空档不擅自建门。

---

## 4. 验收（本 ADR 的可证伪判据）

| # | 判据 | 验证方式 |
|---|---|---|
| V1 | 全仓仍无 GPU 计算依赖 | `Cargo.lock` 中 `wgpu`/`wgpu-core`/`wgpu-hal`/`naga`/`ash`/`d3d12`/`candle*`/`burn` 零命中（文件名枚举 + 内容搜索双通道） |
| V2 | 三运行时未被引入 | `crates/nexus-core/Cargo.toml` 依赖清单仍为 15 项内部/工作区依赖；无新增 runtime crate |
| V3 | 既决 ADR 未被静默改写 | `ADR-095-134-phase1-consolidation.md:23,39,40,42` 四行文本不变（历史文档只加注不改写原则） |
| V4 | 确定性契约不回退 | `compute` 相关测试 `deterministic_bitwise_repeatable` / `compute_invariant_reduce_deterministic_bitwise` / `crossbuild_marker_output` 仍绿；`parliament` `decision_deterministic` / `test_decision_hash_stable` 仍绿 |
| V5 | 事件面不受扰 | `event-bus` 全测 0 failed；`trace_id_propagation` 仍断言 `recv == sent` |
| V6 | 门不新增失败 | `check_doc_drift.sh` / `check_doc_symbol_claims.py` / `adr_index_sync.py` 结果不劣于开工前基线（报告 §10.1） |
| V7 | 零代码影响 | `git status --porcelain` 无 `crates/`、`scripts/`、`.github/` 变更 |

---

## 5. 待委员会裁决的问题（不在本 ADR 内自决）

1. **Q1**：D3 三门的量化系数（G-B 的「阈值 × 10」）是否合理，或应改为「以 `HtsTable` SPRT 序贯检验实测为准」。
2. **Q2**：「nexus-core 最小依赖」是否升格为机器门（补 §3 治理空档）；若升格，取哪个 G-号（现测 `gate_manifest.toml` 已用至 G-84，下一可用号 **G-85**）。
3. **Q3**：ADR-196 Phase 1 三项承接的排期与批次归属（涉及 `TaskKind`/`HtsTable` 扩容，属 L1 契约面变更，需确认是否走穷举守卫同步）。
4. **Q4**：总案 §1.2 的「八大对标系统均无 GPU ⇒ 技术护城河」这一竞争性论断是否纳入项目战略判断，或判定为与本项目 SLO 无关的外部叙事。

---

## 6. 实施与验证状态（2026-10-03 追加；只加注，不改写上方决策/状态）
- **V1 已机器化**：G-86（`check_accel_exclusion.py`）据本 ADR V1 建为 CI 阻断门，`--selftest`+主判定 rc=0；`Cargo.lock` 690 包 GPU/ONNX 族 0 命中（含 mel_spec 可选 feature 面的回归防护）。
- **V4/V5 复核**：`cargo test --workspace`（2026-10-03，495 个 `test result:` 全 ok）内含 det/相等性/Critical 面测，0 failed。
- **C1/C2 对 G-B 的证据**：生产可达点位静态上界全部差阈值×10 的 2.6×~4 个数量级，算术密度 ~0.25 FLOP/byte，无显存常驻面 → 见 `docs/reports/cpu-plane-c1c2-scale-bounding-2026-10-02.md`；支持终局判据“按事实关闭”，D3 三门现况维持 G-A/G-B 不满足。
- **状态语义**：本 ADR 头仍为 Proposed，`certify_if_reviewed` 正式认证属委员会/所有者，未在此自宣 Accepted；本节仅登记已落地的验证事实。

---

## 7. 认证登记（2026-10-07；W4-0 T0-4 认证推进；只加注，不改写上方决策/状态文本）

经所有者 2026-10-07 明确授权（L0-L3 融合打磨方案 W4-0 T0-4），本 ADR 完成正式认证，头部状态由 Proposed 翻为 ✅ Accepted；`certify_if_reviewed` 程序经所有者路径闭环（§5 中 Q2 已闭，其余问题维持登记、不影响认证）：

1. **D1/D2 事实面**：G-86（`check_accel_exclusion.py`，CI 阻断位）+ G-87/G-88/G-89/G-90 五门在册运行零红；`Cargo.lock` GPU/ONNX 族 0 命中（V1 机器化）。
2. **D3 三门现况**：G-A（SLO 六条全绿、无真判红）/ G-B（生产可达点位静态上界差阈值 2.6×~4 个数量级）→ 按事实关闭、非意见关闭；G-C 维持。
3. **Q2 闭合**：`nexus-core` 最小依赖机器门已由 G-90（`check_nexus_core_dep_freeze.py`，提交 `898ab00`，2026-10-06 用户裁决解锁）落地，原治理空档关闭。
4. **承接面**：ADR-196 Phase B2 已落地（本 ADR D6 转接）；ADR-199 候选 B 已执行（D6(1) 相邻面）；第三轮收口 G-87~G-90 四门。

---

## 8. 更正注记（2026-10-08；v3.5 全栈终局总账 W6-2 批次 C-37 / C-39；只加注，上方 §1-§7 决策与状态文本逐字保留）

本轮全栈取证对本 ADR 的**证据链**（非结论）发现四处需更正项，均按仓库实况回写：

1. **§6 V1 与 §7.1 的「GPU/ONNX 族 0 命中」表述失实 → 收窄为可证伪口径**。`Cargo.lock` 现测含 `tract-core` / `tract-data` / `tract-hir` / `tract-linalg` / `tract-nnef` / `tract-onnx` / `tract-onnx-opl` 共 **7 包在盘**；根 `Cargo.toml` 声明 `tract-onnx = "0.22"`，消费者 `crates/nmc-encoder/Cargo.toml` 为**直接生产依赖**，且 `nmc-encoder` 是现役 workspace 成员、未登记于 `scripts/retired_crates.txt`。准确表述应为：**GPU 计算族（wgpu 系 / naga / ash / d3d12）与 ONNX-Runtime（`ort`）族 0 命中；纯 Rust CPU 推理器 `tract-onnx` 系 nmc-encoder 现役依赖，不在 G-86 禁列内**。V1 本体的枚举清单从未包含 ONNX 推理器（失实仅发生在 §6/§7 的散文汇总）。**本更正不改变 D1 否决结论。**
2. **G-86 覆盖盲区登记（经裁决不扩禁列）**。`scripts/check_accel_exclusion.py` 的 `EXACT_FORBIDDEN`（8 项）+ `FORBIDDEN_FAMILIES`（candle / ort）**不含 `tract`**。不扩面的理由：tract 无 GPU backend feature、与 `#![forbid(unsafe_code)]` 一致，且 `nmc-encoder/Cargo.toml` 注释自陈当初弃 `ort` 正是因为其仅支持 MSVC 工具链——禁 tract 反而误伤现役生产面。盲区在案登记；若未来 tract 引入加速后端 feature，需另立判据而非事后追认。
3. **§7.1「五门在册运行零红」需限定口径**。该句对 G-86~G-90 五个具体门逐条为真，**不可外推为「全量治理门全绿」**。2026-10-08 全量 light 现跑为 91 判据中 76 OK / 5 MISMATCH / 3 ENV-FAIL / 7 heavy SKIP（见 `docs/reports/v3.4-w5-closeout-2026-10-08.md`）；本批次的全量复跑读数见 `docs/reports/` 内 `v3.5-w61-*` 报告。
4. **ADR-196 D1 的支撑句永久失效（本 ADR 不裁它，但必须卸掉对其的依赖）**。「CPU 并行度已由卸载解决」这一常被引为 GPU 否决支撑句的表述，在 G-87 台账的组合根口径下覆盖面为 **1/9**（1 `WIRED_COMPOSITION` + 2 `CRATE_API_LIVE` + 1 `PUB_API_DIAGNOSTIC_ONLY` + 5 `DEAD`）。此后 GPU 否决**不得**再以该句为论据，仅依赖两条不依赖卸载点位计数的判据：(a) D3 G-A「SLO 全绿且 CPU 侧算法/并行优化已留档」；(b) ADR-192 CPU 分层已达成检索目标。
5. **本机硬件事实不构成解冻理由**。现测本机有 NVIDIA GeForce RTX 4060 Laptop GPU（Status OK）+ AMD Radeon 780M，16 逻辑核 / 15.19 GB 物理内存；但 D3 三门判据（违例门 / 载荷门 / 豁免门）**与硬件可用性无耦合**，因此「有卡」不是证据。
6. **新发现的治理缺口：G-A 的「判红」 目前不阻断，且 criterion 侧 SLO 无持续监测**。分两层说清（初稿曾误写为「无任何监测」，已按 workflow 实况收窄）：
   - **有监测的一面**：`.github/workflows/slo.yml`（每日 UTC 04:00 cron）以 release + `--profile slo` + `--run-ignored only` 跑 A1 集 13 条绝对毫秒 p95 红线，并用 `check_perf_redlines.py emit-slo-filter` 与本文件 `-E` 清单对拍防漂移（空值先行拦截，避 `grep -F` 空 pattern 恒匹配的假绿）。
   - **不阻断的一面**：A1 job 现挂 `continue-on-error: true`（`slo.yml:68`，注释自述属“观察期”且切阻塞前须保留退出码可验）⇒ **即使判红也不会拦下发布**。
   - **criterion 侧无人跑**：`scripts/gate_manifest.toml` 内 `check_perf_redlines.py slo` 条目现测计数为 **0**，各 workflow 仅调 `--selftest` / `--static-only` / `compare` / `emit-slo-filter` ⇒ `perf_redlines.toml` 六条 `[[slo]]` 的**基准侧读数**无持续监测，目前只能靠人工复跑留档（如 `tmp/slo_rerun.log`）。
   因此 G-A 的完整判据（「`check_perf_redlines.py slo` 出现真实判红」）**不能在无人工介入下自动成立**；是否把 A1 切阻塞、以及是否给 `slo` 子命令建在册门，均属独立裁决（需新 ADR，取号自 ADR-202 起且先查 `adr_index.md` 重复）。本轮**只登记缺口不擅自改动 workflow**，与 §3「治理空档显式化」同一纪律。
7. **G-A 的现势实测（2026-10-08 / v3.5 W6-1 M1；用人工实跑补上无人值守缺口的证据）**：`check_perf_redlines.py slo` 逐条实跑六条 SLO，**全部 PASS**（日志 `tmp/v35-m1-slo.txt`）。实测值 / 红线（= 80% SLO）/ 余量：`window_select` 1.4602 ns / 0.8 ms / ≈5.5×10⁵；`mlc_l2_knn`（10k 档）102.06 µs / 4 ms / 39×；`decay_compute` 71.087 ns / 0.8 µs / 11×；`wiki_knn_100k` 209.68 µs / 40 ms / 191×；`immune_probe` 614.37 ns / 80 ms / ≈1.3×10⁵；`linucb_40arm` 6.9502 µs / 40 µs / **5.8×（本次最小余量）**。
   - 环境如实登记：本机 FreeMem 仅 2.1–2.5 GB（低于协议空闲阈 > 8 GB），属**内存压力档**；结论仍可用，因为**最小余量 5.8× 已大于 W5-C 实测的最差污染系数 ≈ 2.8×**——压力可吃掉的比例不足以造出假绿。
   - 相对 2026-10-05 日志的差值**方向不一致**（-29% 至 +38%，有快有慢），不符合 W5-C 记录的“全局≈1.7× 膨胀”形态⇒本轮无系统性降频；但**这些绝对值仍不得当契约值引用**（权威读数应迁 CI clean-runner）。
   - 结论：G-A「出现真实判红」前置**未满足**，维持否决的判据从“旧留档 + 人工推定”升为**本轮 A 级实测**。
