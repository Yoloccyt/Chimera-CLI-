//! MCP tool provider — exposes registered mesh servers as tools.
//!
//! Tool names use `mcp:<server>/<capability>` so server ids may contain dots.
//! MCP tools are treated as write-capable (approval required) because the
//! capability name alone cannot prove read-only semantics.

use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::tool_plane::{ToolDescriptor, ToolError, ToolInvocation, ToolProvider};
use mcp_mesh::McpMesh;
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// Provider backed by a long-lived `McpMesh` registry.
pub struct McpToolProvider {
    mesh: Arc<McpMesh>,
}

impl McpToolProvider {
    /// New provider.
    #[must_use]
    pub fn new(mesh: Arc<McpMesh>) -> Self {
        Self { mesh }
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    fn provider_id(&self) -> &str {
        "mcp"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        let registry = self.mesh.registry();
        let mut descriptors = Vec::new();
        for server_id in registry.list_all() {
            let Some(server) = registry.get(&server_id) else {
                continue;
            };
            for capability in &server.capabilities {
                descriptors.push(ToolDescriptor {
                    name: format!("mcp:{server_id}/{capability}"),
                    provider: "mcp".to_string(),
                    description: format!("MCP capability {capability} on server {server_id}"),
                    side_effect: SideEffectDecl::Write,
                    timeout_ms: 30_000,
                    max_concurrency: 4,
                    input_schema: None,
                });
            }
        }
        descriptors
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        if !inv.approved {
            return Err(ToolError::AskRequired(
                "mcp tool requires explicit approval".into(),
            ));
        }
        let Some(rest) = inv.tool.strip_prefix("mcp:") else {
            return Err(ToolError::InvalidArgs(format!(
                "mcp tool must look like mcp:<server>/<tool>, got {}",
                inv.tool
            )));
        };
        let Some((server, tool)) = rest.split_once('/') else {
            return Err(ToolError::InvalidArgs(format!(
                "mcp tool must look like mcp:<server>/<tool>, got {}",
                inv.tool
            )));
        };
        tokio::select! {
            result = self.mesh.invoke_tool(server, tool, &inv.args_json, inv.timeout_ms) => {
                result.map_err(|error| ToolError::Provider(error.to_string()))
            }
            _ = cancel.cancelled() => Err(ToolError::Cancelled(inv.call_id.clone())),
        }
    }
}
