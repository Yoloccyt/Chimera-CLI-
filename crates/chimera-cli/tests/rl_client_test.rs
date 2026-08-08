//! rl-client HTTP 通道端到端测试（Milestone C-3）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §6 C-3 验收）：
//! "与 Python 服务联调通过（mock server 先行）"——本地 mock HTTP server
//! 验证 HttpRlClient 的 push_experiences / health_check 端到端。
//! 仅 `rl-client` feature 开启时编译（required-features）。

#![forbid(unsafe_code)]
#![cfg(feature = "rl-client")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use chimera_cli::rl_client::{HttpRlClient, RlClient};
use nexus_contracts::reward::{RewardLayer, RewardSignal, RewardSpec};

/// 最小 mock 训练服务 — 处理 POST /experiences 与 GET /health
fn spawn_mock_server() -> (String, std::sync::mpsc::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口应成功");
    let addr = listener.local_addr().expect("获取地址应成功");
    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        // 只处理一次请求（测试单发）——循环直至收到 /health
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let Ok(n) = stream.read(&mut buf) else {
                continue;
            };
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            if request.starts_with("POST /experiences") {
                // 返回 JSON 确认（accepted = 请求体条数，简化取 1）
                let body = "{\"accepted\":1,\"message\":\"ok\"}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = tx.send(1);
            } else if request.starts_with("GET /health") {
                let body = "{\"status\":\"ok\"}";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = tx.send(0);
            } else {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    });

    (format!("http://{addr}"), rx)
}

/// 构造测试 RewardSignal 批次
fn make_batch() -> Vec<RewardSignal> {
    let spec = RewardSpec::new("rs-test", RewardLayer::L6, "s9_route", 1.0);
    vec![
        RewardSignal::new(&spec, 0.8, 1_000),
        RewardSignal::new(&spec, 0.5, 1_001),
    ]
}

/// 端到端：POST /experiences 上传成功
#[test]
fn push_experiences_end_to_end() {
    let (base, rx) = spawn_mock_server();
    let client = HttpRlClient::new(base).expect("客户端创建应成功");
    let batch = make_batch();

    let summary = client.push_experiences(&batch).expect("上传应成功");
    assert_eq!(summary.accepted, 1);
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
}

/// 端到端：GET /health 健康检查
#[test]
fn health_check_end_to_end() {
    let (base, rx) = spawn_mock_server();
    let client = HttpRlClient::new(base).expect("客户端创建应成功");

    assert!(
        client.health_check().expect("健康检查应成功"),
        "mock 服务应返回 200"
    );
    let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
}

/// 空批次：不上传，直接返回 accepted=0
#[test]
fn empty_batch_is_noop() {
    let client = HttpRlClient::new("http://127.0.0.1:1").expect("客户端创建应成功");
    let summary = client.push_experiences(&[]).expect("空批次不应失败");
    assert_eq!(summary.accepted, 0);
}

/// 连接失败：上传返回错误（不 panic）
#[test]
fn unreachable_server_returns_error() {
    let client = HttpRlClient::new("http://127.0.0.1:1").expect("客户端创建应成功");
    let batch = make_batch();
    assert!(
        client.push_experiences(&batch).is_err(),
        "不可达服务应返回 Err"
    );
}
