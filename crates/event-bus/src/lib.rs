//! 事件总线 — 基于 tokio::broadcast + MessagePack 的跨层通信通道
//!
//! 对应架构层:L1 Core
//! 对应创新点:无(基础设施,所有跨层通信唯一通道)
//!
//! # 核心职责
//! - 提供类型安全的发布订阅(typed broadcast bus)
//! - 定义 144 个 NexusEvent 跨层事件变体(v2.27.0-omega 实测枚举),
//!   修正 4 处依赖方向违规(Part A 分析)
//! - 背压处理与慢消费者隔离,避免孤儿调用(架构红线)
//! - MessagePack 序列化(ADR-004),支持跨进程投递
//!
//! # 快速示例
//! ```no_run
//! use event_bus::{EventBus, NexusEvent, EventMetadata};
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let mut rx = bus.subscribe();
//! bus.publish(NexusEvent::QuestCreated {
//!     metadata: EventMetadata::new("quest-engine"),
//!     quest_id: "q-1".into(),
//!     title: "示例".into(),
//!     task_count: 1,
//! }).await.unwrap();
//! let event = rx.recv().await.unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod backpressure;
pub mod bus;
/// 因果一致性模块(P2-W7.2.1,spec.md L251-254 因果一致性 Scenario)
///
/// 向量时钟跟踪跨膜事件因果关系(Lamport 算法),判定事件偏序:
/// - `happens_before`:a → b(a 先于 b 发生)
/// - `concurrent_with`:a 与 b 无因果关系
///
/// 详见 [`causal::VectorClock`]。
pub mod causal;
/// 事件分类实现(P1-3 拆分,types.rs 上帝文件治理)
///
/// 承接 `NexusEvent` 的 severity()/type_name() 两个巨型 match,
/// types.rs 保留 enum 定义与 metadata()。同 crate 内 `impl NexusEvent`
/// 跨文件分块,调用路径零变化。severity() 判定逻辑留在 event-bus
/// (架构红线:Critical 事件 mpsc 保障),与 bus.rs `is_critical_mpsc_event`
/// 双清单同步红线由守护测试兜底。
pub mod classification;
pub mod error;
/// 分层子枚举 — NexusEvent 按架构层拆分的分类实现
///
/// 将 134 变体的 NexusEvent 按架构层拆分为 8 个子枚举:
/// CoreEvent/MemoryEvent/StorageEvent/SecurityEvent/RouterEvent/
/// ExecutionEvent/QuestEvent/InterfaceEvent。每个子枚举实现
/// `EventClassification` trait(metadata/severity/type_name)。
///
/// # 渐进式方案
/// NexusEvent 变体结构保持不变(消费方零改动),子枚举作为分类
/// 参考与未来迁移目标。
pub mod event_types;
/// 经验卡片总线 — OpenMLE 经验卡片双通道 + 四索引（v3.4.0 §6.1）
///
/// 承载 ExperienceCardBus（broadcast + mpsc 分级投递 + task/node/factor/error
/// 四索引 + 全局统计），使经验卡片成为 Event Bus 一级公民。
pub mod experience_card_bus;
/// FormalVerifier M1 — 事件因果一致性形式化验证(P7-T4,ADR-047 Property #4)
///
/// 验证 EventMetadata 序列(event_id 时序/同 source 时间戳/唯一性),
/// 与 107+ 事件变体载荷解耦;类型复用 `nexus_contracts::formal_props`(L0)。
pub mod formal;
pub mod logging;
/// 膜渗透过滤器(P2-W6.1,ADR-033 后续膜深化)
///
/// 按"是否影响内环状态"分类事件语义类别(EventCategory),结合内环负载档位
/// (InnerLoad)与膜厚度(MembraneThickness)三维度决策事件是否穿膜入内环。
/// 详见 [`membrane::MembraneFilter`]。
pub mod membrane;
/// 事件载荷与辅助类型 — NexusEvent 枚举依赖的结构化数据类型
///
/// 将 `EventMetadata`、`BudgetMetricsPayload`、`ClvSummary` 等辅助类型
/// 独立为 payloads 模块,减少 types.rs 膨胀。通过 types 模块的 `pub use`
/// 重导出保持向后兼容。
pub mod payloads;
/// RCU 单调读状态容器(P2-W7.2.3,§9.1 arc-swap)
///
/// 因果一致性三层之二:内环共享状态的最终一致 + 单调读。
/// 无锁读(~5ns)+ 原子写(~50ns),旧快照在新写入后仍有效(RCU 回收语义)。
/// 详见 [`rcu::MonotonicState`]。
pub mod rcu;
/// Segment-aware PER — 轨迹分段优先级经验回放（v3.4.0 §6.2）
///
/// 承载 SegmentAwarePER + PerBuffer（铁律9 分段身份共享 + prompt-equal
/// denominator + TD 误差权重采样）。
pub mod segment_per;
pub mod subscriber;
/// Token 账本 — Dressage Token 级证据的 append-only 存储（v3.4.0 §6.1 + §5.3）
///
/// 承载 TokenLedger（有序账本 + 会话/实例双索引 + 完整性校验 + 导出通道），
/// 保证"Token Ledger 不可丢失（训练证据完整性）"绝对红线。
pub mod token_ledger;
pub mod topic;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use backpressure::{is_critical_event, BackpressurePolicy, SlowConsumerDetector};
// FormalVerifier M1:事件因果一致性验证器重导出(P7-T4)
pub use bus::{
    deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack, EventBus,
    EventReceiver, DEFAULT_CAPACITY,
};
// §16.5 L1 吞吐量周期报告器(Phase 10 Wave 6)
pub use crate::bus::spawn_throughput_reporter;
pub use error::EventBusError;
pub use formal::CausalConsistencyChecker;
pub use logging::BusLogger;
// P2-W7.2.3: RCU 单调读状态容器(内环最终一致 + 单调读)
pub use rcu::MonotonicState;
// P1-W4.2: TypeState 订阅构建器(强制 subscribe-then-spawn 原子化)
pub use subscriber::{
    CriticalSubscribed, CriticalSubscriberBuilder, Subscribed, SubscriberBuilder, Unsubscribed,
};
// P2-W6.1: 膜渗透过滤器(内环/外环选择性渗透决策)
pub use membrane::{
    EventCategory, InnerLoad, MembraneFilter, MembraneThickness, PermeationDecision,
};
// P2-W7.2.1: 因果一致性(向量时钟,跨膜事件因果关系跟踪)
pub use causal::{CausalRelation, VectorClock};
pub use topic::{EventTopic, FilteredSubscriber};
pub use types::{
    ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ClvSummary, CompatLevel,
    ConsultUrgency, CriticalEventDropped, EventMetadata, EventSeverity, NexusEvent, QuestStatus,
    RouterStatsPayload, TaskPriority, VoteValue,
};
// v3.4.0 §6.1: 经验卡片总线（OpenMLE 双通道 + 四索引）
pub use experience_card_bus::{ExperienceCardBus, GlobalCardStats};
// v3.4.0 §6.1 + §5.3: Token 账本（Dressage 证据完整性）
pub use token_ledger::{LedgerError, LedgerIntegrityReport, TokenLedger};
// v3.4.0 §6.2: Segment-aware PER（铁律9 分段身份）
pub use segment_per::{PerBuffer, PerEntry, PerStats, SegmentAwarePER};
// 分层子枚举与分类 trait(渐进式拆分,消费方可按需导入)
pub use event_types::{
    CoreEvent, EventClassification, ExecutionEvent, InterfaceEvent, MemoryEvent, QuestEvent,
    RouterEvent, SecurityEvent, StorageEvent,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::backpressure::BackpressurePolicy;
    pub use crate::bus::{EventBus, EventReceiver, DEFAULT_CAPACITY};
    pub use crate::error::EventBusError;
    pub use crate::logging::BusLogger;
    pub use crate::topic::{EventTopic, FilteredSubscriber};
    pub use crate::types::{
        ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ConsultUrgency, EventMetadata,
        EventSeverity, NexusEvent, QuestStatus, RouterStatsPayload, TaskPriority,
    };
}
