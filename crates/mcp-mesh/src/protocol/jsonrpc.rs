//! JSON-RPC 2.0 消息模型 + **形状校验**解码。
//!
//! 为什么不用 `#[serde(untagged)]` 把四种消息塞进一个枚举：untagged 会让"解析失败"
//! 变成"试下一个变体"，最终错误信息只剩一句 `data did not match any variant` ——
//! 排障时看不出到底缺了 `jsonrpc` 还是 `id` 类型写错。这里改成**先按成员键分类、
//! 再逐类反序列化**，每条失败都带着能直接读的原因。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::McpError;

/// JSON-RPC 请求/响应 id。规范允许数字或字符串；本项目客户端只用数字自增，
/// 但**解析**必须接受两种（服务器/中间层可能回字符串 id）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// 数字 id
    Number(u64),
    /// 字符串 id
    String(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(value) => write!(f, "{value}"),
            RequestId::String(value) => write!(f, "{value}"),
        }
    }
}

/// 客户端 → 服务器的请求（带 id，期待一个响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 必须等于 `"2.0"`（由 [`decode_incoming`] 校验；serde 只负责取值）
    pub jsonrpc: String,
    /// 请求 id
    pub id: RequestId,
    /// 方法名（如 `tools/list`）
    pub method: String,
    /// 方法参数；缺省即"无参数"
    #[serde(default, skip_serializing_if = "params_absent")]
    pub params: Option<Value>,
    /// 未识别的成员原样保留 —— 服务器/中间层加的扩展字段不能在往返里被吃掉
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 服务器 → 客户端的响应（result 与 error 恰有一个）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 必须等于 `"2.0"`
    pub jsonrpc: String,
    /// 与请求对应的 id
    pub id: RequestId,
    /// 成功结果。注意：JSON 里的 `"result": null` 会解码成 `None`，
    /// 所以"恰好一个成员"的判定在 [`decode_incoming`] 里按**原始键存在性**做。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// 失败错误
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    /// 未识别成员原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// JSON-RPC 错误对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// 错误码
    pub code: i64,
    /// 错误消息
    pub message: String,
    /// 可选附加数据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// 未识别成员原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 不带 id 的通知（单向，不期待响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// 必须等于 `"2.0"`
    pub jsonrpc: String,
    /// 方法名（如 `notifications/tools/list_changed`）
    pub method: String,
    /// 方法参数
    #[serde(default, skip_serializing_if = "params_absent")]
    pub params: Option<Value>,
    /// 未识别成员原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 入站帧的分类结果（**只看我们收到的方向**）。
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 服务器发来的请求（服务端主动调用客户端能力；本波不实现任何 client 能力，
    /// 收到后由调用方回 `MethodNotFound`，见 client 模块）
    Request(Request),
    /// 对某个在途请求的响应
    Response(Response),
    /// 通知
    Notification(Notification),
}

fn protocol(reason: impl Into<String>) -> McpError {
    McpError::Protocol {
        reason: reason.into(),
    }
}

/// 编码时把"无参数"与"显式 null"归一为**省略**。
///
/// WHY：JSON-RPC 里 `"params": null` 与不写 `params` 语义相同，而反序列化
/// `Option<Value>` 会把两者都变成 `None`。若不归一并发，`Some(Null)` 会编码成
/// `"params": null`、解码回 `None` —— 往返不幂等（属性测试 prop_params_roundtrip
/// 抓到的最小反例就是 params=Null）。归一后规则只有一条：null ≡ 缺省。
fn params_absent(value: &Option<Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// 把一条 JSON 帧解码成 [`Incoming`]，并校验 JSON-RPC 2.0 的形状。
///
/// 规则（逐条都有测试）：
/// - 必须是 JSON 对象；`jsonrpc` 必须是 `"2.0"`；
/// - 有 `method` 时：有 `id` → 请求，无 `id` → 通知；
/// - 无 `method` 时必须有 `id`，且 `result` / `error` **恰有其一** → 响应；
/// - 其余一律 `McpError::Protocol`（不 panic、不猜）。
pub fn decode_incoming(frame: &[u8]) -> Result<Incoming, McpError> {
    let value: Value = serde_json::from_slice(frame)
        .map_err(|err| protocol(format!("frame is not valid JSON: {err}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| protocol("frame is not a JSON object"))?;
    match obj.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => {}
        Some(other) => return Err(protocol(format!("jsonrpc must be \"2.0\", got {other:?}"))),
        None => return Err(protocol("frame has no \"jsonrpc\" member")),
    }

    let has_method = obj.contains_key("method");
    let has_id = obj.contains_key("id");

    match (has_method, has_id) {
        (true, true) => serde_json::from_value::<Request>(value)
            .map(Incoming::Request)
            .map_err(|err| protocol(format!("malformed request: {err}"))),
        (true, false) => serde_json::from_value::<Notification>(value)
            .map(Incoming::Notification)
            .map_err(|err| protocol(format!("malformed notification: {err}"))),
        (false, true) => {
            // 按**原始键存在性**判"恰有其一"：`"result": null` 解码后是 None，
            // 用 Option 判会把成功的 null 结果误判成畸形。
            let has_result = obj.contains_key("result");
            let has_error = obj.contains_key("error");
            if has_result == has_error {
                return Err(protocol(
                    "response must carry exactly one of \"result\" or \"error\"",
                ));
            }
            serde_json::from_value::<Response>(value)
                .map(Incoming::Response)
                .map_err(|err| protocol(format!("malformed response: {err}")))
        }
        (false, false) => Err(protocol(
            "frame is neither request (method+id), notification (method) nor response (id+result|error)",
        )),
    }
}

/// 把一条消息编码成 JSON 字节（**不含**传输层分隔符）。
pub fn encode_frame<T: Serialize>(message: &T) -> Result<Vec<u8>, McpError> {
    serde_json::to_vec(message).map_err(|err| protocol(format!("encode failed: {err}")))
}
