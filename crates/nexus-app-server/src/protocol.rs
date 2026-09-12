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

use nexus_contracts::app::{AppEvent, AppOp};

/// JSON-RPC 错误 — 帧层错误（语义错误见 `NexusError`）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// 错误码（-32700 解析错误 / -32600 无效请求 / -32601 方法不存在 / -32602 无效参数）
    pub code: i32,
    /// 人类可读消息
    pub message: String,
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
    pub fn encode_request(op: &AppOp, id: u64) -> Result<String, String> {
        let frame = RpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: "app.op".into(),
            params: serde_json::to_value(op).map_err(|e| format!("AppOp 序列化失败: {e}"))?,
        };
        serde_json::to_string(&frame).map_err(|e| format!("请求帧编码失败: {e}"))
    }

    /// 编码成功响应帧
    pub fn encode_result(id: u64, event: &AppEvent) -> Result<String, String> {
        let frame = RpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: Some(
                serde_json::to_value(event).map_err(|e| format!("AppEvent 序列化失败: {e}"))?,
            ),
            error: None,
        };
        serde_json::to_string(&frame).map_err(|e| format!("响应帧编码失败: {e}"))
    }

    /// 编码错误响应帧
    pub fn encode_error(id: u64, error: &JsonRpcError) -> Result<String, String> {
        let frame = RpcResponse {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error.clone()),
        };
        serde_json::to_string(&frame).map_err(|e| format!("错误帧编码失败: {e}"))
    }

    /// 编码服务端推送帧（AppEvent 下行）
    pub fn encode_notification(event: &AppEvent) -> Result<String, String> {
        let frame = RpcNotification {
            jsonrpc: "2.0".into(),
            method: "app.event".into(),
            params: serde_json::to_value(event).map_err(|e| format!("AppEvent 序列化失败: {e}"))?,
        };
        serde_json::to_string(&frame).map_err(|e| format!("推送帧编码失败: {e}"))
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
}
