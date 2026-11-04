# ADR-235：MCP 语义层与能力发现（工具链第五波）

- **状态**：已接受（2026-11-04）
- **范围**：`crates/mcp-mesh` 新增 MCP JSON-RPC 语义层（`initialize` / `tools/list` 含分页 / `tools/call`）、stdio 传输、发现快照到 L7 `ToolDescriptor` 的映射、`McpServerConfig` 配置面扩展
- **明确不含**：HTTP / Streamable HTTP 传输、真实 spawn 白名单政策变更（`npx`/`node`/`python`）、新的 `NexusEvent` 变体、mesh 既有 2PC 协议的改造
- **相关**：`ADR-233` 追加节六（W4b 成本评估）与追加节十（本波收口）、`ADR-232`（审批分档与 fail-closed）、`ADR-230`（mesh 现有 RPC 决策）、`ADR-229` / `ADR-223`（不为工具调用新增事件）、`ADR-234`（性能/并行判据层，本波不涉性能结论）、`agents.md §2.2 / §4.4 / §6.2`

---

## 1. 背景：为什么必须"另立子系统"而不是补一根线

`ADR-233` 追加节六已经把前提取证钉死：`mcp-mesh` 只有**自研 2PC 注册表**（`TwoPcRequest` / `QueryRequest` / `ToolInvokeRequest`，4 字节长度前缀），`grep -rn "jsonrpc" crates/mcp-mesh/src` 零命中；`ParticipantClient` 没有任何能力列举方法；`client_v2.rs` 头注自陈 2026-08-28 事故后只重建了三个数据结构骨架。
因此"能力发现"不是接线问题，而是**一个新子系统**：需要自己的握手、分帧、能力形状与失败语义。

本波交付它，但**只交付语义层与传输接缝**：

- 语义层是真的（编解码 + 握手 + 分页 + 调用 + 超时/EOF/畸形帧）；
- 传输接缝是真的（`McpTransport` trait + 进程内 `DuplexTransport` + 可用的 `StdioTransport`）；
- 但**默认不启用任何服务器**（`enabled` 默认 false），HTTP 不实现，spawn 白名单不放宽 —— 这三条把"安全姿态"与"能力交付"解耦，使本波可以独立回滚。

---

## 2. 决策

### D1 — 副作用分类：默认全 `Write`；`readOnlyHint` 只在逐 server 显式信任时采信

`readOnlyHint` 是**服务器自报的 hint**，不可信。规则集中在一个纯函数
`mcp_mesh::protocol::side_effect_for(tool, trust)`：

| 条件 | 结果 |
|---|---|
| `trust_readonly_hints == false`（默认） | `Write`（无论 annotations） |
| `trust == true` 且 `read_only == true` 且 `destructive != true` | `ReadOnly` |
| 其余（含无注解、readOnly+destructive 矛盾） | `Write` |

默认路径与 `tool_plane_assembly.rs` 既有断言（"MCP 能力默认必须是 Write"）**逐位一致**，本波没有放宽任何一条审批。

### D2 — 交付边界：语义层 + 传输接缝；stdio 实现但默认不启用；HTTP 本波不做

- `StdioTransport` 已实现（换行分帧、`kill_on_drop`、出站前过 `CommandGate`），但只有配置 `enabled = true` 才会被装配调用；
- HTTP / Streamable HTTP **未交付**；配了 `url` 而没有 `command` 的 server 会被显式 `warn` 后跳过，**不静默当成"发现成功但零工具"**；
- 真实 MCP 服务器（`npx` / `node` / `python`）当前会被 SecCore `ProcessFence` 拒绝 ⇒ 这是**政策未裁**，不是缺陷。放宽白名单是安全姿态变更，须单独 ADR 与签字。

### D3 — 位置：语义层放 `crates/mcp-mesh/src/protocol/`，mesh 既有 2PC 一行不动

同层依赖（L10 → L10）允许。`participant_client` 与 `quantum/*` 未被本波触碰，避免两条协议耦死。`protocol/` 不依赖任何上层 crate。

### D4 — 协议与分帧钉死

- 请求 `protocolVersion = "2025-03-26"`；服务器返回的版本必须落在 `SUPPORTED_PROTOCOL_VERSIONS`（当前仅该值）内，否则 `McpError::UnsupportedVersion`，**fail-closed 不降级猜测**；
- stdio 用**换行分隔 JSON**（MCP 规范），不复用 mesh 的 4 字节长度前缀；
- 单帧上限 `MAX_FRAME_BYTES = 8 MiB`；半帧 EOF 必须报错（`mid-frame`），不得被当成完整消息。

### D5 — 零新事件

发现与调用失败只走 `tracing` + `McpError` / `ToolError`，不新增 `NexusEvent` 变体（`ADR-229:70`、`ADR-230:41`、`ADR-223:33` 的七点位约束）。
现取：`grep -rn "NexusEvent::" crates/mcp-mesh/src/protocol crates/chimera-cli/src/tool_plane` = **0**（唯一 `NexusEvent` 字样是 `protocol/mod.rs` 的文档注释，声明"不发布"，非生产者）。

### D6 — 不可达 ⇒ 跳过 + warn，不阻塞启动

单个 server 的建连/握手/发现失败只影响它自己；装配必须成功返回。`McpPlane::empty()` 是默认路径（无 `enabled` ⇒ **不建 runtime、零成本**）。

### D7 — 装配是同步的，发现用自持 runtime 完成

组合根 `build_tool_runtime` 是同步函数（多处同步测试在调），MCP 发现必须在 Tokio runtime 上跑。做法：仅当存在 `enabled` 的 server 时建一个 **2 worker** 的 runtime，`block_on` 完成发现，并把 runtime 与快照一起留在 `McpPlane` 里 —— 客户端任务跑在该 runtime 上，`McpPlane` 被 drop 时任务与子进程随之结束。
WHY 不把组合根改成 async：会波及 `OnceLock` 缓存与全部调用点，收益只是省一次线程阻塞，而阻塞只发生在**真的配置了 MCP** 时，且每个 server 有超时上限。

### D8 — 配置面：四个新字段，全部 `#[serde(default)]`

`McpServerConfig`（`crates/nexus-core/src/config.rs:607`）新增：

| 字段 | 类型 | 默认 | 语义 |
|---|---|---|---|
| `env` | `BTreeMap<String,String>` | 空 | 在继承白名单之外显式注入的环境变量 |
| `timeout_ms` | `u64` | 0 | 单请求超时；0 = 默认 30s |
| `trust_readonly_hints` | `bool` | **false** | 是否采信该服务器自报的 `readOnlyHint`（D1） |
| `enabled` | `bool` | **false** | 是否启用（D2） |

旧 YAML（无新字段）必须能反序列化并落到最保守默认；`default_mcp_servers()` 的三条全部 `enabled = false`。

---

## 3. 接口与实现映射

| 关注点 | 落点 |
|---|---|
| 协议常量（版本/客户端身份/帧上限） | `crates/mcp-mesh/src/protocol/mod.rs:35-38`、`protocol/transport.rs:30` |
| JSON-RPC 四形态 + `id` 双类型 + 未知字段保留 + 畸形帧分类 | `crates/mcp-mesh/src/protocol/jsonrpc.rs` |
| 传输 trait / 进程内双工 / stdio（fence 先于 spawn） | `crates/mcp-mesh/src/protocol/transport.rs`（`CommandGate` :33） |
| 客户端（读端单任务 + `HashMap<RequestId, oneshot>` + 超时摘除 + EOF 批量失败 + 幂等 shutdown） | `crates/mcp-mesh/src/protocol/client.rs` |
| 发现快照 / `McpTool` / `ToolAnnotations` / 唯一副作用判定点 | `crates/mcp-mesh/src/protocol/discovery.rs` |
| Provider：快照 → `mcp:<server>/<tool>` 描述符；`!approved → AskRequired` | `crates/chimera-cli/src/tool_plane/mcp.rs` |
| 装配期发现（`FenceGate` / `McpPlane` / `assemble` / `discover_one`） | `crates/chimera-cli/src/tool_plane/mcp_connect.rs:33/64/91/144` |
| 组合根接入（`SharedToolPlane._mcp` 保活） | `crates/chimera-cli/src/composition.rs:321/375-379/417` |

命名口径保持 `mcp:<server_id>/<tool_name>`（既有断言不改口径）。

---

## 4. 验证

### 4.1 测试清单（本轮现取）

| 包 / 目标 | 结果 |
|---|---|
| `cargo test -p mcp-mesh`（10 target） | **176 passed / 0 failed / 2 ignored**（ignore 为既有：`integration.rs` 的性能用例 + 1 个 doctest），rc=0 |
| `cargo test -p chimera-cli`（23 target） | **418 passed / 0 failed**，rc=0 |
| `cargo test -p nexus-core` | **273 passed / 0 failed**，rc=0 |
| `mcp_protocol`（12）/ `mcp_client`（11）/ `mcp_stdio`（6）/ `mcp_discovery`（3） | 全绿 |
| `mcp_discovery_assembly`（8） | 全绿；含**端到端正控**（bash 假服务器 spawn → initialize → tools/list ⇒ 1 工具）与**间谍闸门**（断言以原样程序名+参数调用恰一次） |
| `mcp_approval_surface`（3） | 全绿；含"降级 ≠ 免审批"（三工具未审批一律 `AskRequired`） |
| `tool_plane_assembly`（4）/ `composition_root_e2e`（7） | 全绿 |

### 4.2 反证（每条判据都有牙，复原后 sha256 自证）

| 判据 | 反证动作 | 观测 |
|---|---|---|
| stdio 的 fence 先于 spawn 且拒绝被当真 | 反证一：`FenceGate::check` 恒放行 ⇒ 两条 gate 测试红（`calls=[]`；被拒服务器产出 1 工具）；反证二：`discover_one` 换恒放行闸门 ⇒ 同两条红，失败信息精确点名 | 两次红；复原后 `mcp_connect.rs` sha256 = `48804d0c…`（与探针前一致） |
| 默认档全 `Write` | 装配层强制 `trust_readonly_hints: true` | `default_assembly_keeps_every_mcp_tool_write` 红（`left: ReadOnly / right: Write`）；复原 sha256 一致 |
| 信任档只降非破坏只读 | 删 `side_effect_for` 的 `&& !destructive` | `trusted_server_only_downgrades_non_destructive_readonly` 红（`sneaky_write` 变 `ReadOnly`）；复原 sha256 = `a91c4c63…` |

### 4.3 静态面

- `cargo clippy -p chimera-cli -p nexus-core -p mcp-mesh --all-targets --jobs 2 -- -D warnings`：rc=0（分轮执行）；
- `cargo fmt -p <pkg> -- --check`：rc=0（root `--all` 在本机有 `os error 206`，按包逐验）；
- `bash scripts/test_fnlen_smoke.sh`：rc=0；
- `NexusEvent::` 生产引用：0（D5）。

---

## 5. 证明力上限（必须与结论同读）

1. **HTTP / Streamable HTTP 未交付**：配置了 `url` 的 server 一律跳过；"MCP 客户端"在本波的含义仅限 stdio。
2. **真实 MCP 服务器默认不可用**：`npx` / `node` / `python` 不在 SecCore 白名单 ⇒ 现网配置会得到 `Denied` + warn。可用路径是：在 `enabled = true` 且命令落在白名单内时，手工验证（本波用 `bash` 假服务器覆盖了同一传输路径）。
3. **CI 侧未跑**：本波所有读数来自本地工作树；`bench_check.yml` 的 I10/I11 与 hnsw 阈值首跑在 CI，本地不可判定。
4. **heavy 门与 workspace 全量回归仍受磁盘约束**（D 盘现取约 9.5 GiB < 20/25），本波同前几波只能由 CI 判定。
5. **不证明审批面"更安全"**：只证明**未削减**（默认全 Write、未审批一律 `AskRequired`、fence 先于 spawn）。真正的收紧（例如把 `readOnlyHint` 写进策略）不在本波。

---

## 6. 遗留与后续

| 项 | 状态 | 前置条件 |
|---|---|---|
| spawn 白名单政策（放行 `npx`/`node`/`python`） | **未裁** | 单独 ADR + 签字；须给出可执行文件的 pin/校验方案 |
| HTTP / Streamable HTTP 传输 | 未做 | 需要端点 SSRF 校验与 TLS 口径；与 `register_server` 的 SSRF 守卫不是同一条路径 |
| `readOnlyHint` 的更强语义（例如把 annotations 映射进策略层） | 未做 | 需要策略层裁决；D1 是保守下界 |
| `docs/architecture/adr_index.md` 回写 | 待授权 | 索引只在主检出；`adr_index_sync.py` 零门零 workflow 引用（任务 #33） |
| hnsw 阈值收紧（8ms → 2ms 档） | 待 CI 首跑 | 拿到真实 median 后单独提交 |
| `commit_retry_within_default_budget` 的 200ms 墙钟预算 | 已登记 | 本波 96% 内存压力下失败过一次；隔离 3/3 与后续全量均通过。墙钟断言在压力下天生脆，建议波次 6 改为"重试次数/事件"判据 |
| 三条慢门（G-07 / G-08 / G-36）的子进程批处理 | 波次 6 候选 | 实测 107.3s / 157–223s / 82.6s（G-36 的 `--selftest` 82.6s vs 真跑 13.6s） |
