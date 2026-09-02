//! 调度类型 — 控制面领域类型（P3-T2，v4.0 WI-29 契约）
//!
//! 对应架构层: L9 Quest（mas-sched 控制面，ADR-145）
//!
//! # strangler 承接（P3-T2 补，ADR-033 先例）
//! 类型已先移 **L0 nexus-contracts::scheduler_contract**（纯类型契约层），
//! 本模块为 re-export 兼容层——mas-sched 及后续 chimera-mas 拆出依赖方
//! 均经 L0 契约承接（类型先移、接口后接），公开 API 保持不变。
//!
//! 权威定义见 `nexus-contracts/src/scheduler_contract.rs`（含类型文档与测试）。

pub use nexus_contracts::scheduler_contract::{
    ClaimOutcome, DenyReason, Lease, Priority, Quota, RenewOutcome, ShouldRunVerdict, TaskId,
    TodoClaim, HANDOFF,
};
