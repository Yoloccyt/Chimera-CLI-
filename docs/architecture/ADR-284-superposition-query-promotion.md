# ADR-284 -- superposition_query 死臂接线转正：`chimera mcp query` 生产出口 + fanout 并发上界

- 状态：Accepted（2026-11-12）
- 决策者：R7 批次（WP-H，用户裁决"接线转正"——三选一裁决中成本最高、价值最大的选项）
- 关联：工单 T2（`agent-runtime-workorders-2026-11-11.md:691-763`，三选一裁决）/ ADR-283（同族结构死臂的**反例**裁决：删除 vs 转正的判据区分）/ ADR-273（`mcp call` 准入前置先例）/ ADR-280 D2（进程内零延迟客户端口径）
- 影响面：`crates/mcp-mesh/src/quantum/superposition.rs`、`crates/mcp-mesh/src/mesh.rs`、`crates/chimera-cli/src/{cli.rs,commands/mcp.rs}`、`crates/chimera-cli/tests/mcp_query_command.rs`
- 行为影响：`superposition_query` 从零调用方转为 `chimera mcp query` 生产命令；fanout 扇出从无上界改为 `Semaphore(16)` 排队钳制。既有 2PC/工具链路径零变化。

## 1. 背景

| # | 事实 | 证据 |
|---|---|---|
| 1 | `McpMesh::superposition_query` 与自由函数 `execute_superposition_query` 均为 `pub`，但**零生产调用方**（仅自身测试引用）——工单 T2 登记的结构死臂，~520 行 | `agent-runtime-workorders-2026-11-11.md` 工单 T2；`grep -rn superposition_query crates/` 仅定义与测试 |
| 2 | 其 `JoinSet::spawn` fanout **无并发上界**——`fanout_servers` 列表长度即任务数，恶意/误配的超大列表会瞬间打满连接与内存 | `superposition.rs:191-206`（改前） |
| 3 | wire 契约完整可用：`SuperpositionQuery`（UUIDv7 query_id / O-4 trace 贯传 / deadline best-effort）+ `QueryResult`（per-server success/latency/payload），且已有事件发布（`McpMessageReceived`）与 SSRF 注册校验 | `superposition.rs:30-93,164-233`；`server_registry.rs` SSRF 段 |
| 4 | 与 ADR-283 的探活死臂性质不同：探活**接上也没有消费者价值**（ephemeral mesh 无长驻收益）；superposition 有**真实对口的用户场景**（同一查询发多服务器聚合） | ADR-283 D1 判据；本 ADR D2 |

## 2. 决策

### D1 — 转正而非删除（与 ADR-283 的判据区分）

三选一裁决（删除 ~520 行 / 登记死臂 / 接线转正）由用户裁定为**接线转正**。与 ADR-283 删除裁决不矛盾——两案的判据是同一条："**接上是否有真实消费者价值**"。探活无价值故删；superposition 的多服务器聚合查询是 mesh 能力面的真实缺口（命令面此前只有单服务器 `call`），故转正。

### D2 — 生产出口：`chimera mcp query <query> --servers a,b [--deadline-ms] [--network]`

- 分类 `NoContext`（只读查询零装配税，与 `list/inspect` 同档；`spec.rs` 既有 `| Mcp { .. }` 通配分支自动覆盖，无新增装配面）。
- 传输与 `mcp call` **同口径**：默认 `InProcessClient`（零网络副作用，进程内模拟载荷）；`--network` 显式开启 `TcpParticipantClient`（真实 `QueryRequest` 帧）。副作用可见性留痕（ADR-245 D4 同口径）。
- **不经 `ToolRuntime::admit` 准入**（与 `mcp call` 的 ADR-273 前置不同）：admit 判决对象是 `tool_name`，查询没有工具名可匹配；查询的边界防护由注册面 SSRF 校验 + `--network` 显式开关承担。此差异在此显式登记，防后续读者误当缺口。
- 输出：表格（Server ID / 状态 / 延迟 / 载荷截断 48 字符）与 `--json`（typed envelope，含 `total/succeeded/failed` 与部分超时提示）双形态，沿用 `output::emit` 既有模式。

### D3 — fanout 并发上界：`Semaphore` 排队而非拒绝

`execute_superposition_query` 新增 `max_concurrency: usize` 参数，`JoinSet` 任务内 `acquire_owned` 取许可：

- **排队语义**：超出上界的 server 等待许可，全部仍会执行（deadline 语义不变——排队等待可能使其超时标记 `success=false`，属合理资源保护，不破坏 best-effort 聚合契约）。
- 默认值 `DEFAULT_MAX_QUERY_FANOUT = 16`（常量，生产 mesh 与 CLI 共用）；`0` 按 `1` 钳制（防御零并发）。
- WHY 任务内取许可而非 spawn 前逐个 acquire：排队等待发生在 JoinSet 任务里，deadline 超时 `abort_all` 时可一并终止，不留悬挂等待。
- WHY 不加进 `MeshConfig`：查询并发度当前单一常量足够，配置字段会重演 ADR-283 删除的"无人读字段"风险；需要可配时再按真实需求演进。

### D4 — 诚实边界

默认 InProcess 传输下 `QueryResult.payload` 是**进程内模拟载荷**（`result@<server>` 格式），非真实业务响应——与 `mcp call` 的 D8 诚实声明同族（ADR-256 D8）。真实业务查询须 `--network` + 真实监听端点。

## 3. 验证

- mcp-mesh lib **119 passed / 0 failed**（含新增并发峰值测试：15 servers + 上界 4 ⇒ 峰值 ≤ 4 且全部完成；0 上界钳制为 1）
- chimera-cli `mcp_query_command` **5 passed / 0 failed**（分类锁 / 生产路径锁 / 聚合成功 / 未注册拒绝 / 空参数拒绝）+ `mcp_command_admission` 5/5 回归
- clippy `-D warnings` 零告警；fmt 干净（本 WP 触及文件）

## 4. 回滚

单 commit revert。`execute_superposition_query` 签名变更的调用方仅 mesh.rs 一处 + 自身测试；CLI 侧 `McpAction::Query` 移除后 clap 帮助面自动收敛，无配置/持久化残留。
