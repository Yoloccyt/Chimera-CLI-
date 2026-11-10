# ADR-281 -- 混层写屏障分段 + 层内声明序稳定化

- 状态：Accepted（2026-11-10）
- 决策者：W3 波次
- 关联：ADR-252（只读层并发 4.6×）/ ADR-254（执行器能力表）/ ADR-277 D3（层序非稳定定序的教训）/ ADR-269（`--plan-max-steps` 只收紧）
- 影响面：`crates/gqep-executor/src/toolplan_runner.rs`、`crates/gqep-executor/tests/{toolplan_concurrency_proptest,toolplan_capability_table}.rs`、`crates/gqep-executor/benches/toolplan_layers.rs`
- 行为影响：混层（含「可能写」节点）由「整层串行」变为「读段并发 + 写节点屏障」；同层多写节点的执行顺序由**不确定的层序**变为**计划声明序**。全读层/全写层语义不变。

## 1. 背景：两个叠加的退化点

| # | 事实 | 证据 |
|---|---|---|
| 1 | 层内只要存在 `op==ToolCall && 有效副作用 != ReadOnly` 的节点，**整层**串行（层宽可达 64） | `toolplan_runner.rs` 改前的 `layer_requires_serial` → `exec_layer_serial` |
| 2 | `mcp.*` 的能力恒 `None`（`CompositeToolExecutor::capability_of` 只转发 builtin）⇒ 混入 1 个 MCP 调用即整层串行 | `crates/chimera-cli/src/mcp_tool_executor.rs:249-251`；`exec_plan.rs:70-74`（builtin 只读集） |
| 3 | `compute_layers` 的 frontier 由 `HashMap` 迭代构造 ⇒ **层内顺序不确定**；有写节点的层逐节点串行，其写序跨运行漂移 | `toolplan_runner.rs` 改前的 `compute_layers`；ADR-277 D3 已实测「两个终端同层时摘要随并发度漂移」 |
| 4 | 仓内**无生产计划语料**（`exec --plan` 的输入由用户提供；测试/bench 均为合成夹具） | `grep -rln "ToolNode\|ToolPlan" crates/` 仅 CLI/契约/runner/测试/bench；无 fixtures 目录 |
| 5 | ADR-277 只否决了 µs 级固定开销（N≤64），未覆盖「层内并行粒度」 | `ADR-277:20-35` |

第 3 点是本轮新增的**正确性**发现：同层多个写节点的执行顺序今天不可复现。第 1/2 点是性能退化。

**语料边界**：没有生产计划语料，无法用「真实层宽中位 ≥4」证明收益。本 ADR 因此把改动定位为
**能力增量 + 正确性修复**（与 ADR-252 的只读层并发同型：bench 证据 + 不变量锁），而不是
「生产流量驱动的优化」。合成夹具（8 节点层、1 写 + 7 读）是可构造的真实形态。

## 2. 决策

### D1 — 节点级「读安全」判定 + 三路分派

新增 `node_is_read_safe(node, executor)`：纯数据算子恒安全；`ToolCall` 需
`effective_side_effect == Some(ReadOnly)`（显式声明 > 能力表 > 未知=可能写，次序不变）。
`exec_layer` 按层的读安全分布分派：

- 全读 → `exec_layer_concurrent`（既有行为，零变化）；
- 全写 → `exec_layer_serial`（既有行为，零变化）；
- 混层 → `exec_layer_segmented`（新增）。

`layer_requires_serial` 保留为 `node_is_read_safe` 的取反聚合（供文档/调用方语义），
不再参与 `exec_layer` 的分派。

### D2 — 写屏障分段

`exec_layer_segmented` 按层序（见 D3）扫描：
- 连续的读安全节点构成一个**读段**，用既有 `exec_layer_concurrent` 有界并发；
- 每个可能写节点是**屏障**，用既有 `exec_layer_serial` 单节点串行执行；
- **读段与写节点不并发**；读段完成并回填 `env` 后才进入下一个写节点。

不新增第三套预算/取消/回填真值源：全部复用 `exec_layer_concurrent` / `exec_layer_serial`
与 `accumulate`（ADR-277「第二真值源 = 正确性风险」的教训）。层内无边（Kahn 分层保证），
故读节点不依赖同层写节点的输出；分段只改变**外部观察顺序的并发化**，不改变数据依赖。

### D3 — 层内按计划声明序稳定化

`compute_layers` 在构造初始 frontier 与每轮 `next` frontier 后，按
`plan.nodes` 的声明索引排序。效果：

- 层内顺序确定、可复现；
- 全写层的多写节点按**声明序**执行（此前是 `HashMap` 迭代序，跨运行漂移）；
- 只读层并发时顺序不影响结果（本来就无依赖）；
- ADR-277 D3 的摘要定序（声明序倒序取终端）不受影响。

### D4 — 契约锁收窄（显式登记）

`toolplan_capability_table.rs::partial_capability_only_parallelizes_covered_names`
原先锁「层内含未知节点 ⇒ 整层不得并行（peak=1）」。W3 后该契约收窄为：
**被覆盖的只读段并发（本夹具 peak=5），未知节点是屏障**。
断言改为 `peak == 5`，可区分三种实现：整层串行=1、无屏障全并发=8、正确分段=5。
屏障的顺序/不重叠语义由 `toolplan_concurrency_proptest.rs` 的混层不变量 5/6 锁定。

## 3. 实测（本机 Windows GNU，criterion）

### 3.1 bench（`toolplan_layers`，mock `sleep(2ms)`，sample_size 20）

| 组 | c=1 median | c=8 median | 比值 |
|---|---|---|---|
| `readonly8_ms2`（8 只读，对照） | 106.65 ms | 15.154 ms | 7.04× |
| `undeclared8_ms2`（8 未声明，能力表覆盖） | 92.652 ms | 10.030 ms | 9.24× |
| **`mixed7r1w_ms2`（7 读 + 末尾 1 显式 Write）** | **101.32 ms** | **17.41 ms** | **5.82×** |

- 混层 CI：c1 [95.49, 105.39] ms，c8 [14.68, 20.82] ms；**保守比 95.49/20.82 = 4.59×**，仍 >4× 门。
- 绝对墙钟被 Windows 定时器粒度放大（2ms 有效约 13ms），故比值略高于理论的 4×；
  门槛按比值判定，不受绝对放大影响。
- 写节点放末位是刻意的：写放中间会把层切成 3+4 两段读段，理论比值只有 ~2.7×，
  无法验证 ≥4× 门。

### 3.2 测试

```
cargo test -p gqep-executor --test toolplan_capability_table      # 5 passed
cargo test -p gqep-executor --test toolplan_concurrency_proptest  # 8 passed（4 既有 + 4 新增）
cargo test -p gqep-executor --test toolplan_terminal_aggregation  # 1 passed
cargo clippy -p gqep-executor --all-targets --jobs 2 -- -D warnings   # rc=0
rustfmt --edition 2021 --check <改动的 4 文件>                       # rc=0
```

新增混层不变量：

| 测试 | 锁住什么 |
|---|---|
| `mixed_layer_write_is_a_barrier_for_read_segments` | 写前读段并发（peak≥2）、写在中、写后读段；读写不重叠 |
| `mixed_layer_two_writes_keep_declaration_order` | 写节点按声明序、被读段隔开、peak=1 |
| `mixed_layer_summary_equivalent_across_concurrency` | c=1 与 c=8 的 `PlanSummary` 逐字段相等（`duration_ms` 除外） |
| `same_layer_writes_are_stable_across_runs` | 同层多写跨 5 次运行稳定在声明序（防 HashMap 迭代序回潮） |

## 4. 回滚

单个 commit revert：恢复 `layer_requires_serial` 的整层分派、移除 `exec_layer_segmented`
与 `compute_layers` 的声明序排序、恢复 `partial_capability_only_parallelizes_covered_names`
的 peak=1 断言。revert 后混层不变量会红 —— 那是如实反映退化点回来了。

## 5. 未采纳

| 项 | 理由 |
|---|---|
| 读段与写节点并发 | 会破坏「读节点不跨越写节点观察外部状态」的顺序契约；写序/审计链失序 |
| 把 `mcp.*` 声明为只读以换取并发 | 未知副作用必须保守（ADR-254 D1）；把未知当只读是安全回归 |
| 用层序而非声明序做写屏障 | 层序来自 `HashMap`，非稳定定序（ADR-277 D3 已实测漂移） |
| 把 `compute_layers` 改成按深度 DP 重写 | 只需在 frontier 入队后排序即可稳定；重写拓扑算法扩大风险面 |
| 在无生产语料时按「真实层宽」改判 | 语料不存在；本 ADR 以能力 + 正确性定位，bench 为证据 |

## 6. 证据边界

- **无生产计划语料**：收益证据是合成 bench（8 节点层），不是线上流量；因此本 ADR
  不声称「生产端到端 −X%」。若未来出现真实 `exec --plan` 语料，应按其层宽分布复核。
- 混层段的并发上界仍由 `PlanRunner::with_concurrency` 决定；`c=1` 时读段退化为串行，
  与旧行为逐字等价（由 `mixed_layer_summary_equivalent_across_concurrency` 锁定）。
- 同层写序由「不确定」变为「声明序」是**行为变更**（正确性修复）；依赖旧随机序的
  外部消费者不存在（仓内无该契约）。
