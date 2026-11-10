# ADR-280 -- 调用面虚构开销：进程内零延迟客户端 + 非 TTY 流式节流归零

- 状态：Accepted（2026-11-10）
- 决策者：W2 波次（承接 ADR-239「先测后改」、ADR-277「不重提已否决项」）
- 关联：ADR-256（McpToolExecutor 四环接入 / 默认 InProcessClient）、ADR-253（2PC 传输）、ADR-276（ephemeral mesh 的 `event_bus=None`）
- 影响面：`crates/mcp-mesh/src/quantum/participant_client.rs`、`crates/mcp-mesh/benches/mesh_benchmark.rs`、`crates/chimera-cli/src/{mcp_tool_executor,orchestrator}.rs`、`crates/chimera-cli/src/commands/{mcp,run,chat}.rs`
- 行为影响：`mcp call` 的进程内路径不再付模拟 RTT；非 TTY（管道/脚本）下 `run`/`chat` 的流式节流自动归零。TTY 下输出逐字节不变。测试夹具的模拟延迟保留（`InProcessClient::new()` = 旧行为）。

## 1. 背景：两处「不做功的耗时」

| # | 事实 | 证据 |
|---|---|---|
| 1 | 生产默认 `InProcessClient` 的 `prepare`/`commit` 各硬编码 `sleep(1–2ms)`；一次 `mcp call` = 2 次 | `crates/mcp-mesh/src/quantum/participant_client.rs:256-286`（改前） |
| 2 | 两个生产装配点都用它：组合根 `McpToolExecutor::from_config` 与 `commands/mcp.rs::build_mesh`（`use_network=false` 默认） | `crates/chimera-cli/src/mcp_tool_executor.rs:86-89`；`crates/chimera-cli/src/commands/mcp.rs:96`（改前） |
| 3 | `run` 流式每 chunk 固定 20ms，仅 env `CHIMERA_RUN_CHUNK_DELAY_MS` 可关；**无 TTY 检测** | `crates/chimera-cli/src/commands/run.rs:345-372`（改前） |
| 4 | `chat` 同型（`CHIMERA_CHAT_CHUNK_DELAY_MS`），同样无 TTY 检测 | `crates/chimera-cli/src/commands/chat.rs:247-252`（改前） |
| 5 | `recover_from_wal` 全仓零调用 ⇒ WAL 在进程内路径是「只写不读」；但关 `durable` 会一并撤掉写入前的敏感载荷扫描 | `crates/mcp-mesh/src/mesh.rs:844`；`scripts/`/`crates/` grep 仅定义处；`wal.rs:103-114` |

进程内没有网络，模拟 RTT 是**虚构工作**；非 TTY 下逐 chunk 节流是**观感节流**，对管道消费不做功、只拉长墙钟。

## 2. 决策

### D1 — `InProcessClient::instant()`：零延迟构造

- `new()` / `Default` **保留** 1–2ms 模拟延迟（测试夹具依赖「不同 server_id 有不同延迟」的确定性）。
- `instant()` 返回零等待；`delay_for()` 返回 `Option<Duration>`，`instant()` 恒 `None`。
- 不引入分支热路径开销：`Option` 在每次 phase 判一次，可忽略。

### D2 — 生产装配唯一构造点 `in_process_mesh()`

`crates/chimera-cli/src/mcp_tool_executor.rs` 新增 `pub(crate) fn in_process_mesh()`：
`InProcessClient::instant()` + `MeshConfig::default()` + `event_bus=None`（ADR-276 D2）。
`McpToolExecutor::from_config` 与 `commands/mcp.rs::build_mesh(false)` 都改用它。
`MeshConfig::default()` **不变**：WAL durability 与写入前的敏感载荷扫描保留。

### D3 — 流式节流：`env > TTY > 默认`

`crates/chimera-cli/src/orchestrator.rs` 新增：

```rust
pub(crate) fn resolve_stream_chunk_delay(env_value: Option<&str>, stdout_is_tty: bool, default_ms: u64) -> Duration
pub(crate) fn stream_chunk_delay(env_key: &str, default_ms: u64) -> Duration
```

- `run` / `chat` 改为调用 `stream_chunk_delay`；`chat` 的 `chunk_delay_from_env` 退化为薄包装。
- 非 TTY ⇒ `Duration::ZERO`；TTY ⇒ 默认 20ms（输出逐字节不变）；env 显式值始终优先。

### D4 — 明确不做

| 不做 | 理由 |
|---|---|
| `MeshConfig::default().durable = false` | 会撤掉 WAL 写入前的敏感载荷扫描（`wal.rs:103-114`），属安全回归，需另立 ADR |
| WAL group commit | 单事务无 batch 可合；`recover_from_wal` 零消费者（先有恢复路径再谈合批） |
| 移除 2PC / policy / sandbox 环 | 与 ADR-256 的准入语义冲突；本轮只删虚构延迟 |

## 3. 实测（本机 Windows GNU，`--noplot --quick`，criterion median）

新增同形对照组 `mcp_mesh_transaction_instant/{2pc,n}`（`instant()`）与既有
`mcp_mesh_transaction/{2pc,n}`（`new()` 模拟延迟）A/B：

| 服务器数 n | 模拟延迟（对照） | 零延迟（生产） | 变化 |
|---|---|---|---|
| 1 | 13.954 ms | **2.9660 ms** | **−78.7%**（−10.99 ms） |
| 3 | 13.834 ms | **2.6961 ms** | **−80.5%**（−11.14 ms） |
| 5 | 14.221 ms | **2.8199 ms** | **−80.2%**（−11.40 ms） |

**关键发现**：名义延迟只有 1–2ms × 2 = 2–4ms，但实测差 ~11ms。根因是
**Windows 定时器粒度**（约 15.6ms 的 default timer resolution）让短 `sleep`
按 tick 取整；`tokio::time::sleep(1ms)` 在 Windows 上的实际墙钟远大于 1ms。
⇒ 「2–4ms 名义」在 Windows 上的真实代价是 ~11ms，删除它的收益比规格估计更大。

剩余 ~2.7ms 是 2 次 WAL fsync（prepare + commit）与固定开销；在本轮**不动**
（D4：durable 与敏感扫描保留）。

## 4. 验收

```
cargo test -p mcp-mesh --lib participant_client      # 22 passed
cargo test -p chimera-cli --lib -- stream_delay production_
                                                     # 6 passed（3 纯函数 + 2 结构锁 + 1 既有）
cargo clippy -p mcp-mesh -p chimera-cli --all-targets --jobs 2 -- -D warnings   # rc=0
rustfmt --edition 2021 --check <改动的 7 文件>        # rc=0
```

新增测试：

| 测试 | 锁住什么 |
|---|---|
| `instant_client_skips_simulated_delay` | `instant()` 的 `delay_for` 恒 `None` |
| `new_client_keeps_deterministic_simulated_delay` | `new()`/`Default` 保留 1–2ms，且 `Default` 不退化为零延迟 |
| `production_mesh_uses_instant_client_not_simulated` | `mcp_tool_executor` 生产段含 `instant()`、不含 `new()`（结构锁） |
| `production_in_process_mesh_uses_instant_client` | `commands/mcp.rs` 生产段复用 `in_process_mesh()`、不含 `new()` |
| `stream_delay_env_overrides_tty_and_default` | env 显式值压过 TTY 归零/默认 |
| `stream_delay_non_tty_is_zero` | 非 TTY ⇒ 0；TTY ⇒ 20ms |
| `stream_delay_invalid_env_falls_back_to_tty_rule` | 非法 env 回落到 TTY 规则 |

**未做端到端 TTY/非 TTY 墙钟对比**：本机无法可靠模拟 PTY；策略由 3 个纯函数测试锁定，
TTY 分支的输出逐字节不变（只是延迟值不同）。

## 5. 回滚

两个独立 commit：

1. `instant()` + `in_process_mesh()` + 两处注入 + 结构锁；
2. `resolve_stream_chunk_delay` / `stream_chunk_delay` + `run`/`chat` 接入 + 纯函数测试
   （+ `mesh_benchmark` 的对照组）。

revert 第 1 个 commit 后 `new()` 的模拟行为原样恢复；revert 第 2 个后 `run`/`chat`
回到「env 或默认 20ms」的旧语义。

## 6. 证据边界

- A/B 是**同机同 HEAD** 的 criterion 读数，跨轮 `change` 不可归因（ADR-267 教训）；
  本文只引用同一轮内的两组 median。
- `mcp call` 的端到端还含 policy ×2 + sandbox + route + WAL fsync；本轮只删除其中的
  模拟延迟项，不声称「mcp call 全链 −80%」。
- `durable=false` / group commit 未做，故 WAL 成本仍在（~2.7ms/事务，本机）。
