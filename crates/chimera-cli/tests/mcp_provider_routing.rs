//! MCP provider 的调用路由与审批前置（ADR-235 S5）。
//!
//! 两条判据：
//! 1. **未审批 ⇒ `AskRequired` 且零流量**：假服务器一个请求都收不到（审批是
//!    "进 runtime 之前"的闸，不是"进 runtime 之后被拒"）；
//! 2. 已审批 ⇒ 按 `mcp:<server>/<tool>` 路由到**该 server** 的客户端，并把
//!    `tools/call` 的 result 原样回传给上层。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gqep_executor::tool_plane::{
    ToolError, ToolInvocation, ToolOrigin, ToolPriority, ToolProvider,
};
use mcp_mesh::protocol::{
    ClientConfig, DiscoveredServer, DiscoverySnapshot, DuplexTransport, McpClient, McpTool,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

fn tool(name: &str) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: format!("{name} tool"),
        input_schema: None,
        annotations: None,
        extra: serde_json::Map::new(),
    }
}

fn invocation(tool_name: &str, approved: bool) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".to_string(),
        tool: tool_name.to_string(),
        args_json: r#"{"x":1}"#.to_string(),
        timeout_ms: None,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Cli,
        quest_id: None,
        task_id: None,
        approved,
    }
}

#[tokio::test]
async fn invoke_rejects_unapproved_without_touching_the_server() {
    let (transport, server) = DuplexTransport::pair(8192);
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    let server_task = tokio::spawn(async move {
        let (read, _write) = tokio::io::split(server);
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
            counter.fetch_add(1, Ordering::SeqCst);
            line.clear();
        }
    });
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "s1".to_string(),
            tools: vec![tool("echo")],
            trust_readonly_hints: false,
            client: Arc::clone(&client),
        }],
    });
    let provider = chimera_cli::tool_plane::McpToolProvider::new(snapshot);

    let error = provider
        .invoke(&invocation("mcp:s1/echo", false), CancellationToken::new())
        .await
        .expect_err("unapproved must be rejected");
    assert!(
        matches!(error, ToolError::AskRequired(_)),
        "expected AskRequired, got {error:?}"
    );
    // 零流量：审批闸在写帧之前，服务器一个字节都不该收到。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "未审批的调用不得产生任何请求帧"
    );

    client.shutdown().await;
    let _ = server_task.await;
}

#[tokio::test]
async fn invoke_routes_approved_call_and_returns_server_payload() {
    let (transport, server) = DuplexTransport::pair(16 * 1024);
    let seen = Arc::new(std::sync::Mutex::new(json!(null)));
    let recorder = Arc::clone(&seen);
    let server_task = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(server);
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
            let request: serde_json::Value = serde_json::from_str(line.trim_end()).expect("json");
            recorder.lock().unwrap().clone_from(&request);
            let reply = json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "content": [{ "type": "text", "text": "ok:echo" }] }
            });
            let mut bytes = serde_json::to_vec(&reply).expect("serialize");
            bytes.push(b'\n');
            write.write_all(&bytes).await.expect("write");
            write.flush().await.expect("flush");
            line.clear();
        }
    });
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "s1".to_string(),
            tools: vec![tool("echo")],
            trust_readonly_hints: false,
            client: Arc::clone(&client),
        }],
    });
    let provider = chimera_cli::tool_plane::McpToolProvider::new(snapshot);

    let payload = provider
        .invoke(&invocation("mcp:s1/echo", true), CancellationToken::new())
        .await
        .expect("approved call must succeed");
    assert!(
        payload.contains("ok:echo"),
        "result 应原样回传，实得 {payload}"
    );

    let request = seen.lock().unwrap().clone();
    assert_eq!(request["method"], "tools/call");
    assert_eq!(request["params"]["name"], "echo");
    assert_eq!(request["params"]["arguments"]["x"], 1);

    client.shutdown().await;
    server_task.await.expect("server task");
}

#[tokio::test]
async fn unknown_tool_and_malformed_name_are_invalid_args() {
    let (transport, peer) = DuplexTransport::pair(1024);
    drop(peer);
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "s1".to_string(),
            tools: vec![tool("echo")],
            trust_readonly_hints: false,
            client: Arc::clone(&client),
        }],
    });
    let provider = chimera_cli::tool_plane::McpToolProvider::new(snapshot);

    for bad in ["echo", "mcp:s1", "mcp:s1/nope", "mcp:noserver/echo"] {
        let error = provider
            .invoke(&invocation(bad, true), CancellationToken::new())
            .await
            .expect_err("bad name must be rejected");
        assert!(
            matches!(error, ToolError::InvalidArgs(_)),
            "{bad} 应判 InvalidArgs，实得 {error:?}"
        );
    }
    client.shutdown().await;
}
