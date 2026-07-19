//! Agent 子模块 — Agent 元数据、工厂与生命周期管理
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 定义 Agent 类型体系、创建 Agent 实例、管理 Agent 状态机
//!
//! ## 子模块组织
//!
//! - \meta\ — Agent 元数据与类型定义(AgentMeta / AgentType / AgentStatus / ModelConfig)
//! - \actory\ — Agent 工厂,创建 Agent 实例
//! - \lifecycle\ — Agent 生命周期状态机管理
//!
//! ## 相关 Task
//!
//! - Task 7: 实现 Agent 元数据与类型(meta.rs)
//! - Task 8: 实现 Agent 工厂与生命周期(factory.rs + lifecycle.rs)

pub mod factory;
pub mod lifecycle;
pub mod meta;

// === 关键类型重导出 ===
pub use factory::{Agent, AgentFactory};
pub use lifecycle::{AgentLifecycle, LifecycleState};
pub use meta::{AgentMeta, AgentStatus, AgentType, ModelConfig};
