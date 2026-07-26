//! 事件总线 — 基于 tokio::broadcast + MessagePack 的跨层通信通道
//!
//! 对应架构层:L1 Core
//! 对应创新点:无(基础设施,所有跨层通信唯一通道)
//!
//! # 核心职责
//! - 提供类型安全的发布订阅(typed broadcast bus)
//! - 定义 65 个跨层事件类型(Week 1-8 累计),修正 4 处依赖方向违规(Part A 分析)
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
pub mod error;
pub mod logging;
/// 膜渗透过滤器(P2-W6.1,ADR-033 后续膜深化)
///
/// 按"是否影响内环状态"分类事件语义类别(EventCategory),结合内环负载档位
/// (InnerLoad)与膜厚度(MembraneThickness)三维度决策事件是否穿膜入内环。
/// 详见 [`membrane::MembraneFilter`]。
pub mod membrane;
/// RCU 单调读状态容器(P2-W7.2.3,§9.1 arc-swap)
///
/// 因果一致性三层之二:内环共享状态的最终一致 + 单调读。
/// 无锁读(~5ns)+ 原子写(~50ns),旧快照在新写入后仍有效(RCU 回收语义)。
/// 详见 [`rcu::MonotonicState`]。
pub mod rcu;
pub mod subscriber;
pub mod topic;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use backpressure::{is_critical_event, BackpressurePolicy, SlowConsumerDetector};
pub use bus::{
    deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack, EventBus,
    EventReceiver, DEFAULT_CAPACITY,
};
pub use error::EventBusError;
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
    ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ClvSummary, ConsultUrgency,
    CriticalEventDropped, EventMetadata, EventSeverity, NexusEvent, QuestStatus,
    RouterStatsPayload, TaskPriority, VoteValue,
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
