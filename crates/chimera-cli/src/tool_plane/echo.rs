//! Deterministic echo provider — the first real end-to-end tool.
//!
//! Read-only, zero IO. Supports an optional `{"sleep_ms": N}` argument so
//! timeout/cancellation can be exercised end-to-end without spawning a process.

use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_plane::{ToolDescriptor, ToolError, ToolInvocation, ToolProvider};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// Echoes `args_json` back as the payload.
#[derive(Debug, Default)]
pub struct EchoToolProvider;

impl EchoToolProvider {
    /// New provider.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolProvider for EchoToolProvider {
    fn provider_id(&self) -> &str {
        "echo"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            name: "echo.echo".to_string(),
            provider: "echo".to_string(),
            description: "Echo the JSON arguments back (deterministic, read-only)".to_string(),
            side_effect: SideEffectDecl::ReadOnly,
            timeout_ms: 5_000,
            max_concurrency: 8,
            input_schema: Some(
                r#"{"type":"object","properties":{"sleep_ms":{"type":"integer","minimum":0}},"additionalProperties":true}"#
                    .to_string(),
            ),
        }]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&inv.args_json) {
            if let Some(sleep_ms) = value.get("sleep_ms").and_then(serde_json::Value::as_u64) {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {}
                    _ = cancel.cancelled() => {
                        return Err(ToolError::Cancelled(inv.call_id.clone()));
                    }
                }
            }
        }
        Ok(inv.args_json.clone())
    }
}
