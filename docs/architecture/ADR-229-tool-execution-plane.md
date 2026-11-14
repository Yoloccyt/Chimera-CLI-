# ADR-229 —— 工具执行面（Tool Execution Plane）落 L7 + Provider 适配落 L10

- 状态：Accepted（2026-10-31）
- 决策者：架构组（本轮由「现状多维度审计 → 主会话复测 → TDD 落地」三段定稿；证据台账 `tmp/toolplane-baseline-2026-10-31.md`，收口报告见 `docs/reports/toolchain-execution-plane-2026-10-31.md`）
- 关联：ADR-033（L0 恒允许）、ADR-048/185（gqep→qeep 合法向下边）、ADR-049（不新建 crate）、ADR-054（P9-T3/T4 契约上提先例）、ADR-065 决策 4（数据面不进 event-bus）、ADR-160（可达性棘轮）、ADR-161（GATED 装配面）、ADR-177（mca-gateway 长期 GATED）、ADR-210 Option C（降级信号走活体事件，禁止装饰性变体）、ADR-222（L0-A/L0-B 双层）、ADR-230/231/232（本 ADR 的三条下游）
- 影响面：`crates/gqep-executor/src/tool_plane/`（新，L7）、`crates/nexus-contracts/src/spec/tool_plan.rs`（L0-A 契约扩充）、`crates/chimera-cli/src/tool_plane/`（新，L10 适配器）、`crates/chimera-cli/src/commands/tool.rs`（新子命令）、`crates/chimera-cli/src/composition.rs`、`crates/chimera-cli/src/db_open.rs`（新）、`crates/gqep-executor/src/toolplan_runner.rs`

## 1. 背景与问题

审计（2026-10-31 复测）确认工具执行面存在四处结构性缺陷，且都不是"再加一层封装"能解决的：

| # | 缺陷 | 复测证据 | 后果 |
|---|---|---|---|
| P-1 | **没有统一的工具注册/配额面**：L7 只有 `toolplan_runner.rs` 的 DAG 执行器，工具由调用方各自持有 | `crates/gqep-executor/src/` 内除 `toolplan_runner.rs` 外无任何 `Tool*` 抽象；`chimera-cli` 的 wiki/grep 命令直接开 SQLite | per-provider 并发上限无处施加；任何上层（CLI/TUI/LLM/MAS）各搭一套，配额互相不可见 |
| P-2 | **无界并发**：批处理路径先 `Vec::push(tokio::spawn(...))` 后统一 await | `crates/chimera-mas/src/chunker.rs` 修复前实测峰值在飞 = 任务总数（`test_chunker_respects_max_concurrency` 红证：峰值 8 > `max_concurrency=2`） | 1000 个 chunk ⇒ 1000 个同时活的子进程/DB 句柄；违反 §6.1"结果丢了/竞态"两条尸检红线 |
| P-3 | **阻塞 IO 站在 async 运行时上**：`WikiStore::open` 在 `.await` 可达路径内直接执行 | `crates/chimera-cli/src/commands/{wiki,grep}.rs` 修复前直调；`blocking_open_off_runtime.rs` 的负对照证明"内联打开会把同 runtime 上的定时器饿死" | current-thread runtime 上表现为整进程卡顿（违反本仓红线"rusqlite 必须 `spawn_blocking`"） |
| P-4 | **执行面无法被 LLM 循环复用**：若把 provider/审批放进 L10，L7 无从调用；若把循环塞进 L7，L7 反向依赖 L9/L10 | 依赖铁律 `L(N)→L(N+1)` 禁止（`scripts/check_dependency_rules.sh` Check B） | 唯一的合规解是 **trait 定义在下层、适配器在上层**（本 ADR D2） |

## 2. 决策

### D1 执行面落点 = L7 `gqep-executor::tool_plane`，不新建 crate

五模块：`types.rs`（`ToolDescriptor`/`ToolInvocation`/`ToolResult`/`ToolError`/`ToolPriority`/`ToolOrigin`/`ToolProvider` trait）、`registry.rs`（`ToolRegistry`，注册名唯一、全有或全无）、`scheduler.rs`（`ToolScheduler` trait + `BoundedToolScheduler`：双车道 + per-provider 许可 + waiter 上限 + peak 高水位）、`runtime.rs`（`ToolRuntime`/`ToolRuntimeConfig`：绝对 deadline + 取消级联 + payload 截断）、`metrics.rs`（`ToolMetrics`/`ToolMetricsSnapshot`）。

**WHY L7**：执行语义（聚集、超时、取消、结果保序）本层已有（PlanRunner/QEEP）；注册表与配额只是把散在的执行约束收敛到同一处。
**WHY 不新建 crate**：ADR-049 决策 1 延续——同一执行语义拆两 crate 会产生第二个"谁能执行"的口径，且触发可达性棘轮新登记面（ADR-160）。

### D2 依赖方向：契约在 L0、trait 在 L7、适配器在 L10（三层倒置）

```
L0  nexus-contracts::spec::tool_plan   ToolPlan/ToolOp/SideEffectDecl（纯数据，无 IO）
        ↑
L7  gqep-executor::tool_plane          ToolProvider trait + Registry/Scheduler/Runtime
        ↑（只认 trait，不认具体厂商/进程）
L10 chimera-cli::tool_plane            EchoToolProvider / ProcessToolProvider / McpToolProvider
```

L7 的 `Cargo.toml` **不含** `chimera-cli`/`mcp-mesh`/任何 L9-L10 成员；L10 向下依赖 L7。跨层通信仍只走 event-bus / mcp-mesh（§2.2 铁律不变）。

**拒绝项 R1**：拒绝在 L10 再造一个 `ToolRegistry`。两个注册表 ⇒ "工具到底有几套名字空间"不可判定，且 `chimera tool list` 与 LLM 声明会各说各话。
**拒绝项 R3**：拒绝落 L6（Router 层）。L6 只做选择，不承诺执行结果的保序与超时；把执行面放 L6 会让"路由到执行"的失败归因变模糊。

### D3 有界并发是**许可**而非"少开 future"

`BoundedToolScheduler::acquire()` 同时取 **车道许可**（interactive/background）与 **provider 许可**，二者都拿到才进 provider；取不到且 waiter 已满 ⇒ `ToolError::Overloaded`（不排队到无限）。许可用 `OwnedSemaphorePermit` 随 `ToolPermit` 的 `Drop` 归还——`invoke_batch` 可以一次性建出全部 future，**真正打给 provider 的数量被这里钉住**。

取消侧：`CancellationToken::child_token()` 级联 + 绝对 deadline（`tokio::time::Instant` + `timeout_at`）+ 子进程 `kill_on_drop(true)`。
**WHY 绝对 deadline**：相对 `timeout(sleep)` 在批次排队时会把"等待许可的时间"算进执行预算，导致高峰期全员超时（P5 实测教训）。

**拒绝项 R4**：拒绝 `join_all` / 无界 `FuturesUnordered::extend`。本仓 §4.1 明令并发收集用 `FuturesUnordered`，但**有界**是前提；无界版只是把 OOM 换个写法。

### D4 默认关闭 + 三态推进（off / observe / apply）

`CHIMERA_TOOL_PLANE`（`crates/chimera-cli/src/tool_plane/config.rs`）：非法值一律落 `Off`。`Observe` 做校验与预演（解析 plan、算层内并发、列出将执行的节点），**不调 provider、不写文件**。CLI 面 `chimera tool list|call|plan` 默认拒绝并给出可复制的启用命令。

限额同样 env 驱动，且 **组合根把三个车道都用 `max_inflight` 覆盖**（默认 8）：

| 变量 | 默认 | 语义 | 回滚到旧行为 |
|---|---|---|---|
| `CHIMERA_TOOL_PLANE` | off | 三态开关 | 不设即为关 |
| `CHIMERA_TOOL_MAX_INFLIGHT` | 8 | interactive/background/provider 三处同值 | `=1` ⇒ 全串行 |
| `CHIMERA_TOOL_MAX_PARALLEL_PLAN` | 8 | `tool plan` 层内并发（`--parallel` 覆盖） | `=1` |
| `CHIMERA_TOOL_DEFAULT_TIMEOUT_MS` | 30000 | 缺省 per-call 超时 | 调大 |
| `CHIMERA_TOOL_MAX_PAYLOAD_BYTES` | 65536 | payload / proc 输出上限（UTF-8 边界截断） | 调大 |

> **与实施计划 §8.2 的口径差**：计划稿写的是 `CHIMERA_PLAN_MAX_PARALLEL` 与"默认 8/4 双车道"。落地实名为 `CHIMERA_TOOL_MAX_PARALLEL_PLAN`，且组合根当前把三车道都设为 `max_inflight`（L7 `ToolRuntimeConfig::default()` 仍是 interactive 2 / background 6，只是 CLI 组合根显式覆盖）。本文以代码为准；计划稿该节由本报告 §偏差登记追认，不改代码去凑文档。

### D5 事件面零新增变体

`variant_count_test` 与 `scripts/doc_count_freeze.txt` 计数不变。执行面可观测性由两件事承担：`ToolMetrics` 快照（同层读）+ `tracing` 结构化日志。**不**为"工具被调用"新增 `NexusEvent` 变体。
**WHY**：ADR-210 Option C 的判据——零消费者的装饰变体会让事件面虚增而判据不动；预算类红线信号复用既有 `BudgetExceeded`（Critical/mpsc 既有旁路，见 ADR-232 D5）。

### D6 安全边界前置在执行面内，而非调用方

- `proc.exec` 需要 `--yes` 或交互确认（`permission::confirm`），否则拒绝；
- 任何外部调用仍经 L4 `seccore` 沙箱 + `decay-engine` 衰减（§6.1"裸奔"红线）；
- 写/副作用由 `SideEffectDecl::Write` 声明，审批策略是**默认拒绝**（`DenyWrites`），未知工具名按最严口径视为 `Write`（ADR-232 D4）。

### D7 阻塞 IO 一律离开运行时：`chimera-cli/src/db_open.rs`

`on_blocking_thread(work)` 把闭包丢进 `spawn_blocking` 并映射错误；`open_wiki_store(path)` 成为 wiki/grep 两个命令的唯一打开入口。
**机证**：`crates/chimera-cli/tests/blocking_open_off_runtime.rs` 三例，含**负对照** `inline_blocking_would_starve_the_runtime`——不修时该例在 current-thread runtime 上把 10ms 定时器饿到 >250ms，证明测试确实有牙，而不是"断言一个不会失败的方向"。

## 3. 契约快照（供 ADR-230/231/232 引用）

```rust
// L7 gqep-executor::tool_plane
#[async_trait] pub trait ToolProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn descriptors(&self) -> Vec<ToolDescriptor>;
    async fn invoke(&self, inv: &ToolInvocation, cancel: CancellationToken) -> Result<String, ToolError>;
}
pub struct ToolRuntime { /* registry, qeep?, scheduler, cfg */ }
impl ToolRuntime {
    pub async fn invoke_batch(&self, batch: Vec<ToolInvocation>, cancel: &CancellationToken) -> Vec<ToolResult>; // 保序
}
pub struct ToolInvocation { call_id, tool, args_json, timeout_ms, priority, origin, quest_id, task_id, approved }
```

`invoke_batch` 的**保序**是被 LLM 循环依赖的契约（`tool_loop.rs` 逐位 `debug_assert_eq!(result.call_id, invocation.call_id)`）：结果与调用错配 = 模型把 A 工具的输出当成 B 工具，属最危险的静默错误。

## 4. 验证

| 面 | 测试 / 门 | 断言要点 |
|---|---|---|
| 注册表契约 | `crates/gqep-executor/tests/tool_plane_contract.rs` | 注册名唯一、全有或全无、descriptors 与 invoke 一致、`Overloaded` 可达 |
| 不变量属性 | `crates/gqep-executor/tests/tool_plane_proptest.rs` | 属性测试锁三条规则本身（用例数以该文件自报为准，不抄进本表）：截断必须是**不超预算的最长字符边界前缀**；`invoke_batch` 在完成序**被夹具反转**时仍按请求序返回；被拒批次不得留下任何部分注册。**牙口是植入验证的**：把排序键改坏（等价于"按完成序收集"）后本组红，而上一行的例证套件**全绿**——因它用 `sleep_ms=0`，完成序恰等于请求序，对保序无判别力 |
| 有界执行 | `crates/gqep-executor/tests/plan_runner_bounded.rs`、`crates/chimera-mas/tests/delegation_bounded.rs` | **结构式**证明：峰值在飞 ≤ 上限（计数器/Barrier），不用墙钟推断 |
| CLI 端到端 | `crates/chimera-cli/tests/tool_plane_e2e.rs` | off 拒绝并提示；observe 不执行；apply 执行；`proc.exec` 无确认即拒 |
| 阻塞面 | `crates/chimera-cli/tests/blocking_open_off_runtime.rs` | 正例 + 负对照（见 D7） |
| 下游闭环 | `crates/chimera-cli/tests/llm_tool_loop.rs`、`quest_execute_e2e.rs` | 复用本执行面，未另立配额 |
| 计划路径调用面 | `crates/gqep-executor/tests/run_plan_invocation.rs` | `run_plan` 必须把**完整** invocation 交给 provider：`approved` 由节点副作用 + `approve_writes` 决定（缺省 fail-closed）、`call_id` 逐节点唯一、取消令牌是调用方令牌的**血缘**（用"跑完后取消 caller 令牌、看 provider 手里的令牌是否随之翻转"证，不赛跑 mid-flight 取消）。CLI 侧 `tests/tool_plane_e2e.rs` 的判据是**拒绝原因本身变化**（`ask_required` → `denied`），因为退出码在 fail-soft 计划下不区分这两者 |
| 延迟基线 | `crates/gqep-executor/benches/tool_plan.rs`（dev-only 登记） | 趋势基线，不入四表阈值 |
| 依赖铁律 | `bash scripts/check_dependency_rules.sh` | L7 无 L9/L10 边 |

## 5. 回滚

1. 关掉入口：`CHIMERA_TOOL_PLANE` 不设（默认 off）⇒ `chimera tool *` 拒绝、`build_tool_runtime` 不被 quest/llm 路径调用。
2. 逐批 revert：本 ADR 对应 ①L0 契约扩充 ②L7 tool_plane ③L10 适配器+子命令 ④db_open。每批自包含可单独摘除；`ToolRuntimeConfig` 默认值留在 L7，删掉组合根覆盖即回到 2/6 双车道。
3. 不可回滚项：`tool_plane` 一旦成为 quest/LLM 的执行面（ADR-231/232），其 revert 必须连带下游两个入口，否则 quest execute / `--llm` 会以未定义符号红编译。
