# ADR-232 —— LLM 工具循环（多轮闭环、四重硬上限、审批默认拒绝）

- 状态：Accepted（2026-10-31）
- 决策者：架构组（Phase 7 落地批次；证据 `tmp/toolplane-baseline-2026-10-31.md` §Phase 7，收口报告 `docs/reports/toolchain-execution-plane-2026-10-31.md`）
- 关联：ADR-033（L0 契约恒允许）、ADR-065/066/067/068（MCA 体系与 `VendorAdapter`）、ADR-177（`mca-gateway` 长期 GATED）、ADR-210 Option C（复用活体事件，禁装饰变体）、ADR-229（执行面：循环只经 `ToolRuntime`，不自建配额）、ADR-230（MCP 工具作为普通工具出现在声明里）、ADR-231 D6（审批 fail-closed 同一红线）、ADR-053（R2 影子期：本能力默认 off，不参与影子）
- 影响面：`crates/chimera-cli/src/llm/`（新：`client.rs`/`tools.rs`/`approval.rs`/`tool_loop.rs`/`mock.rs`/`mca_client.rs`/`mod.rs`）、`crates/chimera-cli/src/composition.rs`、`crates/chimera-cli/src/commands/run.rs`、`crates/chimera-cli/src/commands/chat.rs`、`crates/chimera-cli/src/action_orchestrator.rs`、`crates/chimera-cli/src/cli.rs`、`crates/chimera-cli/benches/llm_tool_loop_bench.rs`（新，dev-only）

## 1. 背景与问题

`chimera` 有 LLM 通道（`chimera llm`、`mca-gateway`）与工具面（ADR-229），但**两者从未接成闭环**：模型能产出 `ContentBlock::ToolUse`，却没有任何路径把它变成一次真实执行并把结果喂回。审计（2026-10-31）确认四点：

| # | 缺陷 | 证据 / 后果 |
|---|---|---|
| P-1 | 无循环载体：`AffinityResponse.blocks` 的 `ToolUse` 在 CLI 侧被直接丢弃 | `commands/run.rs` 修复前只取文本；工具调用等于不存在 |
| P-2 | 无停止条件：若把"响应里有 ToolUse 就再调一轮"直接写出来，模型重复同一调用会无限转 | 三重悖论"推理悖论"红线的工程形态 |
| P-3 | 无审批闸：工具执行会真写文件/起进程 | §6.2 审批默认拒绝红线；Ask 档此前无实现（ADR-231 D6 同源） |
| P-4 | 无离线可验证面：任何"循环正确性"断言都要求 API key + 网络 | 与项目 ★RL/网络闸门纪律冲突（Python 训练服务禁止实施、CI 离线） |

## 2. 决策

### D1 落点：L10 `chimera-cli::llm`，复用 L0 契约与 L10 网关，**不新建 crate、不新增事件变体**

类型全部来自 L0 `nexus_contracts::affinity`（`AffinityRequest/Response`、`ContentBlock`、`ToolDecl`、`UsageReport`、`MessageRole`、`FinishReason`、`ThinkingPreference`、`ProviderId`）；真实厂商经 `mca-gateway::VendorAdapter`（ADR-065）。执行经 ADR-229 的 `ToolRuntime`，**循环层不持有任何配额**。

**拒绝项 R1**：拒绝新建 `llm-loop` crate。循环是"接口层的编排"（L10），拆 crate 只增加装配面（ADR-049）与可达性棘轮登记负担。
**拒绝项 R2**：拒绝新增 `LlmToolCall*` 事件变体（ADR-229 D5 / ADR-210 判据：零消费者装饰变体）。

### D2 客户端契约与错误三态

```rust
#[async_trait] pub trait LlmClient: Send + Sync {
    fn provider_id(&self) -> &str;
    async fn complete(&self, request: &AffinityRequest) -> Result<AffinityResponse, LlmError>;
}
pub enum LlmError { ProviderUnavailable(String), Transport(String), Protocol(String), BudgetExceeded(String) }
```
CLI 侧退出码映射固定：`Cancelled`→用户取消（`UserCancelled`）、`ProviderUnavailable`→配置错误（`ConfigError`，退出码 1 且给出**可执行**的启用提示）、其余→`EngineError`。
**WHY 三态分离**：把"没配 provider"和"网络断了"混成一个错误，会让用户以为配置无效而反复改配置——本仓 doctor/config 面的历史教训。

### D3 请求映射守恒

- `system_prompt` → `MessageRole::System` 首条消息；**工具结果永不拼进 system**（提示注入红线，§8.1）。
- `Vec<ToolDescriptor>` → `tool_decls()`：`input_schema` 缺失时回退 `FALLBACK_OBJECT_SCHEMA`（`{"type":"object","additionalProperties":false}`）——宁可让厂商拒收一个严格 schema，也不发一个"什么参数都接受"的开放 schema。
- Assistant 消息**整块**入历史，`Thinking` 块逐字保留（`loop_preserves_thinking_blocks` 用例）：只取文本会破坏支持思考回传的厂商的多轮一致性。
- 工具结果以 `MessageRole::Tool` + `ContentBlock::ToolResult{tool_use_id, content, is_error}` 回填，**与调用同序**（依赖 ADR-229 §3 的 `invoke_batch` 保序契约，循环内以 `debug_assert_eq!(call_id)` 逐位钉住）。
- 未知工具 / 未获审批：合成 `is_error=true` 的结果喂回模型（`ASK_REQUIRED_MARKER`），**而不是抛异常终止循环**——模型必须知道自己被拒了，否则它会以"上一轮成功"的世界观继续。

### D4 四重硬上限 + 重复检测，且 stop 语义不可伪装

| 上限 | env | 默认 | 触发后 stop |
|---|---|---|---|
| 轮数 | `CHIMERA_LLM_MAX_TURNS` | 8 | `MaxTurns` |
| 累计调用数 | `CHIMERA_LLM_MAX_TOOL_CALLS` | 32 | `MaxToolCalls` |
| 相同 `(tool,args)` 重复 | `CHIMERA_LLM_MAX_REPEAT` | 3 | `MaxRepeatToolCalls` |
| 累计 token | `CHIMERA_LLM_MAX_TOTAL_TOKENS` | 128000 | `Budget` |
| 单轮并行工具 | `CHIMERA_LLM_MAX_PARALLEL_TOOLS` | 8 | —（窗口，非停止条件） |

**实现纪律**：`LlmLoopOutcome` **不**派生 `Default`；`stop` 初值取 `MaxTurns` 而非 `Final`。每条 return 路径必须显式落 `stop`——若漏落，`MaxTurns` 如实表达"耗尽轮数没收敛"，而 `Final` 会把 bug 伪装成正常结束（这条写在 `tool_loop.rs:145-147` 注释里，因为它是反直觉的）。
调用数上限**先于**重复检测判定：一次调用风暴本身就是打转，不该先被"每个都唯一"放过。

### D5 预算信号复用 `BudgetExceeded`（Critical / mpsc 旁路）

超 `max_total_tokens` 即发布既有 `NexusEvent::BudgetExceeded{ budget_type: "llm_total_tokens", current, limit }`，severity=Critical ⇒ `publish_critical` 走 mpsc 保证送达（§6.2 红线）。
**WHY 在 `tokio::spawn` 里发布**：事件发布不得阻塞循环返回路径；调用方（CLI）拿到 `stop=Budget` 时事件可能尚未落地，二者是独立可观测面，不存在"发布失败就丢结果"的耦合。

### D6 审批三档，默认拒绝；无人值守入口一律只读

```rust
#[async_trait] pub trait ToolApproval: Send + Sync {
    async fn approve(&self, inv: &ToolInvocation, side_effect: SideEffectDecl) -> bool;
}
```
- `DenyWrites`：`SideEffectDecl::Write` 一律拒；**不在工具表里的名字按最严口径视为 `Write`**（`tool_loop.rs` dispatch）。
- `ApproveAll`：仅测试/显式信任场景。
- `CliApproval(PermissionCtx)`：`--yes` 或交互 `permission::confirm`，逐条问。

入口分档（这是安全决策，不是便利决策）：
| 入口 | 审批档 | 理由 |
|---|---|---|
| `chimera run --llm` | `CliApproval` | 有人在终端 |
| `chimera chat`（`CHIMERA_LLM_LOOP=apply`） | `CliApproval` | 同上 |
| `action_orchestrator` 的 `agent.chat`（TUI 命令面板进循环的实际路径，执行体 `route_chat_or_quest`） | 有闸门 ⇒ `TuiApproval`；无闸门 ⇒ `DenyWrites` | TUI 持终端控制权，后台任务不能读 stdin 问人 ⇒ 问人只能走弹窗。**默认 `CHIMERA_LLM_LOOP` off ⇒ TUI 根本不进循环**；开档后若本进程未装配闸门（CLI/headless/测试）即回落 `DenyWrites`：只读放行、写一律拒并回填 `ask_required` —— 新能力接线时不得多放行一个写调用（fail-closed）。"全拒的弹窗"与"不弹的直拒"四口径逐位相等由 `popup_answered_no_is_identical_to_deny_writes` 锁。**未验**＝真 tty 渲染与键回流、人在环时延、`--features mca` 端到端。本表旧版在此另起一行写"当前 TUI 不走 LLM 循环"，与上一行自相矛盾（编排器就住在 TUI 进程里），已合并订正 |

**拒绝项 R3**：拒绝"批量审批一次通过后续全部写工具"。审批粒度必须是 per-invocation，否则第一次确认会为整条循环背书（与 ADR-231 D6 Pre 保序同源：把关不能合并）。
**R3 的现势实现**：票据是 `(ApprovalRequest, oneshot::Sender<bool>)`，而 `oneshot::Sender::send(self)` 按值消费发送端 ⇒ "答一次、复用到后续写调用"在类型上写不出来，不只是纪律约束。行为侧双向守：`one_yes_never_approves_the_next_call`（同工具名、第二个 `call_id` 必须重新发票；应答端带超时出口，实现若记住"已批过"则判红而不是测试挂死）与 `one_yes_does_not_endorse_later_writes`（循环级：w1 答 Yes、w2 无人应答 ⇒ 只有 w1 进 provider，w2 仍按 `ask_required` 回填且位置在 w1 之后）。

### D7 默认关闭 + 旧路径逐位不变

启用面只有两处：`chimera run --llm`（显式旗标）与 `CHIMERA_LLM_LOOP=apply|on`（影响 `chat` repl_loop 与 `agent.chat`）。其余一切默认走旧实现。
`llm_off_legacy.rs` 用 6 个 CLI 子进程用例钉住"没开开关时输出不变"——包括 stdout 形状与退出码。**这类回归测试的价值在于它测的是"我没动的那条路"**：本轮改动全部落在开关之后，但真正的风险是开关判断写反导致默认路径被劫持。

### D8 真实厂商只验装配，不发网络

`McaLlmClient` 仅在 `--features mca` 下编译（`llm/mod.rs` 的 `#[cfg(feature = "mca")]`）；3 个用例只断言装配与错误映射（`mca_client_assembles_from_config` / `mca_client_missing_provider_is_unavailable` / `mca_provider_name_mapping_is_conservative`：未知名必须落 `ProviderId::Custom`，不得冒充已知通道）。
**可达性诚实声明**：`mca-gateway` 经 ADR-177 是**长期 GATED**（optional 边 + `--features mca`）。因此"能连真模型的 LLM 循环"在默认构建里不可达，属预期而非缺陷；离线全链路由 `CHIMERA_LLM_PROVIDER=mock` ⇒ `EchoLlmClient` 承担（CI/演示用，无 ToolUse ⇒ 单轮收敛）。

### D9 流式仍留 Phase 7B（未实施）；TUI 弹窗审批已于第三波 B 批接线

本期输出为终态文本；`--json` 时给 `LlmPayload{text, turns, tool_calls, denied_tool_calls, stop}`。逐 token 流式（`VendorAdapter` 流式数据面按 ADR-065 决策 4 走 bounded mpsc、不进 event-bus）仍是 7B 未实施项，**本 ADR 不为未做能力背书**。

TUI 弹窗审批不再属于"未实施"：本波 B 批已接线（`TuiApproval` + `PopupKind::Approval` + 四路决策回流）。本行原文把它与流式并列为 7B 两项，现该半句已不成立；等价性判据、回滚单位与**未验范围**逐项见追加节。

## 3. 验证

| 面 | 测试 | 断言要点 |
|---|---|---|
| 闭环 | `crates/chimera-cli/tests/llm_tool_loop.rs` | 一轮工具→结果喂回→第二轮收敛；`seen_requests` 里第 2 轮确实带 `Tool` 角色与对应 `tool_use_id`（**闭环唯一硬证据**） |
| 上限/重复 | 同文件 | `MaxTurns`/`MaxToolCalls`/`MaxRepeatToolCalls` 各自可达；并行时**峰值在飞 ≤ `max_parallel_tools`**（结构断言） |
| 审批 | 同文件 | 只读自动放行；被拒的写工具**未进 runtime**（provider `calls()` 为空）；`denied_tool_calls` 计数正确 |
| 审批档等价（本波 B 批） | 同文件 | `popup_answered_no_is_identical_to_deny_writes`：`TuiApproval`（每条都问、人人答 No）与 `&DenyWrites` 四口径逐位相等（调用序/`tool_calls`/`denied`/回填有序三元组）；`one_yes_does_not_endorse_later_writes`：R3 的循环级表达 |
| 审批通道（本波 B 批） | `crates/chimera-cli/tests/approval_gate_properties.rs` | proptest：随机 `call_id` 集合与到达序下，每个 id 至多一次决策、决策值只影响该 id、未获答恒拒（闸门关/回执断/无人应答三态各自固定例） |
| 预算 | 同文件 | `BudgetExceeded` 被订阅者收到且 `stop=Budget` |
| 取消 | 同文件 | 在飞工具被丢弃（`loop_cancel_drops_inflight_tools`：5s 工具在 <2s 内让路） |
| 旧路径 | `crates/chimera-cli/tests/llm_off_legacy.rs` | 6 例：默认关闭时 `run`/`chat` 行为逐位不变 |
| 真实厂商 | `llm_tool_loop.rs` 内 3 个 `#[cfg(feature="mca")]` 例 | 只装配、不发网络；未知名落 Custom |
| 延迟基线 | `crates/chimera-cli/benches/llm_tool_loop_bench.rs` | dev-only（`scripts/bench_inventory_freeze.txt` 登记）；组名 `llm_tool_decls_{8,16,32}`、`llm_loop_{8,16}_tools`、`llm_loop_16_tools_window8`；每例计时前自检"工具真的被执行" |

## 4. 回滚

1. 运行级：`CHIMERA_LLM_LOOP` 不设 + 不传 `--llm` ⇒ 三个入口全部走旧路径（由 D7 的 6 例守护）；`CHIMERA_LLM_PROVIDER=mock` 可在无 key 环境单独验证循环本身。
2. 收紧而不关闭：`CHIMERA_LLM_MAX_TURNS=1` 或 `CHIMERA_LLM_MAX_TOOL_CALLS=1` 或 `CHIMERA_LLM_MAX_TOTAL_TOKENS` 调小 ⇒ 循环退化为"至多一轮工具"，用于灰度观察。
3. 代码级批次：① `llm/` 模块与测试 ② `composition.rs::build_llm_client`（含 `#[cfg]` 双函数）③ 三处入口接线。revert 顺序自下而上（先摘入口，再删装配，最后删模块），否则 `--features mca` 构建会因 `build_llm_client_from_config` 悬空而红。
4. bench 是加法面：删除时**必须同批**删 `scripts/bench_inventory_freeze.txt` 那一行与 `[[bench]]` 注册，否则 `check_perf_redlines.py` 判 `unknown` 红。

## 5. 与实施计划 §5.7 的口径差（以实现为准）

| 计划稿 | 实现 | 性质 |
|---|---|---|
| `run(prompt, tools: Vec<ToolDescriptor>, ...)` | `&[ToolDescriptor]` | 避免每次循环 clone 整张工具表；调用方仍持有所有权 |
| `ToolApproval::approve(&self, inv)` | `approve(&self, inv, side_effect: SideEffectDecl)` | 审批需要副作用档位；在循环内查表后传入，避免让审批实现反向依赖 registry |
| `LlmLoopOutcome{text,turns,tool_calls,usage,stop}` | 另加 `denied_tool_calls` | "被拒了几次"是审批闭环的唯一可观测证据 |
| `max_repeat_tool_calls`（env 名未定） | `CHIMERA_LLM_MAX_REPEAT` | 与 `CHIMERA_LLM_MAX_*` 同族命名 |
| `budget_hint_micro`（计划里由 loop 直发新事件） | 透传给 `AffinityRequest`；超 token 才发 `BudgetExceeded` | 避免装饰变体（D5） |
| `LlmToolLoop` 直接 `Arc<ToolRuntime>` + `Vec<ToolDescriptor>` 构造 | 同上，另 `bus: EventBus` 由调用方注入 | 预算事件必须走调用方所在总线，而非循环自建总线 |

## 追加节（2026-11-02）— 第三波 B 批：TUI 弹窗审批接线后本 ADR 的四处自纠

本 ADR 早前把"TUI 弹窗审批"与"逐 token 流式"并列为 Phase 7B 未实施项，并把 `agent.chat` 的审批档
写成单值 `DenyWrites`。B 批落地后这两处都成了**过期陈述**，逐条就地改掉而不是另立文档：

1. **D6 入口分档表**：`agent.chat` 行的"审批档"由单值改为条件式（有闸门 ⇒ `TuiApproval`；无闸门 ⇒
   `DenyWrites`），并删掉原"TUI 弹窗审批｜未接线"那一行——两行同指一条路径，留着会让读者以为存在
   两个独立档位。表内旧锚 `action_orchestrator.rs:160-169` 已被本批的执行体抽取作废，改为符号锚
   `route_chat_or_quest`（本仓反复登记"勿引行号，会漂移"，自己这条也是同一病灶）。
2. **D9 半句失效**：流式仍未实施（保留原判据），TUI 审批已接线（撤回该半句）。改写为"一项未实施、
   一项已落地"，不让"未实施"顺带替另一项作保。
3. **R3 从纪律升为类型约束**：判据不是"我们约定逐条问"，而是 `oneshot::Sender::send(self)` 让
   "答一次、复用到后续写调用"在类型上写不出来；另有单测与循环级测试双向守（见 §3 新增两行）。
4. **§3 验证表补两行**：审批档等价、审批通道 proptest。

**回滚单位（对 D7"旧路径逐位不变"的补充）**：`gate=None` 回落 `DenyWrites` 是"旧路径不变"的机器表达，
由 `popup_answered_no_is_identical_to_deny_writes` 锁；因此 B 批的可回滚单位是 **B3，或 B2+B3 一并**，
不是逐批倒带（单回 B1 断编译；单回 B2 令 B1 留下的 `approval_rx`/`pending_approvals` 成为 `dead_code`，
在 `-D warnings` 下同样阻塞，只是红因不同）。

**本 ADR 明确不为以下四项背书**（诚实登记，不得由测试绿覆盖）：真 tty 渲染与键回流（`TestBackend` 只验
buffer 断言）；人在环时延，以及 `call_with_budget` 是否覆盖弹窗等待这一段；`--features mca` 下本路径
端到端（零用例）；**入口分档表本身没有门守**——`check_llm_loop_defaults.py` 与
`check_tool_plane_contract.py` 对审批档零命中，本波不擅自立门（新门需四件同批且要 CI 授权）。

## 追加节二（2026-11-04）— 第四波：问人窗口与执行窗口的边界，以及"延后 ≠ 拒绝"

本 ADR 的 D6 定了"要不要问人"（入口分档 + R3 per-invocation），没定"问人时别人能不能干活"。
第四波补的是后者。改的只有 `dispatch` 的形状，档位语义一条未动。

### 决策 8：一条调用一个 future，两把互不相干的 Semaphore

| 窗口 | 包住什么 | 额度 | 为什么必须有 |
|---|---|---|---|
| ask | 仅 `approve()` 这一次 await | `max_parallel_tools`（默认 8，用户裁决⑤） | 同时问人 ≤8；只读/自动放行档几乎不持有它，不被慢的写档拖累 |
| exec | 仅 `runtime.invoke()` | 同值 | 在飞 ≤8，与旧 chunk 尺寸等量 ⇒ **永不**击穿 L7 的 `max_waiters=256` |

两把闸都**在 `dispatch` 入口就地创建**，不进结构体（避免跨轮状态）。计数与档位逐位守恒：
`tool_calls` 派发前逐条 `saturating_add`（"派发了几条"而非"执行了几条"），
`denied_tool_calls` 只在 verdict=false 时增，未获批准的调用**不进 runtime**（合成
`ASK_REQUIRED_MARKER` 结果回填，与旧结构同口径）。

### 决策 9：执行窗口是安全语义，不是冗余限流

L7 调度器在 `waiters > max_waiters` 时返回 `Err(ToolError::Overloaded)` —— 即"排队不上"
被实现成"拒绝"。若派发侧一次交出整批（彻底去屏障），300 条只读就会凭空变错误。
这条不是推演：去掉执行窗口的变异实测产出 `"overloaded: 257 waiters"`（三条样例被
`batch_of_300_readonly_yields_zero_overloaded` 打印出来）。

⇒ 本 ADR 的边界补一条：**"并行优先"不得把延后变成拒绝**。任何"去掉在飞上界"的提议
都必须同时给出 L7 侧 `Overloaded` 语义的改法，否则即为削减。

### 决策 10：并发弹窗对人的可见性（并只做了前一半）

窗口 1→8 带来一个 D6/R3 都没覆盖的后果：最多 8 张审批弹窗叠在栈上，而键派发只作用于
**栈顶**、默认选中仍是 Yes ⇒ 连按 Enter 在类型上合法地逐条批掉一叠写调用。每条各有一票
（R3 未破），被摊薄的是注意力——这是"批量背书"的行为学版本。

- 已落地：弹窗标题 `[N pending]`（N = 栈上待批条数，全栈计数，与 `has_approval` 同口径；
  被 HelpOverlay 盖住时仍计入，理由同该函数注释：层叠不是安全决策）。
  判据 `approval_popup_renders_pending_count_from_stack` 断言 N 随栈变化（2→3），
  并断言旧值不残留 —— 只测一个数字会放过"把标题写死"。
- **待裁**：默认选中 Yes→No。它改 B2 已定语义与一条既有测试的期望值，本波不动。

### 与实施期发现的口径差

- 计划预判"新 UI 文案必须走 `crate::t!()` 且中英各一条键"。实测扫描域：i18n 双门只覆盖
  `panels/`、`actions/`、`app/`、`render.rs`，`popup.rs` 不在其内 ⇒ 走 ASCII，与既有
  Confirm 脚注同源。**不要**据此推广为"popup.rs 永远可以硬编码"：那只是当前的门论域。
- `attribute_results` 保留且每批调一次（单条 `invoke` 形状下它仍是唯一归位点），故
  其源码结构锚点 `attribution_behavior_is_not_debug_only` 未悬空；计划里"锚点需要先更新"
  的担心被现实消解，但**行为面**归位此前无人认领 ⇒ 由
  `results_are_attributed_by_call_id_under_concurrent_completion` 补，且用 zip 变异证牙。
- 本 ADR 的验证表现在还依赖：`cargo test -p chimera-cli --test llm_tool_loop`（22 条）
  含三条本波新增判据；红线六条一字未改。CI 侧新增阻塞步 `govd scheduler self-tests (G-147)`
  不覆盖本 ADR（它守的是调度器自测面），"入口分档表无门守"这条残余**依旧成立**。
