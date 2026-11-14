//! MCP single-server tool invoke contracts (Phase 4 / ADR-230).
//!
//! Uses the default in-process participant client: no network, no external
//! server. Wire types are also round-tripped to lock the JSON shape.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use mcp_mesh::quantum::participant_client::{ToolInvokeRequest, ToolInvokeResponse};
use mcp_mesh::{McpError, McpMesh, MeshConfig, MeshServer};

fn mesh_with_server(server_id: &str) -> McpMesh {
    let mesh = McpMesh::new(MeshConfig::default());
    mesh.register_server(MeshServer::new(
        server_id,
        format!("203.0.113.1:{server_id}"),
        vec!["echo".to_string()],
    ))
    .expect("register server");
    mesh
}

#[tokio::test]
async fn invoke_returns_payload_with_args() {
    let mesh = mesh_with_server("s1");
    let payload = mesh
        .invoke_tool("s1", "echo", r#"{"x":1}"#, None)
        .await
        .expect("invoke");
    assert!(payload.contains("echo"), "payload={payload}");
    assert!(payload.contains(r#"{"x":1}"#), "payload={payload}");
    assert!(payload.contains("s1"), "payload={payload}");
}

#[tokio::test]
async fn invoke_unknown_server_is_not_found() {
    let mesh = McpMesh::new(MeshConfig::default());
    let error = mesh
        .invoke_tool("missing", "echo", "{}", None)
        .await
        .expect_err("unknown server must fail");
    assert!(
        matches!(error, McpError::ServerNotFound { .. }),
        "{error:?}"
    );
}

#[test]
fn tool_invoke_wire_types_roundtrip() {
    let request = ToolInvokeRequest {
        call_id: "c1".to_string(),
        tool: "echo".to_string(),
        args_json: r#"{"x":1}"#.to_string(),
    };
    let encoded = serde_json::to_string(&request).expect("encode request");
    let decoded: ToolInvokeRequest = serde_json::from_str(&encoded).expect("decode request");
    assert_eq!(decoded, request);

    let response = ToolInvokeResponse {
        payload: "ok".to_string(),
    };
    let encoded = serde_json::to_string(&response).expect("encode response");
    let decoded: ToolInvokeResponse = serde_json::from_str(&encoded).expect("decode response");
    assert_eq!(decoded, response);
}
