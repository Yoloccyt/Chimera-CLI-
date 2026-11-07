# ADR-249: 链内全借用化「反转主次」评估 — bench 实测否决（F-04 / W4）

- **状态**：Rejected（2026-11-08，`feat/toolchain-parallel-optimization` W4 收口裁决；**否决亦为正式结论**，ADR-241 D5/D6 直接先例）
- **上游**：终稿 `docs/reports/toolchain-w7r2-panorama-deepdive-2026-11-07.md` §3（W4 规格 + §3.4 静态收益上限 + §3.6 采纳判据）
- **关联**：ADR-241（D5 DashMap 同轮对照否决 / D6「净收益 0」先例）/ ADR-247（runx 8.35×）/ ADR-248（W3 归因计数器）

## 1. 背景

W4 规格（终稿 §3.3）：将四环派发链脊柱从 `Arc<dyn ToolDispatcher>`（owned）
反转为 `Arc<dyn BorrowedToolDispatcher>`（borrowed），owned 入口退化为零分配
Ref 适配器，意在消除借用路径升级分配（静态清单：借用路径 10→7 分配/派发）。
§3.6 定义唯一 L4 判性能硬门：

> 对每对 `(owned_cX, borrowed_cX)`：`borrowed_median ≤ 0.95 × owned_median`
> **且** 两组 median 95% CI 不相交；c1/c4/c8 三对**全部**满足才采纳，任一对
> 不满足 → 按否决落档并恢复原 owned 升级实现（ADR-241 D5 单点回退）。

§3.4 结论 4 诚实预判了风险：「热路径 2 个小 String 分配约占 5-15%……≥5%
median 改善是**可争但非必然**，必须以 bench 实测裁决」。

## 2. 实测裁决（2026-11-08 本机，同轮 6 测点，`--noplot`，sample_size(30)）

| 测点对 | owned median（95% CI） | borrowed median（95% CI） | 比值 | CI 关系 | 判定 |
| --- | --- | --- | --- | --- | --- |
| c1 | 403.45 µs [400.29, 407.45] | 391.71 µs [388.12, 396.31] | 0.971 | 不相交 | ✗（<5%） |
| c4 | 391.70 µs [388.91, 395.38] | 390.78 µs [388.12, 393.70] | 0.998 | 重叠 | ✗ |
| c8 | 390.57 µs [388.46, 393.11] | 389.88 µs [387.75, 392.37] | 0.998 | 重叠 | ✗ |

**三对全部不满足 ≤0.95 硬门 ⇒ 否决**。实测受益 0.2~2.9%，方向为正（c1 对
CI 不相交）但远低于门限；§3.4 结论 3 自陈的「owned 臂 -1 分配（:68 共用切换）
耗时影响远 <5%、大概率淹没在噪声里」被实测证实。原始日志：
`tmp/w4_bench3.log`；复现命令：

```bash
cargo bench -p gqep-executor --bench dispatch_borrowed_chain -- --noplot
```

### 2.1 裁决过程发现并修复的 bench harness 死锁（登记为教训）

本 bench 首版 `drive_n` 在 for 循环内逐项 `Semaphore::acquire_owned().await`
后再 push——但 `FuturesUnordered::push` **不 poll** 新推入的 future（首次
poll 发生在 `set.next().await`），conc < N=64 时第 conc+1 次 acquire 永久
阻塞（前 conc 个许可被未启动的 future 持有）——**该 bench 在本裁决前从未
成功跑完过**，表现为 warm-up 卡死、CPU=0、进程残留（本机两枚僵尸进程即为
历史会话的卡死实例）。已按 `batch.rs:241-257` 已验证的「填满即 poll、完成
即补」装载模式重写（单 push 位点循环，在飞数 ≤ bound 由循环结构保证）。

## 3. 决策

**D1 · 否决借用脊柱采纳**：恢复 owned 脊柱为生产主路径。回退以
`git checkout` 恢复 8 个脊柱文件至 W4 前状态（chain.rs / dispatch.rs /
inproc.rs / tool_runtime/mod.rs / runtime.rs / benches/tool_dispatch_layers.rs /
benches/tool_runtime_chain.rs / tests/tool_runtime_chain_proptest.rs），
`OwnedDispatcherCompat` 显式升级缝随批退役（无生产调用方）。

**D2 · 门独立收益保留（与脊柱形状无关，实测/等价锁各自成立）**：

- seccore `RulePattern::matches`/`evaluate` 泛型化 + `evaluate_ref` 命名
  入口 + `join_args` 手写单次分配 join（消 executor 侧 1 分配/派发，两路径
  共用）；等价锁 `evaluate_ref_matches_owned_verdict_bitwise`（5 程序名 ×
  4 参数形态笛卡尔积逐位一致）。
- `ExecPolicyToolExecutor` 的 N-6 能力透传接线（能力表真值源属 **ADR-254**，
  本批完成其 executor 侧接线与行为锁）：解锁「包装件单独作 PlanRunner
  executor」场景的整层保守串行（测试
  `capability_forwarding` + `runtime_layer_concurrency_unlocked_by_forwarding`
  断言在飞峰值=2）。
- 判决等价矩阵扩容 64→256 cases（`tool_runtime_borrowed_equivalence.rs`）。
- W3 的 `tool_batch_sink.rs` 4 测试（归属 ADR-248，不在本裁决范围）。

**D3 · 裁决基准保留为 dev-only**：`dispatch_borrowed_chain` bench 保留
（回退后 owned 臂 = 原生 owned 链、borrowed 臂 = 升级路径，仍可复跑），
登记 `bench_inventory_freeze.txt` dev-only 行（理由 + 本 ADR 号），**不登记
`bchain_*` 六阈值键**——W4-9 双写是采纳条件性项，否决路径按 §3.6 只要求
「记录实测数与否决理由」，不为否决实验占用 CI bench 窗与 I2 棘轮余量
（I2=1 顶格）。

**D4 · 对后续波次的影响**：R2-2（批量借用载体，前置 = 本 ADR 采纳）前提
失效，**重新定界为暂缓**——批量路径 3 箱/请求的分配面由 R2-1（惰性装载，
在飞分配峰值 N→bound）承担主要收益；若未来出现高频 `dispatch_borrowed`
生产调用方，须以新 ADR 重新过同款硬门。

## 4. 教训（写入项目记忆）

1. **分配数下降 ≠ 墙钟收益**：静态 10→7（-30% 分配数）实测仅 0.2~2.9%
   墙钟差——每次省 2 个 ≤120B 小 String（malloc ≈20-50ns）对全链 ≈6µs/请求
   的占比 <1%。性能主张必须过 L4 同轮硬门，静态走查只能用于圈定候选。
2. **FuturesUnordered harness 死锁模式**：push 不 poll ⇒ 「先取许可后 push
   的 for 循环」在 bound < N 时必死锁；正确模式 = 填满即 poll、完成即补
   （batch.rs 先例）。任何新并发 bench harness 落地时先以 c1 小 N 冒烟。
3. **门先行再施工的纠偏价值**：若先合入后裁决，本批 419 行脊柱改动将带着
   「零实测收益」的复杂度进入主干（第二 trait 入口 + 双形态夹具维护面）。

## 5. 回滚与复现

本 ADR 即回滚记录：脊柱 8 文件已恢复 W4 前状态；保留面见 D2。回滚后验证：
`cargo test -p gqep-executor -p seccore`（260 测试全绿）+
`cargo clippy -p gqep-executor -p seccore --all-targets -- -D warnings`（0 告警）。
重评估触发条件：出现 ≥1 个高频借用路径生产调用方，或 seccore 判决体优化
（ADR-260，R1）落地后分配占比显著上移使 2 次分配节省跨过 5% 门。
