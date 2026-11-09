# ADR-268 -- 截断可见化与 `--plan-strict`：加性优先的失败语义收口

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W2
- 关联：ADR-239（先实测后登记 / 只收紧不放宽）、ADR-241 D3（同一事实不存两份实现）、ADR-260 D5（双重评估不消除）、ADR-261 D4（L0 `DispatchError` 不改成 Cow —— 领域类型稳定性红线）、ADR-267（W1）
- 影响面：`crates/gqep-executor/src/toolplan_runner.rs`（`PlanSummary` + `TruncationReason`）、`crates/chimera-cli/src/commands/exec.rs`、`crates/chimera-cli/src/cli.rs`、`crates/chimera-cli/src/commands/mod.rs`、新增 `crates/chimera-cli/tests/exec_plan_truncation.rs`
- 行为影响：**默认档零变化**；仅显式 `--plan-strict` 时把「截断」升级为退出码 4

## 1. 背景：截断被伪装成成功

`exec --plan` 的两处截断源都不计入失败：

- 计划级超时熔断（`PlanGuards::timeout_ms`）——`toolplan_runner.rs` 层循环 break；
- 回填预算超限（`PlanGuards::budget_bytes`）——`exec_layer` 返回 `true` 后 break。

`run()` 只把 `truncated = true`，而 CLI 的判据只有 `failed_nodes`：

```rust
status: if failed_nodes == 0 { "ok" } else { "failed" },   // exec.rs（改造前）
```

⇒ 一个「因超时/预算熔断而只跑了前几个节点」的计划，会以 `status:"ok"` + **退出码 0**
宣告成功。CI 管道据此判断「计划成功」，而实际上大部分节点从未执行。
且 `grep truncated crates/chimera-cli/tests/exec_plan_wiring.rs` = 0 命中 ——
该语义**零测试覆盖**。

（注：单操作超时**计入** `failed_nodes` 的缺陷已在 `c6eb9b62` 修复并有反向对照测试；
本 ADR 处理的是**计划级**截断，二者是不同入口，不得混谈。）

## 2. 决策

### D1：截断原因由执行器返回枚举，而非 CLI 侧反推

`PlanSummary`（**L7**，`gqep-executor/src/toolplan_runner.rs`，非 L0）加性增
`pub truncation: Option<TruncationReason>`，并新增：

```rust
pub enum TruncationReason { PlanTimeout, BudgetBytes }
impl TruncationReason { pub fn as_str(self) -> &'static str { … } }
```

WHY 枚举而不是 CLI 用 `duration_ms >= plan_ms` 反推：两个截断源的真值只有
`PlanRunner` 自己知道；反推会在「预算超限恰好发生在计划级超时之前」时误判，
把**结构事实**降级成**启发式猜测**。这与本仓反复出现的「账面对」缺陷同型。

代价评估：`PlanSummary` 只有 2 处构造点（1 生产 + 1 测试字面量），加字段成本极低；
它不在 L0，故不触 G-91/G-115/G-116 等契约门，也不涉及 ADR-261 D4 的
`DispatchError` 红线。

### D2：CLI 输出加性增字段，`status` 在默认档逐字不变

`PlanRunReport` 增：

- `truncation_reason: Option<&'static str>`（`plan_timeout` / `budget_bytes`；未截断为 null）
- `unexecuted_nodes: usize`（`plan.nodes.len() - summary.nodes_executed`，仅截断时非零）

`status` 的求值：

```rust
let strict_truncated = plan_strict && summary.truncated;
let status = if failed_nodes > 0 { "failed" }
             else if strict_truncated { "truncated" }
             else { "ok" };
```

⇒ 未开 `--plan-strict` 时 `status` 只可能是 `"ok"`/`"failed"`，与改造前**逐字一致**；
`"truncated"` 是**新增取值**，只在显式严格档出现。既有 JSON 消费者不受影响。

### D3：`--plan-strict` 默认 Off，收紧走既有 `IoError→4` 稳定映射

新增 CLI 旗标 `--plan-strict`（bool，默认 false）。开启且 `summary.truncated` 时：

- `status = "truncated"`；
- 返回 `ChimeraCliError::IoError`（**不新增公共枚举变体** —— 与 `c6eb9b62` 同一手法），
  经 Exec profile 稳定映射为**退出码 4**；
- 消息携带 `{reason}` 与 `{unexecuted_nodes}`，便于脚本定位。

WHY 默认 Off：退出码是脚本契约。把「原本 0」改成「4」属破坏性变更，
必须由调用方显式选择。默认翻转须独立裁决（见 §5）。

### D4：不做「只加字段、永不收紧」

另一种极端是只让截断在 JSON 里可见、退出码永远不变。**否决**：
CI 与 `&&` 链判定的是退出码，不是 JSON 字段；只加字段等于「把问题写进日志但不上报」。
严格档必须真实存在，只是默认不启用。

## 3. 验收（现跑）

| 判据 | 要求 |
|---|---|
| `truncated_plan_default_off_still_ok` | 默认档下**必然截断**的计划仍返回 `Ok`（退出码 0） |
| `truncated_plan_strict_on_yields_exit_4_with_reason` | 严格档返回错误、`exec_exit_code == 4`，消息含 `truncated` + `budget_bytes` + `1 node(s) not executed` |
| `non_truncated_plan_is_ok_under_strict` | 反向对照：未截断计划在严格档下照常成功 |
| `budget_truncation`（既有，已加强） | `truncation == Some(BudgetBytes)` |
| `timeout_fuse`（既有，已加强） | `truncation == Some(PlanTimeout)` |
| 今日行为锁（零修改必须全绿） | `tests/exec_plan_wiring.rs` 的 6 项 + `exec.rs` 的 2 项 + `exec_plan.rs` 的 2 项 |

**为什么前两条必须成对**：只测「默认档仍 Ok」，一个**压根不截断**的实现也会通过
（严格档同样 Ok）—— 那就是空转。第二条同时证明该计划确实被截断，
第一条的「零变化」才有意义。这是本仓「守卫条件本身要测会触发」纪律的直接应用。

### 3.1 追记（待填）

见 §6。

## 4. 回滚

`--plan-strict` 默认 Off ⇒ 运行时零差异。单 commit revert 即回：

- `cli.rs` 去掉旗标；
- `exec.rs` 恢复 `status` 二元判据与 7 参签名；
- `PlanSummary.truncation` 与枚举可**保留**（加性且无害），但若同批撤则须同步
  改回 `exec_plan.rs` 的测试字面量。

## 5. 未采纳 / 待裁决

| 项 | 结论 |
|---|---|
| 默认即严格（把截断直接判失败） | 否决：破退出码兼容，属破坏性变更 |
| CLI 用耗时/大小反推截断原因 | 否决：启发式，见 D1 |
| 改 L0 `DispatchError` 携带截断 | 否决：ADR-261 D4 领域类型稳定性红线 |
| `--plan-strict` 未来默认 On | **待独立裁决**（须评估下游脚本对退出码 0/4 的依赖；本轮不动默认） |

## 6. 实测追记（已填，bench 项见 6.1）

| 命令 | 结果 |
|---|---|
| `cargo test -p chimera-cli --test exec_plan_truncation` | **3 passed / 0 failed**（默认档 Ok / 严格档 exit 4 且含 `budget_bytes` 与 `1 node(s) not executed` / 未截断计划在严格档仍 Ok） |
| `cargo test -p gqep-executor --lib toolplan_runner` | **9 passed / 0 failed**（含 `budget_truncation` → `Some(BudgetBytes)`、`timeout_fuse` → `Some(PlanTimeout)`） |
| `cargo test -p chimera-cli --test exec_plan_wiring` | **18 passed / 0 failed**（今日行为锁，零修改） |
| `cargo test -p chimera-cli --test cli_contract_snapshot` | **3 passed / 0 failed**（新增旗标未破坏 `--help` 文本契约） |
| `cargo test -p chimera-cli --lib spec` | 6 passed（`Commands::Exec` 夹具补 `plan_strict` 后分类表不变） |
| `cargo test -p chimera-cli --lib commands::exec` | 6 passed（退出码矩阵未变） |
| `cargo clippy -p gqep-executor -p chimera-cli --all-targets -- -D warnings` | 通过 |

**过程中的一次真实失败（如实记录）**：首次 clippy 报 `E0063 missing field plan_strict`
于 `spec.rs:201/:292` —— 新增 CLI 字段后，两处 `Commands::Exec` 测试字面量必须同批补齐。
这正是「四件同批」中容易漏的一环（字段加了、夹具没跟），已在本批修好。
教训：给 clap 枚举加字段时，`grep "Commands::Exec {"` 是必做的枚举-夹具对拍。

### 6.1 `cli_parse` 同轮实测（已填）

`cargo bench -p chimera-cli --bench cli_parse`（完整输出 `tmp/w2_bench_cli_parse.log`）：

| 组 | median | 阈值 | 判定 |
|---|---|---|---|
| `cli_parse/help` | 185.93 µs | 500 µs | 未越线（2.7× 余量）；相对 base +17.6% |
| `cli_parse/run` | 168.68 µs | 500 µs | 未越线；相对 base +0.6%（p=0.90，噪声内） |
| `cli_parse/doctor` | 143.48 µs | 500 µs | 未越线；相对 base +11.3% |

**归因诚实声明**：三组相对变化方向不一致（`run` 0.6% / `doctor` 11.3% / `help` 17.6%），
且 cli_parse 的 base 同样是跨轮旧值 —— 与本 ADR-252/267 记录的 base 漂移同型。
故**不声称**「+17.6% 由新增旗标造成」，也**不声称**「无影响」；
可声称的只有：三组均**未越登记阈值**（500 µs），且 `run` 组在噪声内。
若后续要把 cli_parse 也纳入「本批是否退化」的严格判定，须按 ADR-267 §5.1 的做法
做**同代 A/B**（临时还原 `cli.rs`/`mod.rs`/`spec.rs` 再测），不得直接引用 `change`。
