# ADR-245 -- `mcp call` 真实传输显式激活（默认零网络副作用）

- **状态**: Accepted（2026-11-06）
- **关联**: ADR-120（MCP Mesh 2PC）、ADR-186（SSRF 端点校验）、ADR-244（工具运行时装配收敛）
- **代码**: `crates/chimera-cli/src/commands/mcp.rs`、`crates/chimera-cli/src/cli.rs`、`crates/mcp-mesh/src/quantum/participant_client.rs`

---

## 1. 背景

`mcp call <server> <tool> [args]` 长期走 `McpMesh::new(...)`，其默认注入
`InProcessClient` —— 进程内伪传输，`prepare/commit` 恒成功。这带来两个问题：

1. **语义失真**：命令叫 "call"，实际不发任何请求；用户无法判断"成功"是否代表远端真的执行。
2. **无法验证真实链路**：真实 TCP 帧协议（4 字节 BE 长度前缀 + JSON
   `TwoPcRequest`/`TwoPcResponse`）只在 `mcp-mesh` 单测中被覆盖，CLI 无入口。

但直接改成"默认发真实请求"违反两条硬约束：

- CI / 门禁 / 测试**不得**发起真实外网请求（不可控、易 flaky、可能触碰外部服务）；
- 默认行为变更属于破坏性契约（既有脚本依赖"伪成功"的退出码 0）。

## 2. 决策

**D1. 传输客户端由显式开关选择，默认保持不变。**

```rust
let use_network = matches!(action, McpAction::Call { network: true, .. });
let mesh = build_mesh(use_network);   // false → InProcessClient；true → TcpParticipantClient
```

- 默认（无 `--network`）：`InProcessClient`，**零 socket**，行为与既有版本逐字一致；
- `--network`：`TcpParticipantClient::new()`，连接注册端点，**可能产生真实网络请求**。

**D2. 开关位于 CLI 面，不加 feature flag。**

理由：`--network` 是**单次调用**的用户意图，不是编译期能力。feature flag 会造成
「同一二进制两种能力矩阵」，与「禁止因此新增 feature flag」的收尾约束冲突。

**D3. SSRF 守卫仍在开关之前，且不可绕过。**

`ServerRegistry::register` 的 `validate_endpoint` 先于任何 connect：
loopback（`127.0.0.0/8`、`localhost`）、RFC 1918、CGNAT、链路本地与云元数据域名
在注册阶段即被拒。`--network` 只改变"谁来发请求"，不改变"能不能发"。

**D4. 副作用的用户可见性。**

- `--network` 生效时打 `tracing::info!`（stderr），说明本次为真实 TCP；
- clap 帮助文本明确写出「默认 InProcess，无网络副作用；开启后会真实发起请求」；
- 测试与门禁**必须**使用默认路径（MCP 测试用例均不传 `--network`）。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. 默认改为真实 TCP | 破坏既有行为（默认退出码/延迟/离线可用性全变），且会让全部测试与门禁触网；违反收尾约束第 3 条 |
| B. 用环境变量（如 `CHIMERA_MCP_NETWORK=1`）控制 | 隐式副作用：CI 环境污染会静默改变行为；且无法在单次调用内表达意图，排查困难 |
| C. 新增 cargo feature `mcp-network` | 直接违反「禁止因此新增 feature flag」；且编译期开关无法覆盖「同一进程内混合调用」场景 |
| D. 只交付设计不动代码 | 真实能力已存在于 `mcp-mesh`（`TcpParticipantClient`），仅缺 CLI 入口；不接线等于让能力继续空转 |

## 4. 后果

**正向**

- CLI 具备真实 MCP 传输入口，且默认路径可离线、可重复、零副作用；
- 真实 TCP 路径复用 `mcp-mesh` 既有实现，无新增协议面；
- SSRF 防线未因开关而削弱（有测试锁定）。

**代价 / 边界**

- `--network` 端到端仍受 registry SSRF 约束，**无法**连 loopback —— 因此 CLI 层
  不能自带本地 mock server 做端到端测试。真实帧协议的受控验证由
  `mcp-mesh` 的本地 mock server 测试承担（不触网）：
  `crates/mcp-mesh/src/quantum/participant_client.rs` 的 `tests` 模块，
  其中 `test_tcp_client_with_real_echo_server` 覆盖 prepare → commit 成功路径，
  另有 nack / phase_timeout / connection_refused 三条失败路径。
- 真实远端调用失败时的归因仍为 `EngineError`（退出码 3），与既有 `mcp call` 一致；
  不新增错误变体。

## 5. 测试证据

`crates/chimera-cli/tests/mcp_call_network.rs`（新增，2 条，均不触网）：

1. `default_call_never_opens_network`：端点用 RFC 5737 TEST-NET-3
   （`203.0.113.1:9`，可注册但不可路由）。默认路径返回 `Ok` —— 若真发 TCP 必然失败，
   故 `Ok` 只能来自进程内伪传输，即**未打开 socket**。
2. `network_flag_does_not_bypass_ssrf_guard`：`mcp://127.0.0.1:9` + `network: true`
   必须在 connect 之前被 registry 拒绝（`ServerNotFound` 语义）。

实测：`cargo test -p chimera-cli --test mcp_call_network` → 2 passed / 0 failed，0.03s。

**真实帧协议受控证据（loopback mock，零外网）**

`crates/mcp-mesh/src/quantum/participant_client.rs` 的 `tests` 模块用
`TcpListener::bind("127.0.0.1:0")` 起本地 echo server，验证 4 字节 BE 长度前缀 +
JSON 帧的完整 2PC 往返：

```text
cargo test -p mcp-mesh --lib tcp_client
→ 4 passed; 0 failed; 105 filtered out; finished in 0.12s   (EXIT=0)
```

覆盖 `test_tcp_client_with_real_echo_server`（prepare/commit 成功）、
`..._nack`、`..._phase_timeout`、`..._connection_refused`。
全部绑定 `127.0.0.1:0` 由内核分配端口，不访问任何外网地址。

## 6. 回退

删除 `McpAction::Call` 的 `network` 字段并让 `build_mesh` 恒走 `InProcessClient`；
`mcp-mesh` 侧零改动，回退无协议兼容成本。
