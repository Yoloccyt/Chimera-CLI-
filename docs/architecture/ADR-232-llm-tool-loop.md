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
| `action_orchestrator` 的 `agent.chat`（TUI 命令面板进循环的实际路径） | `DenyWrites` | 无人值守自动化 ⇒ 只允许只读工具；TUI 持终端控制权，后台任务不能去读 stdin 问人 |
| TUI 弹窗审批 | 未接线（Phase 7B ⇒ 本波 B 批） | **默认 `CHIMERA_LLM_LOOP` off ⇒ TUI 不进循环**；开档（`=apply`）后 TUI 经上一行的编排器分支进循环（`action_orchestrator.rs:160-169`），此档下审批硬取 `DenyWrites`（写调用被拒并回填 `ask_required`）。缺的是"逐次弹窗问人"这一档——诚实登记而非默认放行。本表旧版在此另起一行写"当前 TUI 不走 LLM 循环"，与上一行自相矛盾（编排器就住在 TUI 进程里），已合并订正 |

**拒绝项 R3**：拒绝"批量审批一次通过后续全部写工具"。审批粒度必须是 per-invocation，否则第一次确认会为整条循环背书（与 ADR-231 D6 Pre 保序同源：把关不能合并）。

### D7 默认关闭 + 旧路径逐位不变

启用面只有两处：`chimera run --llm`（显式旗标）与 `CHIMERA_LLM_LOOP=apply|on`（影响 `chat` repl_loop 与 `agent.chat`）。其余一切默认走旧实现。
`llm_off_legacy.rs` 用 6 个 CLI 子进程用例钉住"没开开关时输出不变"——包括 stdout 形状与退出码。**这类回归测试的价值在于它测的是"我没动的那条路"**：本轮改动全部落在开关之后，但真正的风险是开关判断写反导致默认路径被劫持。

### D8 真实厂商只验装配，不发网络

`McaLlmClient` 仅在 `--features mca` 下编译（`llm/mod.rs` 的 `#[cfg(feature = "mca")]`）；3 个用例只断言装配与错误映射（`mca_client_assembles_from_config` / `mca_client_missing_provider_is_unavailable` / `mca_provider_name_mapping_is_conservative`：未知名必须落 `ProviderId::Custom`，不得冒充已知通道）。
**可达性诚实声明**：`mca-gateway` 经 ADR-177 是**长期 GATED**（optional 边 + `--features mca`）。因此"能连真模型的 LLM 循环"在默认构建里不可达，属预期而非缺陷；离线全链路由 `CHIMERA_LLM_PROVIDER=mock` ⇒ `EchoLlmClient` 承担（CI/演示用，无 ToolUse ⇒ 单轮收敛）。

### D9 流式与 TUI 审批留待 Phase 7B（未实施）

本期输出为终态文本；`--json` 时给 `LlmPayload{text, turns, tool_calls, denied_tool_calls, stop}`。逐 token 流式（`VendorAdapter` 流式数据面按 ADR-065 决策 4 走 bounded mpsc、不进 event-bus）与 TUI 弹窗审批是 7B 的两项，**本 ADR 不为未做能力背书**。

## 3. 验证

| 面 | 测试 | 断言要点 |
|---|---|---|
| 闭环 | `crates/chimera-cli/tests/llm_tool_loop.rs` | 一轮工具→结果喂回→第二轮收敛；`seen_requests` 里第 2 轮确实带 `Tool` 角色与对应 `tool_use_id`（**闭环唯一硬证据**） |
| 上限/重复 | 同文件 | `MaxTurns`/`MaxToolCalls`/`MaxRepeatToolCalls` 各自可达；并行时**峰值在飞 ≤ `max_parallel_tools`**（结构断言） |
| 审批 | 同文件 | 只读自动放行；被拒的写工具**未进 runtime**（provider `calls()` 为空）；`denied_tool_calls` 计数正确 |
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
