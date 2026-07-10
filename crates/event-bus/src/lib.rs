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
pub mod error;
pub mod logging;
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
pub use topic::{EventTopic, FilteredSubscriber};
pub use types::{EventMetadata, EventSeverity, NexusEvent};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::backpressure::BackpressurePolicy;
    pub use crate::bus::{EventBus, EventReceiver, DEFAULT_CAPACITY};
    pub use crate::error::EventBusError;
    pub use crate::logging::BusLogger;
    pub use crate::topic::{EventTopic, FilteredSubscriber};
    pub use crate::types::{EventMetadata, EventSeverity, NexusEvent};
}
