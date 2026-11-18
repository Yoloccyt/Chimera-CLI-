//! compat-2x 路径别名 — 事件类型定义已迁至 `nexus-events`(ADR-322)
//!
//! WHY 保留本模块:`use event_bus::types::{NexusEvent, ...}`(含
//! `use event_bus::types::*`)是下游既有写法,迁出时零改动即可编译。
//! 本模块只做纯路径 re-export,不含任何定义;`payloads::*` 一并转出,
//! 因为迁移前 `types.rs` 顶部就是 `pub use crate::payloads::*;`。
//!
//! 退役时点:与 `nexus-events` 的 compat-2x 同批(v2.31.0-omega)。

pub use nexus_events::payloads::*;
pub use nexus_events::types::*;
