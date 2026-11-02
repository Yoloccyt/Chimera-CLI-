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

---

## 追加节二（2026-11-01）— W1b-1：每次调用的堆分配（描述符、许可、保序）

### 读数（同一份 bench 文件，只差 `tool_plane/{registry,scheduler,runtime}.rs` 两个版本；`--profile bench-lite`，mean）

| 组 | 改前 | 改后 | 差 |
|---|---|---|---|
| `llm_diag_resolve_hit` | 134.80 ns | **21.48 ns** | **−84.1%** |
| `llm_diag_resolve_miss` | 14.70 ns | 13.39 ns | 噪声内（未改语义，用作标定） |
| `llm_diag_scheduler_acquire` | 509.17 ns | **474.06 ns** | −6.9% |
| `llm_diag_invoke_batch_8` | 11.815 µs | **10.730 µs** | −9.2%（三项改动叠加后的整批面） |

改前 `hit`（134.80）与 `miss`（14.70）之差 ≈ 120 ns 就是那四次 `String` 分配本身；改后 `hit` 落到 21.48 ns，与 `miss` 只差一次 `Arc` 计数——**描述符分配已被消掉，不是被挪走**。读数文件：`tmp/w1b1-bench-before-full.log` / `tmp/w1b1-bench-after.log`。

### 决策

**D13 — `ToolRegistry` 内部存 `Arc<ToolDescriptor>`，`resolve` 返回 `Arc<ToolDescriptor>`；`descriptors()` 仍返回 `Vec<ToolDescriptor>`。**
把唯一的拷贝从"每次调用"移到"注册时"（注册是装配期，一次一工具）。`descriptors()` 保持按值返回，是因为它的消费方（`chimera tool list`、`run.rs` 的启动打印、契约测试）只想打印表格——在那里返回 `Arc` 会把"是否共享"这个决定推给每个调用方。
**WHY 只改 `resolve` 不改 `descriptors()`**：两个函数的调用频度差 3~4 个数量级，把它们捆在一起改会让冷路径背上热路径的抽象。

**D14 — 回归锁用"缓冲区指针同一"而不是 `Arc::ptr_eq`。**
`resolve_returns_the_same_descriptor_allocation` 断言 `desc_a.name.as_ptr() == desc_b.name.as_ptr()`（外加 `description` 一处），并以 `Arc::ptr_eq(&provider_a, &provider_b)` 作**对照**（这条改动前就成立，用来证明测试并非空洞地红）。
**WHY 不用 `Arc::ptr_eq(&a.1, &b.1)`**：计划原本这么写，但它在按值返回的实现上**根本编译不过**——那就拿不到"先红"读数，只能拿到一条编译错误。缓冲区身份这条写法在改前改后都编译、含义等价（`String::clone` 必分配，故共享 `name` 缓冲区只能意味着共享描述符），且**能在它所要监督的那次 API 变更两侧都存活**。先红读数（两个不同地址）：`tmp/w1b1-red.log`，`left: 0x2528a0205a0 / right: 0x2528a020980`。

**D15 — `provider_semaphore` 先 `get` 命中即返回，未命中才 `entry(provider.to_string())`。**
`entry` 需要 owned key ⇒ 旧写法在**每一次** acquire 上都分配一个 `String` 并做两次哈希，即使该 provider 道次早已存在。键类型不动（仍 `String`），避免为此引入第二套 provider id 体系。锁内仍只做 map 操作，不跨 `.await`（§4.4 #1）。

**D16 — `invoke_batch` 保序改为"按派发下标写槽位"，并加一条"结果不得丢"的 debug 断言。**
`push Vec<(usize, _)> + sort_by_key + map().collect()` 是 O(n log n) 加一次额外搬运；`slots[index] = Some(outcome)` 是 O(n) 且只一次 `flatten().collect()`。
**WHY 替换是安全的**：保序契约由 `prop_batch_order_survives_reversed_completions` 守着，该属性测试在第一波已实证"把排序键改坏 ⇒ 例证面 14 项全绿、属性面红"（ADR-229 §4 那行机证记的就是这条分界）。换实现不动保证。
**顺带堵一个静默失败面**：槽位写法若因 bug 漏掉某个 index，`flatten()` 会**缩短**结果而不报错——那正是本项目红线"结果丢了？5.4% 孤儿调用"的形状。故加 `debug_assert_eq!(outcomes.len(), total)`：宁可当场中止，不可少一条还判绿。

### 验证与回滚

`cargo test -p gqep-executor -p chimera-cli --no-fail-fast --jobs 2` 全绿（31 target，含 `tool_plane_proptest` 3/3——它是本批"不削减"的正面证据）；`clippy --all-targets -D warnings`、`fmt --check`、`test_fnlen_smoke.sh`（无新超标）、`check_tool_plane_contract.py` 与 `check_perf_redlines.py --static-only` 均 rc=0。bench 文件只**扩组**（`llm_diag_resolve_hit`/`_miss`），未增删文件 ⇒ `bench_inventory_freeze.txt` 无需新行（按文件分类）。
回滚 `git revert` 本批：`resolve` 返回类型回到按值，D14 的断言随之转红（指向本决策的哨兵）；`commands/tool.rs` 与 `runtime.rs` 的调用点只做字段访问，`Arc<ToolDescriptor>` 自动 deref，故回滚不牵连别处。

---

## 追加节三（2026-11-01）— W1b-2：请求体常驻，每轮不再深拷整段历史

### 读数（每组两侧各两次采样，同一会话内交替取；`--profile bench-lite`，mean）

| 组 | 改前两次 | 改后两次 | 判定 |
|---|---|---|---|
| `llm_loop_16_tools` | 13.277 / 13.943 µs | 9.423 / 9.112 µs | **−31.9%** |
| `llm_loop_16_tools_window8` | 27.211 / 26.499 µs | 22.066 / 21.105 µs | **−19.6%** |
| `llm_loop_16_tools_notools` | 4.4205 / 4.4148 µs | 2.4666 / 2.4498 µs | **−44.4%** |
| `llm_diag_request_rebuild/history_26` | 4.4998 / 4.7582 µs | 4.8128 / 4.6138 µs | **噪声带：±7%（该组两侧量的是同一段代码）** |

**为什么这张表必须带最后一行**：`llm_diag_request_rebuild` 在改前/改后**都没有被改动**（它是 bench 里显式写的一次 `messages.clone() + decls.clone()`），所以它两侧之差就是本会话的漂移地板——实测 4.50~4.81 µs，约 ±7%。上面三个 loop 组的降幅是 20~44%，落在漂移带的 3~6 倍处，且方向与改动预测一致。没有这一行，"−44%" 只是一个无法否证的数字。
读数文件：`tmp/w1b2-bench-before.log`、`tmp/w1b2-bench-before2.log`、`tmp/w1b2-bench-after.log`、`tmp/w1b2-bench-after2.log`。

### 决策

**D17 — 请求体常驻 + 每轮原地追加历史，`LlmClient::complete` 签名一行不改。**
计划初稿写的是"把 `complete(request)` 改成借用"。**那个前提是错的**：`client.rs:29` 早就是 `&AffinityRequest`，全部实现与调用点都是借用（这是设计代理的一条反驳，复测后采纳，见本波计划"更正 #4"）。真正的成本源不是签名，而是循环每轮构造一个新 request。故本批改的是**生命期**：`intent_id` / `tools` / 静态档位一次填好，每轮 `request.messages.push(...)`。
`decls.clone()` 与 `messages.clone()` 两处由此从源码里消失（`grep -c 'messages: messages.clone()' = 0`），而不是被"优化掉"。

**D18 — 响应侧仍拷一次，且这条守恒被测试钉住。**
`request.messages.push(AffinityMessage { role: Assistant, blocks: response.blocks.clone() })` 的 clone **保留**：`response` 之后还要交给 `step_of` 判定，属"响应拥有者未变"的必要拷贝，不是本批的目标。
反事实证明历史增长这件事没有因为改造而失去保护：
- N1 不追加 tool 结果 ⇒ `loop_executes_tool_and_feeds_result_back`、`loop_denied_write_is_not_executed`、`loop_unknown_tool_feeds_error_to_model` **三条红**；
- N2 不追加 assistant ⇒ `loop_preserves_thinking_blocks` **红**。

**D19 — 不写"request 未被重建"的结构性断言。**
指针身份类断言（如比对每轮 `&request` 或 `tools.as_ptr()`）在旧实现下**可能偶然成立**：上一轮的 request 在轮末析构，其分配被下一个 `clone()` 复用是 glibc/Windows heap 的常态行为。那样的断言是假绿锁（本项目已有教训：恒真/恒假的守卫条件比没有守卫更糟）。这条主张由**bench A/B + 漂移带**承担，而行为面由 D18 的两条反事实承担——两者合起来才是可否证的。

**D20 — 每个 chunk 不再二次 clone invocation：审批阶段就把 invocation 留在自己的槽位上。**
旧写法 `approved: Vec<(usize, ToolInvocation)>` + `group.iter().map(|(_, inv)| inv.clone())` 会为每个窗口把整批调用再拷一遍（每条 4 个 String）。改成 `invocations: Vec<Option<ToolInvocation>>` + `approved: Vec<usize>`，派发时 `take()` **交出所有权**。
顺带把保序断言的比对基准从"invocation 里的 call_id 副本"换成**原始 `tool_use` id**（`calls[slot].0`）——invocation 已被 move，正好无拷可比，而且比的是更接近来源的那一份。另加 `debug_assert_eq!(executed.len(), group.len())`：槽位写法若少收一条结果，`zip` 会静默截断（"结果丢了 5.4%"那条红线的形状），宁可当场中止。

### 验证与回滚

`llm_tool_loop` 15 例 + `llm_off_legacy` 6 例全绿（未开开关时旧路径逐位不变这条前提仍成立）；`gqep-executor + chimera-cli` 31 target 全绿；`check_llm_loop_defaults.py` 真树 rc=0（四个上限 env 仍钉、事件集仍 ⊆ `{BudgetExceeded}`、`src/llm/**` 仍零 `unwrap/expect`）；`clippy --all-targets -D warnings` rc=0；`fmt --check` rc=0。
`chunks(window)` **保留**：窗口是绑定的界（`llm_tool_loop.rs` 在 L7 配 16/16/16 时断言 `peak ≤ 8` 且 `peak ≥ 2`），删它等于削能力，理由见本 ADR「W5 明确不做」。
回滚 `git revert` 本批：`dispatch` 的槽位改回 `(usize, ToolInvocation)` vec 即可，两处 `debug_assert` 随批消失；请求体回到每轮新建，`llm_diag_*` 读数回到 13.3/27.2/4.4 µs 一档。

---

## 追加节四（2026-11-01）— W1c：工具结果按 `call_id` 归位 + 门补钉并发窗口

### 缺陷形状（改前）

`dispatch` 用 `group.iter().zip(executed)` **逐位**回填，唯一防线是 `debug_assert_eq!(result.call_id, invocation.call_id)`。`debug_assertions` 在 release 档是关闭的——而 release 恰是 `bench_check.yml`、`slo.yml` 与发布构建所在的档位。于是在最需要它的构建里，一次 L7 保序破口会静默变成"把 A 的结果写进 B 的槽位"，回填给模型的 `tool_result` 还带着另一个调用的 `tool_use_id`：模型收到的是一个从未发起过的配对。少回一条时更糟，`zip` 直接截断，那个槽位**根本没有结果**，历史里 `tool_use` 与 `tool_result` 数量不齐。

先红读数（把归位实现退回逐位版、同一组测试，`tmp/w1c-red.log`）：

```
results_are_keyed_by_call_id_not_by_position ... FAILED
  left: [("c","third"), ("b","second"), ("a","first")]
 right: [("a","first"), ("b","second"), ("c","third")]
missing_result_becomes_visible_error_not_a_silent_hole ... FAILED   （槽位为空）
result_with_unknown_call_id_is_not_placed_anywhere ... FAILED
  left: "x"  right: "a"
```

### 决策

**D21 — 归位改键化：`HashMap<&str, Vec<&ToolOutcome>>`，每槽位从自己的 `tool_use_id` 取一条。**
不匹配 ⇒ `warn!` + 合成 `error_result(id, "attribution_mismatch")`：**不 panic、不 unwrap**（`src/llm/**` 的零 panic 面是 G-145 L4 的判据，用 `expect` 会直接把门做红），失败可见而批次不炸。多余结果（批外 id）另有 `consumed != executed.len()` 的 `warn!`，不假装塞得下。
同一 `call_id` 的多条结果逐条取用 ⇒ 一条都不丢（模型侧用同一个 `tool_use_id` 发两次调用是本层不去重的真实情形）。

**D22 — 新增标记 `attribution_mismatch`，不复用 `ask_required`。**
两者语义不同：一个是审批决策，一个是执行面契约破口。混用会把"被拒了几次"和"归位坏了几次"读成同一件事。

**D23 — 档位依赖用结构断言锁，而不是"在 release 档再跑一次"。**
`attribution_behavior_is_not_debug_only` 用 `include_str!` 读自身源码，断言 `attribute_results` 函数体内**不含** `debug_assert`，且含运行时的 `ATTRIBUTION_MISMATCH_MARKER`。
**WHY 不选 release 跑一遍**：那只能证明"没失败"，证明不了**为什么**与档位无关；而"体内没有 debug_assert"是可直接机检的结构事实，且谁在归位路径上加回 `debug_assert` 会立刻红。实测有牙：植入一条 `debug_assert!(true)` ⇒ 仅此例转红（已还原并字节比对）。
`dispatch` 里保留的 `debug_assert_eq!(executed.len(), group.len())` 是**另一件事**：那是"结果数与派发数不等"的响亮中止（debug 档），运行时可见性由 D21 承担，两者不互相替代。

**D24 — 门 G-145 的 L2 判据把并发窗口算进上限面，并配两条会红的植入。**
`LIMIT_FIELDS`/`LIMIT_ENVS` 各加一项（`max_parallel_tools` / `CHIMERA_LLM_MAX_PARALLEL_TOOLS`）。理由：为"并行度"删掉 `chunks(window)` 既不红编译也不红既有测试，而 `tests/llm_tool_loop.rs` 在 L7 配 16/16/16 时断言的 `peak <= 8` 绑定的正是这个窗口——不钉住它，删界的能力削减机器看不见。
selftest 加两条植入（env 读取点改名 / 窗口默认改 0），断言数由 10/10 增至 **12/12**（数以门自报为准，`gate_manifest.toml` 的 note 不抄数，只同步判据描述——G-145/G-146 两条 note 已同步）。
**改既有门判据不需四件同批**（`audit_gate_parity` MISSING/GAP 与 `G-07` 管的是"新增门的半接线"），但新判据必须自带会红的夹具，否则等于没加。

### 验证与回滚

`check_llm_loop_defaults.py --selftest` 12/12 + 真树 rc=0；`check_manifest_schema.py`（144 门）、`run_gate_manifest.py --list`（`selected=137 total=144 skipped=7`）、`audit_gate_parity.py`（no parity gap）三者 rc=0；`chimera-cli --lib` 210 例（含新增 5 例归位测试）+ `llm_tool_loop` 15 例 + `llm_off_legacy` 6 例全绿；`gqep-executor + chimera-cli` 全 target 无失败；`clippy --all-targets -D warnings` rc=0（本批 `&mut Vec` → `&mut [_]` 的 `ptr_arg` 与测试面 `expect_used` 已按要求改掉）；`fmt --check` rc=0。
回滚 `git revert` 本批 ⇒ 归位回到逐位 + 门回到四上限判据；`tool_plane` 侧不受影响（L7 的保序契约与槽位写法在 W1b-1 批里，两批各自独立可 revert）。

---

## 追加节五（2026-11-01）— W2：tick 内的事件流成本（做了三条、按数据否决一条）

### 读数（`--profile bench-lite`，mean）

| 组 | 读数 | 含义 |
|---|---|---|
| `snapshot_swap_segment/256` | **24.360 µs** | 一次 tick 的 256 条 `NexusEvent` 深拷（+`Arc::new` 包裹）——本批要评估的那笔成本 |
| `snapshot_swap_segment/64` | 6.045 µs | 同一操作 ×64 ⇒ 约 94 ns/条，线性 |
| `snapshot_swap_segment/1` | 151.59 ns | 下界（`VecDeque` 骨架 + 一次事件） |
| `snapshot_swap_segment/compare_equal_content/256` | 2.987 µs | 两份内容相同、分配不同的快照之间 `!=` 的元素级代价 |
| `tui_update_cost_events/1 → /256`（**快路径关掉**） | 32.84 → 35.99 / 35.59 µs | 关掉快路径后，事件流长度开始进入对齐成本 |
| `tui_update_cost_events/1 → /256`（**快路径在**） | 33.00 / 32.75 / 32.96 µs | 加上后三个规模回到同一条水平线 ⇒ 省下的正是那 ~2.9 µs |

### 做了什么

**D25 — 先补比较器，并且先承认既有比较器有个洞。**
`tui_update_cost_bench.rs` 的 `synthetic_snapshot` **此前从不填 `latest_events`**——所以"全量对齐"那两组一直在漏测由 Parliament / Log / EventStream 三块面板共同消费的那个字段。填上之外还做了一处形状修正：事件流的 `Arc` 在多次 poll 之间**共享**，因为真实管道在本 tick 无新事件时就是复用旧 `Arc`（`should_replace_event_snapshot`）；不共享的话，夹具给被测对象搭的是一个它永远走不到的舞台，快路径在这条 bench 上必然测不出来。
另新增 `data_pipeline_bench.rs` 的 `snapshot_swap_segment` 组：现存的端到端组把构造、300 ms 等待与 shutdown 全算进每 iter，深拷在其中不可分辨——**不可分辨的比较器等于没有比较器**。

**D26 — `state.rs` 的事件流比较加指针快路径。**
`!Arc::ptr_eq(...) && self.state.latest_events != snapshot.latest_events`。
实测 `Arc: PartialEq` **不做**指针短路（`compare_equal_content/256` = 2.99 µs 就是两侧不同分配、内容相同的情形），而"revision 变了但事件流没变"是常见形态（只有 chat 在增长）。它与 `:89-92` 的 revision 短路不重复：后者在 revision **相同**时跳过全部字段对齐，这条在 revision **不同**而事件流未变时只跳过本项。
**为什么这条不需要新测试**：指针相等 ⇒ 内容必然相等（同一个对象），这是逻辑恒等而不是经验性质；加一条"验证它跳过"的测试只会重复断言同一个同语反复。它的价值由 A/B 读数承担（35.99/35.59 → 32.75/32.96）。

**D27 — 把"未来若引入时间淘汰需改判据"从注释债变成判据。**
`should_replace_event_snapshot(events_this_tick, trimmed_this_tick)` = `events_this_tick > 0 || trimmed_this_tick > 0`；trim 条数由 `drain(..len-max)` 直接算出（与 `while len > max { pop_front }` 逐位等价，并让已登记的超长函数 `new_internal` 少一行 ⇒ 棘轮基线随批 `--emit` 从 467 **降到** 466）。
今天 trim 只在 push 后可达，两个条件在现网等价；写死单一条件的话，任何后来的**非 push 型淘汰**（时间窗、按字节裁剪）都会让快照静默停在旧内容上——面板显示上一批事件，而没有任何测试会红。
牙实测：把判据退回 `events_this_tick > 0` ⇒ `snapshot_replaced_only_on_new_events` 红（消息点名"只发生淘汰（无新事件）时快照仍必须重建"）。
**这条牙差点是假的**：第一次反事实用 `--should_replace` 作过滤词，跑的是**零条测试**却报 GREEN——选择器命中 0 条与判据通过长得一模一样。测试名 `snapshot_replaced_only_on_new_events` 与被调函数名不同名，过滤词必须选后者之外的前者。

### 按数据否决的一条（与本波批准计划的 W2 Step 2/5 相左，理由与翻转条件都在此）

**不做 `latest_events: Arc<VecDeque<Arc<NexusEvent>>>` 的逐元素 Arc 化。**

- 收益上界：一次深拷 256 条 = **24.36 µs**，而这只在"本 tick 有新事件"时发生（≤4 次/秒）⇒ 稳态 ≤**98 µs/秒**，约一个核的 0.01%；同一进程一次全量对齐是 33 µs/次。
- 代价实测面：现取命令 `grep -rc latest_events crates/chimera-tui`，把逐文件数字求和。**量纲钉死**：`grep -c` 数的是**匹配行数**（同一行出现两次只计一次），所以该和是引用点数的下界，不是"引用出现次数"；换口径（`grep -o | wc -l`）得到的数不可与本行比较。**该数随时点变，引用前重跑，勿抄本行**：2026-11-02 复测 231 行 / 47 文件，W2 决策时点为 222 行 / 约 40 文件，两个都是时点值。前列消费者：`panels/event_stream.rs`、`app/tests.rs`、`panels/log.rs`、`types.rs`、`panels/filter_cache.rs`（逐文件数以命令输出为准）。且 `DataPipelineSnapshot` 是**公开类型**——`match NexusEvent::X{..}` 在 `&Arc<NexusEvent>` 上不成立，面板侧逐点要改成 `&**ev`。批准计划把它估成"5 处 + 夹具"，那是错的（本波计划里的 seam 清单只覆盖了当时查到的消费点，未穷举 bench/panel/test 面）。
- 结论：用一次跨数十文件的公开类型迁移换 ≤0.01% 的 CPU，与本仓"避免过度工程化 / 禁止无数据的推荐"两条同时相悖。
- **翻转条件（写死在这里，便于日后重开）**：`max_event_history` 上调、tick 间隔下调、或事件流之外还有别的字段共用同一 deque（使一次替换变成多次深拷）三者任一发生 ⇒ `snapshot_swap_segment` 这条组已在盘上，重跑即可判定，无需再建比较器。Step 5 的"共有元素 `Arc::ptr_eq`"结构断言随本条一起不做——它是为一次未发生的迁移准备的锁。

### 验证与回滚

`cargo test -p chimera-tui --no-fail-fast --jobs 2` 无失败；`clippy --all-targets -D warnings` rc=0；`fmt --check` rc=0；`test_fnlen_smoke.sh` rc=0 且**棘轮基线由 467 降到 466**（`--emit` 同批刷新；`ADR-227` 里那句"467 / 215 行"是 2026-10 的时点引文，不回写）。
回滚：三条各自独立可 revert——判据那条会连带要求把 `fnlen_baseline.txt` 的 466 退回 467；快路径与判据都不改任何公开类型；bench 侧只扩组，未增删文件 ⇒ `bench_inventory_freeze.txt` 无需新行。

---

## 追加节六（2026-11-02）— W4a 连接复用与 SSRF 复验证明；以及一条**前提不成立**的登记

### 先记那条前提不成立（本波计划 W4 Steps 1/2/3/6）

计划写的接线方式是"在 `McpMesh` 加 `discover_tools(server_id)`（经 participant client 发 `tools/list`），`build_tool_runtime` 遍历 `config.mcp.servers` 注册并发现能力"。复测结论：**这条路径在本仓不存在，且不是改线就能接上的**。逐条证据：

| 查的东西 | 现取结果 |
|---|---|
| `ParticipantClient` trait 的方法面 | `prepare` / `commit` / `rollback` / `send_query` / `send_tool_invoke`——**无**任何能力列举方法；`grep -n "list_tools\|tools/list" crates/mcp-mesh/src/quantum/participant_client.rs` **0 命中** |
| 线协议形状 | `TwoPcRequest{Prepare,Commit,Rollback}` + `QueryRequest` + `ToolInvokeRequest`，全是本仓自定义的 2PC/查询/调用消息（`participant_client.rs:70-140`）——不是 MCP |
| 有没有 MCP JSON-RPC 编解码 | `grep -rn "jsonrpc\|JsonRpc\|JSON-RPC\|\"jsonrpc\"" crates/mcp-mesh/src/` **0 命中**——范围＝**仅本 crate**（命令自带 `crates/mcp-mesh/src/` 边界）。**这一行原写作"（全 crate）"是无范围误写**：全仓 tracked 现取 13 文件 / 59 命中，其中 `crates/**/*.rs` 9 文件 / 54 命中，帧形与本仓已有实现同源（见下两指针） |
| `client_v2.rs`（名字像 MCP 客户端） | 其头注自陈：**2026-08-28 磁盘 ENOSPC 事故中数据丢失**，现按 WI-22/ADR-150 语义做"最小但真实的三项**骨架**重建"（`DiscoveryResult` / `SchemaCache` / `RouteRegistry` 三个数据结构），并明写"**未恢复**丢失前的完整连接器/协议握手逻辑" |
| 配置侧 | `McpServerConfig{id, command, args, url, auth}` 无 capabilities；`MeshServer.capabilities: Vec<String>` 只是字符串列表（W0 更正 #2 的根因，与这里同源） |

结论：真实 MCP 装配（`initialize` → `tools/list`，还要覆盖 stdio 与 HTTP/SSE 两种传输）是一个**新子系统**，需要自己的 ADR 与验证面，不属于"性能与可达性波次里补一根线"的规模。

**W4b 成本因此下移（帧层不从零起）**：本仓已有 JSON-RPC v2 帧实现可直接作参照——`crates/nexus-app-server/src/protocol.rs:1`（模块自述"JSON-RPC v1 编解码 — App 协议传输帧"）、`:5-7`（请求/响应/通知三形态示例，含 `"jsonrpc":"2.0"` 字面）、`:77` `JsonRpcError`（带 -32700/-32600/-32601/-32602 标准码位）。于是 W4b 要新增的是 **MCP 语义层**（`initialize` 握手、`tools/list` 能力形状、两种传输的分帧），而不是再立一套帧；上表把 jsonrpc 命中数限定在"仅 `crates/mcp-mesh/src`"正是为了不把这个既有资产抹掉。

**为什么不给 `discover_tools` 加一个返回空的实现先把线接上**：那会让 `chimera tool list` 看上去"接了 MCP"而永远返回空，且 `build_tool_runtime` 的 `descriptors().is_empty()` 分支会静默不注册——一个零生产者的装饰面，比不做更糟（本仓同类红线：不得先加零生产者装饰变体）。因此本波**不做**该子集，把它连同上述证据一起登记为待立 ADR 的独立项。

计划里另一条被本波实测否证的说法（W2 的"消费点 5 处 + 夹具"）见追加节五；两条合起来是本波对批准规格的**第 5、6 处更正**。

### 做了什么（都在"真实存在的那条路径"上）

**D28 — 连接池改成每 endpoint 一条队列。**
原实现 `HashMap<String, TcpStream>` 每 endpoint 只存**一条**，且取出即独占（`pool.remove(endpoint)`）。后果是确定的：并发批次里第一个请求把唯一的槽取走，其余请求都看到"池是空的"⇒ 各自 `TcpStream::connect`；归还时又各自 `pool.insert(endpoint, ...)` **覆盖同一槽**，被覆盖的那条连接当场关闭。于是 (a) 并发下没有复用；(b) 连接被无谓地建立又丢弃（fd 抖动）；(c) 下一批仍只能复用至多一条。
现在每 endpoint 存 `VecDeque<TcpStream>`，上限 `MAX_POOLED_STREAMS_PER_ENDPOINT = 8`（超出即随作用域结束关闭，**不让池无界增长**），取/还都是"锁内只做 pop/push、绝不跨 `.await`"（§4.4 #1），且空队列顺手从 map 里删掉（否则每个见过的 endpoint 永久占一行）。复用失败的那条连接不归还——半帧/对端已关的流会污染后续请求。

**D29 — 逐消息 DNS 复验保持不动，并把它钉成结构判据。**
`prepare/query/invoke` 三条路径每条都在 `send_frame_and_recv` **之前**调 `validate_endpoint_dns`（`participant_client.rs:517/569/615`），池复用不跳过它——本条**没有**改成"带 TTL 的缓存"（计划 Step 5 允许的可选项），因为省下的是一次 `lookup_host`，而牺牲的是"记录变了下一次必须拒绝"这个保证；收益与风险不对等。
锁的形状：`every_frame_roundtrip_is_prefixed_by_dns_revalidation` 扫**生产区**源码，断言每个 `self.send_frame_and_recv(` 调用点之前同一函数体内出现 `validate_endpoint_dns(`。行为断言在这里不可得——本地无法让同一域名在两次解析之间从公网 IP 变成内网 IP（那要一个可控 DNS），而真实回归形状恰恰是"为了省 DNS 把复验挪出热路径"。
**这条扫描器第一次跑抓到了它自己**：它把测试里那个被匹配的字符串字面量也算成一个调用点（报"第 4 处没有复验"，而那第 4 处就是本测试）。修法是把扫描范围截到 `#[cfg(test)]` 之前——写自造扫描器时，**扫描面必须排除扫描器自身**，这是本轮第二次踩同一类坑。

**D30 — W3 装配组成钉。**
新增 `crates/chimera-cli/tests/tool_plane_assembly.rs`：默认装配的 provider 集合**恰为** `{echo, proc}`，并钉住 MCP 命名规则 `mcp:<server>/<capability>`、能力默认副作用为 `Write`（fail-closed），以及"保留地址在 `register_server` 就被 `SsrfBlocked` 拒"这条界（实测：本波接不了真实发现，但"将来接的时候不能把内网端点放进来"现在就钉住）。
测试文件头写明了接入真实发现后该怎么改期望（`{echo, proc} ∪ {mcp 当且仅当有可发现 server}`），以及为什么命名锁与在场性锁刻意分成两个 `#[test]`。

### 读数与牙

- `concurrent_batches_reuse_pooled_connections`：服务端在**同一条连接上循环处理多帧**（否则"复用"与"重连"行为同形，测的是不存在的东西），观测量是 `accept` 次数（不依赖计时）。第一批 4 并发 ⇒ 4 条新连接（池初值为空，允许）；第二批 4 并发 ⇒ **accept 不变**。
  牙：把 `put` 退回"槽位只留一条"（旧实现语义）⇒ 该例红，消息点名"3 条并发会各自新建"。
- `every_frame_roundtrip_is_prefixed_by_dns_revalidation`：把 tool-invoke 路径的复验删掉 ⇒ 该例红。
- `mcp_tool_names_are_prefixed_and_server_scoped`：把命名格式改成 `<server>::<capability>` ⇒ 该例红。
- 三次变异各自红一次，跑完即还原并字节比对（其中一次还原把 LF 文件写成了 CRLF，已改回；教训：**打补丁前读本文件自己的行尾，不要假设全仓一致**）。
- 顺带：`fnlen_baseline.txt` 的 `new_internal` 由 467 → **465**（两次注释收拢 + `drain` 形式 + rustfmt 合并一行）。第一波次 `--emit` 跑在最后一次 `cargo fmt` **之前**，导致登记表比实测多 1 行 ⇒ 顺序规则：**行号/长度类基线必须在格式化工具之后重刷**。

### 验证

`cargo test -p mcp-mesh -p chimera-cli -p gqep-executor --no-fail-fast --jobs 2` 38 target 全绿、零失败；`clippy --all-targets -D warnings` rc=0；`fmt --check` rc=0；`check_dependency_rules.sh` rc=0；`check_composition_root.py` rc=0（本批未新增装配点）；`check_perf_redlines.py --static-only` rc=0；`check_tool_plane_contract.py --selftest` / `check_llm_loop_defaults.py --selftest` 各全绿；`test_fnlen_smoke.sh` rc=0（含 teeth proven）。

### 遗留（不在本波做，逐条给条件）

1. **MCP 真实装配与能力发现**：另立 ADR，范围 = JSON-RPC 编解码 + `initialize`/`tools/list` 握手 + stdio 与 HTTP/SSE 两传输 + `readOnlyHint → SideEffectDecl` 映射 + "server 不可达则跳过并 warn、不阻塞启动"。前置条件：先决定 `McpServerConfig` 是否加 capabilities 覆盖字段（那是 `nexus-core` L1 结构变更）。
2. `mcp.rs:3-5` 头注里"capability 名不能证明只读 ⇒ 一律 Write"的说明**保持原样**（它仍然准确）；只有发现协议落地后才按 Step 6 改写。
3. mesh 外层 200 ms 与内层 50 ms 的双层超时**未动**（计划 Step 7）：数值是运维契约，且本批没有改变任何请求的时序；"谁应当更小"这条裁决要随真实装配一起定——在没有真实链路可测的时候定数值口径，等于凭感觉改运维参数。

## 追加节七（2026-11-02）— 第三波 A 批：把"验收底座"本身修成可判的

本节对象不是计划路径，而是**验收面**：A1–A4 修的四条红全部来自上一波终验判据的**跑点偏置**（同一批门在主检出 rc=0、在工作树判红，我却拿工作树读数宣布"无退化"）。判据不可信时，后面每一波的"绿"都不成立，所以它排在能力项之前。

**D31 — 覆盖率本地不可判，且判读规则在跑之前写死。**
`cargo llvm-cov --summary-only -p nexus-contracts --ignore-filename-regex tests --jobs 2` → rc=101，`error[E0463]: can't find crate for 'profiler_builtins'`，崩在 `quote` / `proc-macro2` 的 build script（尚未触及被测 crate）⇒ 这是**工具链面缺插桩运行时**（host `x86_64-pc-windows-gnu`，rustc 1.97.1），与具体 crate 无关；无 `coverage/*.json` 产出。规则事先写死、不许事后改判读：出现 E0463/缺 `profiler_builtins` ⇒ 维持 `coverage_per_crate_floor.toml` 的本地不可判声明，逐 crate 真数仍归 `coverage.yml` nightly；只有真产出 JSON 才继续 `coverage_gate.py --check`，且首份数值只落报告、抬 floor 另立批。零成本自证：`coverage_gate.py --validate-config` rc=0（R1 棘轮单调，last=85）。⇒ 对用户"新增功能覆盖率 ≥85%"这条要求，本地的诚实回答是**不可判**，本波只承诺测试面（用例 + proptest 属性），不拿估算充数。日志 `tmp/w3-llvmcov-probe.log`（含命令、`rustc -vV`、原文输出与 rc）。

**D32 — 终验改双口径，"投影限制"由两条同时成立的机械判据派生。**
- 修订：上一波的"非绿成员集合逐条相同"改为**逐成员分类相同**（真债集 / 投影限制集各自逐条相同），报告两栏并列，禁止互迁，禁止写"等价于全量绿"。
- 判据（两条都成立才算投影限制）：① 门输出点名的路径在**工作树工作区不存在、主检出工作区存在**；② 同一道门在**主检出跑为绿**。只有 ① 而 ② 不成立 ⇒ 落回真债（否则任何"缺文件"症状都能替真债作保）。
- **批准计划的写法已被实测否证**：原用 `git ls-files --error-unmatch` 判"仅一侧缺"。实测九个被点名文件在主检出**同样是 untracked**（`.gitignore:68 *.md`、`.gitignore:64 .trae/`）⇒ `git ls-files` 两侧都失败，测不出不对称，会把投影限制误判成真债。判据改为两侧 `test -f`。
- 本波实测（A 批收尾时点）：九条点名路径（`docs/architecture/adr_index.md`、`docs/governance/RK-P_risk_register.md`、`.claude/CLAUDE.md`、`docs/reports/phaseR-wave1-closure.md`、`docs/reports/phaseR-release-checklist-v2.28.0.md`、`docs/architecture/README.md`、`docs/architecture/INDEX.md`、`CHANGELOG.md`、`docs/architecture/governance/DOCUMENT_LIFECYCLE_POLICY.md`）全部 main=Y / wt=N；六条非绿门（G-04/G-06/G-11/G-17/G-18/G-72）在主检出逐门现跑**全部 rc=0** ⇒ **真债集为空**。
- 双条件核对用的是**逐门现跑**（六条），不是主检出全量 144 重跑；全量在主检出的口径留到波次终态跑，两类读数不得互相冒充。主检出是用户的在途工作区（17 改 + 4 未跟踪），跑完以 `git status --short` 前后逐字节比对确认未被改动。
- 顺带抖出的门内病理：G-18 的"实算 id 不达下限"在两侧同时存在但方向相反——主检出 ps1 6类/14id、sh 4类/7id（均达下限，rc=0），工作树 ps1 5类/11id、sh 3类/5id（均不达下限，rc=1）。**下限不许降**：它正是抓"检查被静默去掉"的牙，工作树的少跑是缺语料的衍生品，降下限等于把投影问题洗成绿灯。

**D33 — 自造分类器必须先被允许"拒绝出结论"。**
第一版分类探针把 subprocess 的 GBK 解码崩溃吞成 rc=99，然后照常输出"真债集 = 全部七条"——一份完全致盲的读数，长得像一份结论。第二版加两道守卫：任一门未能真正执行 ⇒ `[PROBE-ERROR]` + 退 2、不出分类；`gate_manifest.toml` 解析条数低于地板 ⇒ 判为解析器致盲并退 2。守卫随即在第二个真实缺陷上生效：数组名按 `[[gates]]` 切（实为 `[[gate]]`）⇒ 解析出 0 条 ⇒ 退 2，而不是把七条门全记成"查不到 cmd"再给一份看似结论的表。教训入规：**探针的 rc 不编码判定 ⇒ "探针跑过"与"探针给出结论"是两件事**；凡求和/分类/聚合型工具必须有一条"我没能真正测到"的显式出口，且该出口要在一次真实缺陷上被触发过才算有牙。探针本体 `tmp/w3_projection_classifier.py`（波次工件，不进 `scripts/` 受治面；若日后跨波复用再按"新门四件同批"接线）。

### A 批台账（红→绿均现取，日志落 `tmp/a*.log`）

| 批 | commit | 判据 | 改前 → 改后 | 反事实 |
|---|---|---|---|---|
| A1 | `3c3167a` | G-27 / G-35 | rc=1（`[STALE] participant_client.rs:556`）→ rc=0（16 finding / 8 key，"tables in parity: 8 key(s)"） | 单侧多一键 → `[STALE]+[PARITY] allowlist=9,freeze=8`；单侧少一**活**键 → 只报 `[PARITY] allowlist=8,freeze=7`，rc=1（parity 轴独立有牙） |
| A2 | `67143d4` | G-05 | rc=1（`[GAP-B1] … current crate count (46)`）→ rc=0（整门转绿，`[B1] 2/4`） | 头值改回 `**39 crates**` → rc=1，同条 GAP 复现；`39→**46**（…）` 里的 46 不接名词故不判命中 ⇒ 判据认语义邻接 |
| A3 | `d5ac09e` | G-53 | rc=1（`chimera-cli.SIMPLIFIED-CN = 1`，markers 84）→ rc=0（markers 83 = 主检出口径） | 原句种回 → rc=1、84、点名新行号 :354（第一次植入锚未命中而门仍 rc=0＝空跑与通过同形，assert 先 abort 才发现，重做） |
| A3′ | `e2e93f8` | G-59 | 全量门跑出 `G-59 mismatch`（`MATURITY says …|SIMPLIFIED-CN, disk says …`）→ `--install` 后 rc=0（46/46 有效） | 该红由本会话 A3 造成、由门自己抓出；`--install` 只动 1 文件 1 行，其余 45 份逐字节相同（写入幂等） |
| A4 | `268000d` | doc 三门 | `check_doc_drift` / `check_crate_reachability` / `check_doc_consistency` 各 rc=0；表格行 3 个转义 `\|` 逐字仍在 | 见报告 §5.1/§5.5 与 ADR-232 D6 的现取订正 |
