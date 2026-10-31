//! Tool execution plane (L7) — runtime, providers, scheduler, registry.
//!
//! This module is the single execution surface used by CLI/TUI/protocol hosts,
//! Quest execution and the LLM tool loop. It is deliberately independent of
//! MCP/Hook/SubAgent crates: providers are injected as `Arc<dyn ToolProvider>`
//! by the L10 composition root, keeping the dependency direction L10 → L7.

pub mod metrics;
pub mod registry;
pub mod runtime;
pub mod scheduler;
pub mod types;

pub use metrics::{ToolMetrics, ToolMetricsSnapshot};
pub use registry::ToolRegistry;
pub use runtime::{ToolRuntime, ToolRuntimeConfig};
pub use scheduler::{BoundedToolScheduler, SchedSnapshot, ToolPermit, ToolScheduler};
pub use types::{
    ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolOutcome, ToolPriority, ToolProvider,
    ToolStatus,
};
