//! 领域与事件契约组 — 纯领域类型（上提自 nexus-core）+ 事件元数据/载荷/双轨/图身份（L0 契约）。
//!
//! 成员经 crate 根 re-export，旧路径 `crate::<leaf>` / `nexus_contracts::<leaf>` 不变。
//! 注：组名为 `core_domain` 以避免与 leaf 模块 `domain` 的根绑定冲突。

pub mod checkpoint;
pub mod domain;
pub mod event_metadata;
pub mod event_payload;
pub mod event_v2;
pub mod graph_identity;
pub mod task;
pub mod temporal;
