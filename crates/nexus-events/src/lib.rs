//! CRATE-CONTRACT BEGIN
//! LAYER:    L1
//! ROLE:     事件目录 — NEXUS-OMEGA 跨层通信的纯类型契约面(ADR-322)
//! BACKEND:  memory
//! PRODUCERS: 0 -
//! CONSUMERS: 35 chimera-cli,chimera-mas,chimera-tui,chtc-bridge,cmt-tiering…
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! 事件目录 — NEXUS-OMEGA 跨层通信的纯类型契约面(ADR-322)
//!
//! 对应架构层:L0-B 核心基础面(事件目录;`event-bus` 为同一亚层的运行时面)。
//! 机器层号 L1 —— ADR-222 D1 把 L0 拆成 0A/0B 两亚层而不重排机器层号,
//! 故 L0-B 成员(nexus-core/event-bus/nexus-telemetry/nexus-compute/本 crate)
//! 一律记 L1。`BACKEND: memory` 是 crate_contract 台账的合法取值口径
//! (sqlite|fs|memory|none):本 crate 无 rusqlite、无 `std::fs`,全部状态
//! 都是枚举/结构体定义,实为"纯类型、零 IO 零运行时"。
//! 对应创新点:Ω₄-Event — 事件是唯一的跨层通信契约
//!
//! # 核心职责
//! - 定义全部 `NexusEvent` 变体(计数以 `crates/event-bus/tests/variant_count_test.rs`
//!   的 `VARIANT_COUNT_LOCK` 为权威)及其结构化载荷
//! - 定义 `EventTopic` 十类主题分类 与 `define_event_registry!` 定级注册表
//!   (`severity()` / `type_name()` / `topic()` / `metadata()` 四个无通配符 match)
//! - **不含任何运行时**:没有广播通道、没有序列化编解码、没有遥测/日志。
//!   `event-bus` 在本层之上提供发布订阅、背压、分片与 Critical mpsc 旁路
//!
//! # 迁移与兼容(ADR-322)
//! 本 crate 自 `event-bus` 迁出,`event-bus` 保留一个发布周期的纯路径
//! re-export(`compat-2x`,v2.31.0-omega 移除),下游 `use event_bus::NexusEvent;`
//! 一类写法零改动。回滚 = revert 迁出提交 + `cargo check`。
//!
//! # 快速示例
//! ```
//! use nexus_events::{EventMetadata, EventSeverity, NexusEvent};
//!
//! let event = NexusEvent::QuestCreated {
//!     metadata: EventMetadata::new("quest-engine"),
//!     quest_id: "q-1".into(),
//!     title: "示例".into(),
//!     task_count: 1,
//! };
//! assert_eq!(event.type_name(), "QuestCreated");
//! assert_eq!(event.severity(), EventSeverity::Normal);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod payloads;
pub mod registry;
pub mod topic;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
// WHY glob:迁移前 `event_bus::types` 与 `event_bus::payloads` 都是扁平可见面,
// 下游按名字取用(含 `use event_bus::types::*`)。本 crate 根把两段合流,
// 使 `event_bus` 的 compat-2x re-export 可以逐项照抄、零遗漏。
pub use payloads::*;
pub use topic::EventTopic;
pub use types::*;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::payloads::*;
    pub use crate::topic::EventTopic;
    pub use crate::types::*;
}

#[cfg(test)]
mod tests {
    // prelude 是外部消费方最常用的入口:能编译即证明 NexusEvent /
    // EventMetadata / EventTopic 三者都在 glob 面内且无遮蔽。
    use crate::prelude::*;

    #[test]
    fn prelude_exposes_event_types_and_constructors() {
        let event = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("prelude-probe"),
            quest_id: "q-1".into(),
            title: "t".into(),
            task_count: 1,
        };
        assert_eq!(event.type_name(), "QuestCreated");
        assert_eq!(event.topic(), EventTopic::Quest);
        assert_eq!(event.metadata().source, "prelude-probe");
    }
}
