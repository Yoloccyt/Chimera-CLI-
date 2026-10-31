//! Process provider — the first write-capable tool.
//!
//! Every invocation goes through `seccore::os_backend::ProcessFence`
//! (`default_secure` allowlist + escape checks) before spawning. Writes require
//! explicit approval (`ToolInvocation.approved`), and `kill_on_drop` ensures a
//! runtime timeout/cancellation does not leave an orphan child.

use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gqep_executor::tool_plane::{ToolDescriptor, ToolError, ToolInvocation, ToolProvider};
use nexus_contracts::command_validation::CommandPolicy;
use nexus_contracts::tool_plan::SideEffectDecl;
use seccore::os_backend::{ProcessFence, SandboxMode};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Runs an allowlisted program with sandbox validation.
pub struct ProcessToolProvider {
    fence: Mutex<ProcessFence>,
    max_output_bytes: usize,
}

impl std::fmt::Debug for ProcessToolProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessToolProvider")
            .field("max_output_bytes", &self.max_output_bytes)
            .finish_non_exhaustive()
    }
}

impl ProcessToolProvider {
    /// New provider with a `default_secure` process fence.
    #[must_use]
    pub fn new(max_output_bytes: usize) -> Self {
        let fence = ProcessFence::new(
            Arc::new(CommandPolicy::default_secure()),
            SandboxMode::Standard,
        );
        Self {
            fence: Mutex::new(fence),
            max_output_bytes,
        }
    }
}

#[async_trait]
impl ToolProvider for ProcessToolProvider {
    fn provider_id(&self) -> &str {
        "proc"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            name: "proc.exec".to_string(),
            provider: "proc".to_string(),
            description: "Run an allowlisted program; arguments are validated by ProcessFence"
                .to_string(),
            side_effect: SideEffectDecl::Write,
            timeout_ms: 30_000,
            max_concurrency: 2,
            input_schema: Some(
                r#"{"type":"object","properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["program"]}"#
                    .to_string(),
            ),
        }]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        if !inv.approved {
            return Err(ToolError::AskRequired(
                "proc.exec requires explicit approval (--yes / --no-permission or prompt)".into(),
            ));
        }

        let value: Value = serde_json::from_str(&inv.args_json).map_err(|error| {
            ToolError::InvalidArgs(format!("proc.exec args must be JSON: {error}"))
        })?;
        let program = value
            .get("program")
            .and_then(Value::as_str)
            .filter(|program| !program.is_empty())
            .ok_or_else(|| ToolError::InvalidArgs("proc.exec requires `program`".into()))?;
        let args = parse_args(value.get("args"))?;

        {
            let mut fence = self
                .fence
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            fence
                .check(program, &args)
                .map_err(|error| ToolError::Denied(error.to_string()))?;
        }

        let mut command = tokio::process::Command::new(program);
        command
            .args(&args)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = tokio::select! {
            result = command.output() => result
                .map_err(|error| ToolError::Provider(format!("spawn failed: {error}")))?,
            _ = cancel.cancelled() => {
                return Err(ToolError::Cancelled(inv.call_id.clone()));
            }
        };

        let stdout = truncate_lossy(&output.stdout, self.max_output_bytes);
        let stderr = truncate_lossy(&output.stderr, self.max_output_bytes);
        Ok(serde_json::json!({
            "exit_code": output.status.code(),
            "stdout": stdout,
            "stderr": stderr,
        })
        .to_string())
    }
}

fn parse_args(value: Option<&Value>) -> Result<Vec<String>, ToolError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .ok_or_else(|| ToolError::InvalidArgs("args must be strings".into()))
            })
            .collect(),
        Some(_) => Err(ToolError::InvalidArgs("args must be an array".into())),
    }
}

fn truncate_lossy(bytes: &[u8], max_bytes: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= max_bytes {
        return text.into_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
