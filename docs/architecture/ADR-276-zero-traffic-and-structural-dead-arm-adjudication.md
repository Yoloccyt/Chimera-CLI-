# ADR-276 -- 结构死臂与零流量事件面的维持裁决（W10）

- 状态：Accepted（2026-11-09）
- 决策者：R6 / W10
- 关联：ADR-259（U 项三态裁决 / D9 零新增事件变体）、ADR-256（D7/D8 诚实零流量登记）、ADR-257 D3（subagent 驱动留后续）、ADR-211（mas-sched 控制面）、ADR-234（证据分层）
- 影响面：`permission.rs`、`commands/mcp.rs`、`mcp_tool_executor.rs`、`efficiency-monitor/src/auditor.rs`（**仅注释**）、新增 `crates/chimera-cli/tests/zero_traffic_adjudication_locks.rs`
- 行为影响：**零**（只新增注释与结构锁）

## 1. 背景

本轮全景审计反复命中三类「看起来像缺陷」的形态。若不给它们一个**可机检的裁决锚点**，
下一轮审计会把它们重新提成「待修」，而修法（删臂 / 接总线 / 加消费者）
恰好都会让系统更差。本 ADR 的决定是**维持**，并把理由与锁落到代码里。

## 2. 决策

### D1 `PermissionGate::evaluate` 的 Deny 臂：保留

现状：`evaluate` 只产 `Allow` / `Unknown`；`authorize` 的 `Deny` 臂生产不可达。
**唯一的生产 Deny 源**是 `authorize` 中「非交互 + Unknown ⇒ fail-closed 降级」。

保留理由：

- 三态枚举是**公开契约**（`PermissionDecision`）。删 Deny 臂会把「未知」与「拒绝」
  压成两态，等于宣称「本系统永不规则式拒绝」—— 而规则式拒绝属策略层（ExecPolicy），
  将来接入时无处安放；
- `downgraded: bool` 已经承担了「区分 Deny 来源」的职责，不需要删臂来「澄清」；
- 删臂不会减少任何运行期成本（match 臂是编译期的）。

锁：`permission_deny_arm_is_structural`（行为 4 断言 + 源码臂在位）。

### D2 `McpMesh` 生产构造的 `event_bus` 恒 `None`：保留

现状：`commands/mcp.rs`（2 处）与 `mcp_tool_executor.rs`（1 处）都传 `None`，
故 `McpMeshTransactionCompleted` / `McpMessageReceived` 零触发。

保留理由：

- 这两处 mesh 都是**一次性 ephemeral 实例**，没有长驻订阅者。接总线只会造出
  「有发布、无订阅」的零流量事件 —— 正是 U-10 禁令（不为绿造无消费者构造点）；
- 诊断面若需要 mesh 事故事件，应接**长驻宿主**（tui / serve）的 mesh，
  而不是在一次性命令面补一个 `None → Some`。

锁：`mcp_mesh_event_bus_none_is_deliberate`（三处构造点全部 `None` + 就近登记标记）。

### D3 `AgentTaskDelegated` 消费面：维持预留

现状：生产有两个发出方（`chimera-mas/agent/factory.rs`、`nexus-app-server/subagent_engine.rs`），
消费方只有 `commands/agent.rs` 的 **`#[cfg(test)]`** 订阅（`:443`，用于证明事件可见）。

维持理由：提升为生产消费者需要真实消费语义（谁接、接了什么动作）。当前没有 ——
为「事件有人听」而造一个空消费者，等于把零流量从 producer 侧挪到 consumer 侧。
（ADR-257 D3 已裁：subagent 驱动入口留后续波次。）

### D4 `mas-sched` 默认档位：不翻

现状：`CHIMERA_SCHED_CLAIM` 三态，默认 `off`；`agent spawn` 是唯一真实入口。

维持理由：翻默认需要先测「每次 spawn 的 claim µs 税」；本轮未测。
无实测就翻默认是**把控制面风险引入默认路径**（ADR-239 同纪律）。

### D5 审计第 6 维：把「无载体」写在脸上

现状：`budget_discipline` 在 `total_consumption <= 0` 时返回 `NEUTRAL_SCORE`。
DECB 写侧（`record_cost` / `record_consumption`）生产零构造点 ⇒ 该维**必然** NEUTRAL。

决定：**不改评分**（NEUTRAL 的证据纪律是对的），但在注释中显式写「无载体」，
使 `NEUTRAL` 不被读成「预算纪律合格」。这是**账面诚实化**，不是接 DECB 写侧
（后者属独立裁决）。

### D6 纪律：零新增 `NexusEvent` 变体

本 ADR 不新增任何事件变体（154 不变）、不改 `event_ownership` 四段计数。
任何「为了让某维有数据」而新增变体的提案，须走 ADR-259 D9 的六锁同批。

## 3. 验收（现跑）

| 判据 | 要求 | 结果 |
|---|---|---|
| `zero_traffic_adjudication_locks` | 3/3（D1 行为+结构 / D2 三点位 / D5 标记） | ✅ 3 passed |
| `event_ownership` 四段 | 154 / 136 / 18 / 18 不变 | ✅（见终验门读数） |
| `variant_count_test.rs` | 154 不变 | ✅ |
| `adr_index_sync.py` | rc=0 | ✅ |

## 4. 回滚

纯注释 + 测试，单 commit revert。**不**建议回滚「维持」裁决本身 ——
三种修法都比现状差（见 §2 各条）。

## 5. 未采纳

| 项 | 结论 |
|---|---|
| 删除 `authorize` 的 Deny 臂「清理死码」 | 否决：三态契约坍缩，且省不下运行期成本 |
| 给命令面 mesh 接总线 | 否决：一次性实例无订阅者，零流量变「半接」 |
| 造一个生产版 `AgentTaskDelegated` 消费者 | 否决：无真实消费语义（U-10） |
| 翻 `CHIMERA_SCHED_CLAIM` 默认为 on | 否决：claim µs 税未实测 |
| 把第 6 维改成「无数据 ⇒ Fail」 | 否决：Fail 会把「没接」报成「故障」，与 V6「未启用非故障」同源纪律冲突 |
