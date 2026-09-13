//! JSON-RPC v1 编解码 — App 协议传输帧（WI-01 §6.1）
//!
//! # 协议帧形态
//! ```json
//! {"jsonrpc":"2.0","id":1,"method":"app.op","params":{...AppOp...}}
//! {"jsonrpc":"2.0","id":1,"result":{...AppEvent...}}   // 响应
//! {"jsonrpc":"2.0","method":"app.event","params":{...AppEvent...}}  // 服务端推送
//! ```
//!
//! # 纪律
//! - 帧层与语义层分离: 帧只负责 method/params 包裹,语义为 L0 `AppOp/AppEvent`
//! - `app.op` = 客户端 → 服务端操作; `app.event` = 服务端 → 客户端推送
//! - 错误帧符合 JSON-RPC 2.0 error 对象（code/message）

use serde::{Deserialize, Serialize};
use thiserror::Error;

use nexus_contracts::app::{AppEvent, AppOp};

/// 帧编解码错误 — 分类化 + 保留 `serde_json` source 链（架构方向 F-c/F-b）
///
/// WHY 枚举而非 `Result<_, String>`:此前 4 个 `encode_*` 以 `String` 传递失败,
/// 调用方只能打印、无法区分失败发生在**载荷阶段**(`to_value`)还是**帧阶段**
/// (`to_string`),也无法下钻原始 `serde_json::Error`(排障要靠重新复现)。
/// 本类型以 `#[source]` 保留底层错误,并按阶段分类,便于:
/// - 载荷失败(`Payload`)= 语义对象无法表示为 JSON(多为数据/契约缺陷)→ 告警;
/// - 帧失败(`Frame`)= 帧结构序列化失败(多为实现缺陷)→ 告警 + 不重试。
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// 载荷转 `serde_json::Value` 失败（`AppOp` / `AppEvent` 无法表示为 JSON）
    #[error("{what} payload serialization failed: {source}")]
    Payload {
        /// 载荷语义名（如 "AppOp" / "AppEvent"），用于日志定位
        what: &'static str,
        /// 底层序列化错误（保留 source 链）
        #[source]
        source: serde_json::Error,
    },

    /// 帧序列化为 NDJSON 行失败
    #[error("{what} frame encode failed: {source}")]
    Frame {
        /// 帧语义名（如 "request" / "response" / "error" / "notification"）
        what: &'static str,
        /// 底层序列化错误（保留 source 链）
        #[source]
        source: serde_json::Error,
    },
}

impl ProtocolError {
    /// 是否发生在载荷阶段（`to_value`）
    #[must_use]
    pub fn is_payload(&self) -> bool {
        matches!(self, Self::Payload { .. })
    }

    /// 是否发生在帧阶段（`to_string`）
    #[must_use]
    pub fn is_frame(&self) -> bool {
        matches!(self, Self::Frame { .. })
    }

    /// 载荷阶段构造器（便于调用方显式表达阶段）
    pub fn payload(what: &'static str, source: serde_json::Error) -> Self {
        Self::Payload { what, source }
    }

    /// 帧阶段构造器（便于调用方显式表达阶段）
    pub fn frame(what: &'static str, source: serde_json::Error) -> Self {
        Self::Frame { what, source }
    }
}

/// JSON-RPC 错误 — 帧层错误（语义错误见 `NexusError`）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// 错误码（-32700 解析错误 / -32600 无效请求 / -32601 方法不存在 / -32602 无效参数）
    pub code: i32,
    /// 人类可读消息
    pub message: String,
}

impl std::fmt::Display for JsonRpcError {
    /// 面向日志/错误链的可读形态:错误码 + 消息。
    /// WHY 此处补 impl:`TransportError::Decode(JsonRpcError)` 的 thiserror
    /// `#[error("{0}")]` 需要 `Display`(架构方向 F-c,decode 侧结构化)。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "code={}, message={}", self.code, self.message)
    }
}

impl JsonRpcError {
    /// 创建 JSON-RPC 错误
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// 解析错误（-32700）
    pub fn parse_error() -> Self {
        Self::new(-32700, "parse error")
    }

    /// 无效请求（-32600）
    pub fn invalid_request() -> Self {
        Self::new(-32600, "invalid request")
    }

    /// 方法不存在（-32601）
    pub fn method_not_found() -> Self {
        Self::new(-32601, "method not found")
    }
}

/// JSON-RPC 请求帧
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    /// 协议版本（恒为 "2.0"）
    pub jsonrpc: String,
    /// 请求 ID（客户端自增，回显于响应）
    pub id: u64,
    /// 方法名（"app.op"）
    pub method: String,
    /// 操作载荷（AppOp 序列化形态）
    pub params: serde_json::Value,
}

/// JSON-RPC 响应帧（成功或错误）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    /// 协议版本（恒为 "2.0"）
    pub jsonrpc: String,
    /// 请求 ID（回显）
    pub id: u64,
    /// 成功结果（AppEvent 序列化形态）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// 错误（与 result 互斥）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// 服务端推送帧（AppEvent 下行通道）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcNotification {
    /// 协议版本（恒为 "2.0"）
    pub jsonrpc: String,
    /// 方法名（"app.event"）
    pub method: String,
    /// 事件载荷（AppEvent 序列化形态）
    pub params: serde_json::Value,
}

/// 帧编解码器 — 行分隔 NDJSON（每行一帧）
#[derive(Debug, Default, Clone)]
pub struct RpcCodec;

impl RpcCodec {
    /// 编码请求帧
    ///
    /// # 返回
    /// `Ok(NDJSON 行)`;`Err` 为 [`ProtocolError`]:载荷阶段失败为
    /// [`ProtocolError::Payload`],帧阶段失败为 [`ProtocolError::Frame`]。
    pub fn encode_request(op: &AppOp, id: u64) -> Result<String, ProtocolError> {
        let frame = RpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: "app.op".into(),
            params: serde_json::to_value(op).map_err(|e| ProtocolError::payload("AppOp", e))?,
        };
        serde_json::to_string(&frame).map_err(|e| ProtocolError::frame("request", e))
    }

    /// 编码成功响应帧
    ///
    /// # 返回
    /// 同 [`encode_request`](Self::encode_request) 的错误分类语义。
    pub fn encode_result(id: u64, event: &AppEvent) -> Result<String, ProtocolError> {
        let frame = RpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(
                serde_json::to_value(event)
                    .map_err(|e| ProtocolError::payload("AppEvent", e))?,
            ),
            error: None,
        };
        serde_json::to_string(&frame).map_err(|e| ProtocolError::frame("response", e))
    }

    /// 编码错误响应帧
    ///
    /// # 返回
    /// 同 [`encode_request`](Self::encode_request) 的错误分类语义。
    pub fn encode_error(id: u64, error: &JsonRpcError) -> Result<String, ProtocolError> {
        let frame = RpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error.clone()),
        };
        serde_json::to_string(&frame).map_err(|e| ProtocolError::frame("error", e))
    }

    /// 编码服务端推送帧（AppEvent 下行）
    ///
    /// # 返回
    /// 同 [`encode_request`](Self::encode_request) 的错误分类语义。
    pub fn encode_notification(event: &AppEvent) -> Result<String, ProtocolError> {
        let frame = RpcNotification {
            jsonrpc: "2.0".into(),
            method: "app.event".into(),
            params: serde_json::to_value(event)
                .map_err(|e| ProtocolError::payload("AppEvent", e))?,
        };
        serde_json::to_string(&frame).map_err(|e| ProtocolError::frame("notification", e))
    }

    /// 解码一行帧 → 请求帧（客户端 → 服务端）
    pub fn decode_request_line(line: &str) -> Result<RpcRequest, JsonRpcError> {
        let frame: RpcRequest =
            serde_json::from_str(line).map_err(|_| JsonRpcError::parse_error())?;
        if frame.method != "app.op" {
            return Err(JsonRpcError::method_not_found());
        }
        Ok(frame)
    }

    /// 解码一行帧 → 响应帧（服务端 → 客户端）
    pub fn decode_response_line(line: &str) -> Result<RpcResponse, JsonRpcError> {
        serde_json::from_str(line).map_err(|_| JsonRpcError::parse_error())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::app::{AppOp, ThreadId, UserInput};

    #[test]
    fn request_frame_roundtrip() {
        let op = AppOp::TurnSubmit {
            thread_id: ThreadId::new("t-1"),
            input: UserInput::new("hello"),
        };
        let line = RpcCodec::encode_request(&op, 1).expect("编码成功");
        let frame = RpcCodec::decode_request_line(&line).expect("解码成功");
        assert_eq!(frame.id, 1);
        assert_eq!(frame.method, "app.op");
        let decoded: AppOp = serde_json::from_value(frame.params).expect("载荷反序列化成功");
        assert_eq!(decoded, op);
    }

    #[test]
    fn error_frame_roundtrip() {
        let line = RpcCodec::encode_error(7, &JsonRpcError::method_not_found()).expect("编码成功");
        let frame = RpcCodec::decode_response_line(&line).expect("解码成功");
        assert_eq!(frame.id, 7);
        assert!(frame.result.is_none());
        assert_eq!(frame.error.expect("错误必须存在").code, -32601);
    }

    #[test]
    fn parse_error_on_garbage() {
        let err = RpcCodec::decode_request_line("not json").expect_err("垃圾输入必须报解析错误");
        assert_eq!(err.code, -32700);
    }

    #[test]
    fn wrong_method_rejected() {
        let err = RpcCodec::decode_request_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"other","params":{}}"#,
        )
        .expect_err("未知方法必须拒绝");
        assert_eq!(err.code, -32601);
    }

    // ========================================================
    // F-c:ProtocolError 分类与 source 链
    // ========================================================

    /// 构造一个真实的 `serde_json::Error`（用于 source 链断言）
    fn sample_serde_error() -> serde_json::Error {
        serde_json::from_str::<u32>("not a number").expect_err("必然解析失败")
    }

    #[test]
    fn protocol_error_display_carries_stage_and_source() {
        let e = ProtocolError::payload("AppOp", sample_serde_error());
        let msg = e.to_string();
        assert!(msg.contains("AppOp"), "阶段载荷名应在文案中: {msg}");
        assert!(msg.contains("payload serialization failed"), "阶段语义应可读: {msg}");

        let f = ProtocolError::frame("notification", sample_serde_error());
        let msg = f.to_string();
        assert!(msg.contains("notification"), "帧名应在文案中: {msg}");
        assert!(msg.contains("frame encode failed"), "阶段语义应可读: {msg}");
    }

    #[test]
    fn protocol_error_source_chain_is_traversable() {
        use std::error::Error as _;
        let e = ProtocolError::payload("AppEvent", sample_serde_error());
        let src = e.source().expect("必须保留底层 serde_json 错误");
        assert!(
            src.downcast_ref::<serde_json::Error>().is_some(),
            "source 应可下钻为 serde_json::Error: {src}"
        );
    }

    #[test]
    fn protocol_error_classification_is_exclusive() {
        let p = ProtocolError::payload("AppOp", sample_serde_error());
        assert!(p.is_payload() && !p.is_frame());
        let f = ProtocolError::frame("request", sample_serde_error());
        assert!(f.is_frame() && !f.is_payload());
    }

    /// 成功路径补覆盖:`encode_result` / `encode_notification`
    /// （此前仅 `encode_request` / `encode_error` 有测试）
    #[test]
    fn result_and_notification_frames_encode() {
        // `Item` 载荷为 JSON 形态字符串(协议面不解析内部结构)
        let ev = nexus_contracts::app::AppEvent::ItemChanged {
            item: nexus_contracts::app::Item::new(
                nexus_contracts::app::ItemId::new("i-1"),
                ThreadId::new("t-1"),
                nexus_contracts::app::TurnId::new("turn-1"),
                "quest_state",
                nexus_contracts::app::ItemStatus::Completed,
                r#"{"quest_id":"q-1"}"#,
            ),
        };
        let line = RpcCodec::encode_result(3, &ev).expect("响应帧编码成功");
        let frame = RpcCodec::decode_response_line(&line).expect("响应帧解码成功");
        assert_eq!(frame.id, 3);
        assert!(frame.result.is_some());

        let line = RpcCodec::encode_notification(&ev).expect("推送帧编码成功");
        assert!(line.contains("\"method\":\"app.event\""), "推送帧方法名固定: {line}");
        assert!(line.contains("quest_state"), "载荷应携带事件内容: {line}");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(32))]

        /// 属性:任意阶段名都完整出现在文案中,且分类与构造阶段一致
        /// (防后续新增阶段时漏接分类谓词或文案模板)
        #[test]
        fn prop_stage_name_preserved(what in "[a-z_]{1,24}") {
            // 用 String::leak 得到 'static str(Payload/Frame 字段要求 &'static str)
            let leaked: &'static str = Box::leak(what.clone().into_boxed_str());
            let p = ProtocolError::payload(leaked, sample_serde_error());
            proptest::prop_assert!(p.to_string().contains(&what));
            proptest::prop_assert!(p.is_payload() && !p.is_frame());

            let f = ProtocolError::frame(leaked, sample_serde_error());
            proptest::prop_assert!(f.to_string().contains(&what));
            proptest::prop_assert!(f.is_frame() && !f.is_payload());
        }
    }
}
