//! 全维稀疏协调器 — 工具/上下文/记忆/审计/预算五维度稀疏化调度
//!
//! 对应架构层:L6 Router
//! 对应创新点:OSA / Ω-Sparse(Omni-Sparse Architecture)
//!
//! # 核心职责
//! - 基于 `TaskProfile` 一次性计算五维度稀疏掩码(routing/context/memory/audit/budget)
//! - 复杂度联动稀疏化:按 `complexity_score` 四档产生不同稀疏度掩码
//! - 发布 `OmniSparseMasksComputed` 事件(携带 `mask_hash`、`sparsity`),修正 V1 违规
//! - `mask_hash` 为五维度掩码序列化的 SHA-256 hex,消费者据此去重与拉取
//!
//! # V1 违规修正
//! 原架构:OSA(L6)直接 import HCW(L2)→ 向上依赖违规
//! 修正后:OSA 发布 `OmniSparseMasksComputed` 事件,HCW 订阅消费
//! OSA 不持有 HCW 的引用,仅通过事件传递 `context_mask`
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 所有 async fn 满足 Send 约束
//!
//! # 快速示例
//! ```no_run
//! use osa_coordinator::{OmniSparseCoordinator, TaskProfile, RiskLevel};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let coord = OmniSparseCoordinator::new(bus.clone());
//!
//! let profile = TaskProfile::new("t-1", 0.6, RiskLevel::Medium);
//! let masks = coord.compute_all_masks(&profile).await?;
//! assert!(masks.routing.active_count() > 0);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod config;
pub mod coordinator;
pub mod error;
pub mod masks;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::OsaConfig;
pub use coordinator::{OmniSparseCoordinator, OmniSparseMasks};
pub use error::OsaError;
pub use masks::SparseMask;
pub use types::{
    AffectedScope, ComplexityBand, FileId, MemoryId, OperationId, RiskLevel, TaskId, TaskProfile,
    TaskType, TimePressure, ToolId,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::OsaConfig;
    pub use crate::coordinator::{OmniSparseCoordinator, OmniSparseMasks};
    pub use crate::error::OsaError;
    pub use crate::masks::SparseMask;
    pub use crate::types::{
        AffectedScope, ComplexityBand, FileId, MemoryId, OperationId, RiskLevel, TaskId,
        TaskProfile, TaskType, TimePressure, ToolId,
    };
}
