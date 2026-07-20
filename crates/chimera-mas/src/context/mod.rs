//! Context 子模块 — Agent 独立上下文隔离与 Token 预算管理
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 每个 Agent 拥有独立 1M Token 等效上下文,经 HCW 稀疏化实现隔离
//!
//! ## 子模块组织
//!
//! - `manager` — AgentContext 包装 hcw_window::HcwWindow(ADR-026 决策 7)
//! - `isolation` — ContextIsolationGuard 上下文隔离守卫
//! - `budget` — TokenBudget Token 预算管理
//!
//! ## ADR-026 决策 7: 1M 上下文经 HCW 稀疏化
//!
//! 1M Token 上下文 = 128K 实际加载 + 8× 稀疏压缩(Ω-Compress 单一实现原则)。
//! - `AgentContext` 包装 `hcw_window::HcwWindow`,不自实现压缩逻辑
//! - `OmniSparseMasks` 由 `osa_coordinator::OmniSparseCoordinator::compute_all_masks()` 计算
//! - 50 Agent + 1M 上下文 = 130MB(HCW 模式) vs 305MB(暴力加载,被否决)
//! - Critical 块(system_prompt)永不被压缩,Optional 块(wiki 知识)可完全丢弃
//!
//! ## 相关 Task
//!
//! - Task 9: 实现 AgentContext 上下文管理(manager.rs)
//! - Task 10: 实现 TokenBudget 预算管理(budget.rs)

pub mod budget;
pub mod budget_model;
pub mod isolation;
pub mod manager;

// === 关键类型重导出 ===
pub use budget::TokenBudget;
pub use budget_model::{
    should_compress_at, AdmissionGate, ContextTier, MemoryBudgetModel, COMPRESSION_THRESHOLD,
    SPARSE_FACTOR,
};
pub use isolation::ContextIsolationGuard;
pub use manager::{AgentContext, ContextBlock, ContextPriority};
