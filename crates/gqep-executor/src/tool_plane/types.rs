//! Tool execution plane contracts.
//!
//! These types are intentionally local to L7 `gqep-executor`: the execution
//! plane is a runtime concern, not a cross-layer protocol. L0 `ToolPlan` stays
//! the declarative plan contract; `ToolInvocation`/`ToolOutcome` are the
//! imperative single-call carrier used by schedulers, providers and the LLM
//! tool loop.

use nexus_contracts::tool_plan::SideEffectDecl;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Scheduling lane / fairness hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolPriority {
    /// User-facing calls that should not wait behind background work.
    Interactive,
    /// Default lane.
    #[default]
    Normal,
    /// Batch/background work.
    Background,
}

/// Where the invocation originated (observability / audit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolOrigin {
    /// Non-interactive CLI command.
    #[default]
    Cli,
    /// TUI action or chat.
    Tui,
    /// `nexus-app-server` protocol host.
    AppServer,
    /// Quest executor.
    Quest,
    /// Lifecycle hook.
    Hook,
    /// Sub-agent runtime.
    Subagent,
    /// LLM tool loop.
    Llm,
}

/// One imperative tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// Correlation id (unique per call; used by the LLM loop to match results).
    pub call_id: String,
    /// Fully-qualified tool name (`provider.tool`).
    pub tool: String,
    /// Raw JSON arguments. Kept as text to avoid an extra parse layer in L7/L10.
    pub args_json: String,
    /// Per-call timeout override (ms). `None` uses the descriptor/default.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Scheduling lane hint.
    #[serde(default)]
    pub priority: ToolPriority,
    /// Origin of the call.
    #[serde(default)]
    pub origin: ToolOrigin,
    /// Quest attribution (optional).
    #[serde(default)]
    pub quest_id: Option<String>,
    /// Task attribution (optional).
    #[serde(default)]
    pub task_id: Option<String>,
    /// Explicit approval for Ask-policy tools (write tools in the LLM loop).
    #[serde(default)]
    pub approved: bool,
}

/// Terminal status of one tool call. Never panics; every failure is a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Provider returned a payload.
    Ok,
    /// Call was intentionally skipped by the caller.
    Skipped,
    /// No provider registered for the tool name.
    NotFound,
    /// Invocation/descriptor invalid.
    InvalidArgs,
    /// ExecPolicy denied.
    Denied,
    /// ExecPolicy asked for approval but no approval was supplied.
    AskRequired,
    /// Deadline exceeded.
    Timeout,
    /// Cancellation token fired.
    Cancelled,
    /// Scheduler/waiter limit exceeded.
    Overloaded,
    /// Provider failed (transport/protocol/internal).
    ProviderError,
}

impl ToolStatus {
    /// Whether the call produced a usable payload.
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// Result of one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// Correlation id copied from the invocation.
    pub call_id: String,
    /// Tool name copied from the invocation.
    pub tool: String,
    /// Provider that handled (or would have handled) the call.
    pub provider: String,
    /// Terminal status.
    pub status: ToolStatus,
    /// Payload, if any.
    #[serde(default)]
    pub payload: Option<String>,
    /// Human-readable error, if any.
    #[serde(default)]
    pub error: Option<String>,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Number of provider attempts (1 in Phase 1; retries are provider-owned).
    pub attempts: u8,
    /// Whether the payload was truncated to the runtime budget.
    pub truncated: bool,
}

impl ToolOutcome {
    pub(crate) fn ok(
        inv: &ToolInvocation,
        provider: &str,
        payload: String,
        truncated: bool,
        duration_ms: u64,
    ) -> Self {
        Self {
            call_id: inv.call_id.clone(),
            tool: inv.tool.clone(),
            provider: provider.to_string(),
            status: ToolStatus::Ok,
            payload: Some(payload),
            error: None,
            duration_ms,
            attempts: 1,
            truncated,
        }
    }

    pub(crate) fn failed(
        inv: &ToolInvocation,
        provider: &str,
        status: ToolStatus,
        error: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            call_id: inv.call_id.clone(),
            tool: inv.tool.clone(),
            provider: provider.to_string(),
            status,
            payload: None,
            error: Some(error),
            duration_ms,
            attempts: 1,
            truncated: false,
        }
    }
}

/// Static descriptor registered by a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Fully-qualified tool name (`provider.tool`).
    pub name: String,
    /// Provider id.
    pub provider: String,
    /// Human description (also used as the LLM tool description).
    pub description: String,
    /// Side-effect declaration (read-only vs write).
    pub side_effect: SideEffectDecl,
    /// Default timeout (ms; `0` = runtime default).
    pub timeout_ms: u64,
    /// Provider-side concurrency hint (runtime still enforces its own bound).
    pub max_concurrency: usize,
    /// Optional JSON-schema text for the input.
    #[serde(default)]
    pub input_schema: Option<String>,
}

/// Typed provider/runtime error. All variants are non-panicking by contract.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// Tool name not registered.
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Invalid arguments or duplicate registration.
    #[error("invalid args: {0}")]
    InvalidArgs(String),
    /// ExecPolicy denied the call.
    #[error("denied: {0}")]
    Denied(String),
    /// ExecPolicy asked for approval, but none was supplied.
    #[error("ask_required: {0}")]
    AskRequired(String),
    /// Deadline exceeded.
    #[error("timeout: {operation_id} after {timeout_ms}ms")]
    Timeout {
        /// Correlation id.
        operation_id: String,
        /// Effective timeout.
        timeout_ms: u64,
    },
    /// Cancellation token fired.
    #[error("cancelled: {0}")]
    Cancelled(String),
    /// Scheduler/waiter limit exceeded.
    #[error("overloaded: {waiters} waiters")]
    Overloaded {
        /// Number of waiters observed when the limit was hit.
        waiters: usize,
    },
    /// Provider not available (e.g. missing MCP endpoint, missing feature).
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// Provider failed while executing.
    #[error("provider error: {0}")]
    Provider(String),
}

/// Provider abstraction — the only way the runtime executes a tool.
#[async_trait::async_trait]
pub trait ToolProvider: Send + Sync {
    /// Stable provider id (must match every descriptor's `provider` field).
    fn provider_id(&self) -> &str;
    /// Tools exposed by this provider. Names must be globally unique.
    fn descriptors(&self) -> Vec<ToolDescriptor>;
    /// Execute one call. Must honor `cancel` at await points and return a typed error.
    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError>;
}
