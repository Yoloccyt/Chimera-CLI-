//! 传输 seam — AppTransport 与 stdio 实现（WI-01 §6.2）
//!
//! # capability seam ①
//! `AppTransport` 为传输抽象（stdio/SSE 双实现；SSE 为后续扩展，
//! `chimera serve` 形态 = SSE 传输 + Workspace 绑定 + ACP 子进程托管）。
//!
//! # stdio 传输语义
//! - 每行一帧（NDJSON），行尾 `\n`
//! - `recv_op`: 阻塞读 stdin 一行 → 解码 AppOp
//! - `send_event`: 编码 AppEvent 推送帧 → stdout（事件下行不经响应通道）
//! - 日志/进度不得写 stdout（WI-02 exec stdout 纪律——传输层为最后防线）

use async_trait::async_trait;
use nexus_contracts::app::{AppEvent, AppOp};
use thiserror::Error;

/// 传输错误 — 帧层 IO/编解码错误
#[derive(Debug, Error)]
pub enum TransportError {
    /// IO 错误（stdin/stdout 读写失败）
    #[error("transport io error: {0}")]
    Io(#[from] std::io::Error),
    /// 请求帧解码失败
    #[error("request decode error: {0}")]
    Decode(String),
    /// 事件帧编码失败
    #[error("event encode error: {0}")]
    Encode(String),
    /// EOF（对端关闭）
    #[error("transport closed")]
    Eof,
}

/// 传输抽象 — 协议传输层 seam（WI-01 §6.2）
///
/// # 实现契约
/// - `recv_op` 在无输入时返回 [`TransportError::Eof`]（调用方结束会话）
/// - `send_event` 为下行推送（服务端 → 客户端），不要求请求上下文
#[async_trait]
pub trait AppTransport: Send + Sync {
    /// 接收客户端操作（阻塞至下一帧）
    async fn recv_op(&self) -> Result<AppOp, TransportError>;

    /// 推送服务端事件（下行）
    async fn send_event(&self, ev: &AppEvent) -> Result<(), TransportError>;
}

/// stdio 传输 — NDJSON 行帧（每行一帧）
///
/// # 并发
/// reader/writer 均经 `tokio::sync::Mutex` 包裹以满足 `Send + Sync`
/// （BufReader/BufWriter 本身非 Sync）；读写分离双锁，互不阻塞。
#[derive(Debug)]
pub struct StdinTransport {
    /// stdin 行缓冲读取器（Mutex 包裹满足 Sync）
    reader: tokio::sync::Mutex<tokio::io::BufReader<tokio::io::Stdin>>,
    /// stdout（行写 + flush，Mutex 包裹满足 Sync）
    writer: tokio::sync::Mutex<tokio::io::BufWriter<tokio::io::Stdout>>,
}

impl StdinTransport {
    /// 创建 stdio 传输
    pub fn new() -> Self {
        Self {
            reader: tokio::sync::Mutex::new(tokio::io::BufReader::new(tokio::io::stdin())),
            writer: tokio::sync::Mutex::new(tokio::io::BufWriter::new(tokio::io::stdout())),
        }
    }
}

impl Default for StdinTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AppTransport for StdinTransport {
    async fn recv_op(&self) -> Result<AppOp, TransportError> {
        let mut line = String::new();
        use tokio::io::AsyncBufReadExt;
        // 锁内 await 风险: read_line 可能挂起——但 stdio 读取为会话主循环
        // 独占路径（无并发读者），锁等待者仅事件推送（写锁）不受影响；
        // 持锁跨 await 红线针对共享状态写路径，此处读锁语义安全
        let mut reader = self.reader.lock().await;
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(TransportError::Eof);
        }
        drop(reader); // 提前释放读锁
        let frame = crate::protocol::RpcCodec::decode_request_line(line.trim())
            .map_err(|e| TransportError::Decode(e.message))?;
        serde_json::from_value(frame.params)
            .map_err(|e| TransportError::Decode(format!("AppOp 反序列化失败: {e}")))
    }

    async fn send_event(&self, ev: &AppEvent) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;
        let frame =
            crate::protocol::RpcCodec::encode_notification(ev).map_err(TransportError::Encode)?;
        let mut writer = self.writer.lock().await;
        writer.write_all(frame.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_trait_send_sync() {
        // 编译期断言: AppTransport 可装箱为 Send + Sync trait object
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Box<dyn AppTransport>>();
    }
}
