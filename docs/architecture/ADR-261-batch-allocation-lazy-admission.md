# ADR-261: 批量路径分配精简 — 惰性装载采纳 + 三子项暂缓定界（R2）

- **状态**：Partially Accepted（2026-11-08，`feat/toolchain-parallel-optimization` R2 波次；R2-1 采纳，R2-2/R2-3/R2-4 暂缓并记录定界理由）
- **上游**：已批准方案「工具链系统性深度优化重构方案」R2 节
- **关联**：ADR-249（借用脊柱否决 ⇒ R2-2/R2-3 前提失效）/ ADR-248（BatchAttributionSink）/ ADR-241（batch.rs 先例）

## 1. 背景

批量派发路径（`dispatch_batch_with_sink`）的分配面：① backlog 对全部 N 条
请求**预先** `Box::pin`（在飞装箱峰值 = N 而非并发上界 bound；deadline 放弃
时已装箱未启动的 future 整体作废）；② 拥有型入口每请求 3 次装箱（batch 层
1 + 链头适配 1 + 叶子 owned 适配 1）；③ PlanRunner 经 `ToolExecutor::execute`
每次 2×`to_string`；④ Deny/route-miss 拒绝路径 `format!` 文案分配。

## 2. 决策

**D1 · R2-1 惰性装载（采纳）**：backlog 从 `VecDeque<BatchItemFuture>`
改为下标游标（`next_idx`），`Box::pin` 推迟到入波时（单循环「填满即 poll、
完成即补」单 push 位点，与原两阶段语义逐位等价——future 创建顺序、FIFO
补波、结果回填、双层超时全部不变）：

- 在飞装箱峰值 **N → bound**（64 请求 / bound=8 时峰值降 8×）；
- deadline 放弃时未入波请求**零装箱成本**（旧实现已为全部 N 条付费）；
- `not_started` 记账 = `total - next_idx`（从未入波 ≙ 旧「已装箱未 poll」，
  四段守恒 `succeeded+failed+abandoned+not_started==total` 不变）。

**D2 · R2-2 批量借用载体（暂缓）**：前置条件 = ADR-249 采纳借用脊柱，
已实测否决——owned 脊柱下批量路径维持每请求 3 箱。重评估触发条件同
ADR-249 §5（出现高频借用路径生产调用方，或分配占比显著上移跨过门限）。

**D3 · R2-3 PlanRunner 借用直达（暂缓）**：该项的净收益计算依赖借用
脊柱（直达 borrowed 叶子消 2×`to_string`）；owned 脊柱下
`dispatch_borrowed` 内部仍执行 `to_owned_request`（2 分配），与现状净差 0，
属横向移动而非优化。随 D2 一并重评估。

**D4 · R2-4 错误文案降分配（暂缓）**：`DispatchError` 定义在 **L0
nexus-contracts**（`spec/tool_dispatch.rs:91`）——`String → Cow<'static, str>`
属领域类型稳定性红线范围（核心契约类型变更需 ADR 且波及全仓构造/模式匹配
面与错误文案断言）；R1 后判决成本 26ns，拒绝路径 `format!` ≈100ns 非派发
主导项，为它动 L0 契约收益/风险比不足（避免过度工程化）。重评估触发：
deny/route-miss 路径成为实测热点（如零信任大批量未知工具扫描）时另立 ADR。

## 3. 语义等价证据（L1）

既有 23+ 批量行为测试**零修改全绿**：`tool_batch_sink` 4（含
`none_sink_is_bitwise_equivalent_to_legacy_path` 逐位等价 + 超时归因）/
`tool_runtime_batch_production_invariants` 9 / `bounded_concurrency` 10 /
`tool_runtime_batch_proptest` 2 / lib 内 107。超时归因语义（单操作超时计入
failed 而非 abandoned）由既有断言锁定。

## 4. L4 护栏（回归停机门,非判性能——ADR-234 用法二）

`tbatch_*` 现键实测（2026-11-08，`--noplot`，全部阈值内）：

| 键 | 阈值 | 实测 median | 判定 |
| --- | --- | --- | --- |
| tbatch_serial_zero | 1 ms | 46.4 µs | ✓ |
| tbatch_parallel_zero | 2 ms | 53.3 µs | ✓ |
| tbatch_serial_slow | 1.2 s | 989.4 ms | ✓ |
| tbatch_parallel_slow | 400 ms | 123.5 ms | ✓ |

日志 `tmp/r2_bench.log`；并行开销比 serial_ms0 +15%（编排税，ADR-247 同源口径）。

## 5. 回滚

单 commit revert（单文件单函数域）；等价测试群随改随回。
