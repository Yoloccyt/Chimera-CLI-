//! MCP tool provider —— 消费**装配期发现快照**，按 `mcp:<server>/<tool>` 路由调用。
//!
//! # 与旧形状的区别（ADR-235 S5）
//! 旧实现从 `McpMesh` 注册表的 `capabilities: Vec<String>` 造描述符，名字对、
//! 但那是**自研 2PC 注册表**而不是 MCP 能力发现；且装配时 mesh 恒为空 ⇒ provider
//! 恒被丢弃。现在输入换成 [`DiscoverySnapshot`]：工具名、描述、`inputSchema` 与
//! 注解都来自真实 `tools/list`，客户端句柄与工具清单同属一个 `DiscoveredServer`，
//! 因此"出现在工具表里 ⇒ 一定可调用"是类型事实。
//!
//! # 副作用语义
//! 判定只在 [`mcp_mesh::protocol::side_effect_for`] 一处（ADR-235 D1）：
//! **默认全 `Write`**；只有逐 server 显式 `trust_readonly_hints = true`，且
//! `readOnlyHint == true` 且 `destructiveHint != true` 时才降为 `ReadOnly`。
//! 未获审批的调用一律 `AskRequired`（fail-closed，ADR-232 R3）。
//!
//! # 不在此路径上的东西
//! HTTP/Streamable HTTP 传输与端点 SSRF 校验本波未交付（ADR-235 D2）；
//! mesh 的 `register_server` 自带的 SSRF 守卫一行未动，但**不在本 provider 的
//! 调用链上** —— 别把那条守卫当成这条路径的保护。

use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::tool_plane::{ToolDescriptor, ToolError, ToolInvocation, ToolProvider};
use mcp_mesh::protocol::{side_effect_for, DiscoverySnapshot};
use tokio_util::sync::CancellationToken;

/// Provider backed by an assembly-time discovery snapshot.
pub struct McpToolProvider {
    snapshot: Arc<DiscoverySnapshot>,
}

impl McpToolProvider {
    /// New provider over a frozen discovery snapshot.
    #[must_use]
    pub fn new(snapshot: Arc<DiscoverySnapshot>) -> Self {
        Self { snapshot }
    }
}

fn malformed(tool: &str) -> ToolError {
    ToolError::InvalidArgs(format!(
        "mcp tool must look like mcp:<server>/<tool>, got {tool}"
    ))
}

fn map_error(error: mcp_mesh::McpError) -> ToolError {
    use mcp_mesh::McpError;
    match error {
        McpError::Denied { program, reason } => ToolError::Denied(format!("{program}: {reason}")),
        other => ToolError::Provider(other.to_string()),
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    fn provider_id(&self) -> &str {
        "mcp"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut descriptors = Vec::new();
        for server in &self.snapshot.servers {
            for tool in &server.tools {
                let description = if tool.description.is_empty() {
                    format!("MCP tool {} on server {}", tool.name, server.id)
                } else {
                    tool.description.clone()
                };
                descriptors.push(ToolDescriptor {
                    name: format!("mcp:{}/{}", server.id, tool.name),
                    provider: "mcp".to_string(),
                    description,
                    side_effect: side_effect_for(tool, server.trust_readonly_hints),
                    timeout_ms: 30_000,
                    max_concurrency: 4,
                    input_schema: tool.input_schema.as_ref().map(|schema| schema.to_string()),
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
        let rest = inv
            .tool
            .strip_prefix("mcp:")
            .ok_or_else(|| malformed(&inv.tool))?;
        let (server_id, tool_name) = rest.split_once('/').ok_or_else(|| malformed(&inv.tool))?;
        let (server, _tool) = self
            .snapshot
            .find(server_id, tool_name)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unknown MCP tool {}", inv.tool)))?;
        let arguments: serde_json::Value = serde_json::from_str(&inv.args_json)
            .map_err(|error| ToolError::InvalidArgs(format!("mcp args must be JSON: {error}")))?;
        let result = tokio::select! {
            result = server.client.call_tool(tool_name, arguments) => result,
            _ = cancel.cancelled() => return Err(ToolError::Cancelled(inv.call_id.clone())),
        };
        // 把 `tools/call` 的 result 原样序列化给上层（内容块由调用方解释）。
        result.map(|value| value.to_string()).map_err(map_error)
    }
}
