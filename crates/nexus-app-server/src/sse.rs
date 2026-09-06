//! SSE 传输 — AppTransport 的 Server-Sent Events 实现（P3-T5，D-P11：T6 遗留三项之一）
//!
//! 对应架构层: **L10 Interface**（nexus-app-server）
//! 对应任务: **P3-T5**（手册 W16，T6 遗留:SSE 传输）
//!
//! # 设计
//! `chimera serve` 形态 = SSE 传输 + Workspace 绑定 + ACP 子进程托管（WI-01）;
//! 本模块提供 SSE 传输的**协议层**实现:
//! - [`SseServer`]:`TcpListener` 绑定,accept 后解析简化 HTTP/1.1 POST 请求
//!   （Content-Length body = AppOp JSON）,返回 [`SseConnection`];
//! - [`SseConnection`]:`send_event` 写 SSE 帧（`data: {json}\n\n`）;
//! - 零新依赖（tokio TcpListener + 手写极简 HTTP 头解析——协议面固定,
//!   不引入 hyper 全量依赖,Ω₆ 最小依赖）。
//!
//! # 边界
//! 本实现为**协议适配层**:HTTP 升级/keep-alive/多路复用由宿主接入层承接;
//! 单请求连接模型（POST → SSE 流）,与 `chimera serve` 主循环语义对齐。

use async_trait::async_trait;
use nexus_contracts::app::{AppEvent, AppOp};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::transport::{AppTransport, TransportError};

/// SSE 传输错误
#[derive(Debug, Error)]
pub enum SseError {
    /// IO 错误
    #[error("sse io error: {0}")]
    Io(#[from] std::io::Error),
    /// HTTP 请求解析失败
    #[error("http parse error: {0}")]
    Http(String),
    /// AppOp 反序列化失败
    #[error("app op decode error: {0}")]
    Decode(String),
    /// 连接关闭（EOF）
    #[error("sse connection closed")]
    Closed,
}

/// SSE 服务器 — 绑定监听 + 接受连接
#[derive(Debug)]
pub struct SseServer {
    /// 监听器
    listener: TcpListener,
}

impl SseServer {
    /// 绑定地址
    pub async fn bind(addr: &str) -> Result<Self, SseError> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    /// 本地地址（测试/诊断）
    pub fn local_addr(&self) -> Result<std::net::SocketAddr, SseError> {
        Ok(self.listener.local_addr()?)
    }

    /// 接受一个连接 — 解析 HTTP 请求,返回 (AppOp, SSE 连接)
    ///
    /// 请求格式（极简 HTTP/1.1）:
    /// ```text
    /// POST / HTTP/1.1
    /// Content-Type: application/json
    /// Content-Length: <n>
    ///
    /// {AppOp JSON}
    /// ```
    pub async fn accept(&self) -> Result<(AppOp, SseConnection), SseError> {
        let (stream, _peer) = self.listener.accept().await?;
        // into_split 读写分离:读侧消费 HTTP 请求,写侧承载 SSE 帧推送
        let (read_half, write_half) = stream.into_split();
        let op = parse_http_op(read_half).await?;
        let conn = SseConnection::new(write_half);
        Ok((op, conn))
    }
}

/// SSE 连接 — 下行事件推送（`data: {json}\n\n` 帧）
#[derive(Debug)]
pub struct SseConnection {
    /// TCP 写侧（读侧已由 accept 消费请求体;into_split 承载）
    ///
    /// WHY Mutex:OwnedWriteHalf 无 Clone,`AppTransport::send_event(&self)`
    /// 需共享写句柄;锁内无跨 await 之外的操作（写为原子帧,持锁跨 await 可接受
    /// ——单连接独占写路径,无其他竞争者）
    writer: tokio::sync::Mutex<tokio::net::tcp::OwnedWriteHalf>,
}

impl SseConnection {
    /// 新建连接（accept 后;发送 HTTP 200 + SSE 响应头）
    fn new(writer: tokio::net::tcp::OwnedWriteHalf) -> Self {
        // 响应头:200 + event-stream（写失败仅日志——连接级错误由 send_event 暴露）
        let _ = writer.try_write(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
        );
        Self {
            writer: tokio::sync::Mutex::new(writer),
        }
    }
}

#[async_trait]
impl AppTransport for SseConnection {
    /// SSE 连接为单向推送——接收侧已由 accept 消费（返回 Closed）
    async fn recv_op(&self) -> Result<AppOp, TransportError> {
        Err(TransportError::Eof)
    }

    async fn send_event(&self, ev: &AppEvent) -> Result<(), TransportError> {
        let json = serde_json::to_string(ev).map_err(|e| TransportError::Encode(e.to_string()))?;
        let frame = format!("data: {json}\n\n");
        let mut writer = self.writer.lock().await;
        writer
            .write_all(frame.as_bytes())
            .await
            .map_err(TransportError::Io)?;
        writer.flush().await.map_err(TransportError::Io)?;
        Ok(())
    }
}

/// 解析 HTTP 请求 — 逐行读头到空行 + Content-Length 读体,反序列化 AppOp
///
/// WHY read_until 而非 read:read 会把整个请求（头+体）一次性消费进 head,
/// 后续 read_exact 读 body 时底层已无数据 → 永久等待;read_until 只消费到
/// 行尾,body 保留在 BufReader 内部缓冲,read_exact 可精确消费。
async fn parse_http_op(mut read_half: tokio::net::tcp::OwnedReadHalf) -> Result<AppOp, SseError> {
    let mut reader = BufReader::new(&mut read_half);
    // 逐行读请求头（限制 64KB 防恶意头膨胀）
    let mut head_bytes = 0usize;
    let mut content_length: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .await
            .map_err(SseError::Io)?;
        if n == 0 {
            return Err(SseError::Closed);
        }
        head_bytes += n;
        if head_bytes > 64 * 1024 {
            return Err(SseError::Http("request head too large".into()));
        }
        // 空行（\r\n 或 \n）= 头部结束
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        // Content-Length 解析（大小写不敏感）
        let line_str = String::from_utf8_lossy(&line);
        let lower = line_str.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = Some(
                v.trim()
                    .parse::<usize>()
                    .map_err(|_| SseError::Http("invalid Content-Length".into()))?,
            );
        }
    }
    let content_length =
        content_length.ok_or_else(|| SseError::Http("missing Content-Length".into()))?;
    if content_length > 1024 * 1024 {
        return Err(SseError::Http("body too large".into()));
    }
    // 读体（body 保留在 reader 内部缓冲,read_exact 精确消费）
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.map_err(SseError::Io)?;
    let op: AppOp = serde_json::from_slice(&body)
        .map_err(|e| SseError::Decode(format!("AppOp 反序列化失败: {e}")))?;
    Ok(op)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::app::{Thread, ThreadId};
    use tokio::net::TcpStream;

    /// SSE 会话 E2E — 客户端 POST → 服务端解析 AppOp → SSE 帧推送
    ///
    /// WHY multi_thread:accept 阻塞在 listener + 客户端在同一 runtime 交互,
    /// current_thread 下 spawn 任务与主任务在单线程轮转,accept 就绪通知与
    /// 数据读取的时序在部分平台（Windows GNU）易死锁;multi_thread 双线程解耦。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sse_session_e2e() {
        let server = SseServer::bind("127.0.0.1:0").await.expect("绑定成功");
        let addr = server.local_addr().expect("本地地址");

        // 客户端:连接 + 发送 HTTP POST（内核握手完成,数据入缓冲）
        let mut client = TcpStream::connect(addr).await.expect("连接成功");
        let op = AppOp::ModeSet {
            mode: nexus_contracts::app::PermissionMode::Plan,
        };
        let body = serde_json::to_string(&op).expect("序列化成功");
        let req = format!(
            "POST / HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        client
            .write_all(req.as_bytes())
            .await
            .expect("请求发送成功");

        // 服务端 accept 任务（连接已就绪,accept 立即返回）
        let accept_task = tokio::spawn(async move { server.accept().await });
        let (received_op, conn) =
            tokio::time::timeout(std::time::Duration::from_secs(10), accept_task)
                .await
                .expect("accept 超时(挂起)")
                .expect("accept 任务完成")
                .expect("accept 成功");
        assert_eq!(received_op, op, "服务端必须解析出原 AppOp");

        // 服务端推送 SSE 帧 → 客户端读取
        let ev = AppEvent::ThreadStarted {
            thread: Thread::new(ThreadId::new("g::r"), "g", "r", 1),
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), conn.send_event(&ev))
            .await
            .expect("send_event 超时(挂起)")
            .expect("推送成功");

        // 客户端读响应头 + SSE 帧
        let mut buf = [0u8; 2048];
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), client.read(&mut buf))
            .await
            .expect("客户端读超时(挂起)")
            .expect("读取成功");
        let text = String::from_utf8_lossy(&buf[..n]);
        assert!(text.contains("200 OK"), "必须返回 200: {text}");
        assert!(text.contains("data: "), "必须含 SSE data 帧");
        assert!(
            text.contains("\"ThreadStarted\""),
            "帧内必须含 ThreadStarted"
        );
    }

    /// 缺 Content-Length — 解析失败（协议错误路径）
    #[tokio::test]
    async fn missing_content_length_fails() {
        let server = SseServer::bind("127.0.0.1:0").await.expect("绑定成功");
        let addr = server.local_addr().expect("本地地址");
        let mut client = TcpStream::connect(addr).await.expect("连接成功");
        client
            .write_all(b"POST / HTTP/1.1\r\n\r\n")
            .await
            .expect("发送成功");
        let accept_task = tokio::spawn(async move { server.accept().await });
        let r = accept_task.await.expect("accept 任务完成");
        assert!(r.is_err(), "缺 Content-Length 必须报错");
    }

    /// 非法 JSON 体 — Decode 错误
    #[tokio::test]
    async fn invalid_body_fails() {
        let server = SseServer::bind("127.0.0.1:0").await.expect("绑定成功");
        let addr = server.local_addr().expect("本地地址");
        let mut client = TcpStream::connect(addr).await.expect("连接成功");
        let req = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{{broken", 8);
        client.write_all(req.as_bytes()).await.expect("发送成功");
        // 关闭写侧:read_exact(8) 只收到 7 字节 → EOF → UnexpectedEof 报错（不悬挂）
        let _ = client.shutdown().await;
        let accept_task = tokio::spawn(async move { server.accept().await });
        let r = accept_task.await.expect("accept 任务完成");
        assert!(r.is_err(), "非法 JSON 必须报错");
    }
}
