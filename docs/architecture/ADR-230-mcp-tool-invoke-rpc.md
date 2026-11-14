# ADR-230 —— MCP 单服务器调用 RPC（ToolInvoke）与 2PC 的边界

- 状态：Accepted（2026-10-31）
- 决策者：架构组（Phase 4 落地批次；证据 `tmp/toolplane-baseline-2026-10-31.md` §Phase 4，收口报告 `docs/reports/toolchain-execution-plane-2026-10-31.md`）
- 关联：ADR-033、ADR-049（不新建 crate）、ADR-065 决策 4（数据面不进 event-bus）、ADR-067（MCP 通道亲和）、ADR-229（执行面；`McpToolProvider` 是本 ADR 的唯一消费方）、ADR-232（LLM 循环把 MCP 工具当普通工具调用）
- 影响面：`crates/mcp-mesh/src/quantum/participant_client.rs`（新 RPC 消息 + trait 方法）、`crates/mcp-mesh/src/mesh.rs`（`McpMesh::invoke_tool`）、`crates/chimera-cli/src/commands/mcp.rs`（v2 分支）、`crates/chimera-cli/src/tool_plane/mcp.rs`

## 1. 背景与问题

`McpMesh` 此前只有一条对外调用路径：`execute_transaction`（Prepare→Commit/Rollback 的 2PC）。它在跨服务器原子性上是对的，但被当作"调一个 MCP 工具"的唯一手段时产生三类实际损害（2026-10-31 复测）：

| # | 问题 | 证据 |
|---|---|---|
| P-1 | **语义错配**：单服务器调用也强制走 Prepare+Commit 两阶段，participant 必须实现两阶段状态机才能被调用 | `ParticipantClient` trait 原本只有 `prepare/commit/rollback` 三方法（`participant_client.rs` trait 定义） |
| P-2 | **成本翻倍**：一次工具调用 = 两次往返 + 两次单阶段超时预算，LLM 循环里每轮都付 | `send_phase` 系列按阶段各自 `timeout(phase_timeout_ms)` |
| P-3 | **结果形状不匹配**：2PC 返回"事务是否达成"，调用方要的是工具 payload | `McpCallResult` 只有 `success/transaction_id`，无 payload 字段（CLI 侧靠扩展承载） |

同时必须承认：2PC 本身是正确设计，多服务器写入的原子性不能因为"CLI 想省事"而丢掉。

## 2. 决策

### D1 边界按**原子性需求**划，不按调用便利性划

- **单服务器、无分布式原子性要求** ⇒ `McpMesh::invoke_tool(server_id, tool, args_json, timeout_ms)`（本 ADR 新 RPC，一次请求一次响应）。
- **多服务器、需要 Prepare/Commit/Rollback** ⇒ 仍走 `execute_transaction`，**逐字不变**。
- 判据写在 `invoke_tool` 的 doc 注释上，而不是只写在本文：调用面选择错误最常发生在"读代码的人没读 ADR"。

### D2 新 RPC 是**新消息类型**，不是新事件、也不是新错误变体

```rust
// crates/mcp-mesh/src/quantum/participant_client.rs
pub struct ToolInvokeRequest  { pub call_id: String, pub tool: String, pub args_json: String }
pub struct ToolInvokeResponse { pub payload: String }
```

- `call_id` 由调用方生成（`Uuid::now_v7()`），用于回填关联（LLM 循环的 `tool_use_id` 语义）；
- 帧格式、连接池、`send_frame_and_recv` 全部复用既有实现 ⇒ 老 participant 只要不解析新消息就不受影响，且**新客户端连老服务器时是显式失败**（对方回协议错误 → `ProtocolError`），不是静默降级；
- 超时统一映射为既有 `McpError::NetworkError`，**不新增错误变体**（错误面稳定性优先）。

**拒绝项 R1**：拒绝复用 `TwoPcRequest::Prepare{op}` 表达"单阶段调用"。那会让 2PC 的 prepare 语义出现"有时需要 commit、有时不需要"的分叉，participant 侧无法区分，等于用协议歧义换一行代码。
**拒绝项 R2**：拒绝为工具调用新增 `NexusEvent` 变体（ADR-229 D5 同源判据；MCP 侧既有 `McpMessageReceived` 覆盖"消息进网格"的可观测承诺）。

### D3 安全约束在**每次发送前**再校验一次

`send_tool_invoke_inner` 在写 socket 前调 `validate_endpoint_dns(...)`，与 `register` 阶段的 SSRF 校验（拒内网/保留地址）构成两道闸：
- WHY 二次校验：注册时的 DNS 解析结果可能因 rebinding 在调用时指向内网；单次校验等于把 SSRF 判定建立在"过去某一刻的解析"上。
- 测试面用 RFC 5737 TEST-NET-3（`203.0.113.1`）直接构造 `MeshServer` 绕过 register 校验——这是**测试专用夹具**，生产路径必须经 `register_server`。`127.0.0.1` 在 SSRF 黑名单内，本地 vLLM/自建 MCP 需显式配置允许（对应实施计划 §8.1 风险表"MCA/MCP 端点不允许"一行）。

### D4 CLI 面：v2 显式开关，默认旧行为

`chimera mcp call` 仅当 `CHIMERA_MCP_CALL_V2=on` 才走 `invoke_tool`，且必须显式给 `--endpoint`；否则旧 2PC 分支原样执行。输出仍用同一 `McpCallPayload`，v2 下 `result.transaction_id = None`、`result.payload = Some(...)`（旧消费者读 `success`/`transaction_id` 不受影响）。

### D5 执行面接入：`McpToolProvider` 只在有服务器时注册

`composition.rs::build_tool_runtime` 先构造 `McpToolProvider`，**若其 `descriptors()` 为空则不注册**——`ToolRegistry::register` 是全有或全无语义，注册一个空 provider 会让整张表装配失败。MCP 工具由此自动出现在 `chimera tool list` 与 LLM 声明里，无需另立通路（ADR-229 D2 的三层倒置在此兑现）。

## 3. 验证

| 面 | 测试 | 断言要点 |
|---|---|---|
| RPC 契约 | `crates/mcp-mesh/tests/tool_invoke.rs` | 请求/响应 serde 往返；`ServerNotFound`/`ServerUnreachable`/超时三类失败各自可达；payload 原样返回 |
| 2PC 不变 | 同文件 + `crates/mcp-mesh/src/quantum/participant_client.rs` 既有测试 | 老 `TwoPcRequest/Response` 往返逐字不变 |
| CLI 契约 | `crates/chimera-cli/tests/cli_contract_snapshot.rs`、`crates/chimera-cli/tests/tool_plane_e2e.rs` | 默认关闭时输出与旧路径一致；v2 缺 `--endpoint` 报配置错误 |
| 执行面 | `crates/chimera-cli/src/tool_plane/mcp.rs` 相关用例 | 空 descriptors 不注册（避免全表装配失败） |

## 4. 回滚

1. `CHIMERA_MCP_CALL_V2` 不设 ⇒ CLI 只走 2PC 分支；`McpMesh::invoke_tool` 与 `McpToolProvider` 成为无人调用的加法面（不破坏 wire）。
2. 代码级回滚顺序：① 删 CLI v2 分支 → ② 删 `McpToolProvider` 注册 → ③ 删 `McpMesh::invoke_tool` → ④ 删 `ToolInvoke*` 与 trait 方法。② 必须在 ④ 之前，否则 trait 缺方法红编译。
3. wire 兼容声明：本 ADR 不改动任何既有消息的字段与序列化形态；若未来需要改 2PC 消息，须另立 ADR 并说明 participant 升级次序。
