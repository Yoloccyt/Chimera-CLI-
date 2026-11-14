# ADR-231 —— Quest DAG 执行编排、租约生命周期与 Hook 并行

- 状态：Accepted（2026-10-31）
- 决策者：架构组（Phase 5 落地批次；证据 `tmp/toolplane-baseline-2026-10-31.md` §Phase 5，收口报告 `docs/reports/toolchain-execution-plane-2026-10-31.md`）
- 关联：ADR-026/027/028（chimera-mas 四象限与 INV-7/8/9）、ADR-051（变体隔离）、ADR-054（内环设计意图，NOT-ENFORCED，见 ADR-187）、ADR-145（mas-sched 调度控制面）、ADR-146（nexus-hook 生命周期 Hook）、ADR-229（执行面：Quest 与 Hook 并行都收敛到它的许可语义）、ADR-232（Phase 7 规划器接缝的下游）
- 影响面：`crates/chimera-cli/src/quest_exec.rs`（新）、`crates/chimera-cli/src/commands/quest.rs`、`crates/chimera-cli/src/cli.rs`、`crates/quest-engine/src/{engine,config,lib}.rs`、`crates/mas-sched/src/scheduler.rs`、`crates/chimera-mas/src/{chunker,delegation}.rs`、`crates/nexus-hook/src/{executor,config}.rs`

## 1. 背景与问题

`quest-engine` 能分解 DAG、`mas-sched` 能发租约、`nexus-hook` 能拦前后置，但三者之间**没有执行通路**：分解出来的 `Task` 没有承载者，租约无人续期，Hook 的 Post 阶段串行执行。复测（2026-10-31）确认四个缺陷：

| # | 缺陷 | 证据 |
|---|---|---|
| P-1 | **分解≠执行**：`QuestEngine::decompose` 产出 `quest.tasks` 后，全库无任何生产调用方按依赖推进任务状态 | `quest-engine` 侧只有 `update_task_status` 被动接口；CLI 无 `quest execute` 动作 |
| P-2 | **租约只增不回收**：`SimplePeerScheduler` 有 `claim/renew_lease/handoff`，但没有到期回收入口 | `crates/mas-sched/src/scheduler.rs` 修复前无 `reap_*`；过期租约永久占住 `peer_concurrency` 配额 |
| P-3 | **Hook Post 串行**：`run_post_parallel` 名字承诺并行，实际逐个 `.await` | 新增 `post_hooks_bounded_parallel` 红证（8×500ms 在 `max_parallel=4` 下应 <2.5s） |
| P-4 | **Ask 档可执行**：`TrustLevel::Ask` 与"是否放行执行"没有绑定，存在把"未确认"当可执行的路径 | `crates/nexus-hook/src/config.rs` 修复前无 `allows_execution`；`ask_is_fail_closed` 用例先行钉死 |

## 2. 决策

### D1 执行编排落在 L10（`chimera-cli::quest_exec`），不落 L9

`QuestExecutor` 需要同时调 `quest-engine`(L9)、`mas-sched`(L9)、`gqep-executor::tool_plane`(L7)。**编排者是组合根而非新层**：把 DAG 推进逻辑塞进 `quest-engine` 会让 L9 反向依赖 L7 执行面的装配（而装配知识属 L10），塞进 `mas-sched` 会把"分解语义"漏进调度控制面。
落点：`crates/chimera-cli/src/quest_exec.rs`（`QuestExecConfig`/`TaskPlanner`/`JsonPlanPlanner`/`QuestExecutor`/`QuestState`/`QuestExecReport`）。

### D2 规划器是接缝，Phase 5 只给确定性实现

```rust
#[async_trait] pub trait TaskPlanner: Send + Sync {
    async fn plan_for(&self, task: &QuestTask) -> Result<Vec<ToolPlan>, PlannerError>;
}
```
`JsonPlanPlanner`（读 fixture/文件）是本期唯一实现；LLM 规划属 ADR-232 的循环能力。
`PlannerError::NoPlan` ⇒ 该任务记 **Skipped 而非 Failed**：没有计划是"未覆盖"，不是"执行失败"，把两者混记会污染奖励/信用信号（Ω₅-Credit）与后续统计。

### D3 默认串行；并行是**显式双重**开关

| 开关 | 默认 | 语义 |
|---|---|---|
| `CHIMERA_QUEST_EXEC` | off | `apply`/`on` 才允许 `chimera quest execute`，否则命令报配置错误并给出启用提示 |
| `CHIMERA_QUEST_DAG` | `linear` | `multi_root`/`multi-root` 才允许多根并发 |
| `CHIMERA_QUEST_EXEC_MAX` | 4 | 任务级并发窗口 |
| `CHIMERA_QUEST_TOOL_MAX` | 8 | 单任务内计划并发（透传 `PlanRunOptions::max_parallel`） |
| `CHIMERA_QUEST_TASK_TIMEOUT_MS` | 120000 | 单任务超时，同时决定租约时长与续期节拍 |

`QuestExecConfig::window()` 是**唯一**决定并发窗口的函数：`Linear ⇒ 1`。即使调用方把 `max_concurrent_tasks` 配成 64，线性 DAG 也不会并发——并行度由 DAG 形态决定，不由配置决定（配置只能收紧不能放宽）。

### D4 租约生命周期补"回收"这一维，锁序写进注释

新增 `reap_expired_at(now)`（测试可注入时间推进）与 `reap_expired()`（生产入口，取当前时刻）。
**锁序不变量**：`leases` 写锁恒先于 `peer_concurrency` 写锁取得，且在同一临界区内按该序同持（`scheduler.rs` 注释自陈）。`handoff` 用 `HANDOFF` 哨位标记，`claim` 侧见哨位即让位。
**机证**：`quest_execute_e2e.rs` 里租约 handoff 后断言 `lease_count() == 0`；并发状态更新用例断言峰值 == 2（结构式，不用墙钟推断）。

### D5 委托与 chunker 的并发是**许可**，且**没有 env 开关**

`chunker.rs` 与 `delegation.rs` 的有界化统一采用"先 `acquire_owned()` 取 permit，再 `tokio::spawn`"，permit 随任务 `Drop` 归还 ⇒ 峰值在飞 = 上限而非任务总数。
上限来源是**程序化配置**：`DelegationConfig::with_max_concurrency`（默认 8）、`BatchConfig` 的 `effective_conc`。

> **与实施计划 §8.2 的口径差（有意保留）**：计划稿列了 `CHIMERA_DELEGATION_MAX=1`、`CHIMERA_HOOK_PARALLEL=0` 两个 env 回滚开关，**代码里不存在**。本轮不新增这两个 env，理由是它们与既有配置面重复（`DelegationConfig::with_max_concurrency` / `HookTriggerOpts.max_parallel` 已是同一判据的单一入口），加 env 只会造出"两个源、谁覆盖谁"的新问题。**回滚口径因此是代码级而非运行级**：委托并行回滚 = 调用方把 `max_concurrency` 设为 1；Hook 并行回滚 = 不开 `HookTriggerOpts.parallel_post`（默认 `false`，即旧串行行为）。计划 §8.2 的这两行由报告偏差登记作废。

### D6 Hook：Ask 档 fail-closed，Pre 保序、Post **按需**并行

- `TrustLevel::allows_execution()` 是 `pub const fn`，**只有 `Trusted` 返回 true** ⇒ Ask 档在未获人工批准前不执行（`ask_is_fail_closed` 用例）；`TrustLevel::from_str` 对未知值一律落 `Untrusted`，即"拼错的档位不会 accidentally 获得执行权"。这与 §6.2 审批默认拒绝、ADR-229 D6 同一条红线。
- **默认形态未变**：`HookTriggerOpts{ parallel_post: false, max_parallel: 4, total_timeout_ms: 30_000 }`——Post 并行必须调用方显式开 `parallel_post`，`trigger()` 旧入口行为逐字不变（这也是本 ADR "行为变更默认关"承诺的一部分）。
- **Pre 阶段保持顺序**：前置 Hook 的语义是"逐个把关"，并行化会让后一个 Hook 看到前一个尚未完成的世界（`pre_hooks_keep_order` 用例钉住结果序）。
- **Post 有界并行**：窗口式 `FuturesUnordered`（先填 `max_parallel` 个，每完成一个补一个），结果落回**原始下标**——`run_indexed` 返回 `(index, HookResult)`、写回 `results[index]`，避免"完成序被误当调用序"（E0308 的成因也在此：窗口补位与初始填充必须共用同一个 Future 类型，所以提出 `run_indexed` 而不是就地写两个 async block）。总预算用绝对 deadline（`timeout_at`），到点未完成的槽位补一条"未完成"合成结果，保证 `results.len() == specs.len()`（零孤儿）。
- 总超时到 ⇒ Pre **拒绝**（fail-closed），Post **只记录**（已发生的事不能因预算而伪造结果）。超时放行等于把"验证来不及"转译成"验证通过"。
- **可达性诚实声明**：`nexus-hook` 在 chimera-cli 侧是 **optional 边**（`crates/chimera-cli/Cargo.toml:104`，属 ADR-161 待接线的 GATED 名单；`crates/chimera-cli/src` 内 `nexus_hook` 零引用）。因此 `trigger_with` / `parallel_post` 目前**无生产调用方**——本 ADR 交付的是"能力 + 判据 + 测试"，不是"已上线的并行"。可达性棘轮读数不变（`check_crate_reachability.sh` 2026-10-31 实测 GATED 名单含 `nexus-hook`，frozen=0）。

**拒绝项 R1**：拒绝把 Hook 并行度做成 env 开关（见 D5 理由）。
**拒绝项 R2**：拒绝为"任务被跳过/租约被回收"新增 `NexusEvent` 变体（ADR-229 D5）。

## 3. 验证

| 面 | 测试 | 断言要点 |
|---|---|---|
| Quest 执行 | `crates/chimera-cli/tests/quest_execute_e2e.rs`（含 3 个 CLI 子进程用例） | 扇出（MultiRoot）真并行、依赖因前置失败被跳过、取消 ⇒ `Cancelled`、无计划 ⇒ `Skipped`、租约 handoff 后 `lease_count()==0`、并发状态更新峰值==2 |
| 租约回收 | `crates/mas-sched/src/scheduler.rs` 内 `reap_*` 用例 | 过期被回收且配额释放；未过期不动 |
| Hook | `crates/nexus-hook/src/executor.rs` 的 4 个新用例 | Ask fail-closed / Post 有界并行且索引对齐 / Pre 保序 / 总超时拒绝 |
| 委托与 chunker | `crates/chimera-mas/tests/delegation_bounded.rs`、`chunker.rs` 的 `test_chunker_respects_max_concurrency` | **峰值在飞 ≤ 上限**（计数器证明，非墙钟推断） |
| 依赖铁律 | `bash scripts/check_dependency_rules.sh` | 编排落 L10 后 L9 无新向上边 |

### 3.1 落地期间修掉的一个真实缺陷（值得留档）

`QuestState::from_quest` 只把"被别人依赖时才解除阻塞"作为入队路径，**从未播种初就绪集合** ⇒ 首轮没有任何任务可准入，`quest execute` 对合法计划报 `skipped: N`。红测（7/11 失败）暴露后修正为：无前置且非终态的任务在构造时直接入 `ready`。
**教训**：DAG 驱动循环的"初值"是最容易写成恒假的地方——它不会崩，只会安静地什么都不做。这类缺陷只能靠"断言有东西被执行"（计数/`calls()`）而不是"断言没出错"来发现。

## 4. 回滚

1. `CHIMERA_QUEST_EXEC` 不设 ⇒ `chimera quest execute` 直接拒绝，DAG 编排完全不进入运行路径；`chimera quest` 其余 4 个子动作不受影响。
2. `CHIMERA_QUEST_DAG=linear`（默认）⇒ 即便开了 execute 也只串行。
3. 代码级批次：① D5 有界化（纯收敛，回滚即恢复无界，不建议） ② D6 Hook Ask/并行 ③ D4 租约回收 ④ D1/D2/D3 Quest 编排。②③④互不依赖，可独立 revert；④ revert 时须同步删 `QuestAction::Execute` 与 `cli.rs` 帮助文本，否则契约快照测试红。
4. `reap_expired*` 是加法 API，删除不影响既有租约语义。
