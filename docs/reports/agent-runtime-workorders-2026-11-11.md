# Chimera CLI 全链路性能与资源效率专项：四阶段工程工单集

- **日期**：2026-11-11
- **基线**：`feat/toolchain-parallel-optimization` @ `635a5651`，工作树干净，未 push
- **宿主**：Windows 11 + Git Bash，`stable-x86_64-pc-windows-gnu`，cargo/rustc **1.97.1**，D 盘余 **98 GB**
- **性质**：**零生产代码改动**的工单交付物（用户裁定 4）；实施按工单另批授权
- **前序**：上一批 W0~W6（ADR-278~283，7 commits）已落地；本批不重做已裁决项

---

## 摘要（先读这段）

你要的是五个优先级的全链路重构。盘点后的核心事实是：**五个优先级里，(3) 的靶子在本仓不存在，(1)(2)(4)(5) 的可动增量已被前 7 轮收敛到很小，而本轮真正挖到的最高 ROI 项是一个既有的治理面判红。**

三条硬结论：

1. **优先级 3「GPU 利用率（本地 LLM 推理：批处理 / KV cache / 显存驻留）」在本仓无靶子**【已验证】。Cargo.lock 对 `wgpu / candle / burn / ort / llama / gguf / cuda / vulkan / naga / ash / d3d12` 全部 `^name =` 锚定**零命中**；唯一本地推理是 `nmc-encoder` 的 tract-onnx **CPU 小型嵌入模型**，默认关闭且仓库内零权重文件；LLM 生成全部经 `mca-gateway` 走远端 HTTP/SSE。没有本地 LLM 推理，就没有 KV cache 与显存驻留的宿主。按你的裁定缩窄为 tract-onnx 依赖的 ROI 裁决（工单 T3）。

2. **优先级 5「接口契约保护」已经机器化，不需要新建**【已验证】。L0 公开接口面有 4 台 only-decrease 签名快照门在 CI 跑：G-105（`pub fn` 棘轮）、G-107（根 re-export 台账）、G-115/116（可扩展性分级，227 类型 = 56 open / 171 closed）、G-135（L0-B 签名泄漏，408 pub fn），加 G-125/126 层向门。工单的「接口 diff 校验」直接引这 6 台门，不新造。

3. **本轮最高 ROI 是一个 P1 既有判红**：`scripts/govd/tests/test_govd.py` 实跑 **79 tests → FAILED (failures=3)**，`AssertionError: 142 != 140`。归属已逐 commit 追溯：`fe98e304`~`c3a01eaf` 均为 140/133（与断言一致），**`58a7567d` 把它变成 142/135**，而上一批开工基线 `6aed20b4` 已经是 142 ⇒ **判红早于上一批，与本批和上批均无关**。它漏过历次「全绿」结论的机制是：govd 在 CI 是 **deliberately non-blocking shadow**（`ci.yml:1002` 注释引 ADR-228 §3.6 / I7）。

**工单总览**（ROI 从高到低）：

| 号 | 靶子 | 性质 | ROI | 实施成本 |
|---|---|---|---|---|
| **T0** | govd 普查判红（142/135 vs 断言 140/133），三处同批 | 治理回归修复 | **最高** | 极低（3 处数字/字符串） |
| **T1** | `mixed7r1w` bench 未登记阈值门，ADR-281 的 4.50× 无 CI 防回退 | 验证面补洞 | **高** | 低（2 个 toml + workflow heredoc） |
| **T2** | `superposition_query` 零生产调用方 + JoinSet fanout 无上界 | 结构死臂裁决 | **高** | 低（删 ~520 行或登记死臂） |
| **T3** | tract-onnx 7 包重依赖，编译在盘 / 运行期零加载 | 依赖 ROI 裁决 | **中—待定档** | 中（需先测编译时长与 release 体积） |
| **T4** | `perf_thresholds.toml:5-15` 头注仍声称已被 I9/I10 根修的缺陷 | 文档订正 | 中 | 极低（纯注释） |
| **T5** | mcp-mesh WAL 在 instant 路径 3.253ms 里的占比未知 | 归因测量 | **待测量** | 低（bench 加对照臂） |

**「不优化」清单 21 项**（§1.2），每项附实测数据与权威裁决指针 —— 这是你允许的合法结论。

**四项用户裁定（本文件的前提，2026-11-11）**

| # | 冲突 | 裁定 |
|---|---|---|
| 1 | GPU 优先级 3 无靶子 + 三重关闭 | **缩窄为 CPU 嵌入面**（tract-onnx 可达性裁决）；GPU 维持 ADR-195 + G-86 |
| 2 | 要求 2024 edition / MSRV 1.95 vs 仓库 2021 / 1.97 | **维持 edition 2021 + MSRV 1.97**；2024 迁移单列契约变更清单（§5） |
| 3 | 阶段0 基线怎么跑（磁盘/工具约束） | **定向基线 + CI 委托** |
| 4 | 交付边界 | **只出四阶段工单文档，零生产代码改动** |

---

## 阶段 0：现状全面盘点

### 0.1 并发 / 资源现状地图（模块路径 → 当前实现模型 → 已知瓶颈点）

置信度标注：【已验证】= 本轮命令或读码现取；【推断】= 由证据推出但未直接实测；【待测量】= 需补 benchmark。

#### A. 工具调用四环链（`crates/gqep-executor/src/tool_runtime/`）

| 环节 | 当前实现模型 | 证据锚点 | 已知瓶颈点 | 置信度 |
|---|---|---|---|---|
| PolicyLayer | sync 判决后直转 next；计数为 `AtomicU64`，无锁 | `chain.rs:30,42-47`；`crates/seccore/src/execpolicy.rs:56-60` | 无。实测 33.3 ns，阈值 10 µs，余量 **300×** | 【已验证】 |
| SeccoreSandboxGate | `RwLock<HashMap>` 派生策略缓存 ≤256 条 / 键长 ≤128；缓存 miss 才取写锁 | `sandbox_gate.rs:40-43,94-98,106-115` | **链上唯一 per-request 共享锁**（读多写少、临界区短）。实测 seccore 臂 644 ns / legacy 臂 2.95 µs，阈值 20 µs / 60 µs，余量 31× / 20× | 【已验证】 |
| RouteLayer | sync 查不可变 `HashMap` + fallback | `chain.rs:114-138` | `mcp.*` 不进路由表恒走 fallback（`mcp_tool_executor.rs:259-264` 的 `capability_of` 只转发 builtin）。实测 route_miss 106 ns | 【已验证】 |
| TransportLayer | 纯直转发；组装期 routes/fallback 均被 wrap | `chain.rs:142-156,231-239` | 无 | 【已验证】 |
| InProcDispatcher | async 零克隆转发 | `inproc.rs:16-43` | 无。生产段 `.clone()` 计数 **0** | 【已验证】 |
| ExecPolicyToolExecutor | async；Deny/Ask fail-closed；capability 透传 delegate | `exec_tool_executor.rs:35-99`（fail-closed `:79-84`，透传 `:93-99`） | 无。生产段 `.clone()` 计数 **0** | 【已验证】 |
| CompositeToolExecutor | `tool_name.starts_with(MCP_TOOL_PREFIX)` 前缀分派 | `mcp_tool_executor.rs:236-264` | `mcp.*` 副作用恒未知 ⇒ 计划层按「可能写」保守处理 | 【已验证】 |
| BuiltinToolExecutor | 只读能力表 `BUILTIN_READONLY_TOOLS` 三件 | `exec_plan.rs:41-81`（常量 `:52`） | 未登记者 fail-closed 串行 | 【已验证】 |
| McpToolExecutor | async，经 `mesh.execute_transaction` | `mcp_tool_executor.rs:85-227`（调用 `:203-211`） | `capability_of` 恒 None（`:222-224`）、`declared_tools` 恒空（`:225-227`，ADR-256 D2 诚实语义） | 【已验证】 |

**判读**：四环链本身**无全局锁、无信号量、无队列**；per-request 唯一共享锁是沙箱门的 `RwLock` 读锁。链上全部环节实测在 **ns~µs 量级**，阈值余量 17×~300×。真正的串行点在计划层（下节），且是**设计裁决**而非疏漏。

#### B. 计划层并行度（`crates/gqep-executor/src/toolplan_runner.rs`）

| 构件 | 实现模型 | 证据锚点 |
|---|---|---|
| `compute_layers` | Kahn 入度轮次分层 + **声明序稳定化** | `:561-619`（稳定化 `:582-584,610-611`） |
| `exec_layer` | 三路分派：全读→并发 / 全写→串行 / 混层→分段 | `:290-314` |
| `exec_layer_serial` | 逐节点 await | `:384-402` |
| `exec_layer_concurrent` | `Semaphore::new(concurrency.max(1))` + `FuturesUnordered`；**每层新建信号量** | `:409-447`（信号量 `:417`） |
| `exec_layer_segmented` | 写节点为屏障切读段；读段与写节点不并发 | `:328-376`（ADR-281） |
| `effective_side_effect` | 优先级：声明 > 能力表 > 未知（按可能写） | `:528-535` |
| `node_is_read_safe` | 只读判定 | `:542-549` |

- 并发上界：`DEFAULT_CONCURRENCY = 8`（`:172`），`with_concurrency` 把 0 归一为 1（`:191-194`）；CLI `--plan-concurrency` 默认 8（`cli.rs:478-480`）→ `exec_plan.rs:434`
- `MAX_STEPS = 64` 定义在 `crates/nexus-contracts/src/spec/tool_plan.rs:127`，`toolplan_runner.rs:96` 引为默认
- `PlanGuardOverrides::apply`（`exec_plan.rs:283-294`）是 **min 语义只收紧**（ADR-269）
- **跨层复用信号量已被 ADR-277 D1 否决**（前提 `MAX_STEPS=64` ⇒ µs 级，无重开条款）

#### C. 动作环背压（`crates/chimera-cli/src/action_orchestrator.rs`）

| 构件 | 实现模型 | 证据锚点 |
|---|---|---|
| `ACTION_CONCURRENCY_DEFAULT = 8` | 与 `BatchLimits` 同源 | `:351`（ADR-251 D-W5-A） |
| `resolve_action_concurrency` | env `CHIMERA_ACTION_CONCURRENCY`；`unlimited`→`usize::MAX`；0/非法→8 + warn | `:361-388` |
| `action_semaphore` | 钳 `MAX_PERMITS` | `:392-396` |
| `run_action_loop` | `try_acquire_owned` 成功→spawn；**耗尽→recv 循环内同步执行**（降级串行、不丢事件） | `:408-448`（降级 `:431-436`，Lagged warn 继续 `:439-441`） |

**已知取舍**：许可耗尽时在 recv 循环内 inline await 是**单点串行降级**，ADR-251 5.1 已登记；既有测试 `bounded_loop_caps_inflight_and_delivers_exactly_once` 证明在飞峰值 ≤ bound+1 且每 request_id 恰处理一次 ⇒ 不进 ROI 表。

#### D. 背压载体普查

口径：正则 `Semaphore::new|mpsc::channel\(|unbounded_channel|broadcast::channel\(|JoinSet|FuturesUnordered|buffer_unordered|\.buffered\(`，扫 `crates/**`，生产 = `*/src/` 且剔除 `#[cfg(test)]` 段与 `tests/`、`benches/`。

| 载体 | 生产处数 | 有界性 | 证据锚点 |
|---|---|---|---|
| tokio `Semaphore` | 5 处 / 4 crate | **全 bounded** | `chtc-bridge/src/bridge.rs:49,61`（默认 32，`config.rs:27`）；`chimera-mas/src/knowledge/expert_consult.rs:111`（CPU×2）；`action_orchestrator.rs:392`；`toolplan_runner.rs:417` |
| tokio mpsc bounded | 2 处 | 有界 4096 | `event-bus/src/bus.rs:1429`（容量常量 `:41`）；`experience_card_bus.rs:207`（满时发布方采样丢弃并计数，不阻塞热路径） |
| broadcast | 4 处 | 有界环 1024，溢出 `Lagged` 丢弃 | `bus.rs:625,657,695`（`DEFAULT_CAPACITY` `:33`）；`experience_card_bus.rs:117` |
| `JoinSet` | 3 处 | 2 有界 / **1 无上界** | `pvl-layer/src/concurrency.rs:82`（默认 max_concurrency=4，`:47`）；`nexus-subagent/src/runtime.rs:55,74`（`SWARM_LIMIT=8`，`types.rs:8`）；**`mcp-mesh/src/quantum/superposition.rs:187` 无独立信号量** |
| **unbounded 通道** | **1 处** | 无背压，但为请求-响应形态 | **`repo-wiki/src/store.rs:275`** `std::sync::mpsc::channel()` 写线程通道 |
| `FuturesUnordered` | 集中于 gqep-executor / chimera-mas / parliament / mcp-mesh | 受 `BatchLimits` 钳 | `crates/gqep-executor/src/toolplan_runner.rs:418`；`crates/gqep-executor/src/tool_runtime/batch.rs:241`（钳 `[1, MAX_GATHER_FANOUT=256]`，`crates/gqep-executor/src/config.rs:24,117-118`）；`crates/gqep-executor/src/gatherer.rs:227`；`crates/chimera-mas/src/delegation.rs:590`；`crates/chimera-mas/src/chunker.rs:370`（`MAX_QUADRANT_FANOUT=4`）；`crates/parliament/src/debate/mod.rs:1237`；`crates/parliament/src/immune_system.rs:444`；`crates/mcp-mesh/src/mesh.rs:376,551,666` |
| `buffer_unordered` / `.buffered(` | **生产零使用** | — | 仅存 3 处「WHY 不用」注释：`batch.rs:19-21`、`gatherer.rs:219`、`config.rs:58`（未拉取源项 never polled，ADR-202 否决理由） |

#### E. CPU 计算面卸载（`scripts/compute_offload_reachability_freeze.txt`，G-87，组合根口径 9 锚点）

| # | 锚点 | 状态 |
|---|---|---|
| 1 | `chimera-cli/overwindow_bridge.rs::dense_scores_pooled` | **WIRED_COMPOSITION**（唯一） |
| 2 | `decay-engine/parallel.rs::decay_batch_parallel` | DEAD |
| 3 | `faae-router/parallel.rs::score_parallel` | DEAD |
| 4 | `faae-router/parallel.rs::score_scoring_parallel` | CRATE_API_LIVE |
| 5 | `gsoe-evolution/fitness_parallel.rs::score_batch` | DEAD |
| 6 | `hcw-window/parallel.rs::score_parallel` | PUB_API_DIAGNOSTIC_ONLY |
| 7 | `nmc-encoder/parallel.rs::perceive_parallel` | DEAD |
| 8 | `osa-coordinator/parallel.rs::compute_five_masks_parallel` | DEAD |
| 9 | `repo-wiki/vector/parallel.rs::knn_batch_parallel` | CRATE_API_LIVE |

- `TaskKind` 十变体阈值唯一定义源 `crates/nexus-core/src/compute/hts.rs:120-181`（`default_entry`）：ClvSimilarity 1000 / OsaMask 100 / KnnSearch 5000 / GsoeEvaluate 500 / CscCollapseScore 200 / Generic 10000 / HcwEntryScore 200 / NmcEncode 1000 / ExpertScore 10000 / DecayBatch 10000；由 `dispatch.rs:177-188` 独立 expect 数组双路径锁定
- `spawn_compute_batch`（`bridge.rs:399`）是 **sync fn**（非 async），饱和三策略 Observe/Degrade/Strict（`:394-397`）
- 全仓唯一计算面 `block_in_place` 在 `overwindow_bridge.rs:371`（`:340` 明言「故不用 spawn_blocking」）；`spawn_blocking` 生产用法仅在 IO 面（`cmt-tiering/src/pool.rs:153,165,177`、`chimera-cli/src/permission.rs:278`）
- G-88 GUARDED 5 锚点：`overwindow_bridge.rs::dense_scores`、`faae-router/parallel.rs::score_experts_batch_offloaded` 与 `::score_experts_scoring_batch_offloaded`、`hcw-window/window.rs::select_window`、`osa-coordinator/coordinator.rs::compute_all_masks_batch`；其余 8 行 NO_ASYNC_BOUNDARY
- 7 个 `parallel.rs` 中 4 个头部有「接线三要件」（错误姿态评审 ADR-197 D2 / G-88 让出保护 / 逐位等价断测），例 `decay-engine/src/parallel.rs:7-17`

#### F. 能耗与轮询面

`scripts/idle_wakeup_audit.py` 实跑：**生产 30 / 测试 99**（测试剔除口径 = 文件最后一个顶层 `#[cfg(test)]` 或 `mod tests` 之后，`:39-54` 启发式）；该脚本 **advisory 恒 exit 0**（ADR-264 自陈）。

生产周期 `loop + sleep/interval` **7 处**：

| 位置 | 间隔 | 性质 |
|---|---|---|
| `chimera-cli/src/experience_loop.rs:190` | 60s（常量 `:68`） | 心跳 |
| `event-bus/src/bus.rs:1816-1825` | 60s（调用方传入，`experience_loop.rs:256`） | throughput reporter |
| `decb-governor/src/governor.rs:561` | 10s（默认，`config.rs:71`） | 治理周期 |
| `faae-router/src/edsb.rs:346` | 300s（默认，`config.rs:124`） | 低频维护 |
| `nexus-telemetry/src/report.rs:142` | 60s（`acp.rs:125` / `serve.rs:46` 传入） | saturation reporter |
| `seccore/src/sandbox.rs:982-984` | 60s（`experience_loop.rs:268-271` 传入） | interception reporter |
| `session-store/src/writer.rs:568` | **2ms**（`base_window_ms=2`，`types.rs:248`，自适应 1-4ms） | flush loop |

分布：**2ms×1 / 10s×1 / 60s×4 / 300s×1**。`crates/mcp-mesh/src/mesh.rs:442,632` 是重试退避（`backoff_ms`）非周期轮询。

已无轮询的两处（事件驱动改造已完成）：`quest-engine/src/ambient_mode.rs:210`（`select!` 纯事件等待，`:9-10,27` 自陈「无轮询锁，不 spawn 定时器」；3600s tidy / 300s checkpoint 是节流判据非定时器）、`event-bus/src/shard.rs:61`（等待路径唯一原语 `tokio::sync::Notify`，禁自旋/忙轮询，ADR-129）。

### 0.2 基线测量方案

#### 0.2.1 工具可用性（已核实，避免开出跑不了的方案）

| 工具 | 状态 | 证据锚点 |
|---|---|---|
| hyperfine **1.20.0** | ✅ 已装且已接 CI | `bench_check.yml:647-669`（taiki-e 安装）；`scripts/check_cli_startup.ps1:16`（`--warmup 3`） |
| criterion | ✅ **128** 个 bench 文件 / **56** 条 `[[run_bench]]` / **85** 个 `thresholds_ns` 键 | `ls crates/*/benches/*.rs`；`scripts/perf_thresholds.toml`；键数以 **I9 门自报 85 keys** 为准 |
| `scripts/energy_profile.ps1` | ✅ RSS **双口径**（窗口峰值+均值 / `PeakWorkingSet64` 生存期峰值，ADR-265） | `:5-13`；调用 `.\scripts\energy_profile.ps1 -DurationSec 30 -ProcessName chimera`；dev-only，无需管理员 |
| cargo-flamegraph 0.6.14 / samply 0.13.1 | ⚠️ 已装未接线 | `docs/reports/toolchain-analysis-report.md:109,116` |
| **flamegraph（Windows GNU）** | ❌ 不可用 | `docs/reports/perf-comparison-report.md:7` |
| **tracy / `perf record`** | ❌ **全仓零命中** | 口径：`.github` + `scripts` + `docs` + `crates` 的 rs/toml/yml/md，regex 为 `\btracy\b` 或 `perf record`，大小写不敏感 |
| **`/usr/bin/time -v`** | ❌ 无脚本/CI 接线 | 仅 `docs/architecture/NEXUS_OMEGA_TUI_DESIGN_BIBLE.md:901,910` 的文档目标（TUI RSS<50MB） |

**如实说明**：任务书举例的 `tracy` / `perf` / `/usr/bin/time -v` 在本机 Windows GNU 宿主**不可用**（前两者全仓零命中，第三者无接线）。因此 RSS 走 `energy_profile.ps1` 双口径，CPU 热点走 criterion + `cargo build --timings`，启动延迟走已接线的 hyperfine。不假装能跑不存在的工具。

#### 0.2.2 定向基线命令集（本轮实跑，全部只写 `target/` 与 `tmp/`）

```bash
# 环境（Git Bash；PATH 必须 POSIX 风格，CARGO_HOME/RUSTUP_HOME/TMP 保持 Windows 风格）
export PATH="/d/Chimera CLI/.toolchain/cargo/bin:/d/msys64/mingw64/bin:$PATH"
export CARGO_HOME="D:\Chimera CLI\.toolchain\cargo"
export RUSTUP_HOME="D:\Chimera CLI\.toolchain\rustup"
export TMP="D:\Chimera CLI\tmp"; export TEMP="D:\Chimera CLI\tmp"

# B1 CLI 启动延迟（hyperfine；需 release 二进制，脚本 :37 硬编码 target\release\chimera.exe）
cargo build --release -p chimera-cli
pwsh -NoProfile -File scripts/check_cli_startup.ps1        # 产出 docs/reports/cli-startup-baseline.json
# CI 侧口径：bench_check.yml cli-startup job（ubuntu-latest，-r 20 runs，阈值 median < 100ms）

# B2 计划层并行度（含 ADR-281 混层）
cargo bench -p gqep-executor --bench toolplan_layers --jobs 2 -- --quick --noplot

# B3 四环分层 + 批量派发
cargo bench -p gqep-executor --bench tool_dispatch_layers --jobs 2 -- --quick --noplot
cargo bench -p gqep-executor --bench tool_runtime_batch   --jobs 2 -- --quick --noplot

# B4 MCP 2PC（含上批 A/B 的 instant 对照臂）
cargo bench -p mcp-mesh --bench mesh_benchmark --jobs 2 -- --quick --noplot

# B5 RSS 双口径（需 release 二进制 + 长驻进程）
pwsh -NoProfile -File scripts/energy_profile.ps1 -DurationSec 30 -ProcessName chimera

# B6 tract-onnx 编译面成本
cargo tree -p nmc-encoder -i tract-onnx                    # 反向依赖
cargo build -p nmc-encoder --timings                       # 产出 target/cargo-timings/*.html
du -cm target/debug/deps/libtract*                         # 逐包产物体积

# B7 分配棘轮现值（必须串行窗：两 tokio 测试并行会污染计数）
cargo test -p gqep-executor --test dispatch_alloc_budget --jobs 2 -- --test-threads=1 --nocapture

# B8 静态门与普查
py -3 scripts/check_perf_redlines.py --selftest
py -3 scripts/check_perf_redlines.py invariants
bash scripts/check_perf_redlines.sh --static-only
py -3 scripts/idle_wakeup_audit.py
py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v
```

**采样纪律**（每条都来自本仓已登记的教训）：

1. criterion 用 `--quick --noplot`（降 sample 数但保留置信区间）；报告读数一律取 **median + 置信区间**，不用单点。
2. µs 级门必须**静默态单跑**——`faae-router/src/router.rs:957`、`osa-coordinator/src/coordinator.rs:1207` 的 `#[ignore]` 即此判据。
3. **禁止两个 bench 并发**：上一批实测两个 `mode_redline` 后台并发互撞，致多个 bench `cargo exit=101`。本轮 B2→B3→B4 串行链跑，全 rc=0。
4. 分配计数类测试必须 `--test-threads=1`（`dispatch_alloc_budget.rs:86` 记载的教训）。
5. 同机同 HEAD A/B；记录同时段磁盘余量（本轮起止均 98 GB，无压力）。
6. `estimates.json` 的数值单位是**纳秒**；criterion 的 `change:` 块会额外输出一行带 `%` 单位的 `time:`，解析器必须只认「可解析为时长」的那一行（上一批 RL-07 即因此假 SKIP，已由 `single_criterion_time` 修复并加 selftest-24f 锁）。

### 0.3 baseline 指标表

测量态：2026-11-11，本机 Windows GNU，cargo/rustc 1.97.1，`--quick --noplot`，串行链，D 盘余 98 GB。原始输出：`tmp/wo2026-b234-bench.log`、`tmp/wo2026-b6-tract.log`、`tmp/wo2026-b7-allocbudget.log`、`tmp/wo2026-b8-staticgates.log`、`tmp/wo2026-govd-census.log`。

#### 0.3.1 计划层（B2）

| bench（criterion 路径） | median | 95% 置信区间 | 阈值键 | 阈值 | 余量 |
|---|---|---|---|---|---|
| `toolplan_layers/readonly8_ms2/1` | **82.545 ms** | [79.533, 83.298] | `tplan_declared_serial` | 175 ms | 2.12× |
| `toolplan_layers/readonly8_ms2/8` | **12.247 ms** | [12.152, 12.270] | `tplan_declared_parallel` | 38 ms | 3.10× |
| `toolplan_layers/undeclared8_ms2/1` | **73.794 ms** | [73.635, 73.833] | `tplan_undeclared_serial` | 175 ms | 2.37× |
| `toolplan_layers/undeclared8_ms2/8` | **13.560 ms** | [13.481, 13.877] | `tplan_undeclared_parallel` | 32 ms | 2.36× |
| `toolplan_layers/mixed7r1w_ms2/1` | **98.082 ms** | [97.912, 98.764] | **无键** | — | — |
| `toolplan_layers/mixed7r1w_ms2/8` | **21.777 ms** | [18.133, 22.688] | **无键** | — | — |

阈值证据：`scripts/perf_thresholds.toml:119-122`（值）与 `:281-284`（filter）。

**并行加速比**：readonly **6.74×**（82.545/12.247）/ undeclared **5.44×**（73.794/13.560）/ **mixed 4.50×**（98.082/21.777）。

**混层读数的诚实披露**：ADR-281 落地时报 5.82×，CI 保守值 4.59×，本轮实测 **4.50×** —— 同一份代码三次读数在 4.50~5.82 间波动。根因是 c8 臂置信区间宽达 **25%**（[18.133, 22.688]，因写屏障后的读段调度受机器负载影响）。门槛是 ≥4×，本轮 4.50× **通过但余量仅 12.5%**。这直接决定了工单 T1 的阈值取值不能照抄上批数字（见 §2 T1）。

#### 0.3.2 四环分层（B3）

| bench | median | 阈值键 | 阈值 | 余量 |
|---|---|---|---|---|
| `dispatch_layers/floor_echo_transport` | **56.851 ns** | `dispatch_floor_echo` | 1000 ns | 17.6× |
| `dispatch_layers/policy_evaluate_allow` | **33.300 ns** | `dispatch_policy_eval` | 10000 ns | 300× |
| `dispatch_layers/sandbox_check_allowall` | **207.97 ps** | — | — | — |
| `dispatch_layers/sandbox_check_seccore` | **644.21 ns** | `dispatch_sandbox_cached` | 20000 ns | 31.0× |
| `dispatch_layers/sandbox_check_seccore_legacy` | **2.9503 µs** | `dispatch_sandbox_uncached` | 60000 ns | 20.3× |
| `dispatch_chain/chain_allowall_route_hit` | **129.40 ns** | `dchain_allowall_hit` | 10000 ns | 77.3× |
| `dispatch_chain/chain_seccore_route_hit` | **681.61 ns** | `dchain_seccore_hit` | 40000 ns | 58.7× |
| `dispatch_chain/chain_seccore_route_hit_legacy` | **2.9154 µs** | `dchain_seccore_legacy` | 80000 ns | 27.4× |
| `dispatch_chain/chain_policy_deny` | **75.399 ns** | `dchain_policy_deny` | 2000 ns | 26.5× |
| `dispatch_chain/chain_sandbox_deny` | **96.306 ns** | `dchain_sandbox_deny` | 12000 ns | 124.6× |
| `dispatch_chain/chain_route_miss` | **106.29 ns** | `dchain_route_miss` | 12000 ns | 112.9× |

阈值证据：`perf_thresholds.toml:74-80`（值，已在 §0.2 现取核对）与 `:261-264`（filter）。

**并发读对照（ADR-241 D5 的独立交叉验证）**：`dispatch_sandbox_concurrent/rwlock_read_c4` = **186.91 µs** vs `dashmap_read_c4` = **201.99 µs** ⇒ RwLock 快 **7.5%**；`rwlock_read_c8` = 326.10 µs。ADR-241 D5 于 2026-11-06 以 sample_size=30 否决 DashMap，本轮独立复测**同向**，该否决维持。

#### 0.3.3 批量派发（B3）

| bench | median | 阈值键 | 阈值 | 余量 |
|---|---|---|---|---|
| `tool_batch/serial_ms1/16` | **191.15 ms** | — | — | — |
| `tool_batch/parallel_ms1_bound4/16` | **49.370 ms** | — | — | 3.87× 加速 |
| `tool_batch/serial_ms1/64` | **817.18 ms** | `tbatch_serial_slow` | 1200 ms | 1.47× |
| `tool_batch/parallel_ms1_bound8/64` | **93.039 ms** | `tbatch_parallel_slow` | 400 ms | 4.30× |
| `tool_batch/serial_ms0/64` | **46.841 µs** | `tbatch_serial_zero` | 1000 µs | 21.4× |
| `tool_batch/parallel_ms0_bound8/64` | **50.166 µs** | `tbatch_parallel_zero` | 2000 µs | 39.9× |

阈值证据：`perf_thresholds.toml:102-103`（slow）与 `:272-275`（filter）。

**关键读数（ADR-277 的直接实证）**：1ms 负载下并行加速 **8.78×**（817.18/93.039）；但**零延迟负载下并行反而慢 7.1%**（46.841 µs → 50.166 µs）⇒ spawn + Semaphore 的固定开销在 µs 级负载下超过并发收益。**这不是新发现**：阈值门本身已承认它 —— `tbatch_parallel_zero`(2000 µs) 是 `tbatch_serial_zero`(1000 µs) 的 **2 倍**，即设计者给并行档留了双倍宽松余量。ADR-277 D1 的四项 µs 级否决由此获得本轮独立实证支撑。

#### 0.3.4 MCP 2PC（B4）

| bench | median | 对照 | 收益 |
|---|---|---|---|
| `mcp_mesh_transaction/2pc/1` | **27.553 ms** | `mcp_mesh_transaction_instant/2pc/1` = **3.2532 ms** | **8.47×（−88.2%）** |
| `mcp_mesh_transaction/2pc/3` | **25.334 ms** | instant/3 = **3.6193 ms** | 7.00×（−85.7%） |
| `mcp_mesh_transaction/2pc/5` | **21.548 ms** | instant/5 = **3.3048 ms** | 6.52×（−84.6%） |
| `mcp_mesh_concurrent/100_concurrent_5_servers` | **216.37 ms** | — | — |

口径说明：`mcp_mesh_transaction` 臂走 `InProcessClient::new()`（保留 1-2ms 模拟延迟，供测试夹具），`mcp_mesh_transaction_instant` 臂走 `instant()`（生产路径，ADR-280）。上一批报告的 −78.7% 是 criterion `change` 口径（与 base 对比），本表是**同轮双臂 A/B** 口径，两者不可直接比较；生产路径的权威读数是 **instant 臂 3.25 ms**。

**未闭合归因**：instant 臂的 3.253 ms 对进程内 2PC 仍偏高，构成项未拆分（2 次 WAL fsync + 全载荷 `sensitive::scan` + 2PC 帧编解码）。⇒ 工单 T5。

#### 0.3.5 分配与内存（B7）

| 指标 | 实测 | 登记上界 | 余量 | 证据 |
|---|---|---|---|---|
| 单次派发分配次数 | **6** | 7 | 1 次 | `crates/gqep-executor/tests/dispatch_alloc_budget.rs:77` |
| 批量 8 派发分配次数 | **68** | 68 | **0（贴顶）** | `:80` |

棘轮语义 = 只降不升（`used <= cap`，`:97,114`）。**批量档贴顶**说明该路径分配面已压到极限，无优化空间；同时任何新增分配会立刻判红（门有牙）。零分配锁另有 `crates/seccore/tests/policy_zero_alloc.rs:18` 的 `CountingAlloc`。

**口径边界（重要）**：`CountingAlloc` 只计**分配次数/字节数**，**不测 RSS**（`docs/reports/memory-bound-audit-2026-11-08.md:49`）。故 RSS 峰值只能靠 B5，本轮**未跑**（需 release 二进制 + 长驻进程；按裁定 3 委托 CI）⇒ RSS 标【待测量】。

常驻缓存上界（`memory-bound-audit-2026-11-08.md` §1，11 项全部 file:line 现取）：scc-cache 主缓存 256 / semantic_cache 256 每命名空间 / CMT Hot-Warm-Cold 256-4096-65536（Ice 无限=对象存储语义，盘面非内存）/ CMT experience card Hot 256 / HCW 四级窗口 4096-32768-131072-1048576 token / session-store 4096 行每段 + 微批 64 条每 2ms / event-bus broadcast 1024 / Critical 旁路 bounded mpsc / 沙箱门派生策略缓存 ≤256 条 / 批量派发 slots = 批大小（非常驻）/ TUI sysinfo 5000ms 采样非累积。

#### 0.3.6 依赖面（B6）

| 指标 | 实测 | 证据 |
|---|---|---|
| tract 族包数 | **7**（tract-core / data / hir / linalg / nnef / onnx / onnx-opl） | `Cargo.lock:6265,6291,6317,6328,6356,6376,6394` |
| 唯一反向依赖 | **nmc-encoder** | `cargo tree -p nmc-encoder -i tract-onnx` 输出仅一条边 |
| debug 产物总体积 | **347 MB / 41 文件** | `du -cm target/debug/deps/libtract*` |
| 占 `target/debug/deps` 份额 | **1.7%**（347 / 20769 MB） | 同上 |
| 逐包分布 | tract-core **171** / onnx **48** / nnef **38** / hir **33** / data **29** / linalg **20** / onnx_opl **11** MB | 同上 |
| 编译时长差 | 【待测量】 | 需改 `Cargo.toml` 做有/无 tract 的 A/B，属实施批 |
| release 二进制体积增量 | 【待测量】 | 同上 |

**这组数据实质性削弱了「重量级依赖」的直觉论据**：磁盘面只占 1.7%。T3 的裁决必须建立在编译时长与 release 体积上，而这两项本轮无法测（要改 Cargo.toml）。

#### 0.3.7 启动延迟与静态门（B1 / B8）

| 指标 | 值 | 口径 | 证据 |
|---|---|---|---|
| CLI `--version` median | **26.34 ms**（mean 26.58） | release，hyperfine 20 runs，**时点 2026-09-22** | `docs/reports/cli-startup-baseline.json`（在盘，4175 B） |
| CLI `help` median | **26.06 ms** | 同上 | 同上 |
| CLI `help <cmd>` median | **28.88 ms** | 同上 | 同上 |
| 阈值 | median < **100 ms** | Windows 基线 25.9~27.9ms 的 ~3.6× 余量 | `bench_check.yml:650-653` 注释 + `:669` 步骤 |
| `--selftest` | **PASS，all 71 assertions held**，rc=0 | 本轮实跑 | `tmp/wo2026-b8-staticgates.log` |
| `invariants` | **PASS，all invariants hold**，rc=0 | 本轮实跑 | 同上 |
| I11（上批 W0 新增） | **exec=10 excluded=1** | 门自报 | 同上 |
| I9 / I10 | **85 keys，PASS**（heredoc 键集 == 本表键集） | 门自报 | 同上 |
| 轮询普查 | 生产 **30** / 测试 **99** | advisory 恒 exit 0 | `idle_wakeup_audit.py` |
| **govd 自测** | **FAILED (failures=3)，Ran 79 tests** | 本轮实跑 | `tmp/wo2026-govd-census.log` |

**I9/I10 的 85 keys 是权威键数**（门自报）；`perf_thresholds.toml` 的 awk 逐节计数会给出更大的数（含 `[ratchet]` 等非阈值节），不可混用。这印证「正对照 = 被检对象自己的自报数」。

### 【本阶段完成度自评】

| 要求 | 完成度 | 缺口 |
|---|---|---|
| 现状地图（模块路径 → 实现模型 → 瓶颈点） | **完成**：六节 A~F，覆盖四环链 9 环节 / 计划层 7 构件 / 动作环 4 构件 / 背压 7 类载体 / 卸载 9 锚点 / 轮询 7 处，全部带 file:line | 无 |
| 完整基线测量方案（工具 / 命令 / 采样数） | **完成**：7 类工具逐一核实可用性（3 个 ❌ 如实标注），B1~B8 命令可原样复制，6 条采样纪律各带本仓教训出处 | 无 |
| baseline 指标表 | **大部完成**：36 项指标有实测值 + 置信区间 + 阈值 + 余量 | **3 项【待测量】**：RSS 双口径（B5，需 release + 长驻进程）、tract 编译时长差、tract release 体积增量（后两者需改 Cargo.toml 做 A/B，属实施批） |
| 三级置信度标注 | **完成**：全表逐行标注 | 无 |

**自评结论**：阶段0 达成「独立可验收」。三项【待测量】均给出了完整测量方法与不可本轮测的原因，符合「可预留待测项但必须明确给出对应测量方法」的要求。

### 【下阶段前置依赖说明】

阶段1 的 ROI 排序依赖阶段0 的以下产出，均已就位：

1. **T1 的阈值取值**依赖 §0.3.1 的混层实测（21.777 ms + 25% 置信区间宽度）与 `perf_thresholds.toml:115` 记载的既有惯例（`new = ceil(median × 2.5)`，且 `/1` 组因负载敏感**不设阈值**）。
2. **T3 的定档**依赖 §0.3.6 的依赖面数据 —— 而该数据显示磁盘面仅 1.7%，故 T3 **不能在本轮定档**，必须标【待测量】并给出 A/B 方案。
3. **「不优化」清单的每一条**都需要 §0.3 的实测余量作为支撑（例：四环链 17×~300× 余量支撑 ADR-277 的 µs 级否决；`serial_ms0` vs `parallel_ms0` 的 −7.1% 支撑「零负载下并行净负」）。
4. **T0 的归属判定**依赖逐 commit 追溯（已完成：`58a7567d` 引入，早于上批基线 `6aed20b4`）。

**阻塞项**：无。阶段1 可立即开始。

---

## 阶段 1：瓶颈精准诊断

ROI = 预期指标提升收益 ÷ 实施成本 ÷ 回归风险。三项因子逐条给出可核验的量化依据，不用形容词。

### 1.1 ROI 排序清单（Top 6）

#### T0 —— govd 治理普查判红（P1 既有缺陷，ROI 最高）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `scripts/govd/tests/test_govd.py:163-166`（断言 140/140/133/7）；`scripts/gate_manifest.toml` 实况 **142** 个 `[[gate]]` / light **135** / heavy **7**；`.github/workflows/ci.yml:1023` 步骤名 `govd pr profile (133 light gates)`；`ci.yml:1004` 注释 `run the same 133 light gates` |
| **影响量化** | 实跑 `py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v` ⇒ **Ran 79 tests → FAILED (failures=3)**，rc=1。三个失败：`test_real_census`（`:163`，`AssertionError: 142 != 140`）、`test_list_pr_profile`（`:443`）、`test_module_entrypoint`（`:457`，找 `selected=133 total=140 skipped=7` 而实况是 `selected=135 total=142 skipped=7`） |
| **根因** | `58a7567d`（`feat(toolchain): baseline snapshot of in-flight tool_runtime + permission tri-state work`）向 `gate_manifest.toml` 加了 2 条 light 门（140→142 / 133→135），但**未同批更新** govd 的硬编码普查断言，也未更新 `ci.yml` 两处 133 字面量。逐 commit 追溯证据：`fe98e304`/`60d8f146`/`acf80d26`/`1356ebf2`/`c3a01eaf` 均为 gates=140 light=133；`58a7567d` 起为 142/135；`git merge-base --is-ancestor 58a7567d 6aed20b4` 返回真 ⇒ **早于上一批开工基线** |
| **为什么历次「全绿」没抓到** | govd 在 CI 是 **deliberately non-blocking shadow**：`ci.yml:1002` 注释「governance-shadow — govd pr profile, deliberately non-blocking (ADR-228 §3.6 / I7)」，job 名 `ci.yml:1010` `govd pr shadow (non-blocking)`。⇒ 红不阻塞合入，且本地发布前检查清单（`agents.md` §7.2 / `.claude/CLAUDE.md` §5）不含 govd 自测 ⇒ 无人手跑 |
| **收益** | ① 治理面自测转绿（79/79）；② 消除「非阻塞 shadow 长期掩盖判红」这一类病理的现存实例；③ 使 `ci.yml` 步骤名与实况一致（步骤名里的 133 是可被 grep 的陈旧计数，属本仓明令禁止的「手列计数」形态） |
| **成本** | **极低**：3 处字面量（`test_govd.py:163,164,165` 的 140→142 与 133→135；`ci.yml:1023` 与 `:1004` 的 133→135）。零逻辑改动 |
| **回归风险** | **极低**：改的是断言的期望值以匹配实况，不改被检物。唯一风险是「把判红洗绿」的观感 —— 缓解：commit message 必须写明门数增长来自 `58a7567d` 而非本轮，并附逐 commit 追溯表 |
| **ROI 判定** | 收益（治理面转绿 + 消除病理实例）÷ 成本（3 个数字）÷ 风险（极低）⇒ **最高** |

#### T1 —— `mixed7r1w` bench 未登记阈值门（验证面补洞）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `crates/gqep-executor/benches/toolplan_layers.rs`（ADR-281 新增的 `mixed7r1w_ms2/{1,8}` 两组）；`scripts/perf_thresholds.toml:119-122`（现有 4 个 `tplan_*` 键）与 `:281-284`（对应 filter）；**`scripts/` 全域对 `mixed7r1w` 与 `tplan_mixed` 零命中**（口径：`grep -rn "mixed7r1w\|tplan_mixed" scripts/`） |
| **影响量化** | ADR-281 的混层写屏障分段加速（本轮实测 **4.50×**：98.082 ms → 21.777 ms）**没有任何 CI 防回退**。若后续改动让混层退回整层串行（c8 ≈ c1 ≈ 98 ms），全部现有门仍会绿：`tplan_declared_*` 与 `tplan_undeclared_*` 覆盖的是 `readonly8_ms2` 与 `undeclared8_ms2` 两组，混层组不在任何阈值键的 filter 里 |
| **根因** | 上一批 W3 落地 ADR-281 时，bench 组与测试都加了，但**未同批登记阈值键**。属「代码面落地、验证面漏接」——与本仓已登记的 I11 病理（声明了契约但无执行路径）同族，只是 I11 覆盖 `perf_redlines.toml` 的 redline/slo，不覆盖 `perf_thresholds.toml` 的 bench 组 |
| **收益** | ADR-281 的核心收益获得机器防回退；同时补齐「新 bench 组必须同批登记阈值」的纪律实例 |
| **成本** | **低**：`perf_thresholds.toml` 加 1~2 个 `[thresholds_ns]` 键 + 对应 filter；`bench_check.yml` heredoc 同批加同名键（否则 I9 判红） |
| **回归风险** | **低但有具体陷阱**：混层 c8 臂置信区间宽达 **25%**（[18.133, 22.688] ms），若阈值取太紧会在负载高的 runner 上假红。缓解见 §2 T1 的阈值取值推导 |
| **ROI 判定** | 收益（已落地的 4.50× 获得防回退）÷ 成本（2 个 toml 键）÷ 风险（低且可推导规避）⇒ **高** |

#### T2 —— `superposition_query` 零生产调用方 + fanout 无上界（结构死臂）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `crates/mcp-mesh/src/quantum/superposition.rs`（495 行：生产段 1-234，`#[cfg(test)]` 段 235-495）；`crates/mcp-mesh/src/mesh.rs:704`（`pub async fn superposition_query` 委托方法）、`:39`（use）、`:7`（模块注释）、`:1330,1340`（两个测试）；`crates/mcp-mesh/src/lib.rs:59,76`（两处 re-export）；`crates/mcp-mesh/src/quantum/mod.rs:8,14`（声明） |
| **影响量化（零调用方）** | `grep -rn "superposition_query\|SuperpositionQuery" crates/ --include=*.rs` 排除 `crates/mcp-mesh/` 后 **零命中** ⇒ 全仓无任何生产或测试消费者在 mcp-mesh 之外引用它 |
| **影响量化（无上界）** | `superposition.rs:187` `let mut set: JoinSet<QueryResult> = JoinSet::new();`，`:191` `for server in servers { set.spawn(...) }` ⇒ **无独立 Semaphore**。且 `max_participants = 32` 的钳制在 `mesh.rs:240`（`if participants.len() > self.config.max_participants`）属 `execute_transaction` 路径，**`execute_superposition_query` 不经过它** ⇒ fanout 并发度 = `query.fanout_servers.len()`，由调用方任意指定，无机器上界 |
| **根因** | ADR-253 D2 把 `execute_superposition_query` 从「进程内伪造 payload」改为「经注入的 `ParticipantClient` 真实 fanout」，**修好了传输语义但没接消费者**。ADR-253 §2 的取舍表里，选项 C「保留本地 mock、另开真实查询 API」被否决的理由原文是：**「trait `query` 早已为它而生，不接线等于能力空转」** —— 而现状是接了线**仍然**没有消费者，能力**依然空转**。ADR-253 自己用的判据现在指向相反的处置 |
| **收益** | ① 消除 ~520 行无消费者的公开 API（生产 234 + 测试 260 + mesh.rs 委托与测试 ~60 行）；② 消除 JoinSet fanout 无上界的潜在资源风险；③ 台账诚实（对齐 ADR-160 可达性棘轮与 ADR-283 删 `start_background_probe` 的先例） |
| **成本** | **低**：三选一裁决 + 删码或登记 |
| **回归风险** | **中**：`lib.rs:59,76` 的 re-export 是公开 API。但 **mcp-mesh 属 L10 Interface 层，不是 L0-A/L0-B** ⇒ 不触发「契约变更清单」硬约束（该清单只保护 L0 层间接口）。ADR-253 §4 亦自陈 mcp-mesh 的签名变更属「workspace 内部 API」（该 ADR 的 `execute_transaction`/trait `prepare` 变更涉及 ~25 处调用点，均随 ADR 落地）。另需注意 G-101/102（`check_dead_contract_apis.py`）可能对删码有台账联动 |
| **ROI 判定** | 收益（消除空转 + 消除无上界 fanout）÷ 成本（删码，机械）÷ 风险（中，非 L0 契约面）⇒ **高** |

#### T3 —— tract-onnx 依赖 ROI 裁决（**待测量定档**）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `Cargo.toml:156` `tract-onnx = "0.22"`（workspace 级，注释「纯 Rust ONNX 推理引擎…ort 仅支持 MSVC」）；`crates/nmc-encoder/Cargo.toml [dependencies]` 直接生产依赖、**非 optional、无 feature 门控**；`Cargo.lock:6265-6394` tract 族 7 包 |
| **运行期零加载证据** | `crates/nmc-encoder/src/config.rs:246-253` Default 的 `model_dir: String::new()`，注释「model_dir 为空字符串,表示未启用 ONNX 推理」；`:207-208` `validate()` 仅当 `model_dir` 非空才校验模型文件；`config.rs:94-102` 期望文件名 `clip-vit-b32.onnx` / `videomae-base.onnx` / `whisper-encoder.onnx`，而**仓库内零 `.onnx` 文件**（口径：`find` 全仓排除 `target/`、`.toolchain/`）；`crates/nmc-encoder/src/perceptors/text.rs:97-100` 加载失败即「降级到字节频率嵌入」 |
| **生产构造点** | 加载器在 `crates/nmc-encoder/src/perceptors/onnx_backend.rs:140-153`（`tract_onnx::onnx().model_for_path(&path)...into_runnable()`）；`OnnxBackend`/`ModelType` 经 `crates/nmc-encoder/src/lib.rs:68` 与 `crates/nmc-encoder/src/perceptors/mod.rs:26,36` 公开导出。但生产侧唯一构造的 perceptor 是 `crates/chimera-mas/src/knowledge/wiki_retrieval.rs:85` 的 **`TextPerceptor`**（`:54` 字段类型 `Option<Arc<nmc_encoder::TextPerceptor>>`）⇒ **`OnnxBackend` 无生产构造点** |
| **量化（已测部分）** | 7 包 / debug 产物 **347 MB** / 占 `target/debug/deps`（20769 MB）的 **1.7%** / 唯一反向依赖 nmc-encoder。逐包：tract-core 171 MB、onnx 48、nnef 38、hir 33、data 29、linalg 20、onnx_opl 11 |
| **量化（缺口）** | **编译时长差**与 **release 二进制体积增量**均为【待测量】—— 两者都需改 `Cargo.toml`（注释掉 tract-onnx）做 A/B，属实施批动作，本轮按裁定 4 不做 |
| **根因** | tract-onnx 是为「本地多模态嵌入」引入的能力面，但模型权重不随仓分发、`model_dir` 默认空、无生产构造点 ⇒ 编译面长期在盘而运行期永不执行。这与 `nmc-encoder/parallel.rs::perceive_parallel` 在 G-87 台账里标 **DEAD** 是同一事实的两个侧面 |
| **ROI 判定** | **不可现在排序**。磁盘面 1.7% 已实测**不足以**单独支撑「重量级依赖」论断；真正的判据（编译时长 / release 体积）无数据。⇒ 按你的硬约束「禁止引入新的重量级依赖，除非能提供体积、编译时间、收益的量化对比」，**同一把尺必须反过来用于已在盘的依赖**：没有这三项数据，既不能判「删」也不能判「留」。T3 的第一实施步骤就是补齐测量（见 §2 T3） |

#### T4 —— `perf_thresholds.toml` 头注陈旧（文档订正）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `scripts/perf_thresholds.toml:5-15` 头注原文声称：「bench_check.yml 仍自带 52 键 THRESHOLDS_NS heredoc…check_perf_redlines.py 的 thresholds 模式从未被任何 workflow 调用…⇒ 本文件当前是「第二份表」而非唯一真值源，R1-T6 的切流尚未发生…在那之前，两表差集会静默存在（R2 M44 实例：本表有 hnsw_tiered 而 heredoc 无 ⇒ 该阈值在 CI 永不被断言）」 |
| **实况（已验证，与头注相反）** | `scripts/check_perf_redlines.py:359-376` 已实现 **I9/I10**：I9 = 「heredoc THRESHOLDS_NS 键集 == `[[thresholds_ns]]` 键集（**R2 M44 的根修**）」，I10 = 同构的 PATH_KEYWORDS 对拍（注释自陈「**N-3 幽灵门残留（hnsw_tiered 词条 heredoc 漏接）的根修**」）。本轮实跑 `invariants` ⇒ **I9 PASS (85 keys; dict found=True)** / **I10 PASS (85 keys)**。`:648` 另有 I9 的防伪绿前提（真实 workflow 里确实有 THRESHOLDS_NS dict 可查，否则不猜、直接判不可判定） |
| **影响量化** | 头注描述的缺陷**已被机器门关闭**，但头注没更新 ⇒ 任何按头注行事的读者会得出「两表差集会静默存在」的错误结论，并可能重复去做已经做完的对拍门。这是本仓已登记的病理形态：**充当权威源的事实基座自己也会漂，其「待修冲突」可能已被现实静默闭合** |
| **根因** | I9/I10 落地时只改了 py 核与门登记，未回头订正 toml 头注的散文 |
| **收益** | 消除一份会误导实施者的陈旧权威声明；使头注与 I9/I10 实况一致 |
| **成本** | **极低**：纯注释订正，零逻辑改动 |
| **回归风险** | **零**（注释不参与任何门的解析）。唯一注意点：改注释不得触碰 `[[thresholds_ns]]` / `[[path_keywords]]` 的键集，否则 I1/I9/I10 会红 |
| **ROI 判定** | 收益（消除误导性权威声明）÷ 成本（几行注释）÷ 风险（零）⇒ **中**（收益是文档面而非性能面，故不列高） |

#### T5 —— mcp-mesh WAL 在 instant 路径 3.253 ms 里的占比未知（归因测量）

| 维度 | 内容 |
|---|---|
| **证据锚点** | `crates/mcp-mesh/src/quantum/wal.rs:120-152`（`append`：`sensitive::scan` 全载荷 + `spawn_blocking` + `open`/`write`/`sync_all`）；`crates/mcp-mesh/src/config.rs` 的 `durable` 默认 **true**；本轮 B4 实测 `mcp_mesh_transaction_instant/2pc/1` = **3.2532 ms** |
| **影响量化** | 一次生产 `mcp call` = prepare + commit = **2 次 WAL append**，即 2 次 `open + write + sync_all` + 2 次全载荷敏感扫描。instant 臂已消除模拟延迟（对照臂 27.553 ms ⇒ 模拟延迟占 24.3 ms），**剩余 3.253 ms 的构成未拆分** |
| **根因** | 上一批 W2 只做了「删模拟延迟」的 A/B，没有对剩余 3.253 ms 做构成归因。`fsync` 在 Windows 上的绝对开销未测 |
| **为什么这不是重复提案** | ADR-275 D3 否决的是 **`FileWalCriticalSink::open` 改 `spawn_blocking`**（理由：该路径 env 门控 `CHIMERA_CRITICAL_WAL_PATH` 未设则完全不执行、一次 40 字节文件打开、改异步需把 `composition::build` 变 async）——**那是 seccore 侧的 critical WAL，不是 mcp-mesh 的 `WalStore`**。上一批计划另否决了「WAL group commit」（单事务无 batch 可合、`recover_from_wal` 零调用）与「`durable=false`」（会一并撤掉写入前的敏感载荷扫描，是安全回归）。**「WAL 文件句柄复用」（保持 `File` 打开而非每次 `open`）既不是 group commit 也不是关 durable，未被任何 ADR 覆盖** |
| **收益** | 若 fsync/open 占 3.253 ms 的主导，则句柄复用或写入路径重构有明确的 ms 级收益（生产 `mcp call` 端到端）；若占比小，则正式登记「不值得优化」并关闭这个悬置问题 |
| **成本** | **低（测量部分）**：在 `crates/mcp-mesh/benches/mesh_benchmark.rs` 加一个 `durable=false` 对照臂即可精确归因（`MeshConfig` 已有该字段，零生产码改动）。**实施部分成本取决于归因结果** |
| **回归风险** | 测量阶段**零**（bench 臂不改生产码）。实施阶段视方案而定：句柄复用需保证崩溃语义不变（`sync_all` 的持久性保证不能被削弱），且不得撤掉 `sensitive::scan` |
| **ROI 判定** | **待测量定档**。测量成本极低而潜在收益是 ms 级（对 3.253 ms 基数而言），⇒ 先测后判是正确顺序 |

### 1.2 「不优化」清单（每项附实测支撑 —— 这是任务书允许的合法结论）

| # | 项 | 实测支撑数据 | 权威裁决指针 |
|---|---|---|---|
| 1 | **GPU 卸载**（wgpu / candle / burn / ort） | `Cargo.lock` 对 `wgpu/candle/burn/ort/llama/gguf/cuda/vulkan/naga/ash/d3d12` 全部 `^name =` 锚定**零命中**；`crates/**/*.rs` 对 `wgpu\|cuda\|vulkan\|metal\|naga\|ash\|d3d12\|cudarc` **零命中**（3 处 `ash` 命中系 `mask_hash` 假阳性）；G-A/G-B/G-C 三门**全不满足且方向性证据在远离门槛**（G-A：CPU 计算面 SLO 全绿且热路径刚被优化 30~50%；G-B：MemoryGraph 未接线故无生产常驻批量负载；G-C：`DispatchPlan` 仍是 `{Inline, Rayon, Async}` 三态无 Gpu 分支，`nexus-core/src/compute/dispatch.rs:23-30`） | ADR-195 D1/D3；G-86（`scripts/check_accel_exclusion.py:72-84` `EXACT_FORBIDDEN`，`gate_manifest.toml:964-971` expect=0）；`docs/reports/gpu-unseal-readiness-2026-11.md:16-46,70-72`「维持 C-15 三重关闭」；ADR-195:109「本机有 RTX 4060 Laptop…**有卡不是证据**」 |
| 2 | **本地 LLM 推理**（批处理 / KV cache / 显存驻留） | **该子系统在本仓不存在**：无任何生成式本地模型依赖与权重文件；LLM 生成全部经远端 HTTP/SSE。`mca-gateway` 的 `mca` feature **默认 off**（`chimera-cli/Cargo.toml:17,71`），`main.rs:311-319` 的 `init_mca_gateway` 是 M4 装配态、不做任何主动接线或网络请求 | `mca-gateway/src/transport.rs:213-219`（`reqwest::Client::builder()`）；`adapters.rs:200,250,545`；`sse.rs:1-18`（三方言流式归一）；`spec_loader.rs:330`（`base_url = "https://open.bigmodel.cn/api/paas/v4"` + `api_key_env = "ZHIPU_API_KEY"`）；ADR-177（mca-gateway 长期 GATED） |
| 3 | **edition 2024 迁移 / MSRV 降到 1.95** | CI **16 处**钉定 `dtolnay/rust-toolchain@1.97.1`（ci.yml:68,775,843,883,923,986；bench_check.yml:84,660；coverage.yml:56；perf_redline.yml:75；release.yml:86,202,282；slo.yml:82,184；stress.yml:56）。MSRV 1.95 是**降低**地板：CI 仍用 1.97.1 编译，声明 1.95 不会产生任何 1.95 上的验证。破坏面已量化：全仓 **76 处** `set_var`/`remove_var`，其中**生产段 16 处**（`chimera-cli/src/commands/tui.rs:372` + `chimera-tui/src/app/render.rs:757-862` 共 15 处），而 **46/46 crate 声明 `#![forbid(unsafe_code)]`** ⇒ edition 2024 会让这 16 处全部需要 unsafe 包裹，与「默认零 unsafe」基线硬冲突。涉及 12 个 crate | `Cargo.toml:94`（edition 2021）、`:98`（rust-version 1.97）与 `:95-97` 注释原文；commit `0e4dfc31`（工程工单 C1，2026-11-08）「edition 维持 2021——2024 迁移为独立后续项目，不入性能工单」；本文 §5 契约变更清单 |
| 4 | **ADR-277 四项 µs 级固定开销**（`compute_layers` 索引化 / `inputs_of` 预建邻接表 / 预算增量计数器 / Semaphore 跨层复用） | 前提 `MAX_STEPS = 64` 未变（`--plan-max-steps` 是 min 语义只收紧，`exec_plan.rs:283-294`）。本轮**新增独立实证**：`tool_batch/serial_ms0/64` = 46.841 µs vs `parallel_ms0_bound8/64` = 50.166 µs ⇒ 零负载下并行**慢 7.1%**，spawn+Semaphore 固定开销超过并发收益；且阈值门本身已承认此事实（`tbatch_parallel_zero` 2000 µs = `tbatch_serial_zero` 1000 µs 的 **2 倍**） | ADR-277 D1（**无重开条款**，`:108-111`）；ADR-279「维持，不重提」；阈值证据 `perf_thresholds.toml:102-103` 区段与 `:272-273` |
| 5 | **装配期并行** | 实测四段合计 **453.87 µs** ≪ 重开门槛 5 ms（单段 ≪ 2 ms）；原 ADR 记录 1.31 ms 合计、3 次 spawn ≈0.88 ms、净收益 ≈0.1 ms | ADR-275 D2；重开门槛原文 `ADR-275:70-71`「四段合计 ≥5ms 且单段 ≥2ms，否则不再重提」；ADR-279 复评维持 |
| 6 | **同步 WAL open 改 `spawn_blocking`**（seccore critical WAL） | 该路径 **env 门控**（`CHIMERA_CRITICAL_WAL_PATH` 未设则完全不执行）；一次 **40 字节**文件打开；改异步需把 `composition::build` 变 async，波及全部调用方含测试 | ADR-275 D3（原文：「成本 >> 收益」）。**注意**：此项**不覆盖** mcp-mesh 的 `WalStore`，后者是 T5 |
| 7 | **provider 去重 O(n²) 改 `HashSet`** | N = `model_router.providers` 长度（默认 **≤8**，且仅 `cross_vendor_debate=true` 时才走）；n=8 时 28 次字符串比较 vs 一次 HashSet 分配 + 哈希 ⇒ 无可测差异 | ADR-275 D3 |
| 8 | **借用脊柱**（链内全借用化，省 2 次分配） | 重开前置「ADR-260（R1 判决体优化）落地后分配占比显著上移」**已满足并已重测**，结果**方向反转**：borrowed 反而慢 **1.034× / 1.040×**。本轮 B7 补充证据：单派发分配实测 **6**（上界 7），批量 **68**（贴顶）⇒ 分配面已被棘轮压到极限，省 2 次分配不会改变任何已登记读数 | ADR-249（**Rejected**，重开条件原文 `ADR-249:98`）；ADR-279 复评「维持（重测值 + 触发条件已满足但未跨门）」 |
| 9 | **mimalloc 切默认分配器** | 触发条件①「R1/R2 后分配占比重新成为主导（预算棘轮读数显著上升）」—— 本轮实测棘轮 **6 / 68**，与登记上界 7 / 68 相比**未上升**（单派发还有 1 次余量）；触发条件②「出现高频大分配载荷」—— 无证据 | ADR-263 R4-2（Partially Accepted，mimalloc 未过门保持非默认）；ADR-263:45 触发条件原文；ADR-279 §3 确认未触发 |
| 10 | **CPU 卸载其余 DEAD 锚点接线**（`decay_batch_parallel` / `score_parallel`(faae) / `perceive_parallel`(nmc) / `compute_five_masks_parallel`(osa) / `score_batch`(gsoe)） | 5 个 DEAD 锚点**无生产调用方**；2 个 CRATE_API_LIVE 与 1 个 PUB_API_DIAGNOSTIC_ONLY 的生产真实 N **未达对应 `TaskKind` 阈值**（阈值 100~10000，见 §0.1 E）。接线三要件（错误姿态评审 / G-88 让出保护 / 逐位等价）无一满足 | ADR-282（逐条登记否决，「三前置无一满足」）；G-87 台账 `scripts/compute_offload_reachability_freeze.txt:23-31` |
| 11 | **`buffer_unordered` / `.buffered(` 替代 `FuturesUnordered`** | 生产**零使用**；码内已有 3 处「WHY 不用」注释。技术理由：未拉取的源项 never polled，与批量派发的「惰性装载 + 在飞装箱 ≤ bound」语义冲突 | ADR-202；`crates/gqep-executor/src/tool_runtime/batch.rs:19-21`、`crates/gqep-executor/src/gatherer.rs:219`、`crates/gqep-executor/src/config.rs:58` |
| 12 | **`DashMap` 替代沙箱门 `RwLock`** | 本轮**独立复测同向**：`dispatch_sandbox_concurrent/rwlock_read_c4` = **186.91 µs** vs `dashmap_read_c4` = **201.99 µs** ⇒ RwLock 快 **7.5%**；`rwlock_read_c8` = 326.10 µs。原裁决以 sample_size=30 于 2026-11-06 测得同向结论 | ADR-241 D5（`:68-76`，「已评估、不采纳（关键裁决）」，对照设计为 `tool_dispatch_layers.rs` 内联复刻两种实现）；ADR-239 D-8（每个并行化改造点须有同轮对照基准） |
| 13 | **轮询点计数棘轮门** | 生产轮询点 **30**，其中周期 `loop+sleep/interval` 仅 **7 处**，量级分布 **2ms×1 / 10s×1 / 60s×4 / 300s×1** ⇒ 计数把 60s 心跳与 2ms flush 等同视之，是**坏代理**。Ambient Mode 与 ShardedBus 已纯事件驱动（无定时器） | ADR-264（自陈 dev-only advisory，`idle_wakeup_audit.py` 恒 exit 0）；`ambient_mode.rs:9-10,27,210`；`shard.rs:61`（ADR-129 禁自旋） |
| 14 | **`repo-wiki::VectorIndex` 无界增长**（初判为缺陷，**复测降级为非缺陷**） | 生产 upsert 走 `store.rs:876` → `crates/repo-wiki/src/vector/hnsw_store.rs:417` 的 **HNSW**，不走 `VectorIndex`；`VectorIndex` 的调用方**只有 bench 与测试**：`chimera-mas/benches/mas_benchmark.rs:479`、`quest-engine/tests/e2e.rs:125,312,485`、`repo-wiki/benches/e2e_parallel.rs:54,68`；且设计口径已登记「仅支持 ≤1000 entry 的内存 KNN」（`nexus-contracts/src/vector_math/vector.rs:20,113`）。**口径差异非冲突**：内存审计的范围是「**常驻缓存**」11 项，不含向量**数据面**（= f(索引条目数)），两者不是同一论域 | `memory-bound-audit-2026-11-08.md` §1（11 项缓存清单 + 「零无界增长点」结论，`:22`）；`repo-wiki/src/lib.rs:101,127-131,151-155`（`VectorIndex` 与 `HnswStore` 并存导出） |
| 15 | **`crates/repo-wiki/src/store.rs:275` unbounded 写通道加背压**（初判为靶子，**复测降级为非缺陷**） | 该通道是**请求-响应**形态而非流式生产：`store.rs:366-375` 为 `let (tx, rx) = oneshot::channel();` → `self.write_tx.send(WriteOp::Insert(entry, tx))?;` → `let is_new = rx.await??;`，`InsertBatch` 同型（`:412-416`）⇒ **每个发送方 send 后立即 await 响应**，在飞 op 数 = 并发调用方数，通道深度天然受限于调用方并发度，不是 f(生产速率)。消费侧 `run_writer`（`:1231-1232`）是 `while let Ok(op) = rx.recv()` 单线程循环。**改 bounded 反而有害**：若 bound < 并发调用方数，所有调用方都在等 respond 而通道满 ⇒ 死锁 | 生产段唯一 unbounded 通道（tokio `unbounded_channel` 生产**零使用**，仅 `crates/mcp-mesh/tests/commit_concurrency.rs:39`）；`WriteOp` 七变体均携带 `respond` oneshot（`:1234-1259`） |
| 16 | **`mca-gateway` 三个无驱逐常量的 DashMap** | `mca` feature **默认 off** ⇒ 默认构建**根本不编译** mca-gateway（`chimera-cli/Cargo.toml:17` `mca = ["dep:mca-gateway"]`、`:71` optional）。三处细节：`coalescing.rs:91` 的 `inflight` 生命周期绑定请求且完成即 `remove`（`:127`），键空间 = 在飞请求数；`token_estimate.rs:90` 的 `factors` 键 = `(channel, model)` 对，受 spec 约束；`health.rs:128` 的 `channels` 键 = route_key。均无驱逐常量，但**默认不编译 ⇒ ROI 极低** | ADR-177（mca-gateway 长期 GATED，经 chimera-cli optional 边 + `--features mca` 装配）；`scripts/gated_wiring_baseline.txt` |
| 17 | **冗余 clone 削减 / RSS 峰值优化**（任务书优先级 2 明列项，**定档为不值得优化**） | 按三步纪律测量后定档：① **剔除测试区重算**（口径：文件最后一个顶层 `#[cfg(test)]` 或 `mod tests` 之前的行）—— 热路径子集 11 个文件合计 **28 clone / 3858 生产行 = 7.3 per kloc**；其中四环链核心文件 `tool_runtime/chain.rs`(313 行)、`inproc.rs`(44 行)、`batch.rs`(343 行)、`exec_tool_executor.rs`(101 行) 生产段 clone 数**均为 0**，`execpolicy.rs` 1 处 / 521 行 = 1.9 per kloc，`mcp_tool_executor.rs` 1 处 / 266 行 = 3.8，`toolplan_runner.rs` 5 处 / 735 行 = 6.8。② **密度最高者不在热路径**：`chimera-tui/src/data/sync.rs` 62 clone / 1198 行 = **51.8 per kloc**，但 TUI 数据同步面**不在任何已登记 bench 的覆盖路径上**（`perf_thresholds.toml` 无对应键）⇒ 改它无可测收益；`mca-gateway/adapters.rs` 18.9 per kloc 属 GATED 默认不编译。③ **RSS 归因缺基线**：`CountingAlloc` 不测 RSS，本轮 B5 未跑 ⇒ 无法证明「降 clone 能降 RSS」。**反面实证**：分配棘轮批量档已**贴顶 68**（零余量），说明热路径分配面已压到极限 | `dispatch_alloc_budget.rs:77,80,97,114`；`memory-bound-audit-2026-11-08.md:49`（CountingAlloc 不测 RSS）；ADR-263 R4-2 教训「静态分配 ≠ 墙钟收益」 |
| 18 | **action loop 许可耗尽的串行降级改造** | 既有测试 `bounded_loop_caps_inflight_and_delivers_exactly_once` 证明：N=10 个动作在 bound=2 下**全部处理完成**、每 request_id **恰处理一次**、在飞峰值 **≤ bound+1**（`action_orchestrator.rs` 测试段）。⇒ 降级路径不丢事件、不越界，无缺陷可修 | ADR-251 5.1（已登记取舍）；`action_orchestrator.rs:408-448`（降级 `:431-436`） |
| 19 | **SubAgent `block_on` 异步化** | 无 ≥5% 收益证据；本轮未产生新数据 | ADR-257 D3（已登记独立工单） |
| 20 | **windows-gnu doctest 工件态家族**（E0460/E0462/E0463/E0786） | 计数随工件态在 **20↔21** 间波动，**零断言失败**（错误全为链接/元数据类）；上一批为修 E0786 做 `cargo clean` 释放 67.4 GB 后，同一份**未改动**的 chimera-mas 代码失败数从基线 15 变为 20/21 ⇒ 属工件态重新分布而非代码回归 | 已登记独立工单；**Linux CI 为权威口径**；上一批报告 §9 已如实披露「不声称严格优于基线的 15」 |
| 21 | **`#[ignore]` 重测试纳入常规门** | 全仓 `#[ignore]` 属性行 **131**（src=2 / tests=109 / benches=20）；登记表 `scripts/ignored_test_inventory_freeze.txt` **79 行** = slo-blocking 13 + slo-daily 54 + manual-only 12，已由 `slo.yml --run-ignored` 消费 ⇒ **已在门内**，无需新增 | `slo.yml`；`scripts/ignored_test_inventory_freeze.txt`；src 侧 2 处 `#[ignore]` 的理由是「静默态单跑让出判据」（`faae-router/src/router.rs:957`、`osa-coordinator/src/coordinator.rs:1207`） |

### 1.3 复测纠正登记（初判 → 终态）

本节是本轮纪律的核心证据：**子代理结论与主会话初判都是待证假设**。以下 8 条初判被复测推翻或修正，全部如实登记，包括我自己的错误归属判断。

| # | 初判 | 复测终态 | 纠正依据 |
|---|---|---|---|
| 1 | `perf_thresholds.toml` ↔ `bench_check.yml` **存在双源漂移**（子代理报告「头注自证 52 键 heredoc，两表差集静默存在」），列为 T4 靶子 | **降级为「头注陈旧」**：I9/I10 已实现键集对拍并实跑 PASS（85 keys），头注描述的缺陷已被根修 | `check_perf_redlines.py:359-376` 实现 + 注释自陈「R2 M44 的根修」「hnsw_tiered 幽灵门残留的根修」；本轮 `invariants` 实跑 I9/I10 PASS |
| 2 | `repo-wiki::VectorIndex` 的 `RwLock<HashMap>` **无界增长**，与内存审计「零无界增长点」**冲突** | **降级为非缺陷（口径差异）**：生产走 HNSW，`VectorIndex` 只有 bench/测试消费者；审计范围是「常驻缓存」不含向量数据面 | `store.rs:876` → `hnsw_store.rs:417`；调用方 grep 结果全为 bench/tests；`nexus-contracts/src/vector_math/vector.rs:20` 已登记 ≤1000 entry 口径 |
| 3 | `crates/repo-wiki/src/store.rs:275` unbounded 通道是**生产唯一无背压风险点**，列为 T5 靶子 | **降级为非缺陷**：请求-响应形态（send 后立即 await oneshot），在飞数 = 并发调用方数；改 bounded 会引入死锁 | `store.rs:366-375`、`:412-416`、`:1231-1259` |
| 4 | govd 普查判红是**上一批 W0 造成的**（我自己在会话中的初判，已对外表述） | **纠正归属**：`58a7567d` 引入（142/135），且 `git merge-base --is-ancestor 58a7567d 6aed20b4` 为真 ⇒ **早于上一批开工基线**，与上批和本批均无关 | 逐 commit 数门：`fe98e304`~`c3a01eaf` 均 140/133；`58a7567d` 起 142/135；`6aed20b4`（上批基线）已 142 |
| 5 | `thresholds_ns` 有 **100 个键**（子代理 awk 逐节计数） | **纠正为 85 键**：以门自报为准（awk 口径混入了 `[ratchet]` 等非阈值节） | I9 自报 `85 keys; dict found=True` |
| 6 | tract-onnx 是**重量级依赖**，删除收益显著 | **论据削弱**：debug 产物 347 MB 仅占 `target/debug/deps` 的 **1.7%**；真正判据（编译时长 / release 体积）无数据 ⇒ T3 改为**待测量定档**，不能凭磁盘面裁决 | `du -cm target/debug/deps/libtract*` = 347 MB；`du -sm target/debug/deps` = 20769 MB |
| 7 | ADR-281 混层加速 **5.82×**（上批报告值） | **本轮实测 4.50×**（98.082 / 21.777 ms）；三次读数 4.50 / 4.59(CI 保守) / 5.82 波动，根因是 c8 臂置信区间宽 **25%** ⇒ 仍过 ≥4× 门但余量仅 12.5%，**T1 阈值不得照抄上批数字** | B2 实跑 `tmp/wo2026-b234-bench.log` |
| 8 | MCP 2PC 收益 **−78.7%**（上批报告值） | **本轮双臂 A/B 为 −88.2%**（27.553 → 3.2532 ms，8.47×）。两者口径不同（上批是 criterion `change` 对 base，本轮是同轮双臂），**不可直接比较**；生产路径权威读数取 instant 臂 3.253 ms | B4 实跑，同上日志 |

**方法论结论**：8 条初判里 **5 条被推翻或降级**（#1/#2/#3/#4/#6），其中 3 条来自子代理、1 条来自我自己的归属判断、1 条来自上批报告的口径混用。若照初判直接施工，会：改错 `VectorIndex` 的公开 API 语义、给请求-响应通道加背压从而引入死锁风险、把已有对拍门重做一遍、以及把 govd 判红错记在自己账上导致修复方向错误。**这就是「先盘点、后诊断、再动手」的实际价值。**

### 【本阶段完成度自评】

| 要求 | 完成度 | 缺口 |
|---|---|---|
| ROI 从高到低排序的瓶颈清单 | **完成**：T0~T5 六项，每项含证据锚点 / 影响量化 / 根因 / 收益 / 成本 / 回归风险 / ROI 判定七栏 | **T3 与 T5 无法给出最终 ROI 数值** —— T3 缺编译时长与 release 体积（需改 Cargo.toml），T5 缺 WAL 构成归因（需 bench 加对照臂）。两者均已给出完整测量方案并标【待测量】，且测量本身被列为实施第一步 |
| 每项含证据锚点 | **完成**：全部 file:line / 命令 / 门号锚点，无裸断言 | 无 |
| 每项含影响量化数据 | **完成**：T0 三个失败 + 142/135 vs 140/133；T1 4.50× 无门；T2 零命中 + 无上界；T3 347MB/1.7%/7 包；T4 I9 85 keys PASS；T5 3.253 ms 构成未拆分 | T3/T5 的量化不完整（见上） |
| 每项含根因分析 | **完成**：六项各有根因，且 T2 的根因引用了 ADR-253 自己的判据原文（「不接线等于能力空转」）反向指向处置 | 无 |
| 明确标注所有「不优化」项并给完整支撑理由 | **完成**：**21 项**，每项附实测数据 + 权威裁决指针（ADR 号 + 门号 + file:line） | 无 |
| 任务书优先级 2 的两个明列子项（冗余 clone / RSS 峰值） | **完成**：第 17 项按三步纪律定档为「不值得优化」，附热路径 7.3 per kloc、四环核心 0 clone、批量棘轮贴顶 68 三组实测 | RSS 峰值本身仍【待测量】（B5 未跑），已在第 17 项与 §0.3.5 如实标注 |

**自评结论**：阶段1 达成「独立可验收」。**「未提及」不是合法终态**这条纪律对任务书明列的每个子项都兑现了：优先级 1 → T0/T1/T2 + 不优化 4/10/11/12/18；优先级 2 → T3/T5 + 不优化 8/9/14/16/17；优先级 3 → T3 + 不优化 1/2；优先级 4 → 不优化 13；优先级 5 → §0.1 契约门盘点 + §5 契约变更清单。

### 【下阶段前置依赖说明】

阶段2 的逐项工单依赖以下阶段1 产出，均已就位：

1. **T0 工单**依赖 §1.1 T0 的三处锚点与逐 commit 归属追溯（已完成）。
2. **T1 工单的阈值取值**依赖 §0.3.1 的混层实测 + `perf_thresholds.toml:115` 记载的既有惯例（`new = ceil(median × 2.5)`；`/1` 组因负载敏感不设阈值）+ §1.3 第 7 条的波动披露。
3. **T2 工单的三选一**依赖 §1.1 T2 的删除面清单（生产 234 行 + 测试 260 行 + mesh.rs 委托与测试 ~60 行 + 4 处声明/re-export）与 ADR-253 D2 的原文引用。
4. **T3/T5 工单的第一实施步骤必须是测量**，依赖 §1.1 各自的测量方案（T3：改 Cargo.toml 做 A/B；T5：bench 加 `durable=false` 对照臂）。
5. **每份工单的「接口 diff 校验」**依赖 §0.1 盘点的 6 台契约门命令（G-105/107/115/116/125/126/135，全部 `expect=0`，`{python}` 占位符在本机替换为 `py -3`）。

**阻塞项**：无。阶段2 可立即开始。

---

## 阶段 2：可落地重构方案（逐项标准工单）

每份工单固定七段。所有工单共用的**验证基座**（各工单第 4 段引用，不重复展开）：

```bash
# VB-1 类型 / lint / 格式（格式逐包聚合，规避 Windows 长路径 os error 206）
cargo check --workspace --all-targets --jobs 2
cargo clippy --workspace --all-targets --jobs 2 -- -D warnings
cargo fmt -p <改动 crate> -- --check

# VB-2 全量测试矩阵（必须 --no-fail-fast，否则第一个红会静默吞掉后续 target）
cargo test --workspace --no-fail-fast --jobs 2
# 磁盘前置：debug 全量矩阵实测可达 ~109 GB；D 盘余量 < 40 GB 时改用定向回归
cargo test -p <改动 crate> --no-fail-fast --jobs 2

# VB-3 接口 diff 校验（6 台 L0 契约门，全部 expect=0；{python} 在本机替换为 py -3）
py -3 scripts/check_l0_pubfn_ratchet.py            # G-105 L0 pub fn 只减棘轮
py -3 scripts/check_l0_root_surface.py             # G-107 L0 根 re-export 台账
py -3 scripts/check_l0_extensibility_class.py      # G-115 可扩展性分级（227 类型）
py -3 scripts/check_l0_sublayer.py                 # G-125 L0-A/L0-B 层向
py -3 scripts/check_pub_api_leak.py                # G-135 L0-B 签名泄漏（408 pub fn）
py -3 scripts/check_dead_contract_apis.py          # G-101 死契约 API

# VB-4 性能门与普查
py -3 scripts/check_perf_redlines.py --selftest    # 期望 71 assertions PASS
py -3 scripts/check_perf_redlines.py invariants    # 期望 rc=0，含 I9/I10/I11
bash scripts/check_perf_redlines.sh --static-only  # 期望 VERDICT: PASS
py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py"   # T0 后期望 79/79 OK

# VB-5 依赖与安全（改了 Cargo.toml/Cargo.lock 时必跑）
bash scripts/check_dependency_rules.sh             # G-08
py -3 scripts/check_accel_exclusion.py             # G-86 GPU 禁列
cargo audit --deny unmaintained --deny unsound --ignore RUSTSEC-2026-0190 \
  --ignore RUSTSEC-2026-0002 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141 \
  --ignore RUSTSEC-2025-0119 --ignore RUSTSEC-2026-0253
```

> **CI workflow 改动授权提示**：本仓有长期纪律「自动模式禁止直接改 `.github/workflows`，先贴补丁征求授权」。T0（`ci.yml` 两处字面量）与 T1（`bench_check.yml` heredoc 加键）都触碰 workflow 文件 ⇒ **实施前必须单独取得授权**，不得随代码批一起改。上一批的 workflow 授权范围仅限该批（W0 的 `perf_redline.yml`），不自动延续。

---

### 工单 T0 —— govd 治理普查判红修复（P1，ROI 最高）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| `test_govd.py` 结果 | Ran 79 tests → **FAILED (failures=3)**，rc=1 | **OK (79 tests)**，rc=0 | `py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v` |
| 普查断言与实况一致性 | 断言 140/140/133/7，实况 142/142/135/7 | 逐值相等 | 同上（`test_real_census`） |
| govd 汇总行 | 断言找 `selected=133 total=140 skipped=7`，实况 `selected=135 total=142 skipped=7` | 断言字符串 == 实况字符串 | 同上（`test_list_pr_profile`、`test_module_entrypoint`） |
| `ci.yml` 步骤名与实况一致 | 步骤名写 `133 light gates`，实况 135 | 步骤名写 135 | `grep -n "133 light gates" .github/workflows/ci.yml` 零命中 |

**2. 变更范围**（精确到行）

| 文件 | 位置 | 改动 |
|---|---|---|
| `scripts/govd/tests/test_govd.py` | `:163` | `assertEqual(len(tasks), 140)` → `142` |
| 同上 | `:164` | `assertEqual(len(set(ids)), 140)` → `142` |
| 同上 | `:165` | `assertEqual(sum(... kind == "light"), 133)` → `135` |
| 同上 | `:166` | heavy `7` **不改**（实况仍为 7） |
| 同上 | `:443`（`test_list_pr_profile`） | 找 `selected=133 total=140 skipped=7` 的断言字符串 → `selected=135 total=142 skipped=7` |
| 同上 | `:457`（`test_module_entrypoint`） | 同上 |
| `.github/workflows/ci.yml` | `:1023` | 步骤名 `govd pr profile (133 light gates)` → `(135 light gates)` |
| `.github/workflows/ci.yml` | `:1004` | 注释 `run the same 133 light gates through govd` → `135` |
| `scripts/govd/tasks.toml` | 头注释 | **实施第一步先 grep 确认有无 140/133 字面量**；有则同批改，无则不动 |

**3. 有序实施步骤**（每步一个可独立 revert 的 commit）

1. **步骤 0（前置核查，不改码）**：全仓 grep 陈旧计数字面量，确认改动面完整。
   ```bash
   grep -rn "133 light\|total=140\|selected=133\|, 140)\|, 133)" scripts/ .github/ docs/ 2>/dev/null
   grep -c '^\[\[gate\]\]' scripts/gate_manifest.toml            # 确认仍为 142
   grep -c 'kind = "light"' scripts/gate_manifest.toml           # 确认仍为 135
   grep -c 'kind = "heavy"' scripts/gate_manifest.toml           # 确认仍为 7
   ```
   **判据**：若 grep 命中超出上表范围，说明还有未列出的陈旧点，须先补全改动面再动手（本仓纪律：「必须同改 N 处」式断言里的每个 N 都要各自 grep 出读取方）。
2. **步骤 1（commit A：治理测试面）**：改 `test_govd.py` 的 `:163,164,165,443,457`，跑 `py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v` 至 **OK (79 tests)**。
3. **步骤 2（commit B：CI 文档面，需授权）**：改 `ci.yml:1023,1004` 的 133 → 135，跑 G-141 `py -3 scripts/check_workflow_yaml_lexical.py` 与 zizmor。
4. **步骤 3**：`git status --porcelain` 归零核查，确认无遗漏文件。

**为什么拆两个 commit**：commit A 只动 `scripts/`（无需 CI 授权），commit B 动 `.github/workflows/`（需授权）。若授权未拿到，commit A 可独立落地并让 govd 自测转绿 —— **收益的主要部分不依赖 workflow 改动**。

**4. 验证标准**

- **单元测试**：`py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v` ⇒ **OK (79 tests)**，rc=0（当前 FAILED failures=3）
- **专项 benchmark**：不适用（治理面，零运行时行为）
- **回归检查**：VB-1 的 `cargo check`（应无影响，未改 Rust 码）；VB-4 的 `--selftest` / `invariants` / `--static-only` 全部保持 PASS；commit B 后加跑 G-141/G-142（`check_workflow_yaml_lexical.py` 及其 `--selftest`）与 G-137（`check_ci_manifest_parity.py`）
- **接口 diff 校验**：VB-3 全 6 台门保持 rc=0（未触碰任何 crate）
- **防伪绿要求**：改完必须**反向验证门有牙** —— 手工把 `gate_manifest.toml` 里任一 `[[gate]]` 的 `kind` 从 light 改成 heavy，重跑 govd 自测应**判红**（heavy 计数变 8 ≠ 7），然后 `git checkout` 恢复。若改了不红，说明断言仍是恒真守卫

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | commit A 与 commit B **各自独立** `git revert`（两者无耦合：A 动 scripts/，B 动 .github/） |
| 回滚触发条件 | ① govd 自测出现**新的**失败（不在原 3 项内）；② G-137/G-141 因 ci.yml 改动判红；③ 反向验证发现断言恒真（说明改法错误，不是数字错误） |
| 灰度方式 | 不适用（本地测试 + CI 步骤名，无运行时行为）。commit B 落地后观察 1 晚 `govd pr shadow` job 的 report.json，确认 `selected=135 total=142` |
| 回滚后状态 | 恢复到「判红但已知」的状态，本文 §1.1 T0 即为该状态的登记 |

**6. ADR 骨架**：**不需要**。这是修复断言与实况的不一致，不是架构决策。但 commit message 必须载明归属追溯（`58a7567d` 引入，早于 `6aed20b4`），避免被误读为「本轮新增门」。

**7. 台账同批清单**

| 台账 | 是否需同批改 | 理由 |
|---|---|---|
| `scripts/gate_manifest.toml` | **不改** | 门数 142 是实况，本工单改的是断言去匹配它，不是反过来 |
| `scripts/govd/tasks.toml` | 视步骤 0 grep 结果 | 若头注释含 140/133 字面量则同批 |
| `scripts/gate_predicate_baseline.txt` | **不改** | 未改任何门的判据 |
| govd 普查断言 | **本工单主体** | `test_govd.py:163-166,443,457` |

> **本工单的治理教训（应写入记忆/规则）**：`gate_manifest.toml` 的门数与 `test_govd.py` 的硬编码普查断言是**必须同批的一对**，但 govd 自测**未接阻塞 CI**（只有 non-blocking shadow），所以「新增门忘同步普查」不会被任何红拦住。上一批 W0 的计划里已写明这条联动（「单点更新 `test_govd.py:160-166` 的 140/133/7」），说明纪律已知；缺的是**机器强制**。是否把 govd 自测升为阻塞门属独立裁决，不在本工单范围。

---

### 工单 T1 —— `mixed7r1w` bench 补登记阈值门（验证面补洞）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| 混层 bench 的阈值覆盖 | `scripts/` 对 `mixed7r1w` 与 `tplan_mixed` **零命中** ⇒ 无门 | `[thresholds_ns]` 有 `tplan_mixed_parallel` 键，`bench_check.yml` heredoc 有同名键，I9 对拍 PASS | `grep -rn "tplan_mixed" scripts/ .github/` 非空；`py -3 scripts/check_perf_redlines.py invariants` 的 I9 自报键数由 85 → **86** |
| 混层加速防回退 | 无（退回整层串行也不会红） | c8 median > 阈值即判红 | `bash scripts/check_perf_redlines.sh` 的 thresholds 腿，或 CI `bench_check.yml` 的 Assert thresholds 步 |
| 混层 c8 median | **21.777 ms**（本轮实测，CI [18.133, 22.688]） | 保持 ≤ 阈值 | B2 命令 |

**2. 变更范围**

| 文件 | 位置 | 改动 |
|---|---|---|
| `scripts/perf_thresholds.toml` | `[thresholds_ns]` 节，`:122` 之后 | 加 `tplan_mixed_parallel = 55000000`（推导见下）+ WHY 注释块（含本轮实测三值与置信区间） |
| 同上 | `[[path_keywords]]` 节，`:284` 之后 | 加 `tplan_mixed_parallel = ["toolplan_layers/mixed7r1w_ms2/8"]` |
| `.github/workflows/bench_check.yml` | THRESHOLDS_NS heredoc | 加 `tplan_mixed_parallel` 同名键同值（否则 **I9 判红**） |
| 同上 | PATH_KEYWORDS heredoc | 加同名 filter（否则 **I10 判红**） |

**阈值取值推导**（不得拍脑袋，按既有惯例）：

- 惯例出处：`perf_thresholds.toml:115` 记载 `new = ceil(12.585×2.5)=31.46 → 1ms 粒度 = 32ms`，即 **median × 2.5 后向上取整到 ms 粒度**。
- 本轮混层 c8 median = **21.777 ms** ⇒ `ceil(21.777 × 2.5) = ceil(54.44) = 55` ⇒ **55 ms = 55000000 ns**。
- 棘轮护栏核查：`:115` 提到 `< 160ms 棘轮护栏`；55 ms < 160 ms ✓。
- **`/1` 组（c1 = 98.082 ms）刻意不设阈值**，沿用 `:117-118` 的既有惯例原文：「/1 组（c1 107ms）本机负载敏感，仅作端到端对照不设阈值（c8/c1 ≥4× 判据由本轮 8.5× 实证）」。混层 c1 同理。
- **为什么不用 5.82× 那轮的读数**：见 §1.3 第 7 条 —— 三次读数 4.50 / 4.59 / 5.82 波动，c8 臂置信区间宽 25%。取本轮实测 21.777 ms 作基，×2.5 余量已覆盖上界 22.688 ms 的 2.4 倍。
- **I2 关键词互不为子串核查**：新 filter `toolplan_layers/mixed7r1w_ms2/8` 与既有 `toolplan_layers/readonly8_ms2/8`、`toolplan_layers/undeclared8_ms2/8` 双向互不为子串 ✓。
- **`max_keyword_overlaps` 核查**：`:279-280` 记载既有唯一重叠对是 `diff_engine_bench × writer_ansi`，重叠数 1。新增键不引入新重叠 ⇒ 维持 1 ✓（实施时用门自身的 `keyword_overlaps` 复核，不靠人眼）。

**3. 有序实施步骤**

1. **步骤 1（重测取基）**：静默态单跑 `cargo bench -p gqep-executor --bench toolplan_layers --jobs 2 -- --quick --noplot`，记录 `mixed7r1w_ms2/8` 的 median 与置信区间。**若与本轮 21.777 ms 偏差 > 20%，用新 median 重算 ×2.5**，不照抄本文数字。
2. **步骤 2（commit A：阈值表）**：改 `perf_thresholds.toml` 两处 + WHY 注释块。跑 `py -3 scripts/check_perf_redlines.py invariants` ⇒ 此时 **I9/I10 会判红**（本表有键而 heredoc 无），这是预期的中间态，证明门有牙。
3. **步骤 3（commit B：workflow heredoc，需授权）**：改 `bench_check.yml` 两个 heredoc。重跑 `invariants` ⇒ I9/I10 应转 PASS 且自报 **86 keys**。
4. **步骤 4（负控验证）**：手工把阈值改成 `1000000`（1 ms，必然判红），跑 thresholds 腿确认**真的判红**，再改回。若改了不红 ⇒ 该键未被任何执行路径消费，等于白加（这正是 I11 要防的病理）。

**步骤 2 与 3 能否合并为一个 commit**：**不建议**。I9 是键集**严格相等**对拍，拆开会产生一个中间态红 commit。若 CI 对每个 commit 都跑，则必须合并为单 commit；若只对 PR head 跑，可拆。**实施前需确认 `bench_check.yml` 的触发时机**（该 workflow 由 PR 改 `benches/**`、任一 `Cargo.toml`、`chimera-cli/src/**` 或自身触发 ⇒ 改 `perf_thresholds.toml` 与 `bench_check.yml` 都会触发）。**结论：合并为单 commit 更安全**，但需在同一 commit 内完成 workflow 授权。

**4. 验证标准**

- **单元测试**：`py -3 scripts/check_perf_redlines.py --selftest` ⇒ 71 assertions PASS（本工单不新增 selftest 断言；若判定要加负控夹具，则断言数会上升，须同批更新 selftest 自报）
- **专项 benchmark**：`bash scripts/check_perf_redlines.sh` 的 thresholds 腿对 `toolplan_layers/mixed7r1w_ms2/8` 有断言且 PASS；`cargo bench -p gqep-executor --bench toolplan_layers` 复跑确认 median 在阈值内
- **回归检查**：VB-1 全部（未改 Rust 码，应无影响）；VB-4 的 `invariants`（I9/I10 自报 86 keys）、`--static-only`、`inventory`；G-141/G-142 workflow 词法门
- **接口 diff 校验**：VB-3 全 6 台门 rc=0（未触碰 crate）
- **不变式核查**：I1（`thresholds_ns` 键集 == `path_keywords` 键集）必须仍 PASS —— 两处同批加键即为此

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | 单 commit（`perf_thresholds.toml` + `bench_check.yml` **必须同退**，否则 I9/I10 判红） |
| 回滚触发条件 | ① CI 上混层 bench 连续 2 晚假红（负载敏感）；② I9/I10 因 heredoc 格式问题判不可判定（rc=2）；③ 负控验证发现该键无执行路径消费 |
| 灰度方式 | 若担心 CI runner 负载导致假红，**先走 `[[excluded]]` 而非放宽阈值**：在 `perf_redlines.toml` 加 `[[excluded]]`（`owner` + `expiry` + `reason`，只减不增，过期即红），观察 2 晚后再收紧。**禁止**为了让门绿而把 55 ms 放大到 175 ms |
| 回滚后状态 | 恢复到「ADR-281 收益无 CI 防回退」，本文 §1.1 T1 即为该状态的登记 |

**6. ADR 骨架**：**不需要**。这是补登记既有 bench 的阈值，不引入新判据、不改运行时行为。若步骤 4 的负控发现需要新增不变式（例如「新 bench 组必须同批登记阈值键」的机器强制），则**需另立 ADR**，因为它会是新门（触发四件同批纪律，见第 7 段）。

**7. 台账同批清单**

| 台账 | 是否需同批改 | 理由 |
|---|---|---|
| `scripts/perf_thresholds.toml` | **是**（主体） | 两处加键 |
| `.github/workflows/bench_check.yml` | **是**（同批，否则 I9/I10 红） | heredoc 键集严格相等 |
| `scripts/perf_redlines.toml` | **视裁决** | 若把混层加速判为**红线**（而非普通阈值），则需加 `[[redline_exec]]`（参照 RL-01~RL-11 形态）+ I11 会把它纳入执行路径核查。**倾向不加**：红线是「不可逾越的下界」，混层加速是比值判据，用 `thresholds_ns` 更贴切 |
| `scripts/gate_manifest.toml` | **仅在新增门时** | 若步骤 6 的「新 bench 组必须登记阈值」升格为门，则按**四件同批**：脚本 + manifest 两条（本体 + `--selftest`）+ workflow 一步 + 单点更新 `test_govd.py` 的普查（142→144 / 135→137）与 `tasks.toml` 头注释 |
| `scripts/bench_inventory_freeze.txt` | **不改** | `mixed7r1w` 是既有 bench 文件 `toolplan_layers.rs` 内的新组，该文件已登记（台账行格式为 `gqep-executor\|toolplan_layers\|registered`，见 `perf_thresholds.toml:539-542` 的 `[[run_bench]]`） |

---

### 工单 T2 —— `superposition_query` 死臂裁决（三选一）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| 生产消费者数 | **0**（排除 mcp-mesh 自身后全仓零命中） | 选项 A：≥1 个真实生产调用方 / 选项 B：API 不存在 / 选项 C：台账登记死臂 + 到期日 | `grep -rn "superposition_query\|SuperpositionQuery" crates/ --include=*.rs` 排除 `crates/mcp-mesh/` |
| fanout 并发上界 | 无独立信号量；实际上界 = `min(fanout_servers.len(), registry_capacity=256)`，**不是** `max_participants=32`（该钳制在 `mesh.rs:240` 的 `execute_transaction` 路径，superposition 不经过） | 选项 B：不适用（API 删除）/ 选项 A、C：加 Semaphore 或复用 `max_participants` 钳制 | 读 `superposition.rs:171-191` 确认 spawn 前有无许可获取 |
| 公开 API 面 | `lib.rs:59,76` 两处 re-export + `mesh.rs:704` 委托方法 | 选项 B：全部移除，G-105/G-107 棘轮**不受影响**（mcp-mesh 是 L10，不在 L0 台账内） | VB-3 |

**2. 变更范围**（选项 B 的完整删除面，已逐处核实）

| 文件 | 位置 | 改动 |
|---|---|---|
| `crates/mcp-mesh/src/quantum/superposition.rs` | 整文件 **495 行** | 删除（生产段 1-234：`SuperpositionQuery` `:31-93`、`QueryResult` `:97-142`、`execute_superposition_query` `:164-233`；测试段 235-495，260 行） |
| `crates/mcp-mesh/src/quantum/mod.rs` | `:8`（模块注释）、`:14`（`pub mod superposition;`） | 删除两行 |
| `crates/mcp-mesh/src/mesh.rs` | `:7`（注释）、`:39`（use）、`:703-733`（`superposition_query` 委托方法）、`:735`（辅助方法签名参数）、`:1330-1360`（两个测试） | 删除 |
| `crates/mcp-mesh/src/lib.rs` | `:59`、`:76`（prelude 内） | 删除两处 re-export |
| `crates/mcp-mesh/src/error.rs` | `ServerNotFound` `:19`、`ServerUnreachable` `:29` | **不删**（见下方约束） |

> **硬约束（已核实，不可违反）**：`McpError::ServerNotFound` 与 `ServerUnreachable` **有大量其他消费者**，删 superposition 时**绝不能一并删这两个变体**：`chimera-cli/src/commands/mcp.rs:232,291`、`chimera-cli/tests/cli.rs:840`、`chimera-cli/tests/mcp_call_network.rs:80,97`、`mcp-mesh/benches/registry_lookup.rs:1,6,72,79`、`mcp-mesh/benches/mesh_benchmark.rs:28`、`mcp-mesh/src/error.rs:203,247`（自身测试）。

**3. 有序实施步骤**

1. **步骤 0（前置核查）**：重跑零命中判定，确认无新增消费者。
   ```bash
   grep -rn "superposition_query\|SuperpositionQuery\|QueryResult" crates/ --include=*.rs | grep -v "^crates/mcp-mesh/"
   grep -rn "superposition" scripts/ docs/architecture/adr_index.md
   ```
   **判据**：第一条必须零命中；第二条若有台账/ADR 登记需同批处理（本轮实测 `scripts/*.txt|*.toml` 对 superposition **零登记** ⇒ 无台账联动）。
   **注意 `QueryResult` 是通用名**，需逐一确认命中的是否 mcp-mesh 的那个（本仓有同名符号跨 crate 双义的既往教训）。
2. **步骤 1（commit A：删码）**：按上表删除，跑 `cargo check -p mcp-mesh --all-targets --jobs 2` 至 rc=0，再跑 `cargo test -p mcp-mesh --no-fail-fast --jobs 2`。
3. **步骤 2（同 commit A 内）**：跑 `cargo clippy -p mcp-mesh --all-targets --jobs 2 -- -D warnings` —— 删除后可能暴露 `unused import`（如 `JoinSet`、`Arc`），须一并清理。
4. **步骤 3（commit A 内）**：跑 VB-3 的 G-101（`check_dead_contract_apis.py`）与 G-105/G-107，确认 L0 面不受影响、死契约 API 台账无需更新。
5. **步骤 4（commit B：ADR 落档）**：写 `docs/architecture/ADR-284-superposition-dead-arm-adjudication.md` + 在 `docs/architecture/adr_index.md` 追加登记块。**取号前三向查重**：`ls docs/architecture/ADR-284*`、`grep -rn "ADR-284" docs/architecture/adr_index.md docs/architecture/INDEX.md`、`git log --all --grep "ADR-284"`。

**4. 验证标准**

- **单元测试**：`cargo test -p mcp-mesh --no-fail-fast --jobs 2` 全绿（原有 `test_superposition_query_via_mesh`、`test_superposition_query_publishes_mcp_message_received` 与 `superposition.rs` 的 260 行测试段随删除消失，**这是预期的**，须在 ADR 里如实登记测试数下降）
- **专项 benchmark**：`cargo bench -p mcp-mesh --bench mesh_benchmark --jobs 2 -- --quick --noplot` 与 `registry_lookup` 均应不受影响（两者都不调用 superposition）；B4 的 `mcp_mesh_transaction_instant/2pc/1` = 3.2532 ms 应保持不变（±噪声）
- **回归检查**：VB-1 全部（`cargo check --workspace --all-targets` 尤其重要，确认无其他 crate 引用被删符号）；VB-2 定向回归 `-p mcp-mesh -p chimera-cli`；VB-4 全部门
- **接口 diff 校验**：VB-3 全 6 台门 rc=0。**关键判据**：G-105（L0 `pub fn` 只减棘轮）与 G-107（L0 根 re-export）**只覆盖 L0 crate**，mcp-mesh 是 L10 ⇒ 删它的 pub API **不会**触发这两台门。这正是「不触发契约变更清单」的机器依据
- **防伪绿要求**：删除后必须确认 `cargo check --workspace --all-targets` **真的**编译了所有 target（上一批的教训：漏跑 `--all-targets` 会让 E0063 类错误延迟暴露）

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | commit A（删码）与 commit B（ADR）**各自独立** revert。代码面回滚只需 revert commit A |
| 回滚触发条件 | ① `cargo check --workspace --all-targets` 出现任何 crate 的编译错误（说明存在未被 grep 捕获的消费者，例如经 `prelude::*` 通配导入）；② 发现有 in-flight 分支正在接 superposition 的生产调用方（须先 `git log --all --grep superposition` 与查在途分支）；③ G-101 死契约 API 台账判红且无法同批修 |
| 灰度方式 | 不适用（删除是二值的）。若担心下游未入库代码依赖它，可先走**选项 C**（登记死臂 + 到期日 2026-12-31），到期无消费者再删 —— 这是本仓处理 `start_background_probe` 时**没有**采用的更保守路径（上一批 ADR-283 直接删了，回滚演练证明干净） |
| 回滚后状态 | 恢复到「接了传输没接消费者」，本文 §1.1 T2 即为该状态的登记 |

**6. ADR 骨架（ADR-284，实施时落盘）**

- **背景**：`execute_superposition_query` 提供多服务器并发 fanout 查询能力。ADR-253 D2（2026-11-07）把它从「进程内伪造 payload（`result@<query>`）」改为「经注入的 `Arc<dyn ParticipantClient>` 真实 fanout」，修好了传输语义。但本轮盘点实测：排除 mcp-mesh 自身后全仓对 `superposition_query|SuperpositionQuery` **零命中** ⇒ 接线后**仍无生产消费者**。同时其 `JoinSet` fanout（`superposition.rs:187,191`）无独立信号量，实际上界来自 `registry_capacity = 256` 而非 `max_participants = 32`（后者只钳 `execute_transaction` 路径，`mesh.rs:240`）。
- **可选方案**：
  | 方案 | 评估 |
  |---|---|
  | A. 接生产调用方 | 需先证明存在真实的多服务器并发查询需求。**现无**：`mcp call` 走 `execute_transaction` 单服务器路径，`mcp serve` 未实现（ADR-253 §4 已登记为「已知边界而非缺陷」）。为接线而造需求违反本仓「不需要基线需要流量」的既有判据 |
  | **B. 删除 API（倾向）** | 与上一批 ADR-283 删 `McpMesh::start_background_probe`（零调用、零台账、配置字段也无人读）**完全同型**，且该次删除的回滚演练证明 `git revert --no-commit` 干净应用、`cargo test -p mcp-mesh --lib` 114/114 绿 |
  | C. 登记死臂 + 到期日 | 保守但会留一份无消费者的公开 API 与一个无上界的 fanout 路径；本仓对 `start_background_probe` 未采用此路径 |
- **取舍逻辑**：ADR-253 §2 的取舍表里，选项 C「superposition 保留本地 mock、另开真实查询 API」被否决的理由原文是**「trait `query` 早已为它而生，不接线等于能力空转」**。现状是：线接了，消费者仍然没有 ⇒ **能力依然空转**。用 ADR-253 自己的判据，处置指向 B。另：删码同时消除了「fanout 无独立并发上界」这一潜在资源风险，而选项 A/C 都需要额外加信号量才能达到同等安全性。
- **最终决策**：（实施时填）
- **回滚触发条件**：（同第 5 段三条）
- **必须如实登记的代价**：① 测试数下降（`superposition.rs` 的 260 行测试段 + `mesh.rs` 两个测试）；② **本 ADR 部分推翻 ADR-253 D2 的投入** —— D2 花的成本（trait 参数化 + 传输失败如实标记）随删除而失效。这不是 D2 做错了：D2 修的是「fail-open 伪造成功载荷」这个**真实缺陷**，修完才发现没有消费者。**ADR-284 必须写明这一点，不得静默推翻**。

**7. 台账同批清单**

| 台账 | 是否需同批改 | 理由 |
|---|---|---|
| `scripts/*.txt` 冻结文件 | **不改**（本轮实测 superposition 在 `scripts/` 下的 `.txt` 与 `.toml` 台账里**零登记**） | 无联动 |
| `scripts/gate_manifest.toml` | **不改** | 不新增门 |
| `docs/architecture/adr_index.md` | **是**（commit B） | 追加 ADR-284 登记块（append-only） |
| `docs/architecture/CODE_WIKI.md` | **视情况** | 若 §3 的 mcp-mesh 条目列举了 superposition 能力，需同批订正 |
| `crates/mcp-mesh/src/error.rs` | **不改** | `ServerNotFound`/`ServerUnreachable` 有 6+ 处其他消费者（已核实清单在第 2 段） |

---

### 工单 T3 —— tract-onnx 依赖 ROI 裁决（**第一步必须是测量**）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| tract 族 debug 产物体积 | **347 MB / 41 文件**（占 `target/debug/deps` 20769 MB 的 **1.7%**） | 选项「删」：0 / 选项「留」：不变 | `du -cm target/debug/deps/libtract*` |
| **nmc-encoder 编译时长差** | 【待测量】 | 有量化数值 | `cargo build -p nmc-encoder --timings` 有/无 tract 各一次，比对 `target/cargo-timings/*.html` 的 unit 时间 |
| **release 二进制体积增量** | 【待测量】 | 有量化数值 | `cargo build --release -p chimera-cli` 有/无 tract 各一次，比对 `target/release/chimera.exe` 字节数 |
| 运行期收益 | **0**（`model_dir` 默认空 ⇒ ONNX 路径永不执行；`OnnxBackend` 无生产构造点） | 选项「留」：必须先接线并给出端到端收益 / 选项「删」：不适用 | `grep -rn "OnnxBackend" crates/ --include=*.rs` 排除 nmc-encoder 自身后应零命中 |

**2. 变更范围**（两个分支，实施时二选一）

**分支「不值得留」**：

| 文件 | 改动 |
|---|---|
| `Cargo.toml:156` | 删 `tract-onnx = "0.22"` workspace 依赖声明 |
| `crates/nmc-encoder/Cargo.toml` | 删 `[dependencies]` 里的 tract-onnx |
| `crates/nmc-encoder/src/perceptors/onnx_backend.rs` | 整文件删除（含 `:140-153` 的 tract 加载器、`:566` 的 `mel_spec` 调用） |
| `crates/nmc-encoder/src/perceptors/mod.rs:26,36` | 删 `pub mod onnx_backend;` 与 `pub use onnx_backend::{ModelType, OnnxBackend};` |
| `crates/nmc-encoder/src/lib.rs:68` | 从 pub use 列表删 `ModelType, OnnxBackend` |
| `crates/nmc-encoder/src/config.rs` | `model_dir` `:57`、`with_model_dir` `:130-131`、`model_path` `:173-179`、`validate` 的 ONNX 分支 `:207-208`、Default 的 `:252-253`：需裁决是保留字段（永远为空）还是一并删 |
| `crates/nmc-encoder/src/perceptors/text.rs:90-100` | 保留降级路径（字节频率嵌入），删 ONNX 尝试分支 |
| `Cargo.lock` | 自动重算（tract 族 7 包消失） |
| **连带评估** | `tokenizers`（`Cargo.toml:164`）、`mel_spec`（`:166`）、`hound`（`:168`）、`image`（`:160`）是否也随之失去唯一消费者 —— 须逐个 `cargo tree -i` 核实，**不得连带删还有消费者的包** |

**分支「值得留」**：不改依赖，改为出**接线工单**：补 `model_dir` 部署文档 + 权重获取方式（仓库内零 `.onnx` 文件，需说明从哪下载/如何校验）+ `config.rs:207-208` 的 `validate()` 接部署前检查 + 给 `OnnxBackend` 找生产构造点。

**3. 有序实施步骤**

1. **步骤 1（测量，不改生产码）**：
   ```bash
   # 基线：有 tract
   cargo build -p nmc-encoder --timings --jobs 2
   cp target/cargo-timings/cargo-timing.html tmp/tract-with.html
   cargo build --release -p chimera-cli --jobs 2
   stat -c %s target/release/chimera.exe > tmp/tract-with-size.txt

   # 对照：临时注释掉 tract-onnx（工作树改动，测完 git checkout 恢复）
   #   → 重跑同上两条，存 tmp/tract-without.html 与 tmp/tract-without-size.txt
   git checkout Cargo.toml crates/nmc-encoder/Cargo.toml   # 恢复
   ```
   **纪律**：对照测量必须在**同一台机、同一 HEAD、串行**完成；`git checkout` 恢复前先 `git status` 确认只改了那两个 Cargo.toml（本仓纪律：回滚禁用 `git checkout --` 作用于同批互锁改动，此处是**临时测量**而非改动落盘，属例外，但必须先确认改动面）。
2. **步骤 2（裁决）**：按三因子判 —— 编译时长差、release 体积差、运行期收益（已知为 0）。**判据建议**：若编译时长差 < 全量编译的 3% 且 release 体积差 < 1 MB，则「删」的收益不足以抵偿 API 破坏风险 ⇒ 转「留 + 接线工单」或「留 + 登记为待接线能力」。
3. **步骤 3**：按步骤 2 的结论走对应分支的 commit。
4. **步骤 4（ADR-285 落档）**：取号三向查重同 T2。

**4. 验证标准**

- **单元测试**：`cargo test -p nmc-encoder --no-fail-fast --jobs 2`；分支「删」时 `onnx_backend.rs` 的测试段随之消失（须如实登记测试数下降）
- **专项 benchmark**：nmc-encoder 相关 bench（若有）；`cargo bench -p chimera-mas --bench mas_benchmark` 不受影响（`wiki_retrieval.rs` 用的是 `TextPerceptor`）
- **回归检查**：VB-1 全部；VB-2 全量（依赖变更影响面大）；**VB-5 必跑**（改了 Cargo.toml/Cargo.lock）—— 特别是 G-86 `check_accel_exclusion.py`（tract **不在**禁列内，ADR-195:105 已登记更正注「tract-onnx 系 7 包在盘…不在 G-86 禁列内，本更正不改变 D1 否决结论」）与 `cargo audit`（tract-onnx 0.22.3 是 RUSTSEC-2026-0217/0222/0223 的修复版本，删它不影响 audit，但 `mel_spec`/`tokenizers` 若连带删除需重跑）
- **接口 diff 校验**：VB-3。**关键**：`nmc-encoder` 是 **L2 Memory 层**，不在 L0 台账内 ⇒ G-105/G-107/G-115/G-135 均不覆盖它，删 `OnnxBackend`/`ModelType` **不会**触发这些门。**但这不等于无契约风险**：`OnnxBackend` 是 `lib.rs:68` 的 pub use，属 nmc-encoder 的公开 API，删它是**该 crate 的 SemVer 破坏性变更** ⇒ 须在 ADR-285 里登记，并确认 workspace 内无外部消费者（步骤 0 的 grep）
- **防伪绿要求**：分支「删」后必须确认 `cargo check --workspace --all-targets` 编译了全部 target，且 `cargo tree -p nmc-encoder` 不再出现 tract 族

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | 单 commit（`Cargo.toml` + `Cargo.lock` + nmc-encoder 源码**必须同退**：只退源码不退 lock 会导致 lock 与 manifest 不一致） |
| 回滚触发条件 | ① 发现任何未捕获的 `OnnxBackend`/`ModelType` 消费者；② `tokenizers`/`mel_spec`/`hound`/`image` 中任一个因连带删除而破坏了其他 crate；③ 步骤 1 的测量显示编译时长/体积收益低于步骤 2 的判据门槛（此时应**在实施前**就转向，不该走到回滚） |
| 灰度方式 | 可先把 tract-onnx 改为 **optional + feature 门控**（`onnx = ["dep:tract-onnx"]`，默认 off）作为中间态 —— 这样默认构建不编译 tract（拿到编译时长与体积收益），同时保留 API 面（不破坏 `OnnxBackend` 的可用性，只是需要 feature）。**这是风险最低的落地形态**，且与本仓 `mca = ["dep:mca-gateway"]` 的既有先例同型 |
| 回滚后状态 | 恢复到当前状态（编译在盘、运行期零加载） |

**6. ADR 骨架（ADR-285，实施时落盘）**

- **背景**：tract-onnx 0.22.3 作为 workspace 依赖（`Cargo.toml:156`）被 `nmc-encoder` 直接生产依赖（非 optional、无 feature 门），Cargo.lock 含 tract 族 7 包，debug 产物 347 MB（占 deps 1.7%）。但运行期**永不执行**：`config.rs:246-253` Default 的 `model_dir` 为空字符串即表示未启用，`:207-208` 的 `validate()` 仅在非空时校验模型文件，仓库内零 `.onnx` 权重文件，`text.rs:97-100` 加载失败即降级到字节频率嵌入；生产侧唯一构造的 perceptor 是 `chimera-mas/src/knowledge/wiki_retrieval.rs:85` 的 `TextPerceptor`，`OnnxBackend` 无生产构造点。与之呼应，`nmc-encoder/parallel.rs::perceive_parallel` 在 G-87 台账里标 **DEAD**。
- **可选方案**：A. 删除（依赖 + `OnnxBackend`/`ModelType` + `onnx_backend.rs`）；B. 改为 optional + feature 门控（默认 off）；C. 保留现状 + 出接线工单；D. 保留现状 + 登记为待接线能力（附到期日）。
- **取舍逻辑**：任务书硬约束「禁止引入新的重量级依赖，除非能够提供该依赖与现有方案的体积、编译时间、收益的量化对比数据」—— **同一把尺必须反过来用于已在盘的依赖**。当前已测得：体积 347 MB debug / 占比 1.7%，运行期收益 **0**；未测得：编译时长差、release 体积增量。磁盘面 1.7% **不足以**单独支撑删除（会破坏 nmc-encoder 的公开 API）；但「运行期收益为 0 而编译面长期在盘」这一事实本身需要一个显式裁决，不能继续静默。**方案 B 是唯一同时满足「拿到编译收益」与「不破坏 API 面」的选项**，且与本仓 `mca = ["dep:mca-gateway"]` 先例同型。
- **最终决策**：（实施时按步骤 1 的测量填）
- **回滚触发条件**：（同第 5 段三条）

**7. 台账同批清单**

| 台账 | 是否需同批改 | 理由 |
|---|---|---|
| `Cargo.lock` | **是** | 依赖变更自动重算，必须与 `Cargo.toml` 同 commit |
| `scripts/compute_offload_reachability_freeze.txt` | **视情况** | 第 7 行台账 `nmc-encoder/src/parallel.rs::perceive_parallel\|DEAD` —— 若删 `onnx_backend.rs` 影响到 `perceive_parallel` 的锚点定义，须同批改（G-87 是 exact-set 门，锚点集变化即判红） |
| `scripts/gated_wiring_baseline.txt` | **视方案 B** | 若走 optional + feature 门控，nmc-encoder 的 tract 边会变成 gated 形态，可能需登记（该台账是 only-decrease） |
| `docs/architecture/adr_index.md` | **是** | ADR-285 登记块 |
| `docs/architecture/CODE_WIKI.md` | **视情况** | 若 §3 的 nmc-encoder 条目描述了 ONNX 能力 |

---

### 工单 T4 —— `perf_thresholds.toml` 头注陈旧订正（纯文档）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| 头注与实况一致性 | `:5-15` 声称「本文件当前是第二份表而非唯一真值源，R1-T6 的切流尚未发生…两表差集会静默存在」 | 头注如实反映 I9/I10 已落地并对拍 PASS | 人工比对头注文本与 `check_perf_redlines.py:359-376` 的实现 + `invariants` 实跑输出 |
| I9/I10 状态 | **PASS，85 keys**（本轮实跑） | 不变（本工单不碰键集） | `py -3 scripts/check_perf_redlines.py invariants` |

**2. 变更范围**

| 文件 | 位置 | 改动 |
|---|---|---|
| `scripts/perf_thresholds.toml` | `:5-15` 头注块 | 订正为：I9/I10 已实现 heredoc ↔ 本表键集**严格相等**对拍（`check_perf_redlines.py:359-376`），R2 M44（`hnsw_tiered` 词条 heredoc 漏接）已由 I9 根修，N-3 幽灵门残留已由 I10 根修；本表现在是**受机器对拍保护的双写面**（改一处必须同批改另一处，否则 I9/I10 判红），而非「差集会静默存在」的第二份表。保留历史脉络（2026-10-26 R2 订正的原文可作为历史记录段，但须明确标注已被后续修复关闭） |
| 同上 | `:6-15` 内的「52 键」表述 | 订正为门自报的 **85 keys**（口径：I9 自报行，不用 awk 逐节计数） |

**3. 有序实施步骤**

1. **步骤 1**：跑 `py -3 scripts/check_perf_redlines.py invariants` 记录 I9/I10 的自报原文（作为订正的证据）。
2. **步骤 2（单 commit）**：订正头注。**不改任何键、值、filter**。
3. **步骤 3**：重跑 `invariants` + `--selftest` + `--static-only` 确认全绿（注释不参与解析，应无变化）。
4. **步骤 4**：跑 G-04/G-05（`check_doc_consistency.{ps1,sh}`）确认文档一致性门不受影响。

**4. 验证标准**

- **单元测试**：`py -3 scripts/check_perf_redlines.py --selftest` ⇒ 71 assertions PASS（不变）
- **专项 benchmark**：不适用
- **回归检查**：`invariants`（I1~I11 全 PASS，键数仍 85）、`--static-only`（VERDICT: PASS）、G-04/G-05 文档一致性门
- **接口 diff 校验**：不适用（未触碰 crate）
- **关键判据**：改注释后 I1（`thresholds_ns` == `path_keywords`）与 I9/I10 必须**逐字不变** —— 若键数变了，说明误改了数据行

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | 单 commit |
| 回滚触发条件 | 任何门因注释改动判红（说明该注释被某个门的解析器读取 —— 本仓有「注释里的数字/字段消费主张也是可测面」的既往教训，故必须实跑验证而非假定注释无害） |
| 灰度方式 | 不适用 |

**6. ADR 骨架**：**不需要**（纯文档订正，无决策）。

**7. 台账同批清单**：无。但**必须实跑** G-04/G-05/I1/I9/I10 确认注释不被任何解析器消费。

---

### 工单 T5 —— mcp-mesh WAL 归因测量（**第一步是测量，不是改码**）

**1. 目标指标**

| 指标 | 当前值 X | 目标值 Y | 测量方法 |
|---|---|---|---|
| instant 路径 2PC 构成 | 总 **3.2532 ms**，构成未拆分 | WAL（2 次 `open`+`write`+`sync_all`）/ `sensitive::scan` / 2PC 帧编解码 三者各占比有数值 | bench 加 `durable=false` 对照臂，差值即 WAL 贡献 |
| 生产 `mcp call` 端到端 p50 | 【待测量】 | 有数值 | hyperfine 对 `chimera mcp call` 做 20 runs（需 release 二进制 + 已注册的 in-process server） |

**2. 变更范围**

| 文件 | 改动 |
|---|---|
| `crates/mcp-mesh/benches/mesh_benchmark.rs` | 加一个 `durable=false` 的 mesh 构造 helper + 一个 bench 组（例 `mcp_mesh_transaction_nodur`），与既有 `mcp_mesh_transaction_instant` 同参数同场景，形成三臂对照：`带模拟延迟+durable` / `零延迟+durable`（= 现 instant 臂）/ `零延迟+非durable` |
| **生产代码** | **本工单第一步不改任何生产代码**。归因结果出来后才决定是否需要第二个工单 |

**3. 有序实施步骤**

1. **步骤 1（commit A：bench 对照臂）**：加第三臂。这是 **bench 文件改动，不是生产码**，但仍属代码改动 ⇒ 按裁定 4 需另批授权。
2. **步骤 2（测量）**：静默态单跑 `cargo bench -p mcp-mesh --bench mesh_benchmark -- --quick --noplot`，记录三臂 median。
   - 归因公式：`WAL 贡献 = instant 臂 − nodur 臂`；`模拟延迟贡献 = transaction 臂 − instant 臂`（本轮已知 = 27.553 − 3.253 = **24.30 ms**）；`剩余（帧编解码 + 敏感扫描 + 2PC 逻辑）= nodur 臂`
3. **步骤 3（裁决）**：
   - 若 WAL 贡献 > 1 ms（即占 3.253 ms 的 >30%）⇒ 开第二工单评估**文件句柄复用**（保持 `File` 打开而非每次 `open`），或 `sync_all` → `sync_data`（若 WAL 不需 mtime 持久性，`sync_data` 在部分平台更快）。**两者都必须保证不撤掉 `sensitive::scan`**（上一批已判定 `durable=false` 是安全回归，因为扫描挂在写入路径上）
   - 若 WAL 贡献 < 0.3 ms ⇒ 登记「不值得优化」，关闭这个悬置问题
4. **步骤 4**：把三臂读数与裁决写进 ADR-280 的追加节或新 ADR（视是否产生代码改动）。

**4. 验证标准**

- **单元测试**：`cargo test -p mcp-mesh --no-fail-fast --jobs 2` 全绿（新增 bench 臂不影响测试）
- **专项 benchmark**：三臂对照，每臂记录 median + 置信区间；**必须串行单跑**（上一批两个 bench 并发互撞致 exit=101）
- **回归检查**：VB-1 的 clippy（bench 文件也在 `--all-targets` 内）；`bench_inventory_freeze.txt:79` 的 `mcp-mesh|mesh_benchmark|dev-only` 登记**不变**（同文件加组不改登记行）
- **接口 diff 校验**：VB-3（未触碰 L0）
- **安全判据（若进入步骤 3 的实施）**：`sensitive::scan` 必须仍在写入路径上；`crates/seccore/tests/policy_zero_alloc.rs` 与 `check_write_sensitive_instrumentation.py`（G-84/G-92）必须保持 rc=0

**5. 回滚预案**

| 项 | 内容 |
|---|---|
| 最小回滚单元 | commit A（bench 臂）独立 revert；步骤 3 若产生生产码改动则是另一个独立 commit |
| 回滚触发条件 | ① 三臂读数显示 `nodur` 臂与 `instant` 臂差异在噪声内（说明 WAL 不是主导，无需继续）；② 任何削弱持久性或撤掉敏感扫描的方案 ⇒ **直接否决，不进回滚流程** |
| 灰度方式 | 不适用（测量无运行时影响） |

**6. ADR 骨架**：步骤 1-2 **不需要** ADR（纯测量）。步骤 3 若产生生产码改动则需要，格式同前，且**必须**在背景段说明与三个既有否决的边界：ADR-275 D3 否决的是 seccore 侧 `FileWalCriticalSink::open`（env 门控 + 40 字节），**不是** mcp-mesh 的 `WalStore`；上一批否决的是 WAL group commit（单事务无 batch 可合）与 `durable=false`（撤掉敏感扫描是安全回归）；**「句柄复用」与「`sync_all` → `sync_data`」未被任何 ADR 覆盖**。

**7. 台账同批清单**：`scripts/bench_inventory_freeze.txt` 不改（同文件加组）；若步骤 3 改动 `wal.rs`，须核查 G-84/G-92（`check_write_sensitive_instrumentation.py`）是否覆盖该文件的写入 instrumentation。

### 【本阶段完成度自评】

| 要求 | 完成度 | 缺口 |
|---|---|---|
| 明确目标指标（X→Y + 测量方法） | **完成**：T0~T5 共 19 个指标行，每行有当前值、目标值、可执行测量命令 | T3 的编译时长/体积、T5 的端到端 p50 是【待测量】—— 但两者的测量方法已写成可复制命令，且测量本身被列为实施第一步 |
| 变更范围精确到 crate/模块/函数/行 | **完成**：T0 精确到 5 个行号 + 2 个 workflow 行；T1 到 toml 节与行；T2 到 6 个文件的精确行区间（含 495 行文件的段落划分）；T3 到 9 个文件 + 4 个连带评估包；T4 到注释块行区间；T5 到 bench 文件 | 无 |
| 有序实施步骤，支持分阶段逐步提交 | **完成**：每工单 3~4 步，标注了哪些步骤可拆为独立 commit、哪些必须同 commit（T1 的 toml+heredoc 因 I9 严格相等而必须同批；T3 的 Cargo.toml+lock+源码必须同退） | 无 |
| 验证标准（单测 + 专项 bench + 回归含 clippy/全量矩阵/接口 diff） | **完成**：共用验证基座 VB-1~VB-5 + 每工单四项细化。**每份工单都含「防伪绿要求」**（T0 反向改 kind 验断言有牙；T1 改阈值到 1ms 验真判红；T2 强调 `--all-targets`；T3 验 `cargo tree` 无 tract；T4 验键数逐字不变） | 无 |
| 回滚预案（最小回滚单元 / 触发条件 / 灰度） | **完成**：六份工单各有三栏表格 | 无 |
| ADR 固定格式（背景→可选方案→取舍逻辑→最终决策→回滚触发条件） | **完成**：T2（ADR-284）与 T3（ADR-285）给出完整骨架，五段齐全；T5 说明何时需要 ADR；T0/T1/T4 明确说明**不需要** ADR 及理由 | 两份 ADR 的「最终决策」留空 —— 这是刻意的：本轮零实施，决策属实施批 |
| 台账同批清单 | **完成**：六份工单各有台账表，标明「改 / 不改 / 视情况」及理由 | 无 |

**自评结论**：阶段2 达成「独立可验收」，六份工单均可直接进入实施队列。**两处需用户授权的前置已显式标注**：① T0 commit B 与 T1 步骤 3 触碰 `.github/workflows/`，需按本仓纪律单独授权；② T5 步骤 1 改 bench 文件，虽非生产码但按裁定 4「零代码改动」仍需另批。

### 【下阶段前置依赖说明】

阶段3 的验收表与风险矩阵依赖：

1. **验收表的达标线**直接取阶段2 各工单第 1 段的目标值 Y（已就位）。
2. **验收表的 before 值**取阶段0 §0.3 的实测（已就位）；**after 值全部为「未实施」**（裁定 4），阶段3 须如实标注而非留空。
3. **风险矩阵**依赖阶段1 §1.3 的 8 条复测纠正（已就位，其中 3 条直接转化为风险条目）与阶段2 各工单第 5 段的回滚触发条件（已就位）。
4. **未闭环清单**依赖：阶段0 的 3 项【待测量】、阶段1 的 T3/T5 待定档、阶段2 的两处授权前置、以及上一批报告 §11 的 7 项残余（需逐条核实是否仍开放）。

**阻塞项**：无。阶段3 可立即开始。

---

## 阶段 3：验证与项目收口

### 3.1 验收标准表

**本批零实施**（用户裁定 4）⇒ `after` 列全部为「未实施」。达标线按各工单第 1 段的目标值 Y 预写，实施批直接照此验收。`before` 列全部来自本轮 §0.3 的实测，可复现。

| 工单 | 指标 | before（本轮实测） | after | 达标线 | 测量命令 | 结论 |
|---|---|---|---|---|---|---|
| **T0** | govd 自测结果 | Ran 79 tests → **FAILED (failures=3)**，rc=1 | 未实施 | **OK (79 tests)**，rc=0 | `py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v` | 待实施 |
| T0 | 普查断言 vs 实况 | 断言 140/140/133/7，实况 142/142/135/7 | 未实施 | 逐值相等 | 同上（`test_real_census`） | 待实施 |
| T0 | `ci.yml` 陈旧计数 | `:1023` 步骤名与 `:1004` 注释均写 `133` | 未实施 | `grep -n "133 light gates" .github/workflows/ci.yml` **零命中** | grep | 待实施 |
| **T1** | 混层 bench 阈值覆盖 | `scripts/` 对 `mixed7r1w`/`tplan_mixed` **零命中** | 未实施 | `tplan_mixed_parallel` 键存在于 toml 与 heredoc，I9 自报键数 **85 → 86** | `grep -rn "tplan_mixed" scripts/ .github/`；`py -3 scripts/check_perf_redlines.py invariants` | 待实施 |
| T1 | 混层 c8 median | **21.777 ms** [18.133, 22.688] | 未实施 | ≤ **55 ms**（= `ceil(21.777×2.5)`，沿用 `:115` 惯例） | `cargo bench -p gqep-executor --bench toolplan_layers --jobs 2 -- --quick --noplot` | 待实施 |
| T1 | 混层加速比 | **4.50×**（98.082/21.777） | 未实施 | ≥ **4×**（ADR-281 判据） | 同上 | **本轮已达标**（余量 12.5%） |
| T1 | 负控有牙 | 无门可验 | 未实施 | 阈值改 1 ms 后 thresholds 腿**判红** | `bash scripts/check_perf_redlines.sh` | 待实施 |
| **T2** | superposition 生产消费者 | **0** | 未实施 | 选项 B：API 不存在 / 选项 A：≥1 / 选项 C：台账有行 + 到期日 | `grep -rn "superposition_query\|SuperpositionQuery" crates/ --include=*.rs` 排除 `crates/mcp-mesh/` | 待实施 |
| T2 | fanout 并发上界 | 无独立信号量；实际上界 = `min(fanout_servers.len(), registry_capacity=256)` | 未实施 | 选项 B：不适用 / A、C：有 Semaphore 或复用 `max_participants` 钳制 | 读 `superposition.rs:171-191` | 待实施 |
| T2 | mcp-mesh 测试面 | 含 `superposition.rs` 测试段 260 行 + `mesh.rs` 两个测试 | 未实施 | 选项 B 下测试数下降，**须在 ADR-284 如实登记** | `cargo test -p mcp-mesh --no-fail-fast --jobs 2` | 待实施 |
| T2 | L0 契约门 | 6 台全 rc=0 | 未实施 | **保持 6 台全 rc=0**（mcp-mesh 是 L10，不在 L0 台账内） | VB-3 | 待实施 |
| **T3** | tract 族 debug 产物 | **347 MB / 41 文件**，占 deps **1.7%** | 未实施 | 选项「删/门控」：默认构建下 **0** | `du -cm target/debug/deps/libtract*` | 待实施 |
| T3 | nmc-encoder 编译时长差 | **【待测量】** | 未实施 | 有量化数值（无门槛，用于裁决） | `cargo build -p nmc-encoder --timings --jobs 2` 有/无 tract 各一次 | 待测量 |
| T3 | release 二进制体积增量 | **【待测量】** | 未实施 | 有量化数值 | `cargo build --release -p chimera-cli --jobs 2` + `stat -c %s` 有/无 tract 各一次 | 待测量 |
| T3 | 运行期收益 | **0**（`model_dir` 默认空、`OnnxBackend` 无生产构造点、仓库零 `.onnx`） | 未实施 | 选项「留」：必须先接线并给出端到端收益 | `grep -rn "OnnxBackend" crates/ --include=*.rs` 排除 nmc-encoder 后应零命中 | **本轮已验证为 0** |
| **T4** | 头注与实况一致性 | `:5-15` 声称「差集会静默存在」，实况 I9/I10 **PASS 85 keys** | 未实施 | 头注如实反映 I9/I10 已根修 | 人工比对 + `py -3 scripts/check_perf_redlines.py invariants` | 待实施 |
| T4 | 键集不变 | I1/I9/I10 全 PASS，85 keys | 未实施 | **逐字不变**（85 keys） | 同上 | 待实施 |
| **T5** | instant 路径 WAL 占比 | 总 **3.2532 ms**，构成未拆分 | 未实施 | 三臂对照有数值：`transaction` 27.553 / `instant` 3.2532 / `nodur` 待测 | bench 加 `durable=false` 臂后 `cargo bench -p mcp-mesh --bench mesh_benchmark -- --quick --noplot` | 待测量 |
| T5 | 模拟延迟贡献（已知项） | **24.30 ms**（27.553 − 3.2532） | 不改 | 已由 ADR-280 消除（生产走 `instant()`） | 同上 | **本轮已验证** |
| T5 | 生产 `mcp call` 端到端 p50 | **【待测量】** | 未实施 | 有数值（需 release 二进制 + 已注册 in-process server） | `hyperfine --warmup 3 --runs 20 'chimera mcp call ...'` | 待测量 |

**汇总**：21 个指标行 —— **3 行本轮已达标/已验证**（混层加速比 4.50×、运行期收益 0、模拟延迟已消除 24.30 ms）、**4 行【待测量】**（tract 编译时长、tract release 体积、WAL 占比、`mcp call` 端到端）、**14 行待实施**。

### 3.2 风险矩阵

| # | 风险 | 发生概率 | 影响程度 | 缓解措施 |
|---|---|---|---|---|
| R1 | **把「非缺陷」当缺陷改** —— 本轮已发生 3 次初判错误（`VectorIndex` 无界、`repo-wiki` unbounded 通道、双源漂移），若未复测就施工会改错公开 API 语义 / 给请求-响应通道加背压引入死锁 / 重做已有的对拍门 | **高（已实际发生 3 次，被复测拦下）** | **高** | 复测纪律制度化：子代理结论与主会话初判一律视为待证假设；每条降级项必须登记「非缺陷 + 理由 + 裁决指针」（本文 §1.2 第 14/15 项与 §1.3 即为该登记）；施工前重跑零命中/口径判定 |
| R2 | **归属判断错误导致修复方向错** —— 我本轮曾判 govd 判红是「上一批造成的」，逐 commit 追溯后纠正为 `58a7567d`（早于上批基线） | 中（本轮已发生 1 次并纠正） | 中（会误改上批 commit、误写 commit message、误导后续读者） | 归属必须用 `git merge-base --is-ancestor` + 逐 commit 数值追溯，不用记忆或推断；本文 §1.3 第 4 条已登记完整证据链 |
| R3 | **阈值门为求绿而放宽** —— T1 若把 55 ms 放大到 175 ms 以规避 CI 假红，等于伪造防回退 | 中 | **高**（门失去意义，且比无门更坏——它给了虚假的安全感） | 走 `[[excluded]]`（owner + expiry + reason，只减不增，过期即红）而非降阈值；每份工单第 4 段都有「防伪绿要求」（T1 是「改阈值到 1 ms 必须判红」） |
| R4 | **T1 混层阈值在 CI runner 上假红** —— c8 臂置信区间宽 25%，本机 21.777 ms 而 CI runner 负载特性不同 | 中 | 中（阻塞 CI 1~2 晚） | 阈值取 ×2.5 余量（55 ms，覆盖置信上界 22.688 ms 的 2.4 倍）；先在 `perf_redline.yml` 的 `continue-on-error` 观察档跑 2 晚取 CI 实测再决定是否收紧 |
| R5 | **T2 静默推翻 ADR-253 D2** —— D2 刚花成本把传输接真，删掉会让那笔投入失效，若无说明会被读成治理反复 | 中 | 中（治理争议） | ADR-284 背景段必须写明：D2 修的是「fail-open 伪造成功载荷」这一**真实缺陷**，修完才发现没有消费者；**D2 没做错**，是新事实（零消费者）导致处置变化。取舍逻辑段直接引 ADR-253 自己的判据原文「不接线等于能力空转」 |
| R6 | **T2/T3 删码破坏未捕获的消费者** —— `QueryResult` 是通用名（本仓有同名符号跨 crate 双义的既往教训），`OnnxBackend` 经 `lib.rs:68` pub use | 低（T2 已 grep 零命中；T3 已确认唯一反向依赖） | 中 | 强制 `cargo check --workspace --all-targets`（上一批教训：漏 `--all-targets` 会让 E0063 类错误延迟暴露）；T2 步骤 0 要求逐一确认 `QueryResult` 命中的是否 mcp-mesh 的那个；T3 要求对 `tokenizers`/`mel_spec`/`hound`/`image` 逐个 `cargo tree -i` 核实，不得连带删还有消费者的包 |
| R7 | **本机磁盘不足致基线或回归跑不完** | 中（D 盘余 98 GB；上一批 debug 全量矩阵曾达 ~109 GB 并触发链接器 `No space left on device`；`target/debug/deps` 现 20769 MB） | 中 | 定向基线 + `--quick`；不跑全量 debug 矩阵；委托 Linux CI；**禁止 `rm -rf target/`**（会毁 criterion 基线，导致 A/B 无法对拍）；逐 crate 定向回归实测仅 20 KB~230 MB/次 |
| R8 | **基线读数被内存压力或并发污染** | 中 | 中（会产生假过门/假否决） | 串行链跑（本轮 B2→B3→B4 全 rc=0）；分配计数类测试 `--test-threads=1`；µs 级门静默态单跑；记录同时段磁盘余量（本轮起止均 98 GB）；上一批两个 `mode_redline` 并发互撞致 `cargo exit=101` 是直接先例 |
| R9 | **CI workflow 改动未获授权** —— T0 commit B 与 T1 步骤 3 触碰 `.github/workflows/` | 中 | 中（违反本仓长期纪律） | 已在阶段2 开头显式标注：实施前必须单独取得授权，上一批的授权范围仅限该批（W0 的 `perf_redline.yml`），不自动延续；T0 已拆分为「commit A 只动 scripts/（无需授权）+ commit B 动 workflow（需授权）」，使主要收益不依赖授权 |
| R10 | **口径混用产生假数据** —— 本轮已遇到 3 例：`thresholds_ns` 键数（awk 100 vs 门自报 85）、混层加速比（5.82 vs 4.59 vs 4.50）、MCP 收益（criterion `change` 口径 −78.7% vs 双臂 A/B −88.2%） | **高（已发生 3 次）** | 中 | 计数一律以**被检对象自己的自报数**为正对照（门自报行 > 人工 awk）；性能结论必须标注口径（同轮双臂 A/B / criterion change / 阈值余量）；本文 §0.3 每张表都标了口径与时点 |
| R11 | **文档交付物自身陈旧** —— 本文引用的 file:line 会随后续 commit 漂移 | 高（必然发生） | 低—中 | 本文头部标注基线 commit `635a5651`；所有计数标注「现取命令」而非只给数字；实施批开工前须重跑 §0.2.2 的命令集核对锚点（本仓纪律：path:line 指针按三态审计——缺文件/越界/空行） |

### 3.3 未闭环项清单（诚实登记，不粉饰）

#### 3.3.1 本轮新增未闭环项

| # | 项 | 状态 / 阻塞前提 | 归属 |
|---|---|---|---|
| U1 | **RSS 峰值双口径未测**（B5） | 阻塞：需 release 二进制 + 长驻 chimera 进程；本轮 `target/release/chimera.exe` **不存在**，构建 release 全量成本高且上批有磁盘前科。按裁定 3 委托 CI。**测量方法已给全**：`cargo build --release -p chimera-cli` → `pwsh -NoProfile -File scripts/energy_profile.ps1 -DurationSec 30 -ProcessName chimera` | 阶段0 §0.3.5 |
| U2 | **tract-onnx 编译时长差与 release 体积增量未测**（T3 定档前提） | 阻塞：需临时改 `Cargo.toml` 做 A/B，属代码改动，与裁定 4「零生产代码改动」冲突 ⇒ 归实施批第一步。**测量方法已给全**（T3 步骤 1 的可复制命令块） | 工单 T3 |
| U3 | **mcp-mesh WAL 在 3.253 ms 里的占比未归因**（T5 定档前提） | 阻塞：需在 `mesh_benchmark.rs` 加 `durable=false` 对照臂，属 bench 代码改动 ⇒ 归实施批第一步。**归因公式已给全**（T5 步骤 2） | 工单 T5 |
| U4 | **生产 `mcp call` 端到端 p50 未测** | 阻塞：同 U1（需 release 二进制），另需一个已注册的 in-process server 作为调用目标 | 工单 T5 |
| U5 | **T3 / T5 无法给出最终 ROI 数值** | 阻塞：同 U2 / U3。这是本文最主要的诚实缺口 —— 6 个靶子里有 2 个只能标【待测量】，不能排序 | 阶段1 §1.1 |
| U6 | **两处 CI workflow 改动待授权** | 阻塞：T0 commit B（`ci.yml:1023,1004`）与 T1 步骤 3（`bench_check.yml` heredoc）。按本仓纪律须单独取得授权 | 阶段2 开头提示 |
| U7 | **govd 自测未接阻塞 CI** —— T0 修的是断言，但「新增门忘同步普查」这类病理**仍无机器强制**（govd 只有 non-blocking shadow） | 阻塞：是否把 govd 自测升为阻塞门属**独立裁决**（会改变 CI 阻塞面，需评估 142 门的总时长），不在 T0 范围 | 工单 T0 的治理教训段 |
| U8 | **ADR-284 / ADR-285 的「最终决策」段留空** | 阻塞：本轮零实施，决策属实施批。骨架已按固定五段格式写全，实施时填 | 工单 T2 / T3 |
| U9 | **`#[ignore]` 重测试面本轮未跑**（131 处属性行 / 登记表 79 行） | 阻塞：需 release 档 + `--test-threads=1`，成本高；已由 `slo.yml --run-ignored` 在 CI 消费，本轮不重复 | 阶段1 §1.2 第 21 项 |
| U10 | **`chimera-tui/src/data/sync.rs` 的 51.8 clone/kloc 未逐处判读** | 阻塞：该面不在任何已登记 bench 覆盖路径上，改它无可测收益 ⇒ 已定档「不值得优化」（§1.2 第 17 项）。但若未来 TUI 数据同步进入性能门覆盖范围，此密度值须重新评估 | 阶段1 §1.2 第 17 项 |
| U11 | **本文未被任何仓库文档门覆盖** —— 四道文档门实跑 rc=0，但**论域都不含本文** | 逐门核实论域：**G-72** `check_doc_pointer_integrity.py` 只判 `scripts/doc_pointer_surface.txt` 登记的 **7 份**文档（自报 `[SCAN] 7 docs`，`grep -c "agent-runtime-workorders"` = **0**）；**G-41** `check_doc_symbol_claims.py:42` 的 `DOC_DEFAULT` 是 `docs/architecture/CODE_WIKI.md`；**G-37** `check_doc_drift.sh:169,174` 硬编码 `docs/reports/DOC-AUDIT-FACTS_2026-09-20.md`；**G-05** 同理不覆盖。⇒ **不得把「四门 rc=0」读成「本文已验证」**。替代验证：`tmp/verify_workorders.py`（植入 5 类坏例、实测 **5/5 全抓到**，含根级文件越界这一初版漏抓后修复的牙口）对本文 **113 个行号点位**做三态审计 ⇒ **0 缺陷**（缺文件 0 / 越界 0 / 空白行 0）、fence 平衡 OK、表格内未转义竖线 0（修前 4 处）。**是否纳入 G-72 论域待裁决**：按该门自己的 scope rule「**谁会读它**，不按目录惯例」（`doc_pointer_surface.txt:2-4` 原文：「an agent that follows a dead anchor wastes a whole turn」）与已有的 `CHIMERA_架构减法与职责重组设计方案_v2.30.md # current batch deliverable` 先例，本文作为**要进实施队列的工单集**符合纳入条件；但纳入需同批把 `check_doc_pointer_integrity.py:194,198` 两处 "seven files" / "seven surface docs" 散文改为 eight（属**门脚本改动**，超出本批裁定 4「零代码改动」⇒ 不擅自做）。已核实本文只用三反引号 fence（12 个标记行、0 个 `~~~`），故纳入不会触发该门 `:197-200` 记载的 tilde-fence 已知边界 | 本文 §3.3.3 M4 + 阶段2 各工单的验证段 |

#### 3.3.2 继承上一批的未闭环项（逐条核实**仍开放**）

| # | 项 | 本轮核实结果 | 归属 |
|---|---|---|---|
| V1 | repo-wiki / chimera-mas doctest 工件态（E0460/E0462/E0463/E0786） | **仍开放**。构建态问题，独立工单；Linux CI 为权威口径。本轮未新增 doc 示例 | 上批报告 §11 |
| V2 | `perf_redline.yml` 实跑 job 的观察期 | **仍开放**。本轮核实 `.github/workflows/perf_redline.yml:64` 的 `continue-on-error: true` **仍在**；`:15` 注释仍写「观察期（当前状态）」；切阻塞的条件是连续 2 晚全绿无 SKIP（ADR-278 §2 D6） | 上批报告 §11 |
| V3 | RL-08（50 agent 内存峰值 ≤130 MB）豁免 | **仍开放**。本轮核实 `perf_redlines.toml:62` 的 `[[excluded]]` 行仍在，到期 **2026-12-31**；`:262-263` 载明「不是时间阈值，无法用 criterion time 断言；真实构造轨阻塞于 MAS 多 Agent 常驻进入生产（ADR-263 D3），当前为公式估算」 | 上批报告 §11 |
| V4 | ADR-249 / ADR-263 的重开条件 | **仍开放（已复评维持）**。本轮 B7 补充证据：分配棘轮实测 **6 / 68**，与上界 7 / 68 相比未上升 ⇒ ADR-263 R4-2 的触发条件①仍未满足 | 上批报告 §11 + 本文 §1.2 第 8/9 项 |
| V5 | ADR-281 的真实计划语料 | **仍开放**。无生产计划语料；若出现，按层宽分布复核（ADR-281 §6）。本轮补充：混层加速比三次读数 4.50 / 4.59 / 5.82 波动，c8 臂置信区间宽 25% ⇒ **有真实语料后须重新校准 T1 的阈值** | 上批报告 §11 + 本文 §1.3 第 7 条 |
| V6 | SubAgent `block_on` 异步化 | **仍开放**。ADR-257 D3 独立工单，本轮未产生新数据 | 上批报告 §11 + 本文 §1.2 第 19 项 |
| V7 | 轮询点 30 处（周期 loop 7 处）的门化 | **仍开放（已裁决不设门）**。ADR-264 自陈 advisory；本轮补充量级分布 2ms×1 / 10s×1 / 60s×4 / 300s×1，印证「计数是坏代理」 | 上批报告 §11 + 本文 §1.2 第 13 项 |
| V8 | TUI Eco 自动降档（R5-3） | **仍开放**。设计冻结，待真实终端三场景人工验证（ADR-264） | 上批报告 §11 |

#### 3.3.3 本轮方法论缺口（不是工程项，但必须登记）

| # | 缺口 | 说明 |
|---|---|---|
| M1 | **未做跨 crate 的 RSS 归因** | `CountingAlloc` 只计分配次数/字节数，不测 RSS（`memory-bound-audit-2026-11-08.md:49`）；本机无 `/usr/bin/time -v`、无 tracy、无 `perf record`（全仓零命中）⇒ 任务书优先级 2 的「RSS 峰值降低」在本机**只有 `energy_profile.ps1` 一条路径**，而它需要 release 二进制 + 长驻进程。这是工具面缺口，不是分析缺口 |
| M2 | **CPU 热点归因缺采样器** | flamegraph 在 Windows GNU 不可用（`perf-comparison-report.md:7`）；cargo-flamegraph 0.6.14 与 samply 0.13.1 已装但**未接线**（`toolchain-analysis-report.md:109,116`）⇒ 本轮的 CPU 结论全部建立在 criterion 的墙钟读数上，**没有指令级/函数级热点归因**。若未来要做深度 CPU 优化，接线 samply 是前置 |
| M3 | **`perf_thresholds.toml` 与 `bench_check.yml` 的 heredoc 双写面仍是人工同批** | I9/I10 保证了键集**相等**，但**值**的一致性由 `compare` 模式（`--legacy-out`）承担，而该模式**未被任何 workflow 调用** ⇒ 键集漂移会红，**值漂移不会红**。这是 T4 订正头注后**仍然存在**的真实缺口，本轮未开工单（需先确认 `compare` 模式的接线成本）。**登记为待裁决** |
| M4 | **本文自身无机器门覆盖** | 见 U11。四道文档门 rc=0 但论域均不含本文；本轮靠自建扫描器（已植入坏例证牙口）完成 113 个点位的三态审计。**这是「交付物自检」而非「门检」**，强度弱一档：扫描器只在 `tmp/`（gitignored），不进 CI，后续对本文的任何编辑都不会被自动复核 |

### 【本阶段完成度自评】

| 要求 | 完成度 | 缺口 |
|---|---|---|
| 验收标准表（before/after vs 预设达标线） | **完成**：21 个指标行，每行有 before（本轮实测，可复现）、after（如实标「未实施」）、达标线（含推导依据）、测量命令、结论 | `after` 列全为「未实施」—— 这是裁定 4 的必然结果，不是缺口。**3 行本轮已达标/已验证**、**4 行【待测量】**、**14 行待实施** |
| 风险矩阵（概率 × 影响 × 缓解） | **完成**：**11 条**，三维齐全。其中 3 条（R1/R2/R10）是**本轮实际发生过**的错误，概率标「高（已发生 N 次）」而非估计值 | 无 |
| 未闭环项清单（诚实登记，不粉饰） | **完成**：分三层 —— 本轮新增 **U1~U11**、继承上批 **V1~V8**（逐条核实仍开放并给出核实证据）、方法论缺口 **M1~M4** | 无。**M3 与 U11/M4 是本轮新发现的真实缺口**（键集有门而值无门；本文自身无门覆盖），均已登记为待裁决而非静默略过 |

**自评结论**：阶段3 达成「独立可验收」。**最主要的诚实披露**：① 6 个靶子里 2 个（T3/T5）无法给出最终 ROI 数值，因其判据数据需改代码才能测；② 任务书优先级 2 的「RSS 峰值降低」与优先级 3 的「GPU」在本机分别受工具面缺口（M1）与靶子不存在（§1.2 第 1/2 项）制约，均未产生可实施工单；③ 本轮**未做任何生产代码改动**，所有 before 值是本轮实测、所有 after 值待实施批填；④ **本文自身没有任何仓库门覆盖**（U11/M4）——四道文档门 rc=0 但论域均不含本文，验证靠自建扫描器（已植入坏例证牙口）完成，强度比门检弱一档。

---

## 契约变更清单（专项裁决，本批不实施）

按硬约束「不得破坏 L0-A/L0-B 层各 crate 之间的公开接口；确需变更的必须单独列入本清单提交专项裁决，不得直接实施」。

### 清单结论：**L0-A/L0-B 公开接口零变更需求**

本轮 6 个工单**全部不触碰 L0 层**，逐条核实：

| 工单 | 触碰的 crate | 层级 | 是否触发 L0 契约变更 |
|---|---|---|---|
| T0 | `scripts/govd/`、`.github/workflows/` | 非 crate | **否** |
| T1 | `scripts/`、`.github/workflows/` | 非 crate | **否** |
| T2 | `mcp-mesh` | **L10 Interface** | **否**（不在 L0-A/L0-B 内；G-105/G-107/G-115/G-135 均不覆盖它） |
| T3 | `nmc-encoder`、根 `Cargo.toml` | **L2 Memory** | **否**（同上）。但 `OnnxBackend`/`ModelType` 是 nmc-encoder 的 pub use（`lib.rs:68`），删除属**该 crate 的 SemVer 破坏性变更** ⇒ 须在 ADR-285 登记，**不属 L0 契约清单** |
| T4 | `scripts/` | 非 crate | **否** |
| T5 | `mcp-mesh/benches/` | bench 面 | **否** |

**机器依据**：L0 契约面由 6 台 only-decrease 门守护，全部 `expect=0`，实施批用 VB-3 逐台跑即可证明零漂移：

| 门 | 命令 | 守护对象 | 现势基线 |
|---|---|---|---|
| G-105 | `py -3 scripts/check_l0_pubfn_ratchet.py` | L0 `pub fn` 面，只减不增 | `scripts/l0_pubfn_freeze.txt` |
| G-107 | `py -3 scripts/check_l0_root_surface.py` | L0 根 re-export 面，只减不增 | `scripts/l0_root_reexport_freeze.txt` |
| G-115 / G-116 | `py -3 scripts/check_l0_extensibility_class.py`（+ `--selftest`） | L0 公共类型可扩展性分级 | **227 生产类型 = 56 open / 171 closed**（门自报；103 `pub enum` + 124 `pub struct`） |
| G-125 / G-126 | `py -3 scripts/check_l0_sublayer.py`（+ `--selftest`） | L0-A 不得指向 L0-B（`[PURITY-LEAK]`）、L0-B 不得指向层号 ≥2（`[UPWARD]`） | 门自报 `0A=7 / 0B=3`（ADR-222 D3 判据，`:58-61`） |
| G-135 | `py -3 scripts/check_pub_api_leak.py` | L0-B 公开 API 签名泄漏 | `scripts/l0b_pub_signature_freeze.txt`，门自报 `scanned_pub_fn=408` |

**L0 层构成**（`Cargo.toml:29-36`，ADR-222 D1：机器层号 0/1 **不重排**，避免 116+ 份历史 ADR 的字面引用失真）：

- **L0-A（机器层号 0，纯契约 7 crate）**：`nexus-contracts`、`nexus-contracts-core`、`nexus-contracts-sparse`、`nexus-contracts-safety`、`nexus-contracts-seams`、`nexus-contracts-mca`、`nexus-contracts-memory`
- **L0-B（机器层号 1，核心基础面 3 crate）**：`nexus-core`、`event-bus`、`nexus-telemetry`

L0 公开接口面规模（本轮现取，量纲 = 各 `src/lib.rs` 的 `pub mod` / `pub use` 行数）：`nexus-contracts` pub mod 5 / pub use **157** / 903 行，`pub mod prelude` 在 `lib.rs:773`；6 个 sibling 各 pub mod 1 / pub use 4~13 / 38~48 行且**均无 prelude**；`nexus-core` 15/22（prelude `lib.rs:90`）；`event-bus` 23/27（prelude `lib.rs:272`）；`nexus-telemetry` 4/4（无 pub prelude）。

### 变更项 CC-1：edition 2024 迁移（**唯一一项，本批不实施**）

| 段 | 内容 |
|---|---|
| **变更对象** | `Cargo.toml:94` `edition = "2021"` → `"2024"`，波及全部 **46** 个 crate |
| **为什么进本清单** | edition 是 workspace 级设置，迁移会改变 **RPIT 生命周期捕获的默认语义**，这可能使 L0-A/L0-B 的公开 `impl Trait` 签名的**有效捕获集**发生变化 —— 即「签名文本不变但语义变」，是 G-105/G-107/G-135 三台**文本快照门检测不到**的一类漂移。⇒ 必须专项裁决，不得随性能工单混批 |
| **破坏面 1：`set_var` 变 unsafe（已量化）** | 全仓 **76 处** `env::set_var`/`env::remove_var`：**生产段 16 处**（`chimera-cli/src/commands/tui.rs:372`；`chimera-tui/src/app/render.rs:757,780,787,789,799,806,813,820,822,832,839,846,853,860,862` 共 15 处）、`src/` 内联测试段 **33 处**、`tests/` 与 `benches/` 目录 **27 处**；涉及 **12 个 crate**（chimera-cli、chimera-tui、cmt-tiering、decay-engine、faae-router、hcw-window、mlc-engine、nexus-contracts、nexus-core、nmc-encoder、osa-coordinator、repo-wiki）。**硬冲突**：**46/46 crate 的 `lib.rs` 声明 `#![forbid(unsafe_code)]`**（本轮现取，量纲 = `crates/*/src/lib.rs` 计数）⇒ edition 2024 会让这 76 处全部需要 `unsafe { }` 包裹，而 `forbid` 是**不可被内层 `allow` 覆盖**的（`forbid` 语义即禁止任何降级）⇒ 必须改为 `deny` 或重构掉全部 76 处调用。**这直接违反任务书的「默认零 unsafe 规则」** |
| **破坏面 2：RPIT 生命周期捕获语义** | edition 2024 下 RPIT 默认捕获所有生命周期参数，可能过度捕获而需 `+ use<>` 收窄。**影响面无法静态量化**（需逐 crate 编译看错误）⇒ 这是「签名文本不变但语义变」的风险源，也是本项必须进契约变更清单的核心理由 |
| **破坏面 3：`#[unsafe(naked)]` 属性语法** | **本仓零影响** —— 全仓无 naked 函数（生产代码 `unsafe` 关键字命中均为字符串字面量，如 `event-bus/src/bus.rs:2191` 的 `block_reason: Some("unsafe".into())`，无 unsafe 块） |
| **与既有裁决的冲突** | `Cargo.toml:95-97` 注释原文：「MSRV floor（工程工单 C1 裁决，2026-11-08）：与 CI 钉定 `dtolnay/rust-toolchain@1.97.1` 对齐…**edition 维持 2021——2024 迁移（RPIT 捕获/unsafe 属性/set_var 语义）为独立后续项目，不入性能工单**」；commit `0e4dfc31`。⇒ 实施 CC-1 **需要一份 superseding ADR** 来推翻工程工单 C1 的这条裁决 |
| **MSRV 部分（附带裁决）** | 任务书要求 MSRV **1.95**，仓库现值 **1.97**，CI **16 处**钉定 `1.97.1`。降到 1.95 是**降低地板**：CI 仍用 1.97.1 编译，声明 1.95 不会产生任何 1.95 上的验证，且与 C1 裁决「与 CI 钉定值对齐」直接矛盾 ⇒ **建议维持 1.97**。附注：`MSRV` 与 `edition 2024` 两个词在 `docs/`、`AGENTS.md`、`CONTRIBUTING.md` **零命中**（regex 全文，排除 `target/`）⇒ C1 裁决只落在 `Cargo.toml` 注释 + commit message，**无 ADR 编号**，这本身是一个治理缺口（见下） |
| **成本 / 风险 / 收益** | 成本：**高**（46 crate 迁移 + 76 处 `set_var` 重构或 `forbid`→`deny` 降级 + 全量回归）。风险：**高**（RPIT 语义漂移无法静态量化；`forbid(unsafe_code)` 降级会削弱本仓的核心安全叙事）。收益：**无性能收益**（edition 迁移不产生墙钟改善；MSRV 降级产生负收益）。⇒ **ROI 为负，本批不实施，建议维持现状** |
| **建议裁决** | **维持 edition 2021 + MSRV 1.97**（用户裁定 2 已确认）。若未来要迁移，前置条件是：① 先给 C1 裁决补一份正式 ADR（消除「无 ADR 编号的裁决」这个治理缺口）；② 单独立项，不与任何性能工单混批；③ 先解决 76 处 `set_var` 的处置方案（重构 vs `forbid`→`deny`），并对后者做安全叙事影响的专项评审 |

### 变更项 CC-2（治理缺口，非接口变更，附记）

工程工单 C1 的 MSRV/edition 裁决**只存在于 `Cargo.toml:95-97` 注释与 commit `0e4dfc31` 的 message**，`docs/`、`AGENTS.md`、`CONTRIBUTING.md` 对 `MSRV` 与 `edition 2024` **零命中** ⇒ 该裁决无 ADR 编号、不在 `adr_index.md`。这与本仓「决策变可机检记录」的治理底座（N-4 绑定登记表 / G-87 verdict 台账）不一致。**建议**：补一份 ADR 追认 C1 裁决（成本极低，纯文档），使「为什么不迁 2024」有一个可被引用的权威出处，而非只有一条注释。**本批不实施**（裁定 4）。

---

## 附录 A：复现命令（可直接执行）

```bash
# ---- 环境（Git Bash；PATH 必须 POSIX 风格，其余保持 Windows 风格）----
cd "/d/Chimera CLI"
export PATH="/d/Chimera CLI/.toolchain/cargo/bin:/d/msys64/mingw64/bin:$PATH"
export CARGO_HOME="D:\Chimera CLI\.toolchain\cargo"
export RUSTUP_HOME="D:\Chimera CLI\.toolchain\rustup"
export TMP="D:\Chimera CLI\tmp"; export TEMP="D:\Chimera CLI\tmp"

# ---- 阶段0 §0.3 的全部实测（本轮原始输出在 tmp/wo2026-*.log）----
# B2/B3/B4：criterion 定向基线（必须串行，禁并发）
cargo bench -p gqep-executor --bench toolplan_layers     --jobs 2 -- --quick --noplot
cargo bench -p gqep-executor --bench tool_dispatch_layers --jobs 2 -- --quick --noplot
cargo bench -p gqep-executor --bench tool_runtime_batch   --jobs 2 -- --quick --noplot
cargo bench -p mcp-mesh       --bench mesh_benchmark      --jobs 2 -- --quick --noplot

# B6：tract-onnx 依赖面
cargo tree -p nmc-encoder -i tract-onnx
du -cm target/debug/deps/libtract* | tail -1          # 347 MB
du -sm target/debug/deps                              # 20769 MB → 占比 1.7%

# B7：分配棘轮（必须串行窗）
cargo test -p gqep-executor --test dispatch_alloc_budget --jobs 2 -- --test-threads=1 --nocapture

# B8：静态门与普查
py -3 scripts/check_perf_redlines.py --selftest        # PASS, all 71 assertions held
py -3 scripts/check_perf_redlines.py invariants        # PASS, I11 exec=10 excluded=1, I9/I10 85 keys
bash scripts/check_perf_redlines.sh --static-only      # VERDICT: PASS (rc=0)
py -3 scripts/idle_wakeup_audit.py                     # 生产 30 / 测试 99
py -3 -m unittest discover -s scripts/govd/tests -p "test_govd.py" -v   # FAILED (failures=3) ← T0

# ---- 阶段1 的关键零命中判定（否定式结论的检索口径）----
# GPU / 本地 LLM 依赖
for k in wgpu candle burn ort tract onnx llama gguf vulkan cuda naga ash d3d12 whisper; do
  printf '%s -> ' "$k"; grep -c "^name = \"$k\"" Cargo.lock
done
grep -rniE "wgpu|cuda|vulkan|metal|naga|ash|d3d12|cudarc" crates/ --include=*.rs | head

# superposition 零生产消费者
grep -rn "superposition_query\|SuperpositionQuery" crates/ --include=*.rs | grep -v "^crates/mcp-mesh/"

# mixed7r1w 未登记阈值
grep -rn "mixed7r1w\|tplan_mixed" scripts/

# tract-onnx 无生产构造点
grep -rn "OnnxBackend" crates/ --include=*.rs | grep -v "^crates/nmc-encoder/"
find . -name "*.onnx" -not -path "./target/*" -not -path "./.toolchain/*"
```

## 附录 B：本文引用的权威源清单

| 类别 | 权威源 |
|---|---|
| crate 数 / 版本 / edition / MSRV | `Cargo.toml`（`workspace.members` 46、`workspace.package.version` 2.29.0-omega、`:94` edition、`:98` rust-version） |
| 依赖解析后版本 | `Cargo.lock`（tract 族 7 包 `:6265-6394`） |
| 门清单与判据 | `scripts/gate_manifest.toml`（**142** 个 `[[gate]]` / light **135** / heavy **7**，本轮现取） |
| 性能阈值 | `scripts/perf_thresholds.toml`（键数以 **I9 门自报 85 keys** 为准，不用 awk 逐节计数） |
| 性能红线与执行映射 | `scripts/perf_redlines.toml`（`[[redline]]` 11 / `[[slo]]` 6 / `[[redline_exec]]` 10 / `[[excluded]]` 1，I11 门自报） |
| L0 层边界 | ADR-222（`:27-33` D1 双层口径与不重编号理由、`:58-61` D3 判据原文）+ `scripts/check_l0_sublayer.py` |
| GPU 封印 | ADR-195（`:29` D1、`:38` D3 三门、`:105` tract 更正注、`:109`「有卡不是证据」）+ G-86 + `docs/reports/gpu-unseal-readiness-2026-11.md`（`:16-46` 三门判定、`:70-72` 维持结论） |
| 并发/调度裁决 | ADR-202 / 229 / 231 / 234 / 241（`:68-76` D5）/ 249（`:98` 重开条件）/ 251 / 252 / 253（D2 + §2 取舍表 + §4 代价）/ 262 / 263（`:45` 触发条件）/ 275（`:70-71` 重开门槛）/ 277（`:108-111`，无重开条款）/ 279 / 281 / 282 / 283 |
| 内存上界 | `docs/reports/memory-bound-audit-2026-11-08.md`（`:22` 结论、`:30-31` 棘轮、`:49` CountingAlloc 口径边界） |
| 卸载可达性 | `scripts/compute_offload_reachability_freeze.txt`（`:23-31` 9 锚点）+ `scripts/sync_offload_guard_freeze.txt`（G-88） |
| 上一批执行记录 | `docs/reports/agent-runtime-deep-optimization-2026-11-10.md`（§11 未闭合 8 项，本轮逐条核实仍开放） |
| 本批计划 | `C:\Users\30324\.qoder-cn\plans\wandering-timber-pigeon.md` |
