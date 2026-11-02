# ADR-184: NexusError 无 source 字段的取舍落档（wire 契约设计约束，非遗漏）

- **状态**: Accepted（2026-09-13，架构方向 F-b 收口；勘察 + 分层示范后终裁）
- **决策者**: 架构批次（F-c 错误链收敛系列，用户批准批次 spec 后终裁）
- **关联**:
  - ADR-033（统一错误层级例外扩展——NexusError 引入背景）
  - ADR-181（影子能力状态归类——EXPERIMENTAL-UNWIRED 三态标签）
  - `docs/reports/arch-refactor-directions-v2-2026-09-12.md` §7.9–§7.14（F-c 错误链收敛系列：分层示范的证据链）

## 背景

Explore-1 全库审计（架构方向 F）曾把 `NexusError`（`nexus-contracts/src/errors.rs`）
与其它 `Result<_, String>` 边界一并列为"错误链欠账"——疑问是：**库层 thiserror
惯例的 `#[source]`/`#[from]`（底层错误链）为何 NexusError 没有？是否应补？**

F-c 系列收敛过程中对此完成了系统勘察与分层示范（2026-09-12/13 共 5 个批次），
本 ADR 把取舍结论落档，防止未来"补 source"的破坏性改动。

## 勘察事实

1. **`NexusError` 是 wire 契约类型**：
   `#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]` ——
   跨膜（事件 payload / 协议面 `AppEvent::Error`）序列化 + `Clone`（跨
   `tokio::spawn`/任务边界复制）+ `PartialEq`（测试断言）。
2. **`source` 与上述 derive 互斥**：`std::error::Error::source()` 需要
   `Box<dyn Error>` / `Arc<dyn Error>` 字段——`dyn Error` **不可 Serialize、
   不可 Clone、不可 PartialEq**。补 source 必须放弃至少三个 derive，
   或为它们手写实现（Serialize 手写膨胀、错误链 PartialEq 语义歧义——
   "两个错误相等"无意义）。
3. **错误上下文已聚合进 `#[error]` 消息**：每个变体的 Display 文本即完整
   上下文（如 `ToolTimeout { tool_name, duration_ms }` →
   `"tool execution timeout: {tool} after {ms}ms"`）——消息即数据，
   上屏/日志/协议 `message` 字段单点取用，无信息丢失。
4. **source 链的职责已由分层自有错误类型承载**（F-c 系列示范，全部
   库层 thiserror + `#[source]` + proptest）：
   - `nexus-subagent::SubAgentError`（`#[source]` 保留 serde_json 链，
     取消/panic/业务失败分类——ADR 见报告 §7.9）
   - `nexus-app-server::ProtocolError`（Payload/Frame 两阶段 +
     `#[source] serde_json::Error`，报告 §7.10）
   - `nexus-app-server::TransportError::Decode(JsonRpcError)`（code 语义
     结构化保留，报告 §7.12）

## 决策

1. **`NexusError` 保持无 `source` 字段**——这是 **wire 契约类型的设计约束**
   （Serialize/Clone/PartialEq 优先），不是遗漏；
2. **错误上下文聚合进 `#[error]` 消息**的既有模式维持（消息即数据）；
3. **底层错误链保真的职责下沉到各层自有错误类型**：跨膜边界（L0
   `NexusError`）只携带聚合消息；库层内部（crate 级 `*Error`）用
   `#[source]`/`#[from]` 保留结构化链——先例：`SubAgentError` /
   `ProtocolError` / `TransportError`（F-c 系列全部落档于
   `arch-refactor-directions-v2-2026-09-12.md` §7.9–§7.14）；
4. **否决的替代方案**：`#[serde(skip)] Arc<dyn Error>` 附加字段——破坏
   `Clone`/`PartialEq` derive、`Serialize` 需手写膨胀、错误链相等比较
   语义歧义；收益（`source()` 可下钻）不抵成本。

## 后果

- **正面**: wire 契约稳定（serialize/clone/partialeq 零负担）；错误上下文
  单点聚合（消息即数据）；分层错误链先例完整（F-c 系列可复制模式：
  新错误类型 → 变体分类 → `#[from]` 接入上层 → 旧签名兼容壳）。
- **负面**: 仅凭 `NexusError` 自身 debug 时无底层错误链——由变体消息的
  完整聚合文本补偿；各层自有错误类型的 source 链维护是持续义务
  （F-c 模式已有测试范式：source 可下钻断言 + proptest）。
- **验证**: 本 ADR 纯文档（无行为变更）；分层示范的回归证据见
  `arch-refactor-directions-v2-2026-09-12.md` §7.9–§7.14
  （nexus-subagent 23/23、nexus-app-server 47/0、chimera-cli 全量绿）。

## 相关文档

- `docs/reports/arch-refactor-directions-v2-2026-09-12.md`（F-c 收敛系列全文）
- `crates/nexus-subagent/src/error.rs`（分层示范①：SubAgentError）
- `crates/nexus-app-server/src/protocol.rs`（分层示范②：ProtocolError）
- `crates/nexus-app-server/src/transport.rs`（分层示范③：Decode/Payload 结构化）
