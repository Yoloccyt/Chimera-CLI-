# ADR-275 -- 组合根分段计时落地；装配期并行与两处「顺手优化」正式否决（W9）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W9
- 关联：ADR-259（D9 组合根唯一入口）、ADR-234（证明力分层）、ADR-239（先实测后登记 / 收益不显著即否决）、ADR-262（线程 spawn 成本先例）
- 影响面：`crates/chimera-cli/src/composition.rs`（仅新增计时）、新增 `crates/chimera-cli/tests/composition_phase_timing.rs`
- 行为影响：**零**（只加 `Instant` 采样与一条 `tracing::debug!`）

## 1. 背景（F-12）

`assemble_ms` 只覆盖整段（实测 1.2–2.6ms），无法回答两个问题：

1. 四段彼此独立的构造（gea / gqep / sched / scc）各占多少？
2. 「装配期并行」是否值得（thread spawn ≈293µs/线程，ADR-262 先例）？

没有分段数据就引入并行，等于**在未知量级上付固定成本**。

## 2. 决策

### D1 先计时（零行为变化）

在 `composition::build` 内为 5 个构造段各取一次 `Instant`，末尾一条
`tracing::debug!(gea_ms, gqep_ms, sched_ms, scc_ms, tool_runtime_ms, "composition phase timings")`。

- 用 `debug!` 而非 `info!`：这是诊断归因面，不应污染常驻宿主的 info 日志；
- 不引入 metrics 计数器 / 事件：那会新增事件面与锁点，收益不抵成本。

### D2 装配期并行：**否决**

本机 dev 实测（`RUST_LOG=debug chimera doctor`，单次）：

```text
composition phase timings gea_ms=0.2759 gqep_ms=0.8285 sched_ms=0.0277 scc_ms=0.1744 tool_runtime_ms=0.392
startup phase: assemble assemble_ms=2.5891
```

| 段 | 耗时 |
|---|---|
| gea | 0.28 ms |
| gqep | 0.83 ms |
| sched | 0.03 ms |
| scc | 0.17 ms |
| tool_runtime | 0.39 ms |
| **四段合计** | **1.31 ms** |

- 理想并行（零开销）最多省 ~1.0ms；要付 3 次线程 spawn ≈ **0.88ms**（293µs/线程），
  净收益 ≈ 0.1ms，且引入跨线程构造（`bus.clone()` 跨线程、错误传播改写）。
- **收益不显著 ⇒ 否决并行**（ADR-239）。保留计时即可 —— 下一次若某段变重，
  分段数据会先告诉我们「是哪一段」。
- 附带结论：`assemble_ms` 2.59ms 中，四段只占 1.31ms，其余是 engine/subagent/
  critical-subscriber 等**有顺序依赖**的段（不可并行）。

### D3 两处「顺手优化」也否决

| 项 | 否决理由 |
|---|---|
| 同步 WAL open（`FileWalCriticalSink::open`）改 `spawn_blocking` | 该路径 **env 门控**（`CHIMERA_CRITICAL_WAL_PATH` 未设则完全不执行）；一次 40 字节文件打开；改异步需要把 `composition::build` 变 async，波及全部调用方（含测试）。成本 >> 收益 |
| provider 去重 O(n²) 改 `HashSet` | N = `model_router.providers` 长度（默认 ≤8，且仅 `cross_vendor_debate=true` 时才走）；n=8 时 28 次字符串比较 vs 一次 HashSet 分配 + 哈希 —— 无可测差异 |

## 3. 验收

| 判据 | 要求 | 结果 |
|---|---|---|
| `composition_phase_timing_present` | `composition.rs` 含 5 个分段字段与 debug 日志；且 **不** 含 `thread::spawn`/`join!`（防未来偷偷并行） | ✅ |
| 既有组合根测试 | 零修改绿（`check_composition_root` canonical 7 / bypass 0） | ✅ |
| `assemble_ms` | 仍在 `mod.rs` 的 info 日志中（不变） | ✅ |

## 4. 回滚

单 commit revert（撤计时 + 测试）。若未来某段显著变重，重开并行裁决的门槛是：
**分段实测显示「四段合计 ≥ 5ms 且单段 ≥ 2ms」**，否则不再重提。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 装配期 `join!` / `thread::scope` 四段并行 | 否决：净收益 ≈0.1ms，见 D2 |
| WAL open `spawn_blocking` | 否决：env 门控 + 一次小文件打开，改 async 波及全调用方 |
| 去重改 `HashSet` | 否决：n≤8，无可测差异 |
| 把分段计时改成 metrics 计数器 / 新事件 | 否决：新增事件面与锁点，收益不抵成本（且违 D9 零新增事件纪律） |
