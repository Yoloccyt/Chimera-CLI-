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

/// 泛型 IO 传输 — NDJSON 行帧（每行一帧）
///
/// # 泛型化（架构方向 F-c 终章遗留,2026-09-13）
/// reader/writer 可注入（`with_io`），解锁端到端协议测试：
/// `Cursor<Vec<u8>>` 驱动真实 `recv_op` 错误路径与 `send_decode_error`
/// 回帧 I/O（此前具体类型 `BufReader<Stdin>` 不可注入,端到端测试被阻塞）。
///
/// # 向后兼容
/// `StdinTransport` 为 **类型别名**（`IoTransport<Stdin, Stdout>`）——
/// 既有调用点（`serve.rs`/`acp.rs` 的 `StdinTransport::new()`）零改动。
///
/// # 并发
/// reader/writer 均经 `tokio::sync::Mutex` 包裹以满足 `Send + Sync`
/// （BufReader/BufWriter 本身非 Sync）；读写分离双锁，互不阻塞。
///
/// # 边界说明
/// `R` 用 `AsyncRead` 而非 `AsyncBufRead`:`Stdin` 只实现 `AsyncRead`,
/// `AsyncBufRead` 由存储层的 `BufReader<R>` 提供(`R: AsyncRead` 时
/// `BufReader<R>: AsyncBufRead`)。
#[derive(Debug)]
pub struct IoTransport<R, W>
where
    R: tokio::io::AsyncRead + Unpin + Send,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    /// 行缓冲读取器(Mutex 包裹满足 Sync;`BufReader<R>` 提供 AsyncBufRead)
    reader: tokio::sync::Mutex<tokio::io::BufReader<R>>,
    /// 行写 + flush(Mutex 包裹满足 Sync)
    writer: tokio::sync::Mutex<tokio::io::BufWriter<W>>,
}

/// stdio 默认形态(向后兼容别名;`new()`/`Default` 沿用)
pub type StdinTransport = IoTransport<tokio::io::Stdin, tokio::io::Stdout>;

impl<R, W> IoTransport<R, W>
where
    R: tokio::io::AsyncRead + Unpin + Send,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    /// 注入自定义 reader/writer(测试用 `Cursor` 驱动端到端协议路径)
    pub fn with_io(reader: R, writer: W) -> Self {
        Self {
            reader: tokio::sync::Mutex::new(tokio::io::BufReader::new(reader)),
            writer: tokio::sync::Mutex::new(tokio::io::BufWriter::new(writer)),
        }
    }

    /// 消费传输,取回底层 writer(测试取回已写帧;关闭前 flush 场景)
    pub fn into_writer(self) -> W {
        self.writer.into_inner().into_inner()
    }
}

impl StdinTransport {
    /// 创建 stdio 传输(stdin/stdout 句柄直连)
    ///
    /// WHY 独立固有 impl 而非泛型 `new()`:`tokio::io::Stdin/Stdout` 不实现
    /// `Default`,泛型 `new() where R: Default` 对 stdio 形态不可用——
    /// stdio 专用构造放在 alias 的固有 impl 上,泛型形态一律走 `with_io`。
    ///
    /// WHY 无 Default impl(显式允许 clippy::new_without_default):
    /// `Default` 要求 `Stdin: Default`(不成立);stdio 句柄是进程级资源
    /// 连接,显式 `new()` 语义比隐式 `default()` 更准确。
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::with_io(tokio::io::stdin(), tokio::io::stdout())
    }
}

#[async_trait]
impl<R, W> AppTransport for IoTransport<R, W>
where
    R: tokio::io::AsyncRead + Unpin + Send + Sync,
    W: tokio::io::AsyncWrite + Unpin + Send + Sync,
{
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
        // 帧构造提取为纯函数(decode_error_frame):wire 形态受单元测试锁定
        // (解析往返 + proptest 字段保真),此处仅负责 I/O。
        let frame = decode_error_frame(error)?;
        let mut writer = self.writer.lock().await;
        writer.write_all(frame.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }
}

/// 构造解码失败的错误响应帧（NDJSON 行,不含行尾 `\n`）
///
/// # 形态（受测试锁定的 wire 契约）
/// `{"jsonrpc":"2.0","id":0,"result":null,"error":{...}}` —— **id=0 约定**
/// 表示「无法关联请求」（解码失败时请求 id 不可知；JSON-RPC 2.0 规范对
/// parse error 用 `id: null`,但本协议 [`RpcResponse::id`] 为 `u64`,
/// wire 形态不变更故以 0 约定替代;客户端合法 id 自 1 起,对未知 id 的
/// 响应按「丢弃/日志」宽容处理,不会误关联）。
///
/// # 参数
/// - `error`: 帧解码错误（含 JSON-RPC `code`）
///
/// # 返回
/// `Ok(NDJSON 行字符串)`;`Err` 仅在 `JsonRpcError` 序列化失败时出现
/// （理论上不可达——纯数据结构,防御性保留与 `send_event` 同构的错误形态）。
fn decode_error_frame(error: &JsonRpcError) -> Result<String, TransportError> {
    serde_json::to_string(&RpcResponse {
        jsonrpc: "2.0".into(),
        id: 0,
        result: None,
        error: Some(error.clone()),
    })
    .map_err(|e| TransportError::Payload { source: e })
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

    /// 回帧 wire 契约:decode_error_frame 产出的 NDJSON 行可被客户端按
    /// `RpcResponse` 解析,且 id=0 / error.code / result=None 逐字段保真
    /// (F-c 终章回帧的客户端侧契约锁定;端到端 stdin 需泛型化改造,独立项)。
    #[test]
    fn decode_error_frame_is_parseable_rpc_response() {
        let frame = decode_error_frame(&JsonRpcError::parse_error())
            .expect("错误帧构造必须成功");
        let parsed: RpcResponse = serde_json::from_str(&frame).expect("帧必须可解析");
        assert_eq!(parsed.id, 0, "id=0 约定:无法关联请求");
        assert!(parsed.result.is_none(), "错误帧无 result");
        let je = parsed.error.expect("必须含 error 对象");
        assert_eq!(je.code, -32700, "code 语义保留");
        assert_eq!(je.message, "parse error");
        // 不含行尾换行(行写由调用方补,与 send_event 同构)
        assert!(!frame.ends_with('\n'), "帧本体不含行尾换行");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]

        /// 属性:任意 code/message 的错误帧均可解析,且 id=0 约定与
        /// error 字段逐字段保真(防序列化形态漂移)。
        #[test]
        fn prop_error_frame_roundtrip(
            code in -32700i32..-32000i32,
            message in "[a-z ]{0,60}",
        ) {
            let je = JsonRpcError::new(code, message.clone());
            let frame = decode_error_frame(&je).expect("错误帧构造必须成功");
            let parsed: RpcResponse = serde_json::from_str(&frame)
                .expect("错误帧必须可解析");
            proptest::prop_assert_eq!(parsed.id, 0);
            proptest::prop_assert!(parsed.result.is_none());
            let err = parsed.error.expect("必须含 error 对象");
            proptest::prop_assert_eq!(err.code, code);
            proptest::prop_assert_eq!(err.message, message);
        }
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

    // ========================================================
    // F-c 终章遗留闭环:IoTransport 泛型化解锁的端到端测试(2026-09-13)
    // ========================================================

    /// 端到端:真实 I/O 下的「坏帧 → 解码错误(code 保留) → 合法帧成功 → EOF」
    ///
    /// WHY 此前不可行:`StdinTransport` 的 `BufReader<Stdin>` 不可注入,
    /// 真实 decode 错误路径无法在测试中驱动;泛型化后 `Cursor` 直接注入。
    #[tokio::test]
    async fn recv_op_reports_decode_error_then_accepts_valid_frame() {
        use nexus_contracts::app::ThreadStartParams;
        // 第 1 行 = 坏帧;第 2 行 = 合法 ThreadStart 请求帧
        let valid = RpcCodec::encode_request(
            &AppOp::ThreadStart(ThreadStartParams::new("g1", "r1")),
            7,
        )
        .expect("合法帧编码成功");
        let input = format!("not json\n{valid}\n");
        let transport = IoTransport::with_io(
            std::io::Cursor::new(input.into_bytes()),
            tokio::io::stdout(),
        );
        // 第一次:坏帧 → Decode 错误,code 保留(真实 I/O + 真实 decode 路径)
        let err = transport.recv_op().await.expect_err("坏帧必须报错");
        match &err {
            TransportError::Decode(je) => assert_eq!(je.code, -32700, "code 语义保留"),
            other => panic!("应为 Decode 变体, 实际: {other}"),
        }
        // 第二次:合法帧成功解析(坏帧消费后流位置正确推进)
        let op = transport.recv_op().await.expect("合法帧必须成功");
        assert!(
            matches!(op, AppOp::ThreadStart(_)),
            "第二帧应为合法 ThreadStart"
        );
        // 第三次:流耗尽 → EOF
        assert!(matches!(
            transport.recv_op().await,
            Err(TransportError::Eof)
        ));
    }

    /// 端到端:send_decode_error 经真实 writer 写出**可解析的错误响应行**
    /// (上批遗留「真实回帧行为以单元级保证」的闭环——I/O 路径现已受测)。
    #[tokio::test]
    async fn send_decode_error_writes_parseable_frame() {
        let transport = IoTransport::with_io(
            std::io::Cursor::new(Vec::new()),
            Vec::new(),
        );
        transport
            .send_decode_error(&JsonRpcError::parse_error())
            .await
            .expect("错误回帧必须成功");
        // 从注入的 writer 取回已写字节(tokio Mutex::into_inner 同步取)
        let written = transport.writer.into_inner().into_inner();
        let line = String::from_utf8(written).expect("回帧必须是合法 UTF-8");
        let parsed: RpcResponse = serde_json::from_str(line.trim()).expect("回帧必须可解析");
        assert_eq!(parsed.id, 0, "id=0 约定");
        assert_eq!(
            parsed.error.expect("必须含 error").code,
            -32700,
            "code 语义经真实 I/O 保留"
        );
        // 行尾恰一个换行(NDJSON 纪律)
        assert!(line.ends_with('\n') && !line.ends_with("\n\n"));
    }
}
