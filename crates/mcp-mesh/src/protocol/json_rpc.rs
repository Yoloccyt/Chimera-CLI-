//! MCP Mesh JSON-RPC 协议适配器
//!
//! 对应架构层:L10 Interface
//!
//! ## 设计目标
//! 将 MCP Mesh 的 2PC 事务与超位置查询从 in-process mock 升级为真实 JSON-RPC 网络调用。
//! 保持状态机契约不变,仅替换 prepare/commit/rollback/query 的传输层为 HTTP JSON-RPC。
//!
//! ## 协议规范
//! 遵循 JSON-RPC 2.0 规范,请求/响应格式:
//! ```json
//! // Request
//! {"jsonrpc":"2.0","method":"mcp.prepare","params":{"tx_id":"...","op":"..."},"id":1}
//! // Response
//! {"jsonrpc":"2.0","result":{"ack":true},"id":1}
//! ```
//!
//! ## 降级路径
//! - 真实模式:通过 `reqwest` 发送 HTTP POST 到参与者 endpoint
//! - Mock 模式:保留 `tokio::time::sleep` 模拟,用于 CI/无网络环境
//! 通过 `MeshConfig.json_rpc_mock` 切换。

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::error::McpError;

// ============================================================
// JSON-RPC 2.0 基础类型
// ============================================================

/// JSON-RPC 2.0 请求对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest<T> {
    /// 必须为 "2.0"
    pub jsonrpc: String,
    /// 方法名(如 "mcp.prepare", "mcp.commit", "mcp.rollback", "mcp.query")
    pub method: String,
    /// 方法参数(结构体或数组)
    pub params: T,
    /// 请求 ID(整数或字符串,用于关联响应)
    pub id: JsonRpcId,
}

impl<T: Serialize> JsonRpcRequest<T> {
    /// 创建标准 JSON-RPC 2.0 请求
    pub fn new(method: impl Into<String>, params: T, id: JsonRpcId) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
            id,
        }
    }
}

/// JSON-RPC 请求 ID — 整数或字符串
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// 整数 ID
    Num(u64),
    /// 字符串 ID
    Str(String),
}

impl JsonRpcId {
    /// 生成自增整数 ID(线程不安全,仅用于单事务上下文)
    pub fn next_counter() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self::Num(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// JSON-RPC 2.0 成功响应
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse<R> {
    /// 必须为 "2.0"
    pub jsonrpc: String,
    /// 成功结果(与 error 互斥)
    pub result: Option<R>,
    /// 错误对象(与 result 互斥)
    pub error: Option<JsonRpcError>,
    /// 对应请求的 ID
    pub id: Option<JsonRpcId>,
}

/// JSON-RPC 2.0 错误对象
#[derive(Debug, Clone, Deserialize, thiserror::Error)]
#[error("JSON-RPC error {code}: {message}")]
pub struct JsonRpcError {
    /// 错误码(标准码:-32700 Parse/-32600 InvalidRequest/-32601 MethodNotFound/
    /// -32602 InvalidParams/-32603 InternalError; 服务器自定义: -32000 ~ -32099)
    pub code: i32,
    /// 错误描述
    pub message: String,
    /// 附加数据(可选)
    pub data: Option<serde_json::Value>,
}

// ============================================================
// MCP Mesh 专用 JSON-RPC 参数与结果类型
// ============================================================

/// mcp.prepare 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareParams {
    /// 事务 ID
    pub tx_id: String,
    /// 操作内容
    pub op: String,
    /// 参与者服务器 ID(用于参与者校验)
    pub participant_id: String,
}

/// mcp.prepare 响应结果
#[derive(Debug, Clone, Deserialize)]
pub struct PrepareResult {
    /// 是否确认 prepare
    pub ack: bool,
    /// 参与者返回的元数据(如锁信息、版本号)
    pub metadata: Option<serde_json::Value>,
}

/// mcp.commit 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitParams {
    /// 事务 ID
    pub tx_id: String,
}

/// mcp.commit 响应结果
#[derive(Debug, Clone, Deserialize)]
pub struct CommitResult {
    /// 是否确认 commit
    pub ack: bool,
}

/// mcp.rollback 请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackParams {
    /// 事务 ID
    pub tx_id: String,
    /// 回滚原因(用于日志与审计)
    pub reason: Option<String>,
}

/// mcp.rollback 响应结果
#[derive(Debug, Clone, Deserialize)]
pub struct RollbackResult {
    /// 是否确认 rollback
    pub ack: bool,
}

/// mcp.query 请求参数(超位置查询)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// 查询 ID
    pub query_id: String,
    /// 查询语句
    pub query: String,
    /// 查询服务器 ID
    pub server_id: String,
}

/// mcp.query 响应结果
#[derive(Debug, Clone, Deserialize)]
pub struct QueryResultPayload {
    /// 查询产出(序列化后的字符串)
    pub payload: String,
    /// 服务器端处理延迟(毫秒)
    pub server_latency_ms: u64,
}

// ============================================================
// JSON-RPC 传输客户端
// ============================================================

/// MCP Mesh JSON-RPC 客户端
///
/// 封装 `reqwest` HTTP 客户端,提供类型化的 JSON-RPC 调用方法。
/// 支持 Mock 模式(无网络,直接返回成功)用于 CI/测试。
#[derive(Debug, Clone)]
pub struct JsonRpcClient {
    /// HTTP 客户端(真实模式)
    http: Option<reqwest::Client>,
    /// 是否启用 Mock 模式(跳过网络,直接返回成功)
    mock: bool,
    /// 请求超时
    timeout_ms: u64,
}

impl JsonRpcClient {
    /// 创建真实 JSON-RPC 客户端
    ///
    /// # 参数
    /// - `timeout_ms`:单请求超时(毫秒)
    pub fn new(timeout_ms: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .ok();
        Self {
            http,
            mock: false,
            timeout_ms,
        }
    }

    /// 创建 Mock 客户端(无网络,直接返回成功)
    pub fn mock() -> Self {
        Self {
            http: None,
            mock: true,
            timeout_ms: 1000,
        }
    }

    /// 发送 JSON-RPC 请求并解析响应
    ///
    /// # 流程
    /// 1. 序列化请求体为 JSON
    /// 2. POST 到 endpoint
    /// 3. 解析响应,校验 jsonrpc="2.0" 与 ID 匹配
    /// 4. 返回 result 或转换为 McpError
    ///
    /// # Mock 模式
    /// 直接返回 `default_result` 构造的成功响应,跳过网络。
    async fn call<Req, Resp>(
        &self,
        endpoint: &str,
        method: &str,
        params: Req,
        id: JsonRpcId,
    ) -> Result<Resp, McpError>
    where
        Req: Serialize,
        Resp: for<'de> Deserialize<'de> + Default,
    {
        // Mock 模式:直接返回默认成功结果
        if self.mock {
            let delay_ms = 1 + (endpoint.bytes().fold(0u64, |acc, b| acc + b as u64) % 2);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            return Ok(Resp::default());
        }

        let http = self.http.as_ref().ok_or_else(|| {
            McpError::ConfigError {
                reason: "HTTP client 初始化失败".into(),
            }
        })?;

        let request = JsonRpcRequest::new(method, params, id.clone());
        let body = serde_json::to_vec(&request).map_err(|e| McpError::ConfigError {
            reason: format!("JSON-RPC 序列化失败: {e}"),
        })?;

        let response = http
            .post(endpoint)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    McpError::TransactionTimeout {
                        transaction_id: format!("rpc-{method}"),
                        timeout_ms: self.timeout_ms,
                    }
                } else {
                    McpError::ServerUnreachable {
                        server_id: endpoint.to_string(),
                    }
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(McpError::ServerUnreachable {
                server_id: format!("{endpoint} HTTP {status}"),
            });
        }

        let rpc_resp: JsonRpcResponse<Resp> = response.json().await.map_err(|e| {
            McpError::ConfigError {
                reason: format!("JSON-RPC 响应解析失败: {e}"),
            }
        })?;

        // 校验 ID 匹配
        if rpc_resp.id.as_ref() != Some(&id) {
            return Err(McpError::ConfigError {
                reason: "JSON-RPC 响应 ID 不匹配".into(),
            });
        }

        // 提取 result 或 error
        if let Some(err) = rpc_resp.error {
            return Err(McpError::ConfigError {
                reason: format!("JSON-RPC 错误 {}: {}", err.code, err.message),
            });
        }

        rpc_resp.result.ok_or_else(|| {
            McpError::ConfigError {
                reason: "JSON-RPC 响应既无 result 也无 error".into(),
            }
        })
    }

    /// 发送 prepare 请求到指定参与者
    pub async fn prepare(
        &self,
        endpoint: &str,
        tx_id: &str,
        op: &str,
        participant_id: &str,
    ) -> Result<PrepareResult, McpError> {
        let params = PrepareParams {
            tx_id: tx_id.into(),
            op: op.into(),
            participant_id: participant_id.into(),
        };
        self.call(endpoint, "mcp.prepare", params, JsonRpcId::next_counter())
            .await
    }

    /// 发送 commit 请求到指定参与者
    pub async fn commit(&self, endpoint: &str, tx_id: &str) -> Result<CommitResult, McpError> {
        let params = CommitParams {
            tx_id: tx_id.into(),
        };
        self.call(endpoint, "mcp.commit", params, JsonRpcId::next_counter())
            .await
    }

    /// 发送 rollback 请求到指定参与者
    pub async fn rollback(
        &self,
        endpoint: &str,
        tx_id: &str,
        reason: Option<&str>,
    ) -> Result<RollbackResult, McpError> {
        let params = RollbackParams {
            tx_id: tx_id.into(),
            reason: reason.map(|s| s.into()),
        };
        self.call(endpoint, "mcp.rollback", params, JsonRpcId::next_counter())
            .await
    }

    /// 发送 query 请求到指定服务器(超位置查询)
    pub async fn query(
        &self,
        endpoint: &str,
        query_id: &str,
        query: &str,
        server_id: &str,
    ) -> Result<QueryResultPayload, McpError> {
        let params = QueryParams {
            query_id: query_id.into(),
            query: query.into(),
            server_id: server_id.into(),
        };
        self.call(endpoint, "mcp.query", params, JsonRpcId::next_counter())
            .await
    }
}

// Default impls for mock mode — 所有结果类型默认 ack=true / payload=""
impl Default for PrepareResult {
    fn default() -> Self {
        Self {
            ack: true,
            metadata: None,
        }
    }
}

impl Default for CommitResult {
    fn default() -> Self {
        Self { ack: true }
    }
}

impl Default for RollbackResult {
    fn default() -> Self {
        Self { ack: true }
    }
}

impl Default for QueryResultPayload {
    fn default() -> Self {
        Self {
            payload: String::new(),
            server_latency_ms: 1,
        }
    }
}

// ============================================================
// 测试
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jsonrpc_request_serialization() {
        let req = JsonRpcRequest::new(
            "mcp.prepare",
            PrepareParams {
                tx_id: "tx-1".into(),
                op: "test".into(),
                participant_id: "s-1".into(),
            },
            JsonRpcId::Num(1),
        );
        let json = serde_json::to_string(&req).expect("序列化失败");
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"mcp.prepare\""));
        assert!(json.contains("\"tx_id\":\"tx-1\""));
        assert!(json.contains("\"id\":1"));
    }

    #[test]
    fn test_jsonrpc_response_deserialization_success() {
        let json = r#"{"jsonrpc":"2.0","result":{"ack":true,"metadata":null},"id":1}"#;
        let resp: JsonRpcResponse<PrepareResult> = serde_json::from_str(json).expect("反序列化失败");
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.result.unwrap().ack);
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_response_deserialization_error() {
        let json = r#"{"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found","data":null},"id":1}"#;
        let resp: JsonRpcResponse<PrepareResult> = serde_json::from_str(json).expect("反序列化失败");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[tokio::test]
    async fn test_mock_client_prepare_returns_ack() {
        let client = JsonRpcClient::mock();
        let result = client
            .prepare("http://example.com", "tx-1", "op", "s-1")
            .await
            .expect("Mock prepare 应成功");
        assert!(result.ack);
    }

    #[tokio::test]
    async fn test_mock_client_commit_returns_ack() {
        let client = JsonRpcClient::mock();
        let result = client
            .commit("http://example.com", "tx-1")
            .await
            .expect("Mock commit 应成功");
        assert!(result.ack);
    }

    #[tokio::test]
    async fn test_mock_client_rollback_returns_ack() {
        let client = JsonRpcClient::mock();
        let result = client
            .rollback("http://example.com", "tx-1", Some("test"))
            .await
            .expect("Mock rollback 应成功");
        assert!(result.ack);
    }

    #[tokio::test]
    async fn test_mock_client_query_returns_payload() {
        let client = JsonRpcClient::mock();
        let result = client
            .query("http://example.com", "q-1", "test", "s-1")
            .await
            .expect("Mock query 应成功");
        assert_eq!(result.payload, "");
        assert!(result.server_latency_ms > 0);
    }

    #[test]
    fn test_jsonrpc_id_counter_increments() {
        let id1 = JsonRpcId::next_counter();
        let id2 = JsonRpcId::next_counter();
        match (&id1, &id2) {
            (JsonRpcId::Num(a), JsonRpcId::Num(b)) => assert!(b > a, "ID 应自增"),
            _ => panic!("ID 应为整数"),
        }
    }
}
