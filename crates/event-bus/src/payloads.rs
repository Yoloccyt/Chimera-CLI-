//! compat-2x 路径别名 — 事件载荷类型已迁至 `nexus-events`(ADR-322)
//!
//! WHY 保留本模块:`event_bus::payloads::X` 是下游既有路径(含
//! `use event_bus::payloads::*;`),迁出时零改动即可编译。
//!
//! 退役时点:与 `nexus-events` 的 compat-2x 同批(v2.31.0-omega)。

pub use nexus_events::payloads::*;
