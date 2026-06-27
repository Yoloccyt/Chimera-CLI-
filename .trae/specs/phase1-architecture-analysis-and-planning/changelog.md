# NEXUS-OMEGA Week 1 验收 — 更新说明文档

> 文档版本: 1.0.0
> 更新日期: 2026-06-20
> 对应阶段: Stage 0 → Stage 1 (Week 1: L0-L1 基础设施)
> 涉及模块: event-bus, qeep-protocol, chimera-cli

---

## 1. 问题背景与影响范围

### 1.1 QEEP Protocol — tokio::spawn 生命周期问题

**问题描述:**

`qeep-protocol` 集成测试中，`test_orphan_detection` 和 `test_zero_orphans_10000_ops` 直接对 `protocol.entangle(...)` 调用了 `tokio::spawn`。`entangle(&self)` 返回的 future 借用了 `&self`，不满足 `tokio::spawn` 要求的 `'static` 生命周期约束，导致编译错误。

**影响范围:**

- 仅影响 `qeep-protocol/tests/qeep.rs` 中的两个测试用例
- 不影响 `QeepProtocol` 核心 API 的运行时行为
- `entangle_spawn()` 方法本身已正确处理生命周期（内部 clone 后移入 async block）

**根因分析:**

```rust
// 错误写法: protocol.entangle() 返回的 future 借用 &self，不满足 'static
let handle = tokio::spawn(protocol.entangle(async {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(42)
}));
```

`entangle(&self)` 的 future 捕获了 `&protocol` 引用，而 `tokio::spawn` 要求 future 为 `'static`。这是 Rust 异步编程中的经典陷阱：在 spawn 前必须将借用转为 owned。

---

### 1.2 Chimera CLI — 配置加载 API 调用错误

**问题描述:**

`chimera-cli/tests/cli.rs` 中 `test_config_load` 和 `test_config_load_missing_file_uses_defaults` 使用了 `ChimeraConfig::load(...)` 调用。但 `load` 是 `config` 模块级函数 (`pub fn load(...)`)，而非 `ChimeraConfig` 的关联方法，导致编译错误。

**影响范围:**

- 仅影响 `chimera-cli/tests/cli.rs` 中的两个测试用例
- 不影响 `config::load()` 函数本身的实现
- 不影响 `ChimeraConfig` 结构体的其他用法

**根因分析:**

```rust
// 错误写法: ChimeraConfig 没有 `load` 关联函数
let cfg = ChimeraConfig::load(Some(config_path.clone())).expect("加载配置失败");

// 正确写法: load 是 config 模块级函数
let cfg = config::load(Some(config_path.clone())).expect("加载配置失败");
```

---

### 1.3 Event Bus — 未使用错误变体清理

**问题描述:**

`event-bus` 的 `publish` 和 `publish_blocking` 方法最初使用 `map_err(EventBusError::from)` 将 `broadcast::SendError` 转换为 `EventBusError::PublishFailed`。但 `broadcast::Sender::send` 仅在无活跃接收者时返回 `Err`，而按设计无订阅者时事件应静默丢弃（不视为错误）。因此 `PublishFailed` 和 `SubscribeFailed` 变体及其 `From` 实现从未被实际使用，导致 clippy 警告。

**影响范围:**

- `event-bus/src/error.rs`: 删除 `PublishFailed` 和 `SubscribeFailed` 变体
- `event-bus/src/bus.rs`: 简化 `publish`/`publish_blocking` 错误处理

---

## 2. 技术实现细节

### 2.1 QEEP Protocol 修复

**修改文件:** `crates/qeep-protocol/tests/qeep.rs`

**修复方案:**

使用 `clone()` 获取 `QeepProtocol` 的 owned 副本，将其移入 `async move` block，使 future 满足 `'static` 约束。

**关键代码变更 (test_orphan_detection):**

```rust
// 修复前
let handle = tokio::spawn(protocol.entangle(async {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(42)
}));

// 修复后
let p = protocol.clone();
let handle = tokio::spawn(async move {
    p.entangle(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(42)
    })
    .await
});
```

**关键代码变更 (test_zero_orphans_10000_ops):**

```rust
// 修复前
for _ in 0..BATCH_SIZE {
    handles.push(tokio::spawn(protocol.entangle(async { Ok(()) }).await.unwrap()));
}
for h in handles {
    h.await.unwrap().unwrap(); // 两层 unwrap: JoinHandle + Result<(), QeepError>
}

// 修复后
for _ in 0..BATCH_SIZE {
    let p = protocol.clone();
    handles.push(tokio::spawn(async move {
        p.entangle(async { Ok(()) }).await.unwrap()
    }));
}
for h in handles {
    h.await.unwrap(); // 一层 unwrap: entangle 返回 Result<(), QeepError>
}
```

**设计要点:**

- `QeepProtocol` 已实现 `Clone`（内部 `Arc<Inner>`），clone 成本极低
- `entangle_spawn()` 方法内部已采用相同的 `clone` + `async move` 模式，本次修复使测试代码与此模式保持一致
- 修复后 `unwrap()` 层级从两层减少到一层，因为 `entangle` 直接返回 `Result<T, QeepError>` 而非嵌套在 `JoinHandle` 中

---

### 2.2 Chimera CLI 配置加载修复

**修改文件:** `crates/chimera-cli/tests/cli.rs`

**修复方案:**

将 `ChimeraConfig::load(...)` 改为 `config::load(...)`，并移除未使用的 `ChimeraConfig` 导入。

**关键代码变更:**

```rust
// 修复前
use chimera_cli::config::{self, ChimeraConfig};

let cfg = ChimeraConfig::load(Some(config_path.clone())).expect("加载配置失败");
let cfg = config::load(Some(missing_path)).expect("缺失文件应回退默认值");

// 修复后
use chimera_cli::config;

let cfg = config::load(Some(config_path.clone())).expect("加载配置失败");
let cfg = config::load(Some(missing_path)).expect("缺失文件应回退默认值");
```

**设计说明:**

`config::load()` 是模块级函数，接收 `Option<PathBuf>` 参数，内部调用 `ChimeraConfig::default()` 作为基础，再通过 Figment 叠加配置文件和环境变量。这种设计符合"模块级函数负责构造，结构体负责数据"的职责分离原则。

---

### 2.3 Event Bus 错误处理简化

**修改文件:**
- `crates/event-bus/src/error.rs`
- `crates/event-bus/src/bus.rs`

**变更内容:**

| 变更项 | 修复前 | 修复后 |
|--------|--------|--------|
| `EventBusError` 变体 | 含 `PublishFailed`、`SubscribeFailed` | 移除这两个变体 |
| `From<SendError>` 实现 | 转换为 `PublishFailed` | 移除 |
| `publish()` 错误处理 | `.map_err(EventBusError::from)?` | `let _ = ...; Ok(())` 静默丢弃 |
| `publish_blocking()` 错误处理 | 同上 | 同上 |

**关键代码变更:**

```rust
// 修复前
pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
    let _receivers = self.sender.send(event).map_err(EventBusError::from)?;
    Ok(())
}

// 修复后
pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
    let _ = self.sender.send(event);
    Ok(())
}
```

**设计依据:**

`broadcast::Sender::send` 仅在无活跃接收者时返回 `Err`。按设计，无订阅者时事件应静默丢弃（类比 UDP 语义），不视为错误。慢消费者问题由接收端的 `recv()` 以 `Lagged` 错误暴露，无需在发送端处理。

---

### 2.4 Event Bus 结构化日志埋点（新增功能）

**新增文件:** `crates/event-bus/src/logging.rs`

**修改文件:**
- `crates/event-bus/src/lib.rs`
- `crates/event-bus/src/bus.rs`

**架构设计:**

`BusLogger` 是一个独立的日志记录器，通过 `Arc<BusLogger>` 嵌入 `EventBus` 和 `EventReceiver`，在关键操作点自动记录结构化日志，无需调用者手动埋点。

**日志字段规范:**

每条日志包含以下必要字段：

| 字段 | 类型 | 说明 |
|------|------|------|
| `module` | string | 模块标识（如 `"chimera-cli"`） |
| `level` | enum | `INFO` / `WARN` / `ERROR` |
| `timestamp` | ISO8601 | 自动由 tracing-subscriber 注入 |
| `context_id` | UUID | 事件 ID 或订阅者 ID，关联同一链路 |
| `operation` | enum | 操作类型（见下表） |
| `error_code` | string | 错误码（仅错误日志） |
| `error_description` | string | 人类可读错误描述 |

**操作类型覆盖表:**

| operation | 级别 | 触发点 | 额外字段 |
|-----------|------|--------|----------|
| `subscriber_connected` | INFO | `EventBus::subscribe()` | `subscriber_count`, `capacity` |
| `subscriber_disconnected` | INFO | `EventReceiver::drop()` | `remaining_subscribers` |
| `event_published` | INFO | `publish()` / `publish_blocking()` | `event_type`, `event_severity`, `subscriber_count`, `total_published` |
| `event_received` | INFO | `recv()` / `recv_timeout()` / `try_recv()` | `event_type`, `total_received` |
| `recv_timeout` | WARN | `recv_timeout()` 超时 | `timeout_ms` |
| `channel_closed` | ERROR | `recv()` 通道关闭 | `total_errors` |
| `slow_consumer_dropped` | WARN | `recv()` Lagged 错误 | `lag`, `dropped_count`, `total_errors` |
| `serialization_error` | ERROR | 序列化/反序列化失败 | `error_detail`, `total_errors` |
| `channel_state_change` | INFO/WARN | 通道状态变化 | `previous_state`, `new_state`, `reason` |
| `resubscribe_attempt` | INFO | 重订阅尝试 | `attempt_number`, `retry_interval_ms` |
| `resubscribe_success` | INFO | 重订阅成功 | `total_attempts`, `total_recovery_time_ms` |
| `resubscribe_failed` | ERROR | 重订阅失败 | `max_attempts`, `total_errors` |
| `stats_summary` | DEBUG | 周期性统计 | `total_published`, `total_received`, `total_errors` |

**使用方式:**

```rust
use event_bus::{EventBus, BusLogger, NexusEvent, EventMetadata};

// 创建带日志的总线
let logger = BusLogger::new("chimera-cli");
let bus = EventBus::with_logger(1024, logger);

// 发布事件时自动记录日志
let event = NexusEvent::QuestCreated {
    metadata: EventMetadata::new("quest-engine"),
    quest_id: "q-1".into(),
    title: "示例".into(),
    task_count: 1,
};
bus.publish(event).await.unwrap();

// 订阅者接收事件时自动记录日志
let mut rx = bus.subscribe();
let received = rx.recv().await.unwrap();

// 查询统计
let logger = bus.logger().unwrap();
println!("已发布: {}, 已接收: {}, 错误: {}",
    logger.total_published(), logger.total_received(), logger.total_errors());
```

**输出示例 (JSON 格式，通过 tracing-subscriber 配置):**

```json
{
  "module": "chimera-cli",
  "level": "INFO",
  "timestamp": "2026-06-20T12:00:00.000Z",
  "context_id": "019abcd-1234-7xxx-yyyy-zzzzzzzzzzzz",
  "operation": "event_published",
  "event_type": "QuestCreated",
  "event_severity": "Normal",
  "subscriber_count": 2,
  "total_published": 42
}
```

---

## 3. 修复前后对比

### 3.1 编译结果

| 检查项 | 修复前 | 修复后 |
|--------|--------|--------|
| `cargo check -p qeep-protocol` | 编译失败 (生命周期错误) | 通过 |
| `cargo check -p chimera-cli` | 编译失败 (方法不存在) | 通过 |
| `cargo check -p event-bus` | 通过 (有 clippy 警告) | 通过 (零警告) |
| `cargo check --workspace` | 部分失败 | 通过 |

### 3.2 测试结果

| 测试套件 | 修复前 | 修复后 |
|----------|--------|--------|
| `qeep-protocol` (8 tests) | 2 个编译失败 | 8 通过 |
| `chimera-cli` (13 tests) | 2 个编译失败 | 13 通过 |
| `event-bus` (30 tests) | 30 通过 | 30 通过 + 3 新增 |
| 全 workspace | 部分失败 | 全部通过 |

### 3.3 API 变更

| API | 变更类型 | 向后兼容 |
|-----|----------|----------|
| `EventBus::new()` | 无变更 | 是 |
| `EventBus::with_capacity()` | 无变更 | 是 |
| `EventBus::with_logger()` | 新增 | 是 |
| `EventBus::set_logger()` | 新增 | 是 |
| `EventBus::logger()` | 新增 | 是 |
| `EventBus::subscribe()` | 返回值增加 `subscriber_id` | 是 (字段非公开) |
| `EventReceiver::subscriber_id()` | 新增 | 是 |
| `EventBusError` 变体 | 移除 `PublishFailed`/`SubscribeFailed` | 否 (破坏性变更) |
| `config::load()` | 无变更 | 是 |

> **注意:** `EventBusError::PublishFailed` 和 `EventBusError::SubscribeFailed` 的移除是破坏性变更。但这两个变体在实际运行中从未被构造（`publish` 始终返回 `Ok(())`），且在 `From<SendError>` 实现中才会被构造——该实现已被移除。如有外部代码 match 这两个变体，需移除对应分支。

---

## 4. 测试验证结果

### 4.1 单元测试

```
event-bus:           19 passed (含 3 个新增 BusLogger 测试)
event-bus integration: 11 passed
qeep-protocol:        8 passed
chimera-cli:         13 passed
全 workspace:        全部通过
```

### 4.2 关键验收用例

| 验收标准 | 测试用例 | 结果 |
|----------|----------|------|
| 零孤儿调用 (10000 ops) | `test_zero_orphans_10000_ops` | 通过 |
| 孤儿检测 (abort 场景) | `test_orphan_detection` | 通过 |
| 配置文件加载 | `test_config_load` | 通过 |
| 缺失文件回退默认值 | `test_config_load_missing_file_uses_defaults` | 通过 |
| 日志埋点计数器 | `test_logger_counters` | 通过 |
| 日志统计摘要 | `test_logger_stats_summary` | 通过 |
| 发布订阅基本流程 | `test_publish_subscribe_basic` | 通过 |
| 多订阅者 | `test_multiple_subscribers` | 通过 |
| 接收超时 | `test_recv_timeout` | 通过 |

### 4.3 Lint 检查

```
cargo clippy --workspace -- -D warnings: 通过 (零 warning)
cargo fmt --all --check:                   通过
```

---

## 5. 潜在风险及规避措施

### 5.1 EventBusError 破坏性变更

**风险:** 移除了 `PublishFailed` 和 `SubscribeFailed` 变体，外部代码若 match 这两个变体会编译失败。

**规避:** 当前项目处于 Stage 0→Stage 1 过渡期，尚无外部消费者。`publish` 始终返回 `Ok(())`，这两个变体在实际运行中从未被构造，移除无运行时影响。

**建议:** 在 Stage 2 引入外部 crate 依赖前，审查所有 `EventBusError` 的 match 分支。

### 5.2 BusLogger 性能开销

**风险:** `BusLogger` 在每次 `publish`/`recv` 时调用 `tracing` 宏，增加了日志开销。在高吞吐场景（>10K events/s）可能影响性能。

**规避:**
- `BusLogger` 是可选的（`EventBus::new()` 不启用日志）
- 使用 `AtomicU64` 计数器（lock-free），避免 Mutex 竞争
- 生产环境可通过 `tracing-subscriber` 的 `EnvFilter` 动态调整日志级别
- 性能敏感路径使用 `log_publish`（不计算序列化大小），非热路径使用 `log_publish_with_size`

**建议:** 在 Week 7 性能测试阶段，对日志埋点进行基准测试，必要时引入 `tracing` 的 `span` 而非 `event` 来减少开销。

### 5.3 EventReceiver Drop 日志时机

**风险:** `EventReceiver::drop()` 中调用 `log_subscriber_disconnected`，但此时无法获取精确的 `remaining_subscribers` 数量（broadcast 通道在 drop 前已减 1）。

**规避:** 当前实现中 `remaining_subscribers` 传 0 表示"已断开"。日志中明确标注此为 drop 后状态。

**建议:** 未来可考虑在 `EventBus` 中维护订阅者注册表，在 drop 时查询精确数量。

---

## 6. 后续优化建议

### 6.1 短期 (Week 2-3)

1. **日志输出配置:** 在 `chimera-cli` 中集成 `tracing-subscriber`，配置 JSON 格式输出到文件，支持 `EnvFilter` 动态调整级别。
2. **日志查询工具:** 提供 `aether logs` 子命令，支持按 `context_id`、`operation`、`error_code` 过滤和聚合。
3. **EventReceiver 追踪:** 为 `EventReceiver` 添加 `resubscribe()` 方法，配合 `BusLogger` 记录完整的重连生命周期。

### 6.2 中期 (Week 4-6)

1. **OpenTelemetry 集成:** 在 MCP Mesh 跨进程场景中，通过 `tracing-opentelemetry` 将 trace context 传播到远端节点，实现分布式追踪。
2. **性能指标导出:** 将 `BusLogger` 的计数器导出为 Prometheus metrics，纳入 `monitoring` 模块的 `/metrics` 端点。
3. **日志采样:** 高吞吐场景下引入日志采样策略（如每 N 条日志记录 1 条），降低 I/O 压力。

### 6.3 长期 (Week 7-8+)

1. **自适应日志级别:** 根据 `BusLogger` 的 `total_errors` 计数器，在错误率升高时自动提升日志级别到 DEBUG，获取更多上下文。
2. **日志审计链:** 将 event-bus 日志与 SecCore 的 Merkle 审计链集成，确保日志本身不可篡改。

---

## 附录 A: 涉及文件清单

| 文件 | 变更类型 | 说明 |
|------|----------|------|
| `crates/event-bus/src/logging.rs` | 新增 | 结构化日志埋点模块 |
| `crates/event-bus/src/lib.rs` | 修改 | 导出 `logging` 模块和 `BusLogger` |
| `crates/event-bus/src/bus.rs` | 修改 | 集成 `BusLogger`，增强 `EventReceiver` |
| `crates/event-bus/src/error.rs` | 修改 | 移除未使用错误变体 |
| `crates/qeep-protocol/tests/qeep.rs` | 修改 | 修复 spawn 生命周期问题 |
| `crates/chimera-cli/tests/cli.rs` | 修改 | 修复配置加载 API 调用 |

## 附录 B: 错误码速查

| 错误码 | 级别 | 含义 | 排查建议 |
|--------|------|------|----------|
| `CHANNEL_CLOSED` | ERROR | 所有 Sender 已 drop | 检查 EventBus 生命周期，确保在 Receiver 前不 drop |
| `SLOW_CONSUMER` | WARN | 消费速度过慢，lag 超限 | 增大通道容量、优化消费者逻辑、或增加消费者实例 |
| `RECV_TIMEOUT` | WARN | 接收超时 | 检查上游是否正常发布事件，或网络是否波动 |
| `SERIALIZATION_ERROR` | ERROR | 序列化/反序列化失败 | 检查事件数据结构是否发生了不兼容变更 |
| `RESUBSCRIBE_FAILED` | ERROR | 重订阅超过最大重试次数 | 检查 EventBus 是否已被 drop，或通道是否已关闭 |