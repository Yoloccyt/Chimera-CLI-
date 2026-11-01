# ADR-233 — 计划路径的完整调用面：审批信号、节点可追溯性与取消血缘

- **状态**：Accepted（2026-11-01 落档；实现随同一批提交）
- **相关**：ADR-229（三层倒置执行面）、ADR-231（Quest DAG 执行面）、ADR-232（LLM 工具循环与审批档）、§2.2 依赖铁律、§6.2 审批默认拒绝
- **范围**：`gqep-executor`（L7）的 `ToolRuntime` / `PlanRunner` 接缝，`chimera-cli`（L10）的 `tool plan` 装配。**不改 L0 契约**，不新增 `NexusEvent` 变体，不新增依赖边。

---

## 1. 背景与问题

ADR-229 把工具执行面建成三层倒置后，本波复测在**计划路径**上发现一条能力缺口，且它此前在结构上不可见。

`crates/gqep-executor/src/tool_plane/runtime.rs` 里 `impl ToolExecutor for ToolRuntime`（为兼容旧 `ToolExecutor` trait 而写的适配器）在每次节点执行时**自己伪造**一个调用：

| 字段 | 适配器原值 | 后果 |
|---|---|---|
| `approved` | 恒 `false` | 写类工具（`proc.exec`、MCP）在 `chimera tool plan` / `chimera quest execute` 下**永远不可达** |
| `call_id` | `compat-{tool_name}` | 同一工具的两个节点在 outcome / metrics / 日志里**不可区分** |
| `cancel` | 每节点新建 `CancellationToken::new()` | 计划级取消**传不到 provider**，进程类工具只剩 `kill_on_drop` 一道防线 |
| `origin` / `quest_id` / `task_id` | 恒 `Cli` / `None` / `None` | 归因信息在计划路径上被丢弃 |

为什么测试没抓到：所有 `run` / `run_with` 测试都把 `Mem` / `Peak` / `Fast` 这类 `impl ToolExecutor` 夹具喂给 `PlanRunner`（`crates/gqep-executor/tests/plan_runner_bounded.rs`、`benches/tool_plan.rs`），**没有一条测试用真实 `ToolProvider` 跑过 `ToolRuntime::run_plan`**。也就是说，被测的那条接缝恰好是没有人穿过的那条。

**一条必须如实记下的治理更正**：本缺口的延期**此前只写在本地 gitignored 台账**（`tmp/toolplane-baseline-2026-10-31.md` §4.1 第 6 条），三份 ADR 里没有任何一条记载它（`grep approved\|AskRequired\|审批` 在 ADR-229/231/232 中均无该缺口的记录）。"登记过"与"登记在只有本机看得见的地方"不是一回事——本 ADR 承担这次清偿，并把该缺口本身变成决策对象而非脚注。

## 2. 决策

**D1 — 上下文经"带默认实现"的新方法下发，不改 `execute` 签名。**
`ToolExecutor` 新增 `async fn execute_node(&self, ctx: &NodeExecCtx<'_>, tool: &str, args: &str)`，默认实现丢弃上下文并退回 `execute`。
**WHY**：`execute` 有 10 个实现（含全部测试夹具）。直接扩签名会把一次编排层修复扩散成 10 处无关改动，并把"legacy 路径行为不变"这个前提变成要靠人核对的承诺。默认实现让不变性由编译器保证。

**D2 — `approved` 由节点声明与显式放行位共同决定，读只节点按"无待批事项"编码。**
`approved = opts.approve_writes || node.side_effect != Some(SideEffectDecl::Write)`。
**WHY 读只节点得到 `true`**：本执行面对 `approved` 的语义是"该调用没有尚未满足的审批要求"，与 ADR-232 下 LLM 循环对**每个真正派发出去的**调用都置 `approved = true` 的既有约定一致（`tool_loop.rs` 派发前已过审批档）。把"不需要批准"编码成 `false` 会让 provider 的唯一判据（`if !inv.approved`）在读只工具上产生假拒。
**fail-closed 方向不变**：`approve_writes=false` 时写类节点仍被拒——只是拒绝原因从"适配器默认值"变成"显式审批语义"。

**D3 — `call_id = {plan_id}-{node_id}`。** 计划内唯一即全局可关联；不再有 `compat-` 前缀的坍缩。

**D4 — 取消令牌讲血缘，不新建根。**
有 caller 令牌时取其 `child_token()`（取消可下传）；没有时取 `RuntimeExecutor` 构造时那**一个** root token 的克隆。
**WHY 不每节点 `new()`**：那正是本缺口的成因；顺带说明为什么不每节点 clone root 之外再造层级——`child_token()` 实测 275.59 ns（`llm_diag_cancel_token_only`，ADR-232 §验证），比再造一个根再手工串联更便宜且语义正确。

**D5 — `chimera tool plan` 的显式放行口是全局 `--yes` / `--no-permission`。**
`approve_writes = PermissionCtx::should_skip_prompt()`。
**WHY 不做逐节点问人**：那属 ADR-232 D6 明写未接线的交互面（TUI 弹窗审批），CLI 侧在一条计划里逐节点阻塞 stdin 会把"计划"退化成"一串问答"。缺省仍 fail-closed。

**D6 — legacy `PlanRunner::run` 显式传 `approve_writes:false, cancel:None`。**
以前它根本没有这两个概念；显式写出后语义与旧行为逐位相同，`legacy_run_summary_unchanged` 一类测试继续当哨兵。

## 3. 一条现实：运行时的 Ask 门在生产装配里不可达

`ToolRuntime::invoke` 的 `PolicyAction::Ask`/`Deny` 分支整段包在 `if let Some(policy) = &self.policy` 里（`runtime.rs:118-144`），而全仓**唯一**的生产构造点 `composition.rs:328` 传的是 `policy: None`（`grep -rn "ToolRuntime::new(" --include=*.rs crates/ | grep -v tests/ | benches/` 命中 1 处）。

结论与本决策的关系：

- **今天 `inv.approved` 是唯一活着的审批信号**，真正的 fail-closed 点在 provider（`tool_plane/process.rs:75`、`tool_plane/mcp.rs:61` 都以 `if !inv.approved { AskRequired }` 开头）。
- 所以 D2 修在适配器上是**有效**的：它改的正是唯一有人读的那个字段。
- 也所以本 ADR **不宣称**"运行时统一审批已生效"。要在 L7 统一 Ask/Deny，需要先把 `ExecPolicy` 装配进 `build_tool_runtime` 并定义它与 provider 自查的优先级——那是独立决策（含"两层都拒绝时谁的理由胜出"这类问题），本波不预先接线。
- 顺带登记一个可观测性后果：`ToolStatus::Denied` / `AskRequired` 两态在生产路径上今天只能由 provider 产生，不由运行时产生。任何"按状态码统计审批拒绝"的面板，其数据来源是 provider 而非运行时。

## 4. 验证（先红后绿，全部现取可复跑）

新增 `crates/gqep-executor/tests/run_plan_invocation.rs`：**真实 `ToolRuntime` + 真实 `ToolProvider`** 跑 `run_plan`，5 例。测试里的 recording provider 显式复刻真实写 provider 的行为（`if self.write && !inv.approved → AskRequired`），因为按 §3，运行时不会替它拦。

| 例 | 改前 | 钉住的东西 |
|---|---|---|
| `provider_token_descends_from_the_callers_token` | **红**：`the provider's token is not linked to the caller's` | 取消血缘（D4）。用"跑完后取消 caller 令牌，看 provider 手里的令牌是否随之翻转"的**血缘判据**，而不是赛跑一次 mid-flight 取消——本机时钟粒度 1/64 s，赛跑版会 flaky 或什么都不断言 |
| `call_ids_are_unique_per_node_not_per_tool` | **红**：`["compat-rrec.tool", "compat-rrec.tool"]` | D3 的撞车，改前是可见的失败而非推断 |
| `write_node_denied_without_approval_and_threads_it_with` | **红**（part b：`approved` 未透传） | D2 双向：不带 `approve_writes` 时 provider 不被调用；带时 `inv.approved == true` |
| `read_only_node_needs_no_approval` | 绿（改前也绿） | 读只不需批准，且 `approved` 的编码约定不跑偏 |
| `already_cancelled_plan_does_not_succeed` | 绿（改前也绿） | 取消计划的错误形状不变 |

CLI 侧新增：`crates/chimera-cli/tests/fixtures/tool_plan_write_proc.json` + `tool_plane_e2e.rs` 两例，判据是**拒绝原因本身发生变化**——不带 `--yes` 得到 `ask_required`，带 `--yes` 得到 `denied`（进程白名单不含 `rm`，见 `command_validation.rs:121` 的白名单规则；因此不会真去 spawn）。

**WHY 用"原因变化"而不是退出码**：`plan` 走 fail-soft（`fail_fast:false`），两种情形都可能以 0 退出并只错一个词——而"从缺批准变成被策略拒"正是审批**确实透传到了 provider** 的唯一可移植证明。真启进程做正例在 Windows 上会依赖 `PATH` 与 Git 的 `echo.exe`，属可预见的 flaky 源，故留给 L7 测试面用假 provider 证明。

其余复验：`cargo test -p gqep-executor -p chimera-cli --no-fail-fast --jobs 2`；`cargo clippy --all-targets -D warnings`；`cargo fmt -p`；`bash scripts/test_fnlen_smoke.sh` rc=0（`run_with` 未越 200 行红线）；`check_tool_plane_contract.py` / `check_llm_loop_defaults.py` 各 `--selftest` 全绿 + 真树 rc=0；`check_composition_root.py` rc=0（本批未新增装配点）。

## 5. 回滚

单批 `git revert`。关键性质：`ToolExecutor::execute_node` 有**默认实现**，revert 后不存在"某实现少一个方法"的编译面外溢；`PlanRunOptions` 与 `ToolInvocation` 字段未增删，L0 契约未动，故不会牵连 ADR-229 的门（G-143/G-145 判据均不涉及本批改动的文本面）。回滚后 `chimera tool plan` 重新变成"写节点不可达"，`tests/run_plan_invocation.rs` 会转红——那是预期的、指向本决策的哨兵，不是新缺陷。

## 6. 后续（本 ADR 明确不做）

1. 把 `ExecPolicy` 真正装配进 `build_tool_runtime`（并定义它与 provider 自查的优先序与状态归因）——独立决策，见 §3。
2. 计划路径逐节点人工审批（依赖 ADR-232 D6 的 TUI 弹窗面）。
3. `origin` / `quest_id` 的真实归因：今天生产无消费方（`grep` 显示 provider 与 runtime 均不读这三个字段），先不伪造。等"按来源做指标/配额"成为需求时一并接。

---

## 追加节（2026-11-01）— W1a：`run_with` 从层屏障改为依赖就绪即派发

> 与本 ADR 前半同一波次、同一治理口，故在此追加而非另立（前半 D1–D6 讲的是"调用面完整性"，本节讲的是"调度形状"）。机器层号与 ADR 编号不重排。

### 背景

`PlanRunner::run_with`（ADR-229 交付）按 `compute_layers` 的**层**推进：一层一个 `FuturesUnordered`，排空才进下一层。层的定义是"入度清零的轮次"，所以同层节点互无依赖是**充分**条件，但它把"同层全部完成"当成了**必要**条件——一个与后继毫无关系的慢节点，会把它所在层的整层后代都按住。

实测代价（`crates/gqep-executor/benches/tool_plan.rs`，改前读数 `tmp/w1a-bench-before.log`；`chain_plus_slow_sibling` = a1→a2→a3→a4 加一个只与 a1 同层的无关 `slow`，FAST=1 ms、SLOW=60 ms）：

| 组 | 改前 mean | 改后 mean | 判读 |
|---|---|---|---|
| `tool_plan_edges/chain_plus_slow_sibling` | 110.35 ms | **62.66 ms** | −43.2%。层屏障模型预测 ≈60 + 3×15.6 ≈ 107（实测吻合），eager 模型预测 ≈max(60, 4×15.6) ≈ 62.5（实测吻合）——两个模型分别指向改前/改后，说明差异**确实**来自屏障而非噪声 |
| `tool_plan_edges/two_chains_lockstep` | 189.60 ms | 188.81 ms | 不回退。该组下界由慢链 3×60 ms 决定，快链提前完成不改变总时长——它的价值是**结构**断言（`fast_chain_finishes_while_slow_chain_is_stuck`），不是计时断言 |
| `tool_plan/serial_16` | 250.73 ms | 249.68 ms | 无变化（噪声内）；该组 `edges` 为空 ⇒ eager 与分层等价 |
| `tool_plan/parallel_16` | 31.39 ms | 31.25 ms | 同上 |

改前/改后的 `serial_16`/`parallel_16` 是同一份 bench 文件、只差 `toolplan_runner.rs` 两个版本各测一次（`tmp/w1a-bench-before-base.log` vs `tmp/w1a-bench-after.log`），不是拿旧波次读数来对比。

### 决策

**D7 — 派发单位是"依赖是否就绪"，不是"是否同层"；`run`/`compute_layers` 一行不改。**
`run_with` 消费 `build_graph`（O(V+E) 一次建表，节点以 `plan.nodes` 下标索引）；legacy `run` 继续用 `compute_layers`。
**WHY 不迁移 `run`**：`legacy_run_summary_unchanged` 之所以能当哨兵，前提是那条路径逐位不变。把两入口同时改掉，就等于用同一个改动去验证它自己。`prop_single_window_matches_legacy_run` 因此在**未迁移**的 legacy 实现与 eager 实现之间对拍——这才是独立对照。

**D8 — 提交只发生在 `commit` 一处，故提交序列是 DAG 偏序的一条线性化。**
不变式从"层 k 全部完成"换成可测的 `dispatch(n) ⇒ ∀p∈preds(n), committed(p)`，并把 `inputs_of_precomputed` 里静默吞掉缺失前驱的 `filter_map` 配上 `debug_assert_eq!(parts.len(), parents.len())`。
**WHY 这条断言是"当场检查"而非事后追溯**：集成测试跑在 debug 档 ⇒ 断言活着；一旦谁把派发条件改松，属性测试直接在该节点被派发的那一刻中止，而不需要等到输出内容对不上。M3 反事实（把 `ready` 播种成"全部节点"）实测就是这个断言先红。

**D9 — `ready` 播种按节点声明序。**
`compute_layers` 从 `HashMap` 迭代播种，层内序本来就不确定；本节把它换成 `plan.nodes` 的下标序，并在 `single_window_dispatch_is_deterministic` 里钉成 `["x","y","z"]`。
**这是一次确定性改进，不是行为保持**：同一计划同参数下，`max_parallel == 1` 的执行序从此可复现。

**D10 — 并发下的字节分配序刻意**不**规格化，精确内容断言只放在 `max_parallel == 1` 档。**
两个同时就绪的节点争 `budget - used` 是 eager 引入的自由度（层内并行今天也已是任意序）。属性测试因此只断言可断言的：`truncated` 恰在真实总字节达到预算时置位（地面真值取 `value_for` 的长度，而非运行器自报）、预算耗尽后不再提交任何节点、派发序尊重每条边、每节点恰好执行一次。

**D11 — `succ`/`remaining` 按**原始边表**建立，重复边计多次；入队条件带"此前非零"。**
按节点去重计数会让 `remaining` 与 `succ` 不同步：要么节点永不就绪（少执行），要么在多条重复出边上二次满足"为零"而**二次入队**——后者意味着副作用节点被执行两遍，是本执行面最坏的一类错误（`proc.exec` 跑两次不可回滚）。故入队判 `before > 0 && *left == 0`，并由 `duplicate_edges_do_not_dispatch_twice` + M2 反事实钉住。
**同时保持输入字节不变**：`predecessor_map` 对重复边给重复前驱，`remaining` 同步按边计数 ⇒ 节点输入的 join 内容与改前逐位相同（这是 D8 断言能成立的前提）。

**D12 — fail-fast 的在飞数量可能多于层屏障时代，暴露类别不变。**
屏障版在首个失败返回时，同层在飞节点已被排空；eager 版可能有更多节点在飞。这些 future 属于本进程、由 `tokio::select!` + drop 收敛，进程类工具由 `kill_on_drop` 兜住——与屏障版"放弃同层在飞结果"是同一处置，不新增孤儿类别。

### 一处自我更正（计划里写错了的判别形状）

计划 W1a Step 1 说 `plan_diamond`（A→{B,C}→D，B 慢 C 快）"屏障让 D 白等"。**这是错的**：D 同时依赖 B 与 C，屏障与 eager 在此形状下的**总时长**相同（层表 `[[A],[B,C],[D]]`，两版都要等 B）。真正判别的是"链 + 同层无关慢旁支"。实施时按实况落成两条互补断言：

- `fast_chain_finishes_while_slow_chain_is_stuck`——liveness：快链**整体完成**而 `b1` 仍被门住（屏障版连 `a2` 都无法派发）；
- `diamond_sink_waits_for_both_branches`——safety：D 在两条支路未齐时**不得**派发。这条正是"eager ≠ 提前派发"的反向锁。

### 验证（先红后绿 + 四条反事实）

先红读数（层屏障实现下，`tmp/w1a-red.log`）：

```
test dependent_starts_without_waiting_for_unrelated_slow_sibling ... FAILED
a2's only predecessor is committed, yet a2 never started — that is a layer barrier waiting on an unrelated node
test result: FAILED. 1 passed; 1 failed
```

改后 `crates/gqep-executor/tests/plan_runner_eager.rs` 10 例全绿（6 例证 + 3 条属性 + 1 条 halt）。四条变异各红一次（每次改 `toolplan_runner.rs`，跑完即还原并字节比对）：

| 变异 | 会红的断言 | 红在哪 |
|---|---|---|
| M1 去掉 `if counters.halted { break }` | `budget_exhaustion_halts_the_run` | 预算耗尽后仍继续提交 |
| M2 `remaining` 按节点去重 + 去掉"此前非零" | `duplicate_edges_do_not_dispatch_twice` | `each node runs once` 失败——重复边二次派发 |
| M3 `ready` 播种成全部节点 | `diamond_sink_waits_for_both_branches` | **`inputs_of_precomputed` 的 debug 断言先中止**（src:671），即 D8 的当场检查生效 |
| M4 排空整窗才补位 | `fast_chain_finishes_while_slow_chain_is_stuck` | 快链被拖到慢链之后 |

其余复验：`cargo test -p gqep-executor -p chimera-cli --no-fail-fast --jobs 2`（31 target 全绿）、`cargo clippy --all-targets -- -D warnings` rc=0、`cargo fmt -p gqep-executor -- --check` rc=0、`bash scripts/test_fnlen_smoke.sh` rc=0（无新超标）、`check_perf_redlines.py --static-only` rc=0（扩展既有 bench 文件的组不需新 freeze 行）、`check_dependency_rules.sh` rc=0、两把执行面门 `--selftest` 10/10 + 真树 rc=0。
**非绿集合不变**：`check_await_across_guard.py`（G-27）与 `check_lock_await.sh`（G-35）本批实测 rc=1，二者均在 ADR-229/232 登记的既有红集合 {G-04, G-05, G-18, G-27, G-35, G-53} 内，且失败原因指向本波未触碰的 `crates/mcp-mesh/src/quantum/participant_client.rs:556` 陈旧豁免条目——W4 要动该文件时**顺带删掉这两行过期豁免**（删除即转绿，不需新增豁免）。

### 回滚

`git revert` 本批即回到层屏障版：`run`/`compute_layers` 未动，legacy 路径不受影响；`plan_runner_eager.rs` 与 bench 的 `tool_plan_edges` 组随批回滚（bench 只扩组、未加文件 ⇒ `bench_inventory_freeze.txt` 无需改动）。回滚后 `dependent_starts_…` 与 `fast_chain_finishes_…` 转红，那是指向本决策的哨兵。
