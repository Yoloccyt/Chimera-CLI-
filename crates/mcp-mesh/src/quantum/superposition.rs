//! 超位置查询 — 并发 fanout 至多服务器并聚合结果
//!
//! 对应架构层:L10 Interface
//!
//! ## 量子超位语义
//! 同一查询同时发到多个服务器,如同量子叠加态,直到"观测"(收集结果)才坍缩为
//! 单一结果集。任一服务器响应失败不阻断其他服务器,只将其结果标记为失败。
//!
//! ## 实现说明
//! 使用 `tokio::task::JoinSet` 实现并发 fanout(等价于 `FuturesUnordered` +
//! `spawn`),优势:
//! - 无需引入 `futures` crate(workspace 未收录)
//! - JoinSet 提供更好的任务取消语义(整体 abort)
//! - `join_next().await` 按完成顺序返回结果,与 FuturesUnordered 行为一致
//!
//! 整体查询通过 `tokio::time::timeout` 包装 `deadline_ms`,超时返回已收集的部分结果
//! (best-effort,不因部分超时而丢弃已成功响应)。

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::error::McpError;
use crate::protocol::json_rpc::JsonRpcClient;
use crate::server_registry::ServerRegistry;

/// 超位置查询请求 — 描述一次并发 fanout 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperpositionQuery {
    /// 查询 ID(UUIDv7,自动生成)
    pub query_id: String,
    /// 查询语句(语义由具体 MCP 服务器解释)
    pub query: String,
    /// fanout 目标服务器 ID 列表
    pub fanout_servers: Vec<String>,
    /// 查询截止时间(毫秒),超时返回部分结果
    pub deadline_ms: u64,
}

impl SuperpositionQuery {
    /// 创建超位置查询,query_id 自动生成 UUIDv7
    pub fn new(query: impl Into<String>, fanout_servers: Vec<String>, deadline_ms: u64) -> Self {
        Self {
            query_id: Uuid::now_v7().to_string(),
            query: query.into(),
            fanout_servers,
            deadline_ms,
        }
    }

    /// 使用指定 query_id 创建查询(主要用于测试)
    pub fn with_id(
        query_id: impl Into<String>,
        query: impl Into<String>,
        fanout_servers: Vec<String>,
        deadline_ms: u64,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            query: query.into(),
            fanout_servers,
            deadline_ms,
        }
    }
}

/// 单服务器查询结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryResult {
    /// 关联的查询 ID
    pub query_id: String,
    /// 响应服务器 ID
    pub server_id: String,
    /// 查询是否成功
    pub success: bool,
    /// 响应负载(成功时为查询产出,失败时为错误描述)
    pub payload: String,
    /// 单服务器响应延迟(毫秒)
    pub latency_ms: u64,
}

impl QueryResult {
    /// 创建成功结果
    pub fn ok(
        query_id: impl Into<String>,
        server_id: impl Into<String>,
        latency_ms: u64,
        payload: impl Into<String>,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            server_id: server_id.into(),
            success: true,
            payload: payload.into(),
            latency_ms,
        }
    }

    /// 创建失败结果
    pub fn err(
        query_id: impl Into<String>,
        server_id: impl Into<String>,
        latency_ms: u64,
        error: impl Into<String>,
    ) -> Self {
        Self {
            query_id: query_id.into(),
            server_id: server_id.into(),
            success: false,
            payload: error.into(),
            latency_ms,
        }
    }
}

/// 执行超位置查询 — 并发 fanout 至所有目标服务器,聚合结果
///
/// # 流程
/// 1. 校验参与者:所有 fanout_servers 必须在 registry 中注册且 alive
/// 2. 用 `JoinSet` 并发 spawn 每个服务器的查询任务
///    - 若提供 `rpc_client`,通过 JSON-RPC 发送真实 `mcp.query` 请求
///    - 若 `rpc_client` 为 None,使用 Mock 延迟模拟
/// 3. `tokio::time::timeout` 包装整体收集,超时返回已收集的部分结果
/// 4. 任一服务器失败不阻断其他,失败结果标记 `success=false`
///
/// # 错误
/// - `ServerNotFound`:fanout_servers 中存在未注册服务器
/// - `ServerUnreachable`:fanout_servers 中存在心跳超时服务器
///
/// # 返回
/// 成功返回所有服务器的 `QueryResult` 列表(顺序按完成时间,非输入顺序)。
/// 即使部分服务器查询失败,也返回完整列表(由调用方过滤 `success=true`)。
pub async fn execute_superposition_query(
    query: &SuperpositionQuery,
    registry: &ServerRegistry,
    heartbeat_timeout_ms: u64,
    rpc_client: Option<&JsonRpcClient>,
) -> Result<Vec<QueryResult>, McpError> {
    // 1. 校验所有参与者已注册且 alive
    for server_id in &query.fanout_servers {
        let server = registry
            .get(server_id)
            .ok_or_else(|| McpError::ServerNotFound {
                server_id: server_id.clone(),
            })?;
        if !server.is_alive(heartbeat_timeout_ms) {
            return Err(McpError::ServerUnreachable {
                server_id: server_id.clone(),
            });
        }
    }

    // 2. 并发 fanout:每个服务器 spawn 一个查询任务
    let mut set: JoinSet<QueryResult> = JoinSet::new();
    let query_id = query.query_id.clone();
    let query_text = query.query.clone();

    for server_id in &query.fanout_servers {
        let sid = server_id.clone();
        let qid = query_id.clone();
        let qtext = query_text.clone();
        let client = rpc_client.cloned();
        let endpoint = registry.get(&sid).map(|s| s.endpoint.clone());
        set.spawn(async move {
            let start = std::time::Instant::now();

            match (client, endpoint) {
                // 真实 JSON-RPC 模式
                (Some(client), Some(ep)) => {
                    match client.query(&ep, &qid, &qtext, &sid).await {
                        Ok(result) => {
                            let latency_ms = start.elapsed().as_millis() as u64;
                            QueryResult::ok(qid, sid, latency_ms, result.payload)
                        }
                        Err(e) => {
                            let latency_ms = start.elapsed().as_millis() as u64;
                            QueryResult::err(qid, sid, latency_ms, format!("{e}"))
                        }
                    }
                }
                // Mock 模式(无 rpc_client 或 endpoint)
                _ => {
                    let processing_ms = 1 + (sid.bytes().fold(0u64, |acc, b| acc + b as u64) % 3);
                    tokio::time::sleep(Duration::from_millis(processing_ms)).await;
                    let latency_ms = start.elapsed().as_millis() as u64;
                    QueryResult::ok(qid, sid, latency_ms, format!("result@{qtext}"))
                }
            }
        });
    }

    // 3. 带超时收集结果,超时返回已收集的部分结果(best-effort)
    let deadline = Duration::from_millis(query.deadline_ms);
    let mut results = Vec::with_capacity(query.fanout_servers.len());

    let collect_all = async {
        while let Some(join_res) = set.join_next().await {
            // JoinHandle 内任务不会 panic(无 unwrap),失败也作为 QueryResult 返回
            if let Ok(r) = join_res {
                results.push(r);
            }
        }
    };

    if tokio::time::timeout(deadline, collect_all).await.is_err() {
        // 超时:abort 剩余任务,返回已收集的部分结果
        set.abort_all();
        tracing::warn!(
            query_id = %query.query_id,
            collected = results.len(),
            total = query.fanout_servers.len(),
            "超位置查询部分超时,返回已收集结果"
        );
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MeshConfig;
    use crate::server_registry::{MeshServer, ServerRegistry};
    use chrono::Utc;

    fn make_registry_with_servers(n: usize) -> ServerRegistry {
        let registry = ServerRegistry::new(MeshConfig::default().registry_capacity);
        for i in 0..n {
            let sid = format!("s-{i}");
            // 使用 RFC 5737 TEST-NET-3 地址,绕过 SSRF 校验
            let server = MeshServer::new(sid, format!("203.0.113.1:{i}"), vec![]);
            registry.register(server).expect("注册失败");
        }
        registry
    }

    #[tokio::test]
    async fn test_superposition_single_server() {
        let registry = make_registry_with_servers(1);
        let query = SuperpositionQuery::new("test", vec!["s-0".into()], 100);
        let results = execute_superposition_query(&query, &registry, 5000, None)
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].server_id, "s-0");
    }

    #[tokio::test]
    async fn test_superposition_multi_server_fanout() {
        let registry = make_registry_with_servers(5);
        let query = SuperpositionQuery::new(
            "test",
            {
                let mut v: Vec<String> = (0..5).map(|i| format!("s-{i}")).collect();
                v.sort();
                v
            },
            200,
        );
        let results = execute_superposition_query(&query, &registry, 5000, None)
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn test_superposition_unregistered_server_rejected() {
        let registry = make_registry_with_servers(1);
        let query = SuperpositionQuery::new("test", vec!["s-0".into(), "unknown".into()], 100);
        let err = execute_superposition_query(&query, &registry, 5000, None)
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }

    #[tokio::test]
    async fn test_superposition_dead_server_rejected() {
        let registry = ServerRegistry::new(MeshConfig::default().registry_capacity);
        // 注册一个服务器,但其 last_heartbeat 是很久以前(已超时)
        let mut server = MeshServer::new("s-dead", "203.0.113.1:1", vec![]);
        server.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        registry.register(server).expect("注册失败");

        let query = SuperpositionQuery::new("test", vec!["s-dead".into()], 100);
        let err = execute_superposition_query(&query, &registry, 1000, None)
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerUnreachable { .. }));
    }

    #[tokio::test]
    async fn test_superposition_with_mock_rpc_client() {
        let registry = make_registry_with_servers(1);
        let client = JsonRpcClient::mock();
        let query = SuperpositionQuery::new("test", vec!["s-0".into()], 100);
        let results = execute_superposition_query(&query, &registry, 5000, Some(&client))
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[test]
    fn test_query_result_ok_err_constructors() {
        let ok = QueryResult::ok("q-1", "s-1", 10, "data");
        assert!(ok.success);
        assert_eq!(ok.payload, "data");

        let err = QueryResult::err("q-1", "s-2", 5, "timeout");
        assert!(!err.success);
        assert_eq!(err.payload, "timeout");
    }
}
