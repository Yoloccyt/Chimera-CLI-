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

use crate::protocol::{JsonRpcError, ProtocolError, RpcResponse};

/// 传输错误 — 帧层 IO/编解码错误
#[derive(Debug, Error)]
pub enum TransportError {
    /// IO 错误（stdin/stdout 读写失败）
    #[error("transport io error: {0}")]
    Io(#[from] std::io::Error),
    /// 请求帧解码失败——**保留 JSON-RPC 错误码语义**
    ///
    /// WHY 结构而非 String(架构方向 F-c 收尾,2026-09-13):`decode_request_line`
    /// 返回的 `JsonRpcError` 携带 `code`(-32700 解析 / -32601 无效请求 / -32602
    /// 无效参数),是未来「服务端回错误帧」(`encode_error`)的协议完备性依据;
    /// 此前 `.map_err(|e| e.message)` 拍平丢失 code。当前消费方(serve/acp 主循环)
    /// 仅 Display 打日志,结构化不增加其负担(Display 已实现)。
    #[error("request decode error: {0}")]
    Decode(JsonRpcError),
    /// 载荷反序列化失败（`frame.params` → `AppOp`）
    ///
    /// WHY 独立变体 + `#[from]`:与帧解码(`Decode`,协议层语义)区分——
    /// 载荷失败是 `serde_json` 层错误,保留 source 链供排障(同 `Encode` 的
    /// `ProtocolError` 对偶,decode/encode 两侧错误面均结构化)。
    #[error("AppOp payload deserialization failed: {source}")]
    Payload {
        /// 底层反序列化错误（保留 source 链）
        #[source]
        source: serde_json::Error,
    },
    /// 事件帧编码失败
    ///
    /// WHY `#[from] ProtocolError`(架构方向 F-c):保留协议层的阶段分类与
    /// `serde_json` source 链——此前以 `String` 承载会同时丢失两者,排障需重新复现。
    #[error("event encode error: {0}")]
    Encode(#[from] ProtocolError),
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

    /// 回送协议级错误帧 — 帧解码失败时的 JSON-RPC error response（架构方向 F-c 终章）
    ///
    /// # 默认实现（降级语义）
    /// 不回帧、直接 `Ok(())`：错误回帧是 **best-effort**——主循环对解码失败的
    /// 策略是「告警 + 继续等下一帧」，回帧失败/不支持都不应终止会话。
    /// 需要真实回帧能力的实现（如 [`StdinTransport`]）覆写本方法。
    ///
    /// WHY 默认方法而非必需方法:`SseConnection` 为单向推送（recv 恒 `Eof`，
    /// 无请求上下文可关联）、`MockTransport` 等测试桩无需空壳实现——
    /// 默认实现让未覆写者自动获得安全的降级行为。
    ///
    /// # 参数
    /// - `error`: 帧解码错误（含 JSON-RPC `code`：-32700 解析 / -32601 无效请求）
    async fn send_decode_error(&self, error: &JsonRpcError) -> Result<(), TransportError> {
        let _ = error;
        Ok(())
    }
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
        // decode 失败保留完整 JsonRpcError(code + message,不再拍 .message);
        // 载荷反序列化失败经 #[from] serde_json::Error 转 Payload(source 链保留)
        let frame = crate::protocol::RpcCodec::decode_request_line(line.trim())
            .map_err(TransportError::Decode)?;
        serde_json::from_value(frame.params).map_err(|e| TransportError::Payload { source: e })
    }

    async fn send_event(&self, ev: &AppEvent) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;
        // `?` 经 `#[from] ProtocolError` 自动转换:阶段分类与 serde_json source 链均保留
        let frame = crate::protocol::RpcCodec::encode_notification(ev)?;
        let mut writer = self.writer.lock().await;
        writer.write_all(frame.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    /// 覆写错误回帧 — stdio 通道写 JSON-RPC error response 行
    ///
    /// # id=0 约定
    /// 解码失败时请求 id **不可知**（帧未成功解析）。JSON-RPC 2.0 规范对
    /// parse error 用 `id: null`，但本协议的 [`RpcResponse::id`] 为 `u64`
    /// （非 Option，wire 形态不变更）——约定 **id=0 表示「无法关联请求」**
    /// （客户端合法 id 自 1 起，protocol_client 侧对未知 id 的响应按
    /// 「丢弃/日志」宽容处理，不会误关联）。
    async fn send_decode_error(&self, error: &JsonRpcError) -> Result<(), TransportError> {
        use tokio::io::AsyncWriteExt;
        // NDJSON error response 行:{"jsonrpc":"2.0","id":0,"result":null,"error":{...}}
        let frame = serde_json::to_string(&RpcResponse {
            jsonrpc: "2.0".into(),
            id: 0,
            result: None,
            error: Some(error.clone()),
        })
        .map_err(|e| TransportError::Payload { source: e })?;
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
    use crate::protocol::{JsonRpcError, RpcCodec};

    #[test]
    fn transport_trait_send_sync() {
        // 编译期断言: AppTransport 可装箱为 Send + Sync trait object
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Box<dyn AppTransport>>();
    }

    // ========================================================
    // F-c 收尾:decode 侧错误结构化(2026-09-13)
    // ========================================================

    /// 帧解码失败保留 JSON-RPC 错误码(修复前:仅 .message,code -32700 丢失)
    ///
    /// 驱动 `decode_request_line` 的**真实错误路径**(垃圾行 → parse_error),
    /// 经与 `recv_op` 相同的 `map_err(TransportError::Decode)` 形态断言:
    /// 错误变体携带完整 `JsonRpcError`(code + message)。
    #[test]
    fn decode_error_preserves_json_rpc_code() {
        let je = RpcCodec::decode_request_line("not json")
            .expect_err("垃圾行必须报 parse_error")
            ;
        assert_eq!(je.code, -32700, "parse_error 语义");
        let err: TransportError = RpcCodec::decode_request_line("not json")
            .map_err(TransportError::Decode)
            .expect_err("同 recv_op 的转换形态");
        match &err {
            TransportError::Decode(je) => {
                assert_eq!(je.code, -32700, "code 必须保留(修复前丢失)");
                assert_eq!(je.message, "parse error");
            }
            other => panic!("应为 Decode 变体, 实际: {other}"),
        }
        // Display 链:日志里应能读到 code
        assert!(err.to_string().contains("-32700"), "Display 应含 code: {err}");
    }

    /// 载荷反序列化失败保留 serde_json source 链(修复前:format! 拍平)
    #[test]
    fn payload_error_keeps_source_chain() {
        use std::error::Error as _;
        // 真实失败路径:字符串不是合法 AppOp(from_value 报 serde_json::Error)
        let payload_err = serde_json::from_value::<nexus_contracts::app::AppOp>(
            serde_json::json!("not-an-op"),
        )
        .expect_err("字符串必须反序列化失败");
        let err = TransportError::Payload {
            source: payload_err,
        };
        // Display 语义:指明载荷与阶段
        assert!(
            err.to_string().contains("AppOp"),
            "Display 应指明载荷语义: {err}"
        );
        // source 链:Error::source() → serde_json::Error 可下钻
        let src = err.source().expect("必须保留底层 serde_json 错误");
        assert!(
            src.downcast_ref::<serde_json::Error>().is_some(),
            "source 应可下钻为 serde_json::Error: {src}"
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]

        /// 属性:任意 message 文案都完整出现在 JsonRpcError 的 Display 与
        /// TransportError::Decode 的 Display 中(防文案模板回归)。
        #[test]
        fn prop_json_rpc_error_display(message in "[a-z ]{0,60}") {
            let je = JsonRpcError::new(-32601, message.clone());
            proptest::prop_assert!(je.to_string().contains(&message));
            let err = TransportError::Decode(je);
            proptest::prop_assert!(err.to_string().contains(&message));
        }
    }
}
