# ADR-277 -- PlanRunner 固定开销正式否决；只修「聚合取拓扑终端」的正确性缺陷

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W11
- 关联：ADR-239（收益不显著即否决 / 先实测后登记）、ADR-252（`toolplan_layers` 的 4.6× 并发收益实测）、ADR-268（W2 的截断语义：截断 ≠ 聚合取错节点，两者是不同缺陷）
- 影响面：`crates/gqep-executor/src/toolplan_runner.rs`（聚合块）、新增 `crates/gqep-executor/tests/toolplan_terminal_aggregation.rs`
- 行为影响：聚合结果在「声明序末位不是拓扑终端」的计划上会**改变**（这是修复）；其余计划逐字不变

## 1. 背景：四个「优化候选」其实是 µs 级固定开销

`toolplan_runner.rs` 里有四处可被误当作性能热点的地方：

| # | 位置 | 形态 | 复杂度 |
|---|---|---|---|
| 1 | `compute_layers` | 每弹出一个节点 `plan.nodes.iter().find(id)` + 整节点 `clone()` | O(N²) |
| 2 | `inputs_of` | 每节点遍历全部 `plan.edges` | O(N·E) |
| 3 | 预算判定 | 每节点重算 `env.values().map(len).sum()` | O(N²) |
| 4 | `Semaphore` | 每层重建一次 | O(层数) |

**但 N 有硬上界**：`plan.validate(self.guards.max_steps)`，`MAX_STEPS = 64`。
⇒ 这四项的绝对成本是 **µs 级**，而 ADR-252 已实测本路径的收益来源是**层内并行**
（`readonly8_ms2` 4.6×、`undeclared8_ms2` 收紧后 ~4.4×），不是层间固定开销。

**决策：四项性能项正式否决，不实施。** 记录在此以防后续批次反复重提
（这与 ADR-241 D5 否决 DashMap、ADR-263 判 mimalloc 未过门是同一类记录）。

### 1.1 为什么「顺手优化」在这里是负收益

- 把 `compute_layers` 改成索引化会引入 `HashMap<&str, usize>`，在 N≤64 时
  哈希开销与线性查找同量级，净收益接近 0，却增加一处需要维护的索引一致性；
- 预算增量维护需要在串行/并发两条路径各维护一个计数器 —— 而这两条路径
  恰恰是「必须逐字同口径」的地方（`exec_layer_serial` / `exec_layer_concurrent`），
  引入第二个真值源是**正确性风险**换 µs 收益；
- `Semaphore` 每层重建的分配在 µs 级开销中占比可忽略，跨层复用会让
  「每层独立配额」的语义变得含糊。

## 2. 唯一施工项：聚合取**拓扑终端**（正确性）

旧实现：

```rust
let summary = match plan.nodes.last() {           // ← 声明序末位
    Some(last) => env.get(&last.id).cloned().unwrap_or_else(|| "{}".into()),
    None => "{}".into(),
};
```

**缺陷**：`plan.nodes.last()` 是**声明序**末位，不保证是拓扑终端。当末位声明节点
有后继时，它会先于后继执行 ⇒ 聚合把「中间节点」的输出当成计划输出。

修复：按**声明序倒序**找「已执行的无出边节点」——

```rust
let summary = plan
    .nodes
    .iter()
    .rev()
    .filter(|n| !plan.edges.iter().any(|e| e.from == n.id))
    .find_map(|n| env.get(n.id.as_str()).cloned())
    .unwrap_or_else(|| "{}".into());
```

### 2.1 定序为什么必须是「声明序」而不是「层序」（本轮实测推翻初稿）

初稿用的是「先求终端集，再按层序展平倒序取首个」。这条实现在
`readonly_dag_summary_equivalent`（既有不变量，零修改）上**打红**：

```
left: "{\"id\":\"t0\"}"   (c=1)
right: "{\"id\":\"t1\"}"  (c=2)
```

根因：`compute_layers` 的 frontier 由 `HashMap` 迭代构造，**层内顺序不确定**；
两个终端落在同一层时，「层序倒序首个」等价于掷硬币。也就是说层序**不是**
一个稳定定序 —— 用它做「取最后一个终端」会引入**随并发度漂移的摘要**。

声明序（`plan.nodes` 的 `Vec` 顺序）是计划本身的固有属性，与调度实现无关，
是这里唯一可用的稳定定序。选它还有两个附带好处：

- 无需物化 `HashSet<&str>` 终端集（一次线性扫描即可判定），少一次分配；
- `find_map` 直接跳过「未执行」（截断场景）的终端，天然满足「取已执行的最末终端」。

### 2.2 确定性与边界

- 多个终端时取**声明序最后**的一个 —— 稳定，且不依赖 `HashMap` 迭代顺序
  （`env` 是 HashMap，直接取 `env` 的最后一个会非确定）；
- 无节点 / 终端未执行（截断）⇒ `"{}"`，与旧实现在「末节点无结果」时的行为一致；
- 该修复**不改变**执行顺序、不改变失败语义、不改变 `truncated` 语义
  （后者是 W2 的范围，两者是不同缺陷）。

## 3. 验收（现跑）

| 判据 | 要求 |
|---|---|
| `plan_summary_uses_topological_terminal` | 计划声明序末位 c 有后继（c→b）⇒ 摘要必须取 b 的输出；旧实现会取到 c |
| `toolplan_concurrency_proptest` 四条不变量 | 零修改全绿（聚合改动不得扰动并发/串行语义）；**其中 `readonly_dag_summary_equivalent` 是层序初稿的负对照** |
| `cargo clippy -p gqep-executor --all-targets -- -D warnings` | 通过 |

## 4. 回滚

单 commit revert。revert 后 `plan_summary_uses_topological_terminal` 会红 ——
那是**如实反映缺陷回来了**，不是误报；若需临时安静，正确做法是同时撤测试并登记债务。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| `compute_layers` 索引化 / `inputs_of` 预建邻接表 | 否决：N≤64，µs 级；引入索引一致性维护成本 |
| 预算增量计数器 | 否决：串行/并发两路径需各维护一份，第二真值源是正确性风险 |
| `Semaphore` 跨层复用 | 否决：稀释「每层独立配额」语义，收益可忽略 |
| 用 `env.values().last()` 兜底 | 否决：`HashMap` 迭代顺序不确定 ⇒ 非确定性摘要 |

## 6. 实测追记（2026-11-09 现跑）

```
cargo test -p gqep-executor --test toolplan_terminal_aggregation
  test result: ok. 1 passed; 0 failed

cargo test -p gqep-executor --test toolplan_concurrency_proptest
  test result: ok. 4 passed; 0 failed; finished in 4.75s

cargo clippy -p gqep-executor --all-targets -- -D warnings
  Finished `dev` profile ... (0 warnings)
```

**同期推翻记录**（保留以便复现，不删）：

| 序 | 实现形态 | 结果 |
|---|---|---|
| 1 | `plan.nodes.last()`（旧） | `plan_summary_uses_topological_terminal` 红（取到中间节点 c） |
| 2 | 终端集 + **层序**倒序首个 | 新测试绿，但 `readonly_dag_summary_equivalent` **红**（c=1 得 t0 / c=2 得 t1）—— 层序非稳定定序 |
| 3 | **声明序**倒序首个（最终） | 新测试绿 + 四条不变量绿 + clippy 绿 |

**方法论教训**：当「取最后一个」被用在由 `HashMap` 迭代派生的容器上时，必须问
「这个顺序是否与调度实现无关」。本轮初稿把 `compute_layers` 的层序当成执行序，
而它只是**分层结果**，层内无序；既有不变量测试恰好覆盖了这条边界（多终端同层），
所以缺陷在提交前被自己的回归网捕获 —— 这也说明「新增测试绿」不能替代「既有不变量红/绿」。
