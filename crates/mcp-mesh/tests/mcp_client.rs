//! `McpClient` 的行为锁（ADR-235 S3）。
//!
//! 全部用**进程内 duplex 假服务器**：零外部进程、零 Python 依赖、可复现。
//! 每条"必须失败"的用例都同时断言**失败类型**（不只是 rc/Err），因为
//! "超时"与"连接关闭"在只看 Err 的断言里无法区分。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcp_mesh::protocol::{ClientConfig, DuplexTransport, McpClient, MCP_PROTOCOL_VERSION};
use mcp_mesh::McpError;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

struct FakeServer {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl FakeServer {
    fn new(stream: DuplexStream) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(read),
            writer: write,
        }
    }

    /// 读一条 JSON 帧；EOF 返回 None。
    async fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line).await {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                Some(serde_json::from_str(line.trim_end()).expect("server parses client frame"))
            }
        }
    }

    async fn write(&mut self, value: &Value) {
        let mut bytes = serde_json::to_vec(value).expect("serialize");
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.expect("server write");
        self.writer.flush().await.expect("server flush");
    }
}

fn client_with_timeout(transport: DuplexTransport, ms: u64) -> McpClient {
    McpClient::spawn(
        transport,
        ClientConfig {
            request_timeout_ms: ms,
        },
    )
}

#[tokio::test]
async fn initialize_pins_protocol_version_and_sends_initialized_notification() {
    let (transport, server) = DuplexTransport::pair(16 * 1024);
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorder = Arc::clone(&seen);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let request = server.read().await.expect("initialize request");
        assert_eq!(request["method"], "initialize");
        assert_eq!(request["params"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(request["params"]["clientInfo"]["name"], "chimera-cli");
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": {
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "serverInfo": { "name": "fake-server", "version": "9.9" }
                }
            }))
            .await;
        if let Some(notification) = server.read().await {
            recorder.lock().unwrap().push(notification);
        }
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let info = client
        .initialize()
        .await
        .expect("initialize should succeed");
    assert_eq!(info.protocol_version, MCP_PROTOCOL_VERSION);
    assert_eq!(info.name, "fake-server");
    assert_eq!(info.version, "9.9");

    // notify() 只保证"已投递进发送队列"；服务器读到它是异步的，故轮询等待。
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while seen.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let notifications = seen.lock().unwrap().clone();
    assert_eq!(
        notifications.len(),
        1,
        "初始化后必须发一条 initialized 通知"
    );
    assert_eq!(notifications[0]["method"], "notifications/initialized");
    assert!(
        notifications[0].get("id").is_none(),
        "通知不得带 id（带了就成了请求）"
    );

    client.shutdown().await;
    server_task.await.expect("server task");
}

#[tokio::test]
async fn initialize_rejects_unsupported_protocol_version() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let request = server.read().await.expect("initialize request");
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "protocolVersion": "1999-01-01" }
            }))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let error = client.initialize().await.expect_err("must reject");
    assert!(
        matches!(error, McpError::UnsupportedVersion { .. }),
        "expected UnsupportedVersion, got {error:?}"
    );
    client.shutdown().await;
    let _ = server_task.await;
}

#[tokio::test]
async fn list_tools_follows_next_cursor_and_dedupes_by_name() {
    let (transport, server) = DuplexTransport::pair(16 * 1024);
    let cursors = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorder = Arc::clone(&cursors);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let first = server.read().await.expect("page 1 request");
        recorder.lock().unwrap().push(first["params"].clone());
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": first["id"],
                "result": {
                    "tools": [
                        {"name": "alpha", "description": "a"},
                        {"name": "beta"}
                    ],
                    "nextCursor": "p2"
                }
            }))
            .await;
        let second = server.read().await.expect("page 2 request");
        recorder.lock().unwrap().push(second["params"].clone());
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": second["id"],
                "result": {
                    "tools": [
                        {"name": "beta", "description": "dup"},
                        {"name": "gamma", "annotations": {"readOnlyHint": true}}
                    ]
                }
            }))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let tools = client.list_tools().await.expect("list_tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["alpha", "beta", "gamma"],
        "保序 + 按名去重（先到者胜）"
    );
    assert_eq!(
        tools[2].annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(true)
    );

    let cursors = cursors.lock().unwrap().clone();
    assert!(
        cursors[0].is_null(),
        "首页不得带 cursor，实际 {}",
        cursors[0]
    );
    assert_eq!(cursors[1]["cursor"], "p2");
    client.shutdown().await;
    server_task.await.expect("server task");
}

#[tokio::test]
async fn list_tools_detects_pagination_loop() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        while let Some(request) = server.read().await {
            server
                .write(&json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": { "tools": [], "nextCursor": "again" }
                }))
                .await;
        }
    });
    let client = client_with_timeout(transport, 2_000);
    let error = client.list_tools().await.expect_err("loop must be fatal");
    assert!(
        format!("{error}").contains("pagination loop"),
        "expected loop error, got {error:?}"
    );
    client.shutdown().await;
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_are_matched_by_id_not_arrival_order() {
    let (transport, server) = DuplexTransport::pair(16 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let first = server.read().await.expect("request 1");
        let second = server.read().await.expect("request 2");
        // 先回第二个请求：按到达序配对的实现会把结果张冠李戴。
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": second["id"],
                "result": { "echo": second["method"] }
            }))
            .await;
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": first["id"],
                "result": { "echo": first["method"] }
            }))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let (one, two) = tokio::join!(client.request("m1", None), client.request("m2", None));
    assert_eq!(one.expect("first"), json!({"echo": "m1"}));
    assert_eq!(two.expect("second"), json!({"echo": "m2"}));
    client.shutdown().await;
    server_task.await.expect("server task");
}

#[tokio::test]
async fn request_timeout_clears_pending_and_session_survives() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let _slow = server.read().await.expect("slow request");
        // 故意不回第一个（让它超时）；第二个请求一到就回 —— WHY 不等固定时长：
        // 若服务器也睡 250ms，第二个请求会撞上它自己的 150ms 超时窗，测的就不是
        // "超时后会话仍可用"而是"两个请求都超时"。
        let fast = tokio::time::timeout(Duration::from_secs(2), server.read())
            .await
            .expect("第二个请求应在 2s 内到达")
            .expect("fast request");
        server
            .write(&json!({"jsonrpc": "2.0", "id": fast["id"], "result": {"ok": true}}))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 150);
    let error = client
        .request("slow", None)
        .await
        .expect_err("must time out");
    assert!(
        matches!(error, McpError::RequestTimeout { .. }),
        "expected RequestTimeout, got {error:?}"
    );
    assert_eq!(client.in_flight(), 0, "超时后 pending 必须被摘除，不许泄漏");

    let ok = client
        .request("fast", None)
        .await
        .expect("session survives a timeout");
    assert_eq!(ok, json!({"ok": true}));
    client.shutdown().await;
    let _ = server_task.await;
}

#[tokio::test]
async fn eof_fails_pending_with_closed_instead_of_hanging() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let _request = server.read().await.expect("request");
        // 直接关闭对端，不回包
        drop(server);
    });

    // 5s 超时：若 EOF 没把 pending 立即失败，这条测试会等满 5s（并报 RequestTimeout）
    let client = client_with_timeout(transport, 5_000);
    let error = client
        .request("no-reply", None)
        .await
        .expect_err("must fail");
    assert!(
        matches!(error, McpError::Closed),
        "expected Closed from EOF, got {error:?}"
    );
    assert!(client.is_closed());
    let _ = server_task.await;
}

#[tokio::test]
async fn malformed_frame_is_fatal_for_the_connection() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let _request = server.read().await.expect("request");
        server
            .writer
            .write_all(b"this is not json\n")
            .await
            .expect("garbage write");
        server.writer.flush().await.expect("flush");
    });

    let client = client_with_timeout(transport, 5_000);
    let error = client.request("x", None).await.expect_err("must fail");
    assert!(
        matches!(error, McpError::Protocol { .. }),
        "expected Protocol error, got {error:?}"
    );
    assert!(client.is_closed());
    let _ = server_task.await;
}

#[tokio::test]
async fn unknown_response_id_is_ignored_and_session_continues() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let request = server.read().await.expect("request");
        // 先发一条无人认领的 id，再发正确的
        server
            .write(&json!({"jsonrpc": "2.0", "id": 999, "result": "stray"}))
            .await;
        server
            .write(&json!({"jsonrpc": "2.0", "id": request["id"], "result": {"ok": 1}}))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let value = client
        .request("ping", None)
        .await
        .expect("session survives stray id");
    assert_eq!(value, json!({"ok": 1}));
    client.shutdown().await;
    let _ = server_task.await;
}

#[tokio::test]
async fn call_tool_sends_name_and_arguments() {
    let (transport, server) = DuplexTransport::pair(16 * 1024);
    let seen = Arc::new(Mutex::new(json!(null)));
    let recorder = Arc::clone(&seen);
    let server_task = tokio::spawn(async move {
        let mut server = FakeServer::new(server);
        let request = server.read().await.expect("tools/call");
        recorder.lock().unwrap().clone_from(&request);
        server
            .write(&json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "content": [{ "type": "text", "text": "hi" }] }
            }))
            .await;
        while server.read().await.is_some() {}
    });

    let client = client_with_timeout(transport, 2_000);
    let result = client
        .call_tool("echo", json!({"text": "hi"}))
        .await
        .expect("call_tool");
    assert_eq!(result["content"][0]["text"], "hi");
    let request = seen.lock().unwrap().clone();
    assert_eq!(request["method"], "tools/call");
    assert_eq!(request["params"]["name"], "echo");
    assert_eq!(request["params"]["arguments"]["text"], "hi");
    client.shutdown().await;
    server_task.await.expect("server task");
}

#[tokio::test]
async fn shutdown_completes_promptly_even_after_peer_failure() {
    let (transport, server) = DuplexTransport::pair(8 * 1024);
    drop(server);
    let client = client_with_timeout(transport, 1_000);
    let started = std::time::Instant::now();
    client.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown 在已 EOF 的连接上不得等满 5s：耗时 {:?}",
        started.elapsed()
    );
}
