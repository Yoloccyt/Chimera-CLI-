# ADR-274 -- gatherer 惰性装载：装箱峰值绑定并发上界（W8）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W8
- 关联：ADR-261 D1（批量路径惰性装载 —— 本 ADR 把同一范式移植到 gather）、ADR-202（`FuturesUnordered` 载体红线 + I5/I6/I7 不变量）、ADR-234（证明力分层）、ADR-239（不主张无证据的收益）
- 影响面：`crates/gqep-executor/src/gatherer.rs`、新增 `crates/gqep-executor/tests/gather_lazy_admission.rs`
- 行为影响：**零行为变化**（四段守恒 / FIFO 补波 / 超时语义 / 孤儿归因全不变）；仅装箱时机与结果容器容量变化

## 1. 背景（F-11）

`gather_generic` 旧实现的装载顺序是「**先全量包装，再分批入流**」：

```rust
let wrapped: Vec<GqepFuture<T>> = futures.into_iter()
    .map(|future| Box::pin(async move { qeep.entangle(...) }))
    .collect();                       // ← N 次 Box::pin
let mut backlog: VecDeque<_> = wrapped.into_iter().collect();
```

包装本身**零副作用**（QEEP 注册与 `OrphanGuard` 创建都发生在 async 体首次 poll），
所以这 N 次装箱纯属预付：峰值装箱数是 **N**，而真正同时在飞的只有 **bound** 个。

`ADR-261 D1` 在批量路径已经采纳了「backlog 存下标、入波才 `Box::pin`」的范式；
gatherer 是同一形态的第二处，此前未被覆盖。

## 2. 决策

### D1 backlog 存**源** future，入波才包装

```rust
fn admit_future<T: Send + 'static>(qeep: QeepProtocol, future: GqepFuture<T>) -> GqepFuture<T> {
    Box::pin(async move { qeep.entangle(mapped).await ... })
}
```

- 首波与补波**共用** `admit_future`（旧实现里补波直接 `push` 已包装项 ——
  装载路径存在两处，一旦分叉无测试察觉；现收敛为单一路径）；
- backlog 元素类型仍是 `GqepFuture<T>`（源 future 本来就被调用方 boxed），
  所以 `not_started = backlog.len()` 的记账口径**逐字不变**。

### D2 结果容器按已知 `total` 定容

`values` / `errors` 由 `Vec::new()` 改为 `Vec::with_capacity(total)`。
`total` 在 gather 入口已知 —— 旧实现却让两个容器在收集过程中反复扩容（每次扩容
分配 + 搬移，峰值可短暂翻倍）。

### D3 载体仍是 `FuturesUnordered`（不换 `buffer_unordered`）

未入波项由既有 `not_started` 显式归因。换 `buffer_unordered` 会让未拉取项
**从未被 poll** 且**无任何归因出口** —— 那正是 ADR-202 否决的静默丢失。

### D4 不并发化孤儿发布

`publish_orphan_events` 保持 sequential：Critical 事件的发布顺序是审计链的一部分，
且该路径无流量压力。并发化收益为零而风险非零。

## 3. 判据与「收益」的诚实定界

| 维度 | 结论 |
|---|---|
| 行为等价 | 四段守恒、FIFO 波序、`not_started`/`abandoned` 口径、超时语义全不变（既有 10 + 4 + 6 测试零修改绿） |
| 装箱峰值 | **N → bound**（源码结构锁；N 为源 future 数，bound = `effective_bound()`） |
| 分配节省（代数） | 每批 gather 少 `N − bound` 次 `Box` 分配；另加 2 次定容（`with_capacity`） |
| 墙钟收益 | **不主张**。N 典型值 11（doctor 探针）/ ≤64（批量），`N − bound` 次装箱在总耗时中低于噪声；本机未做同血统 criterion A/B（要重编 HEAD 的 bench profile 两次，成本远超收益），故**不声称**任何百分比 |
| 内存峰值 | 装箱峰值减到 bound；容器定容消除扩容时的双份峰值 |

**关于「收益 <5% 即否决」**（方案 W8 原文）：本波**不把它当作性能优化**来主张。
按纯性能门槛严格读，`N − bound` 次装箱在墙钟上不可能过 5%，应当否决。
本 ADR 的采纳理由是**结构性**的：峰值资源绑定并发上界，且不留第二条装载路径。
若后续复核认为「未过 5% 即不得存在」，回滚方式见 §5 —— 该判断留给下一轮裁决，
本 ADR 不把它包装成性能收益。

## 4. 验收（现跑）

| 判据 | 要求 | 结果 |
|---|---|---|
| `gather_lazy_admission` | 2/2（结构锁 + 未入波零 poll） | ✅ 2 passed |
| `bounded_concurrency`（既有 I4–I8） | 10/10 零修改绿 | ✅ 10 passed |
| `gatherer_test`（全局 deadline） | 4/4 零修改绿 | ✅ 4 passed |
| `concurrent`（100 并发/孤儿） | 5/5 + 1 ignored（性能断言，既有标记） | ✅ 5 passed |
| `cargo clippy -p gqep-executor --all-targets -- -D warnings` | 0 warning | ✅ |

**判据选择说明（诚实标注，ADR-234 分层）**：装箱峰值在行为上不可观测（未 poll 的
async 体无副作用），全局分配器也无法归因（同进程 tokio/事件总线在分配）。
故采用 **L1 源码结构锁**（3 条，各防一种退化）+ **L3 可观测代理**
（未入波源 future 的 poll 计数为 0），不声称做过分配器级测量。

## 5. 回滚

单 commit revert（恢复 eager 包装块 + `Vec::new()`）。revert 后
`gather_lazy_admission::lazy_admission_is_structural` 会红 —— 那是如实反映
结构约束不在了，不是误报。

## 6. 未采纳

| 项 | 结论 |
|---|---|
| `buffer_unordered` 替代 `FuturesUnordered` | 否决：未拉取项无归因出口（ADR-202 静默丢失） |
| backlog 存下标而非源 future | 不必要：源 future 已被调用方 boxed，`VecDeque<GqepFuture<T>>` 已是惰性的最小载体 |
| 并发化 `publish_orphan_events` | 否决：Critical 审计链顺序 + 零流量压力 |
| 为 `values/errors` 用 `Vec::with_capacity(total)` 之外的自定义 arena | 否决：过度设计，N≤64 |
