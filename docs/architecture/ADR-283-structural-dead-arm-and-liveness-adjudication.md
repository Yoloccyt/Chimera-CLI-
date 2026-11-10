# ADR-283 -- 结构死臂与活性裁决：删除零调用后台探活 API；action loop 降级维持

- 状态：Accepted（2026-11-10）
- 决策者：W5 波次
- 关联：ADR-251 D-W5-A（action loop 有界并发 + 降级取舍）/ ADR-255（死配置字段删除先例）/ ADR-257 D3（SubAgent async 独立工单）/ ADR-262 §4（轮询治理前提被证伪）
- 影响面：`crates/mcp-mesh/src/{mesh.rs,config.rs}`；`docs/architecture/ADR-283-*`
- 行为影响：删除从未被调用的 `McpMesh::start_background_probe` 与 `MeshConfig::background_probe_interval_ms`。旧配置文件含该字段仍可反序列化（无 `deny_unknown_fields`）。action loop 零变化。

## 1. 背景：一个「在册但从不运行」的周期任务与一个已裁决的取舍

| # | 事实 | 证据 |
|---|---|---|
| 1 | `McpMesh::start_background_probe()` 是 `pub`，但全仓**零调用方**（`src`/`tests`/`benches` 均无） | `crates/mcp-mesh/src/mesh.rs:1061`（改前）；`grep -rn start_background_probe crates/` 仅定义处 |
| 2 | 它唯一的配置读者 `background_probe_interval_ms` 也只被它读取；默认 60s，字段无任何实际效果 | `crates/mcp-mesh/src/config.rs:68,81`（改前） |
| 3 | ADR-262 §4 曾把它记为「正当周期探活任务」，但实测它从未启动 —— 该记录为**幽灵活动** | `docs/reports/toolchain-w4w5-r1r5-execution-2026-11-08.md` 同族；本轮 grep 复核 |
| 4 | action loop 的「许可耗尽 → recv 循环内同步执行」是 ADR-251 的**已登记取舍**，不是缺陷 | `crates/chimera-cli/src/action_orchestrator.rs:398-407,423-437` |
| 5 | 既有测试已证明降级路径**恰处理一次**：bound=2、N=10、每个 10ms，断言每 request_id 去重后 = N，且在飞峰值 ≤ bound+1 | `action_orchestrator.rs:1227-1291`（`bounded_loop_caps_inflight_and_delivers_exactly_once`） |

## 2. 决策

### D1 — 删除零调用探活 API 与配置字段

删除 `McpMesh::start_background_probe` 与 `MeshConfig::background_probe_interval_ms`。

- **不接线**：接上它只会造出一个「有 spawn 无消费者价值」的周期任务（注册表的僵尸清理在
  ephemeral mesh 中无意义：mesh 与进程同生命周期，且 heartbeat 在 `is_alive` 读取时已足够）。
- **不登记为死臂**：死臂登记适用于「因结构原因必须保留的空分支」；这里是一个可删的
  `pub` API + 一个无人读的配置字段，保留只会让下一个读者以为它生效。
- **向后兼容**：`MeshConfig` 无 `deny_unknown_fields`，旧 JSON 含该字段仍可反序列化；
  `test_config_serde_backward_compatible_wal_path_default` 保留旧字段并断言可读，
  额外锁住「删除字段不破坏旧配置」。

### D2 — action loop 降级：维持，不改

- 既有测试已证明：许可耗尽时在 recv 循环内同步执行**不丢事件**（N=10、bound=2，恰处理一次，
  峰值 ≤ bound+1）。
- 只有超过 broadcast 容量（1024）的**突发**才会触发 `SlowConsumerDropped`；TUI 动作是
  用户交互级速率，远低于该量级。**无丢事件证据** ⇒ 维持 ADR-251 的取舍。
- 若未来出现「>1024 动作突发」的真实场景，再按 ADR-251 的重开方式（独立 worker / 异步
  许可 acquire）另立 ADR，不在本波顺手改。

### D3 — 明确不做

| 不做 | 理由 |
|---|---|
| SubAgent `block_on` async 化 | ADR-257 D3 已登记独立工单；无 ≥5% 证据 |
| 轮询点计数棘轮门 | ADR-264 自陈 advisory；计数不区分 60s 心跳与 1ms 自旋，是坏代理 |
| `expert_consult` / `delegation` 的既有取舍 | 无丢事件/超时证据，改动是猜测 |
| 为探活 API 造一个消费者 | 违反 U-10「不为绿造无消费者构造点」 |

## 3. 验收（现跑）

```
grep -rn "start_background_probe\|background_probe_interval_ms" --include=*.rs crates/
  → 仅 config.rs 的向后兼容夹具注释与 JSON 字段（刻意保留）
cargo test -p mcp-mesh --lib --jobs 2          # 114 passed
cargo clippy -p mcp-mesh --all-targets --jobs 2 -- -D warnings   # rc=0
rustfmt --edition 2021 --check mesh.rs config.rs                 # rc=0
```

`bounded_loop_caps_inflight_and_delivers_exactly_once` 作为 D2 的证据：本轮未修改
`action_orchestrator.rs`，该测试保持原样通过（若未来改动降级路径，此测试是回归网）。

## 4. 回滚

单 commit revert：恢复 `start_background_probe` 函数、`background_probe_interval_ms`
字段与默认值、恢复 config 测试中的字段断言。revert 后零行为变化（该 API 本就无人调用）。

## 5. 证据边界

- 删除的是 `pub` API：SemVer 上属破坏性变更，但该 crate 是 workspace 内部 crate 且
  全仓零调用方（含 tests/benches）；若外部消费者存在，`cargo` 编译会直接报错，不会静默。
- action loop 的「无丢事件」仅覆盖 N=10 的非突发场景；>1024 突发的行为是 broadcast
  的既有属性，本 ADR 不声称已消除它，只声称「无证据表明现实中会发生」。
