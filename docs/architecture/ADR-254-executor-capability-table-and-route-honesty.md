# ADR-254 -- 工具副作用属性的真值源＝执行器能力表（并驱动路由表如实化）

- 状态：Accepted（2026-11-07）
- 决策者：统一总方案 X3（并行真解锁）
- 关联：ADR-239（阈值先实测后登记）、ADR-238/240/244（四环链与单一装配点）、ADR-241 D3（借用/拥有必须共用同一套链，禁两份实现）、ADR-242 §3.5（安全面不提供降级开关）、ADR-252（本批次的测量锚与波次口径）
- 影响面：`crates/gqep-executor/src/toolplan_runner.rs`、`crates/gqep-executor/src/tool_runtime/runtime.rs`、`crates/chimera-cli/src/exec_plan.rs`、`crates/gqep-executor/tests/toolplan_capability_table.rs`
- 取号说明：原拟「ADR-254/255 两支」，但 **ADR-255 已被并行的「MCP/插件/技能修复批次」占用**（盘上存在 `ADR-255-mcp-config-honesty-closure.md`），故本支合并承载两项决策（能力表 + 路由表），二者同源于「谁有资格回答『这个工具是否只读』」这一个问题。

## 1. 背景与问题

`PlanRunner` 的层内串/并发判据是（`toolplan_runner.rs:373-377`，改造前）：

```rust
layer.iter().any(|n| n.op == ToolOp::ToolCall && n.side_effect != Some(SideEffectDecl::ReadOnly))
```

而 `ToolNode.side_effect` 是 `#[serde(default)] Option<SideEffectDecl>`（`nexus-contracts/src/spec/tool_plan.rs:56`），**缺省 `None`**。
⇒「未声明」与「显式 Write」被压成同一类，整层串行。

ADR-252 已把这一压成的代价量化（`cargo bench -p gqep-executor --bench toolplan_layers`，20 samples）：

| 值组 | concurrency=1 | concurrency=8 | 并发是否生效 |
|---|---|---|---|
| `readonly8_ms2`（显式声明 `read_only`） | 69.086 ms | 14.901 ms | 是（4.6×） |
| `undeclared8_ms2`（省略字段） | 69.814 ms | 63.804 ms | **否**（≈串行） |

⇒ 生产上「所有可并行环节全部并行化」这一目标**今天对任何不带 `side_effect` 字段的计划都不成立**，而该字段是可选的、LLM 生成的计划与第三方接入计划普遍不写。
分支名就叫 `feat/toolchain-parallel-optimization`，而并行分支在默认计划形态下形同不存在 —— 这是本批要收口的主矛盾。

## 2. 决策

### D1：真值源＝执行器自身，按「计划声明 > 执行器能力表 > 未知」三级取值

`ToolExecutor` trait 增加两个**非 async、带默认实现**的方法（默认 `None` / 空集 ⇒ 所有既有实现零改动）：

- `capability_of(&self, tool_name) -> Option<SideEffectDecl>`
- `declared_tools(&self) -> Vec<String>`

判决改为 `node.side_effect.or_else(|| executor.capability_of(name))`，
两者皆无 ⇒ `None` ⇒ 仍按「可能写」保守串行。

**显式 `Write` 永远压过能力表**。WHY:若让实现方自述可以推翻计划的显式声明，
接入方就能把写操作自报为只读来换并发 —— 层内写序不再由计划决定，
`toolplan_runner.rs` 注释所述的「不可复现 / 审计链无法对齐」契约同时失效。
该优先级由 `explicit_write_overrides_capability_table` 锁死（断 `peak()==1`）。

### D2：不采用仓内现成的名字前缀表

`streaming_dispatch.rs:45-58` 的 `SideEffect::classify` 已存在，且注释自陈
「未识别默认 Write=保守」「接入方可按注册表覆盖」——看起来正合适。实测否决：
它只认 `read:` / `query:` / `search:` 前缀，对本仓生产上**仅有的三个工具**
`text.echo` / `fs.read` / `fs.list` 一律判 `Write` ⇒ 接上等于没接。
名字嗅探既非真值来源，又会把「工具命名规范」升格为安全语义的一部分。

（同时记录一项词汇重复：`SideEffect`（`streaming_dispatch.rs:38`）与 L0
`SideEffectDecl`（`tool_plan.rs:61`）是同一概念的两套枚举。本批**不新增第三种**
—— `capability_of` 直接复用 L0 的 `SideEffectDecl`；两套并存的收敛属独立议题，
未纳入本批范围。）

### D3：能力视图在构造点就地保留，不做四层转发

`exec --plan` 的 `PlanRunner.executor` 是装饰器链
`CountingExecutor → OpTimeoutExecutor → RuntimeToolExecutor → ToolRuntime → (ExecPolicyToolExecutor) → BuiltinToolExecutor`。
判决点拿到的是**最外层**，故能力信息必须能走通这条链。

选择：`ToolRuntime` 增字段 `capabilities: Option<Arc<dyn ToolExecutor>>`，
在 `new()` / `with_optional_delegate()` **包装前**就地保留原 delegate；
CLI 侧三个装饰器各加转发。
WHY 就地保留:纯转发方案要跨 4 个实现，漏任一层 ⇒ 能力表在现网**静默失效**，
而单测因为直接使用裸执行器仍然全绿 —— 正是「测试证明不了接线」的典型形态
（ADR-241 D3 禁两份实现、ADR-211 §6.4 要求指名消费者，同一类病理）。
就地保留只有 1 处可能漏，且 `capability_of` 的转发路径由
`tests/toolplan_capability_table.rs` 与 ADR-252 的 bench 值组**双重**覆盖。

已知残留（如实登记，不掩饰）:`ExecPolicyToolExecutor` 单独作为 `PlanRunner`
executor 使用时不转发能力（走默认实现 ⇒ 保守串行）。方向是 fail-closed，
故不阻断发布；若将来出现该用法需同批补转发。

### D4：路由表由 `declared_tools()` 驱动（把「永不裁决的环」如实化）

`runtime.rs` 的 `with_route(Some(HashMap::new()))` ⇒ `RouteLayer` 每次派发
都是必然失败的哈希查找 + fallback（`chain.rs:101-104`）。本批改为按执行器自报的
工具名建表，`fallback` 保留 ⇒ 未注册名仍落到 `unknown builtin tool`，
`tests/tool_runtime_wiring.rs` 的归因断言不变。

**理由不是性能**：一次空 `HashMap::get` 是 ns 级（`dchain_route_miss` 阈值 12µs，
其中大头是拒绝文案的 `format!`）。理由是结构诚实 —— 一个恒不裁决的环写进
「四环链」文档即为漂移（类比 ADR-190 反对「门失效化」）。
本项与 D1 同批，因为两者的真值源是同一个 `declared_tools()`：
一个机制两个消费者，符合 ADR-211 §6.4 的接线判据。

### D5：顺带堵住只读投影的绕过（F-A21）

`guards.readonly_only`（「Plan 模式投影：拒绝写」）旧判据只拒 `Some(Write)` ⇒
**省略 `side_effect` 字段即可让写工具在只读模式下执行**。判据改用 D1 的有效属性。
生产影响面实测为零:`readonly_only` 在 `crates/*/src` 中除定义/默认值外
无任何置 true 的调用点（grep 现取），故这是纯语义修复而非行为变更。

## 3. 验收证据（全部现跑，非引用）

| 判据 | 尺子 | 结果 |
|---|---|---|
| 未声明 + 能力表 ⇒ 真并行 | `capability_table_parallelizes_undeclared_readonly`（断在飞峰值 > 1） | ok |
| 显式 Write 压过能力表 | `explicit_write_overrides_capability_table`（断峰值 == 1） | ok |
| 未知 ⇒ 保守串行 | `unknown_capability_stays_serial_fail_closed`（断峰值 == 1） | ok |
| 能力表非全局开关 | `partial_capability_only_parallelizes_covered_names`（5/8 覆盖 ⇒ 峰值 == 1） | ok |
| 只读投影认得未声明的写 | `readonly_projection_uses_capability_table`（断 `is_err()`） | ok |
| **既有语义未被改动** | `tests/toolplan_concurrency_proptest.rs` 四条不变量**一条未改** | 4 passed / 0 failed |
| 链判决未被改动 | `tests/tool_runtime_borrowed_equivalence.rs` 5 项 | 5 passed / 0 failed |
| 端到端吞吐 | `toolplan_layers/undeclared8_ms2/{1,8}`（ADR-252 登记键，判据 c8/c1 ≥ 4×） | 见 ADR-252 §6 实测追记 |

## 4. 功能不削减的论证

- trait 方法均带默认实现 ⇒ 既有 `ToolExecutor` 实现（含全部测试夹具）**零改动**；
- 未声明且能力表无项 ⇒ 与今日逐字相同（串行）；
- 计划若显式声明，声明优先 ⇒ 任何既有计划的行为不被实现方自述改写；
- `SideEffectDecl` / `ToolPlan` / `PlanError` 等 L0 签名一字未动 ⇒ 不触 G-91/G-115/G-116；
- 既有测试零删除、proptest 断言方向未放宽；
- 路由表建表后 `fallback` 仍在 ⇒ 未知工具的错误归因与文案不变（由
  `tool_runtime_wiring.rs` 的反向断言守护）。

## 5. 回滚

两个 commit 各自独立 revert：
1. 能力表（`toolplan_runner.rs` + `exec_plan.rs` 转发 + `BuiltinToolExecutor` 实现）；
2. 路由表（`runtime.rs` 建表部分，保留 `capabilities` 字段）。

revert ① ⇒ `layer_requires_serial` 回到今日判据，`undeclared` 阈值退回 ADR-252
登记的「改前上界」160ms；revert ② ⇒ 路由表回到空表，`capabilities` 字段成为
未被路由使用的冗余视图（须同批摘除，否则 `check_dead_contract_apis` 精神不符）。
安全前置（policy / sandbox 两环）在两种状态下都完整生效 —— 本批不触碰判决顺序。

## 6. 实测追记（**待填 —— 本 ADR 落档时 bench 尚未跑完**）

本批的端到端吞吐判据是 `toolplan_layers/undeclared8_ms2/{1,8}` 的 c8/c1 ≥ 4×。
该组数字**尚未取得**：`cargo bench -p gqep-executor --bench toolplan_layers`
在撰写本节时仍在运行。

- 完整输出目标文件：`tmp/x3_bench_toolplan.log`
- 改前 estimates 快照：`tmp/x1_criterion_backup/toolplan_layers_pre_x3`
- 阈值收紧动作（160 ms → 实测 median × 2.5）**在该比值实测前不得执行**。

⇒ 在 §6 被真实读数填满并复核前，**本 ADR 的 D1 只能凭 §3 的结构性测试成立，
不能声称「吞吐已达标」**。ADR-239/234 的「先实测后登记」在此按字面执行：
未实测即不登记，宁可留白也不填推测值。
（依 ADR-239 §2.6 与 ADR-241 D6 的先例：负结果与空白都是合法记录，编造不是。）

