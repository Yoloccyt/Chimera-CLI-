# ADR-256: McpToolExecutor 接入四环传输环（P1 / F-10 主体）

- **状态**：Accepted（2026-11-08，Tranche 2 工单 D；deepdive §6 规格照单执行）
- **上游**：deepdive §6（五元组 + §6.3 接口签名 + §6.4 TDD 矩阵 + §6.5 对照 + §6.6 回滚）
- **关联**：ADR-244（组合根唯一装配）/ ADR-253（零 JSON-RPC 边界调和）/ ADR-254（能力表语义）/ ADR-246（policy 声明式源，Z-1 口径）

## 1. 背景

工具执行面与 MCP 面此前未打通：`DispatchRequest` 无法路由到 MCP 参与者。
P1 目标 = `mcp.<server>.<tool>` 形态工具调用接入四环（policy → sandbox →
route → transport），前置完整无旁路；**零新增事件变体、零新增 crate、零
JSON-RPC 栈**（ADR-253 边界）。L10→L7 向下依赖合法（ToolExecutor trait）。

## 2. 决策（D1~D8，deepdive §6.7 摘要照单）

- **D1** server_id 白名单 `[A-Za-z0-9_-]{1,64}` + **首点切分**（A 案采纳）：
  配置层 id 无校验（nexus-core/src/config.rs:682 McpServerConfig），解析消歧
  在执行器入口收紧、不动 L1 schema（避免 parity 门重派生）。连续点号归入
  IllegalServerId（白名单排除点号的消歧前提被破坏）。
- **D2** 独立 struct `CompositeToolExecutor` + 前缀分派（非 enum dispatch：
  两分支生命周期/构造依赖不同）；能力/路由表**只转发 Builtin**——mcp 分支
  `None`/空集是诚实声明（副作用未知 fail-closed、`mcp.*` 不进路由表恒走
  fallback）。
- **D3**（Z-2 订正采纳）：解析错误机制归因 `DispatchError::Transport`、文案
  前缀保留 `route:`；`McpRouteErrorKind::ServerNotRegistered` 从枚举移除
  （parse 纯函数查不了注册表——该 variant 不可达），「server 未注册」由
  execute() 第 3 步内联 `Err` 文案承载（同 `route:` 前缀、不同机制）。
- **D4** 传输缺省恒 `InProcessClient`（零网络副作用，与 commands/mcp.rs
  `build_mesh(false)` 同范式）；本轮不引入新配置段。
- **D5** 非 CLI 上下文不直调 permission::confirm——破坏性操作确认由 policy 环
  Ask 语义承担（组合根 `tool_runtime_policy` 规则可定向 `mcp.<server>.<tool>`
  **全名**）。
- **D6**（Z-1 订正采纳）：`RulePattern` 的 program 匹配是**精确小写**
  （execpolicy.rs:153），`mcp.*` 通配不可行——policy 定向须写全名（样例
  `{ pattern: "mcp.github.create_issue", action: ask }`）。
- **D7** ADR-253 边界调和：复用同一 2PC 参与者面（`execute_transaction`），
  两面共享 registry 与 SSRF 守卫（server_registry.rs validate_endpoint/
  is_reserved_ip）；`mcp serve` 保持 NotImplemented（显式排除）。
- **D8 · 诚实性声明**：默认档 InProcessClient prepare/commit 恒 ACK ⇒
  成功回执是**协议级 ACK 非业务级工具结果**——回执 = 单层 JSON
  `{success, transaction_id}`；真实业务结果待真实 server 接入批次。
  本 ADR 明文登记以防「mcp 调用已可用」的过度宣称。

## 3. 实施

| 件 | 位置 |
| --- | --- |
| `McpToolExecutor`（from_config 注册语义逐字复用 mcp.rs:102-137 + parse_tool_name 纯函数 + execute 五步流） | `crates/chimera-cli/src/mcp_tool_executor.rs`（新建，lib.rs 挂载） |
| `CompositeToolExecutor`（前缀分派 + 能力/路由表仅转发 Builtin） | 同上 |
| 组合根装配（唯一装配点不变，ADR-244 D1） | `composition.rs` tool_delegate 行 |

**ExecutionCompleted 澄清（〔修订·审读〕条款照录）**：该变体的生产发布者是
quest-engine 的 Quest 完成路径（quest-engine/src/engine.rs:544/:553），不在
四环派发链上——P1 **零新增发布点、不为 mcp 调用虚构发布**；执行观测面 =
错误归因文案 + 批量计数器（ADR-248 Sink）。

## 4. 验收（deepdive §6.7 命令套件全绿，2026-11-08）

- 单元 7/7（parse 边界 4 + 诚实语义 1 + 复合转发 1 + **prop_parse_boundary
  512-case 任意串分派等价 proptest**）
- 集成 8/8（未注册 server route 文案 / InProcess 零网络协议 ACK / unknown
  builtin 回归 / policy 定向拦截 / sandbox 注入拦截 / 组合根唯一装配反向 /
  零第二 JSON-RPC 栈 / **prop_no_network_on_default 64-case**）
- 回归四套件：exec_plan_wiring 18 + tool_runtime_wiring 8 + tool_policy_config
  7 + tool_runtime_borrowed_equivalence 5 全绿
- check_composition_root.py：canonical 7 点 + 0 bypass 不变（ASSEMBLY_CALL
  正则为固定类型清单，CompositeToolExecutor::new 不在其中）
- invariants I1-I10 PASS / event_ownership [OK]（154 不变）/ clippy -D
  warnings 0 告警 / fmt 收敛
- 启动分段对照：`from_config` 仅读 config + 空注册表构造（cli_startup_config
  现键守护，无 criterion 新键——P1 无性能主张）

## 5. 回滚

P1 单 commit 独立可 revert：composition.rs 回 `Arc::new(BuiltinToolExecutor)`
单体、mcp_tool_executor.rs 随 commit 消失；`tool_runtime_policy` /
`with_optional_delegate` 签名零改动。语义回退面 = `mcp.*` 调用回退为
`unknown builtin tool` Err（非安全层降级——四环顺序/缺环构造/fail-closed
不动）。配置面零残留（from_config 只读 config.mcp.servers，无新增配置段）。
