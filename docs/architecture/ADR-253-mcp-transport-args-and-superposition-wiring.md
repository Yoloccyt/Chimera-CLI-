# ADR-253 — MCP 传输层语义补全：args 帧级透传与超位置查询接入统一传输抽象

- **状态**: Accepted（2026-11-07）
- **关联**: ADR-245（`mcp call` 显式联网）、ADR-120（MCP Mesh 2PC）、ADR-186（SSRF 端点校验）、ADR-150（WI-22 client_v2）、ADR-244（装配点收敛——本 ADR 同承"消除双语义轨"精神）
- **代码**: `crates/mcp-mesh/src/quantum/participant_client.rs`、`crates/mcp-mesh/src/quantum/superposition.rs`、`crates/mcp-mesh/src/mesh.rs`、`crates/chimera-cli/src/commands/mcp.rs`

---

## 1. 背景：两个残余语义缺口

ADR-245 解决了"传输介质"（默认 InProcess / `--network` 真实 TCP），但留有两个缺口：

1. **args 静默丢弃**：`mcp call <server> <tool> [args]` 的 `args` 只出现在
   permission 文案与 dry-run 预览中，`execute_transaction(vec![server], tool)`
   从不将其带入任何协议消息。`TwoPcRequest::Prepare` 仅有 `transaction_id` +
   `op` 两字段——参与者收不到实参，"调用"语义失真。
2. **超位置查询绕过传输抽象**：`ParticipantClient::query`（Task 0.7
   v2.9.0-omega SubTask 0.7.10）的 trait 文档自述"超位置查询需通过统一抽象
   访问参与者，而非直接 in-process 模拟"，且 `TcpParticipantClient::query`
   有真实 `QueryRequest`/`QueryResponse` 帧实现；但
   `execute_superposition_query` 一直进程内伪造 payload（`result@<query>`，
   1-3ms 假延迟）。即便 `--network` 注入了 `TcpParticipantClient`，查询仍
   不发任何帧，**真实传输失败被伪造为成功**（fail-open）。

另有治理级发现：faae-router `routing_history.rs` 对 `client_v2
discover_and_register` 的注释引用指向全仓不存在的函数（ENOSPC 事故后骨架），
本轮以注释如实化处理（不属本 ADR 决策面，见 §5）。

## 2. 决策

### D1. Prepare 帧携带 args（wire 向后兼容）

- `TwoPcRequest::Prepare` 新增 `#[serde(default)] args: Vec<String>`；
- `ParticipantClient::prepare` trait 增加 `args: &[String]` 参数
  （InProcess / Tcp / Mock 三实现同步）；
- `McpMesh::execute_transaction(participants, op, args)` 三层
  （`run_2pc` → `prepare_phase`）全量透传。

**兼容性论证**：旧帧缺 `args` 键反序列化为空 vec（测试锁定）；新帧对旧接收方
为被忽略的多余键（serde 默认忽略未知字段，未启用 `deny_unknown_fields`）。
默认 `InProcessClient` 行为不变（参数收下，mock 语义不动）。

### D2. 超位置查询经注入客户端 fanout

- `execute_superposition_query` 增加 `Arc<dyn ParticipantClient>` 参数，
  JoinSet 任务改调 `client.query(...)`；
- 传输错误如实映射 `QueryResult::err`（携带 `McpError` 文案），不再伪造成功载荷；
- 默认路径 payload 格式由 `result@<query>` 收敛为 `InProcessClient::query`
  既定格式 `result@<query>@<query_id>@<server_id>`——该格式本就存在于
  InProcessClient，与 superposition 旧 mock 属历史双轨，本次收敛为单轨
  （integration 测试仅断言 success/count，无精确 payload 断言，无破坏）。

### D3. stdio 登记去迂回

`register_configured_servers` 对 stdio 型**直接显式 warn 跳过**，不再合成
`stdio://localhost/<id>` 必败端点借道 SSRF 拒绝。行为等价（registry 本就不
收录 stdio），消除"依赖无关防线的副作用来产生日志"的迂回模式；SSRF 守卫
本身不受影响（ADR-245 D3 姿态不变）。

### D4. 不改错误面与命令语义

- `mcp call` 失败归因仍为 `EngineError`（退出码 3），不新增错误变体；
- `mcp serve` 维持 `NotImplemented`（退出码 2）——协议服务器端是独立工程，
  不与本传输语义修复捆绑；
- 不动 `mcp-mesh` 对外事件面（`McpMeshTransactionCompleted` 等不变）。

## 3. 否决方案

| 方案 | 否决理由 |
|------|---------|
| A. args 编码进 `op` 字符串 | 结构化信息降维为文本，参与者无法可靠解析；协议层明文字段才是正解 |
| B. cargo feature 控制协议版本 | 与 ADR-245 D2 同款否决：编译期开关无法表达单次调用意图；wire 层 `serde(default)` 已足够 |
| C. superposition 保留本地 mock、另开真实查询 API | 双轨并存即双语义，违背 ADR-244 "装配点收敛"精神；trait `query` 早已为它而生，不接线等于能力空转 |
| D. 捆绑恢复 client_v2 discover 闭环（WI-22） | ENOSPC 骨架恢复是独立工程，与本传输语义修复无依赖关系；强行捆绑扩大爆炸半径 |

## 4. 后果

**正向**

- `mcp call --network` 的 args 真实抵达参与者帧；超位置查询在 `--network`
  下走真实 `QueryRequest` 帧（复用连接池与 DNS rebinding 防御）；
- 传输失败在查询结果中可见（`success=false` + 错误文案），消除查询面 fail-open；
- stdio 配置的处置意图直达，不再借道 SSRF。

**代价 / 边界**

- `execute_transaction` / trait `prepare` 签名变更（workspace 内部 API，
  ~25 处调用点机械更新，全部随本 ADR 落地）；
- 默认 superposition payload 格式变化（见 D2，测试无精确载荷断言，实测无破坏）；
- `mcp call` 仍不说 MCP JSON-RPC 协议（私有 2PC 帧协议，与 ADR-245 边界一致）；
  协议服务器端（`mcp serve`）仍未实现——这两项为**已知边界**而非缺陷。

## 5. 同批诚实化修复（非决策面，随批登记）

- `faae-router/src/routing_history.rs`：`ExpertKind::Mcp` 注释如实化——
  `client_v2` 为 2026-08-28 ENOSPC 后骨架，`discover_and_register` 恢复前
  该变体在生产中不产生实例（消除注释与代码断链）；
- `chimera-tui/src/panels/chtc.rs`：面板文档适配器清单与 chtc-bridge
  `IdeAdapterKind` 五变体对齐（VSCode/IntelliJ/Vim/Emacs/Zed）；
- `chimera-tui`：`/mcp` slash 注册为 Orchestrated 却落笼统 HonestTodo，违反
  B1 复评"逐命令定稿"标准——补 `slash.unwired.mcp` 具体指引（zh/en i18n +
  parser 分支 + 测试，对齐 fork/undo/redo 模式）；
- `repo-wiki` ProgressiveSkillLoader：无 body_provider 时的占位 body 由
  `// Loaded {id}`（伪装已加载）改为显式 `body_provider not wired` 声明，
  并新增 SkillGraph→索引映射→渐进加载的全链组合测试（生产上游 pattern
  生产者受 R2 冻结治理，接线前提见其测试注释）。

## 6. 测试证据

```text
cargo test -p mcp-mesh          # 协议/超位置/事务全绿(含新增 4 项)
cargo test -p chimera-cli       # mcp list/serve/call/inspect 行为保持
cargo test -p chimera-tui       # /mcp hint + i18n 键覆盖
cargo test -p repo-wiki         # 全链组合测试
```

关键新增锁定项：

1. `test_two_pc_request_prepare_args_wire_backward_compat`——旧帧（无 args 键）
   必须可反序列化且缺省空 vec；
2. `test_tcp_client_with_real_echo_server`（升级）——本地 echo server 捕获
   对端帧，断言 args 逐字到达（`loopback mock`，零外网，同 ADR-245 §5 姿态）；
3. `test_superposition_uses_injected_participant_client`——tagged 桩客户端，
   payload 必须来自注入抽象而非进程内伪造；
4. `test_superposition_marks_transport_failure`——注入传输失败后所有结果
   `success=false` 且携带错误文案。

## 7. 回退

还原 `TwoPcRequest`/trait/`execute_transaction`/`execute_superposition_query`
签名即为完整回退；wire 层无持久化数据（`#[serde(default)]` 仅前向兼容），
回退零迁移成本。
