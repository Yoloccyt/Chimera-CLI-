//! 传输接缝：一帧 = 一条 JSON（不含分隔符），分隔符由实现负责。
//!
//! WHY 用 trait 而不是直接写 stdio：语义层（握手/发现/调用）必须能在**零外部进程**
//! 下测试 —— 本机与 CI 都没有现成的 MCP 服务器。`DuplexTransport` 用
//! `tokio::io::duplex` 提供进程内双向管道；`StdioTransport`（下一片）才是生产实现。

use std::future::Future;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

use crate::error::McpError;

/// 单帧上限。`tools/list` 响应可能很大，但不能无界：无换行的对端会让缓冲区一直
/// 增长，必须在**读入过程中**就有一道闸门（超限即判错并断开），而不是读完再查。
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 传输接缝。实现方负责分帧（stdio/duplex 用换行；将来的 HTTP 用 EOF/SSE 事件）。
pub trait McpTransport: Send + 'static {
    /// 发送一帧（实现负责追加分隔符并 flush）。
    fn send(&mut self, frame: &[u8]) -> impl Future<Output = Result<(), McpError>> + Send;

    /// 接收一帧；`Ok(None)` 表示对端**正常关闭**（EOF 且无半帧）。
    ///
    /// 实现不得把 EOF 伪装成空帧，也不得把"半帧后 EOF"当成正常关闭 ——
    /// 两者都会让上层把截断当成完整消息。
    fn recv(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, McpError>> + Send;
}

fn transport(err: impl std::fmt::Display) -> McpError {
    McpError::Transport {
        reason: err.to_string(),
    }
}

/// 进程内双向管道传输（仅测试/夹具使用）。
pub struct DuplexTransport {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    max_frame: usize,
}

impl DuplexTransport {
    /// 用默认帧上限包装一端。
    #[must_use]
    pub fn new(stream: DuplexStream) -> Self {
        Self::with_limit(stream, MAX_FRAME_BYTES)
    }

    /// 指定帧上限（小上限用于测"超限必须报错"）。
    #[must_use]
    pub fn with_limit(stream: DuplexStream, max_frame: usize) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            reader: BufReader::new(read),
            writer: write,
            max_frame: max_frame.max(1),
        }
    }

    /// 配一对：返回（客户端传输, 服务器侧原始流）。服务器侧由测试按行读写。
    #[must_use]
    pub fn pair(capacity: usize) -> (Self, DuplexStream) {
        let (client, server) = tokio::io::duplex(capacity);
        (Self::new(client), server)
    }
}

impl McpTransport for DuplexTransport {
    async fn send(&mut self, frame: &[u8]) -> Result<(), McpError> {
        if frame.len() > self.max_frame {
            return Err(transport(format!(
                "outbound frame {} bytes exceeds limit {}",
                frame.len(),
                self.max_frame
            )));
        }
        self.writer.write_all(frame).await.map_err(transport)?;
        self.writer.write_all(b"\n").await.map_err(transport)?;
        self.writer.flush().await.map_err(transport)?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, McpError> {
        let mut frame = Vec::new();
        loop {
            let available = self.reader.fill_buf().await.map_err(transport)?;
            if available.is_empty() {
                // EOF：干净结束 vs 半帧截断，必须分开报。
                return if frame.is_empty() {
                    Ok(None)
                } else {
                    Err(transport(format!(
                        "connection closed mid-frame after {} bytes",
                        frame.len()
                    )))
                };
            }
            if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
                if frame.len() + pos > self.max_frame {
                    return Err(transport(format!(
                        "inbound frame exceeds limit {}",
                        self.max_frame
                    )));
                }
                frame.extend_from_slice(&available[..pos]);
                self.reader.consume(pos + 1);
                break;
            }
            let taken = available.len();
            if frame.len() + taken > self.max_frame {
                return Err(transport(format!(
                    "inbound frame exceeds limit {}",
                    self.max_frame
                )));
            }
            frame.extend_from_slice(available);
            self.reader.consume(taken);
        }
        // 容许 CRLF（某些实现会带 \r）；只剥一个尾随 \r。
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
        Ok(Some(frame))
    }
}
