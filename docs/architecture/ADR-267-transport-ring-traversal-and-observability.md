# ADR-267 -- 第四环（TransportLayer）真实进入热路径，及其可验证性边界

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W1（残余收口）
- 关联：ADR-238（强制沙箱派发链，四环主张）、ADR-240/244（生产接线与单一装配点）、ADR-241 D3（禁两份实现）、ADR-266（W0 台账波）、ADR-120/127/199（唯一派发入口，本批不新建入口）
- 影响面：`crates/gqep-executor/src/tool_runtime/chain.rs`（仅 `DispatcherBuilder::build()`）、新增 `crates/gqep-executor/tests/transport_ring_traversal.rs`
- 行为影响：生产路径**语义逐位不变**（`TransportLayer::dispatch` 是纯直转发），改变的是「第四环是否真的在链上」这一结构事实

## 1. 背景与问题

`DispatcherBuilder::build()` 原实现：

```rust
let transport: Arc<dyn ToolDispatcher> = Arc::new(TransportLayer::new(transport));
let route = Arc::new(RouteLayer::new(routes, self.fallback));
let sandbox = Arc::new(SandboxLayer::new(sandbox, route));
let head: Arc<dyn ToolDispatcher> = Arc::new(PolicyLayer::new(policy, sandbox));
Ok(SandboxedDispatcher { head, transport })
```

`TransportLayer` 只被存进 `transport` **字段**，而 `RouteLayer` 的命中臂
（`routes` 的值）与 fallback 臂都指向**未包装**的 dispatcher。生产装配下
（`runtime.rs`）`routes` 值、`fallback`、`transport` 参数三者同源为同一个 `inproc`，
因此实际派发路径是 `Policy → Sandbox → Route → inproc` —— **只有三环**，
而 ADR-238 与代码文档均称「四环」。`SandboxedDispatcher::transport()`
（`chain.rs:221-227`，doc 自陈「供诊断与上层复用同一传输实例」）**全库零调用方**。

### 1.1 为什么既有测试抓不到（关键）

`chain.rs` 的 5 处 builder 测试与 `tests/tool_runtime_chain_proptest.rs`
都把**同一个** `CountingTransport` 同时喂给 `with_route` 与 `with_transport`。
于是无论第四环是否被穿越，该 spy 的计数都相同 ⇒ 测试在结构上**不具判别力**。

## 2. 决策

### D1：在 `build()` 内把路由命中臂与 fallback 各自包进 `TransportLayer`

```rust
let wrap_transport = |target: Arc<dyn ToolDispatcher>| -> Arc<dyn ToolDispatcher> {
    Arc::new(TransportLayer::new(target))
};
let routes: HashMap<String, Arc<dyn ToolDispatcher>> = routes
    .into_iter().map(|(name, target)| (name, wrap_transport(target))).collect();
let fallback = self.fallback.map(wrap_transport);
```

- 环序不变：`Policy → Sandbox → Route → Transport`；仍是 `SandboxedDispatcher::dispatch`
  **单头**派发，**不新建第二派发入口**（ADR-120/127/199 论域未被触碰）。
- 不改 `RouteLayer`/`TransportLayer` 的类型与 trait，不改 builder 的公开签名，
  不改「四环缺一不可」的构造期契约（缺环仍返回 `DispatchError::Route("missing …")`）。
- 生产语义逐位不变：`TransportLayer::dispatch` 就是 `self.inner.dispatch(req)`，
  多一层包装只多一次 `Arc` 解引用。

### D2：**订正原方案的测试设计** —— 计数锁不具判别力，改用结构锁 + 负对照

原方案（R6 计划 W1）写的是「用两个不同 spy，断言 `B.calls == 1`」。**该设计不成立**，
理由有两层，必须如实记录而不是含糊带过：

1. `TransportLayer` 是**行为透明**的纯直转发。"是否被穿越"不改变任何可观测计数：
   修与不修两种形态下，目标 spy 的读数完全相同。
2. builder 的 `transport` 参数与 `routes` 的值在语义上是不同对象；把 route 目标包起来
   并不会让 dispatch 走到 builder 的 `transport` 参数上，故「断言 B 被调用」
   要么恒假、要么测的是另一件事。

**替代方案（已实施）**：判别力由**结构锁 + 负对照**提供 ——
`fourth_ring_wrapping_is_structural` 断言 `build()` 中存在
`(name, wrap_transport(target))` 与 `self.fallback.map(wrap_transport)`，
并断言**旧形态 `RouteLayer::new(routes, self.fallback)` 必须消失**。
该测试在修复前**实测为红**（见 §3），修复后转绿 ⇒ 判别力有实证，不是宣称。

同时保留**可观测**的行为锁：命中臂可达、fallback 臂可达、policy/sandbox 拒绝时
传输零触达。这些是真正能被行为区分的不变量。

> 教训沉淀：给「零成本包装层」写回归锁时，先问「这个改动在行为上可观测吗」。
> 若不可观测，就用结构锁/文本锁（本仓既有范式：`panorama_adjudication_locks`、
> `ledger_parity_locks`），并**必须带负对照**证明它在修复前会红；
> 否则就是给自己发一张假的验收合格证。

### D3：给 `transport()` 一个真实消费者，并诚实登记其边界

新增 `transport_handle_reaches_builder_transport_inner`：经 `dispatcher.transport()`
派发，断言到达 builder 传入的传输内核、且不经路由臂。这正是该访问器 doc 所述目的
（「供诊断与上层复用同一传输实例」）的契约化，使其不再是零调用死 API。

**诚实登记**：截至本批，它的唯一消费者是该契约测试，**生产代码仍无调用方**。
若后续波次未出现生产消费者，应在一次架构减法批中删除该访问器与字段，
而不是长期保留一个只被测试使用的公开面。本 ADR 不假装它已上生产。

### D4：不采纳「让第四环可观测」的改造

曾考虑给 `TransportLayer` 加 traversal 计数以便用行为锁验证。**否决**：
那会把零成本包装变成有成本（每派发一次原子 RMW），与 `trchain_allow_echo`
阈值棘轮冲突，且为了可测性而给热路径加开销属本末倒置。结构锁已足够。

## 3. 验收证据（现跑）

| 判据 | 修复前 | 修复后 |
|---|---|---|
| `fourth_ring_wrapping_is_structural`（结构锁 + 负对照） | **FAILED**（`build() 必须把路由命中臂目标包进 TransportLayer`） | **ok** |
| `transport_ring_traversal` 全 6 项 | 4 passed / 2 failed（另一红为测试自身夹具缺陷，见下） | **6 passed / 0 failed** |
| `cargo test -p gqep-executor --lib tool_runtime::chain` | — | **7 passed / 0 failed**（既有短路/缺环断言原样绿） |
| `tests/tool_runtime_chain_proptest`（3×64 cases） | — | **3 passed / 0 failed** |

**测试夹具自身的一处缺陷（已修，如实记录）**：`fallback_path_reaches_fallback_target`
首跑红，原因不是生产缺陷，而是我用了 `allow_policy("echo")` 却请求 `not_registered`：
无规则命中 ⇒ 落到 `ExecPolicy::new()` 的零信任默认拒绝 ⇒ 在 policy 环就被拦，
根本走不到路由。改为 `allow_policy("not_registered")` 后该测试才真正测到 fallback 臂。
这条若不修，就会把一个「测试写错」误报成「fallback 缺陷」。

### 3.1 性能与更大范围回归

见 §5 追记（同轮 bench 与全量回归在本 ADR 落档时仍在跑，**不预填数字**）。

## 4. 回滚

单 commit revert（`chain.rs` 的 `build()` + 新测试文件同批）。
revert 后结构锁会红 —— 那是**如实反映第四环又不在链上**，不是回归误报；
若需临时安静，正确做法是同时撤掉该锁并在 ADR 登记债务，而不是改松断言。

## 5. 实测追记（已填）

### 5.1 同代代码 A/B（唯一可归因的性能证据）

`cargo bench -p gqep-executor --bench tool_runtime_chain` 在同一 checkout 上
**先还原 HEAD 形态测一轮，再恢复 W1 形态测一轮**（文件按字节备份/恢复，
两次 `sha256` 均一致，确认切换无残留）：

| 形态 | median | 区间 |
|---|---|---|
| HEAD（route 两臂直挂裸 dispatcher） | **170.81 ns** | [167.11, 174.65] |
| W1（两臂各经 TransportLayer） | **166.06 ns** | [163.09, 169.09] |

⇒ 差值 −2.8%，落在噪声/代码布局量级内；**未触发**计划的「退化 >10% 即否决」判据，
**采纳本实现形态**。阈值 `trchain_allow_echo = 20000 ns` 有 ~120× 余量。

### 5.2 一条必须记录的计量教训：criterion 自报的 −66% 是假象

首次跑 bench 时 criterion 打印 `change: [-67.75% -66.21% -64.65%]`。
若照单全收，会得出「W1 提速 66%」的错误结论。真相是：
`base/` 来自**更早的代码轮次**（早于 ADR-260 的 seccore 优化），与当前形态不同代。
同一 checkout 上 HEAD 形态实测已是 170.81 ns，而非 base 的 ~563 ns。

⇒ **criterion 的 `change` 只在「base 与当前代码同代」时可归因**；
跨轮 base 漂移会把别人的优化记到本批头上。凡涉及「本批是否退化/提速」的结论，
必须在同一 checkout 做**同代 A/B**（本节做法），不得直接引用 `change`。

### 5.3 其余回归（现跑）

| 命令 | 结果 |
|---|---|
| `cargo test -p gqep-executor`（全量） | 187 passed / 0 failed / 2 ignored |
| `cargo test -p chimera-cli --test tool_runtime_wiring --test mcp_tool_executor_wiring` | 8 passed + 7 passed / 0 failed（CLI 侧归因断言未受影响） |
| `cargo clippy -p gqep-executor --all-targets -- -D warnings` | 通过（此前被 `dispatch_alloc_budget.rs` 的**既存** lint 债挡住，见 §5.4） |
| `rustfmt --check`（本批 3 个 Rust 文件） | 全部 clean（`chain.rs`/`dispatch_alloc_budget.rs` 实跑 fmt；后者 HEAD 本就 clean，只动我新增的注释块） |

### 5.4 顺带清的既存 clippy 债（独立 commit，单独归因）

`cargo clippy -p gqep-executor --all-targets -- -D warnings` 在 HEAD 即红，
三处全在 `crates/gqep-executor/tests/dispatch_alloc_budget.rs`（ADR-263 D1 的分配棘轮，
落地于 `6b30b9c8`，**与本批无关**）：
`expect()` 未 allow、两处 `usize -> usize` 冗余 cast。该文件是全仓唯一不能加
`#![forbid(unsafe_code)]` 的测试（它 `unsafe impl GlobalAlloc` 做分配计数），
故只补 `allow(unwrap_used, expect_used)` + 去冗余 cast + 把「为何不能加 forbid」写进模块文档。
它挡住的是**全 crate 的 clippy 门**，属阻塞性门债，故单列一个 commit 修复并在此登记归属。

## 6. 未采纳的替代

| 替代 | 否决理由 |
|---|---|
| 架构减法：删 `TransportLayer` 与 `transport()`，文档改称三环 | 用户已裁 A2「保留四环语义、修接线」；且删层需同批改 ADR-238 与全部四环主张，属另一决策 |
| 给 `TransportLayer` 加 traversal 计数以便行为验证 | 把零成本层变成有成本，与阈值棘轮冲突；为可测性给热路径加开销属本末倒置（D4） |
| 让 `RouteLayer` 转发到 builder 的 `transport` 环 | 会使 `routes` 的值被忽略 ⇒ 路由环变成假环（比现状更坏） |
| 用两个 spy 断言 `B.calls==1`（原方案设计） | 不具判别力，见 D2；已用结构锁 + 负对照替代并实证其修复前为红 |
