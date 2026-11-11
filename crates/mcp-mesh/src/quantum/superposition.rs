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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use uuid::Uuid;

/// 超位置查询的默认 fanout 并发上界（WP-H/ADR-284 转正时加界）
///
/// WHY 需要上界：本模块是工单 T2 登记的结构死臂（零生产调用方），
/// `JoinSet::spawn` 扇出无上界——转正为 `chimera mcp query` 生产出口后，
/// 恶意/误配的超大 `fanout_servers` 列表会瞬间打满连接与内存。上界语义是
/// **排队**（`Semaphore` 许可）而非拒绝：超出并发的 server 等待许可，
/// deadline 语义不变（排队等待可能使其超时标记失败，属合理资源保护）。
pub const DEFAULT_MAX_QUERY_FANOUT: usize = 16;

use crate::error::McpError;
use crate::quantum::participant_client::ParticipantClient;
use crate::server_registry::ServerRegistry;

/// 超位置查询请求 — 描述一次并发 fanout 查询
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperpositionQuery {
    /// 查询 ID（UUIDv7，自动生成）
    pub query_id: String,
    /// 查询语句（语义由具体 MCP 服务器解释）
    pub query: String,
    /// fanout 目标服务器 ID 列表
    pub fanout_servers: Vec<String>,
    /// 查询截止时间（毫秒），超时返回部分结果
    pub deadline_ms: u64,
    /// O-4 跨进程 trace 贯传（深审 2026-09-25）：发起方已有的 trace_id。
    ///
    /// WHY `#[serde(default)]`：wire 向后兼容——旧序列化负载无本字段仍可通过，
    /// 新字段对旧接收方为被忽略的多余键（`EventMetadata::trace_id` 同方言，
    /// 见 nexus-contracts 双向兼容用例）；None 时网格自行种新根，行为与历史一致。
    #[serde(default)]
    pub trace_id: Option<String>,
}

impl SuperpositionQuery {
    /// 创建超位置查询，query_id 自动生成 UUIDv7（无上游 trace）
    pub fn new(query: impl Into<String>, fanout_servers: Vec<String>, deadline_ms: u64) -> Self {
        Self {
            query_id: Uuid::now_v7().to_string(),
            query: query.into(),
            fanout_servers,
            deadline_ms,
            trace_id: None,
        }
    }

    /// 使用指定 query_id 创建查询（主要用于测试）
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
            trace_id: None,
        }
    }

    /// 附加上游 trace_id（链式构造器，与 `with_id` 风格对齐）
    #[must_use]
    pub fn with_trace(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// 构造网格侧事件元数据（O-4 wire 收敛点）：
    /// 携带上游 trace 时延续同一追踪链（外部进程入口视上游为根，直接赋值
    /// 而非 `child_of`——本进程不在上游事件总线上，无 parent metadata 可借）；
    /// 无上游 trace 时保持历史行为（fresh 元数据，下游自行种根）。
    pub(crate) fn trace_meta(&self) -> event_bus::EventMetadata {
        let mut meta = event_bus::EventMetadata::new("mcp-mesh");
        meta.trace_id = self.trace_id.clone();
        meta
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
/// 2. 用 `JoinSet` 并发 spawn 每个服务器的查询任务,**经注入的
///    [`ParticipantClient`] 抽象发查询**(Task 0.7.10 的既定设计:统一传输
///    抽象,而非进程内伪造)——默认 `InProcessClient` 保持原进程内模拟语义
///    (延迟 1-2ms + `result@…` 格式),`TcpParticipantClient` 走真实
///    `QueryRequest`/`QueryResponse` 帧并复用连接池与 DNS rebinding 防御;
///    传输失败如实标记 `success=false`,不再伪造成功
/// 3. `tokio::time::timeout` 包装整体收集,超时返回已收集的部分结果
/// 4. 任一服务器失败不阻断其他,失败结果标记 `success=false`
///
/// # 错误
/// - `ServerNotFound`:fanout_servers 中存在未注册服务器
/// - `ServerUnreachable`:fanout_servers 中存在心跳超时服务器
///
/// # 参数
/// - `max_concurrency`:fanout 在飞并发上界（`Semaphore` 排队语义,非拒绝;
///   0 按 1 处理）。生产调用方传 [`DEFAULT_MAX_QUERY_FANOUT`]
///
/// # 返回
/// 成功返回所有服务器的 `QueryResult` 列表(顺序按完成时间,非输入顺序)。
/// 即使部分服务器查询失败,也返回完整列表(由调用方过滤 `success=true`)。
pub async fn execute_superposition_query(
    query: &SuperpositionQuery,
    registry: &ServerRegistry,
    heartbeat_timeout_ms: u64,
    client: Arc<dyn ParticipantClient>,
    max_concurrency: usize,
) -> Result<Vec<QueryResult>, McpError> {
    // 1. 校验所有参与者已注册且 alive(顺带收集 server 快照,避免任务内二次查表)
    let mut servers = Vec::with_capacity(query.fanout_servers.len());
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
        servers.push(server);
    }

    // 2. 并发 fanout:每个服务器 spawn 一个查询任务,经注入的传输抽象发请求
    //    WP-H/ADR-284:`Semaphore` 上界钳制在飞并发（排队而非拒绝），
    //    消除死臂时代的无上界扇出资源风险。0 视为 1（防御：无意义的零并发）。
    let permits = Arc::new(Semaphore::new(max_concurrency.max(1)));
    let mut set: JoinSet<QueryResult> = JoinSet::new();
    let query_id = query.query_id.clone();
    let query_text = query.query.clone();

    for server in servers {
        let sid = server.server_id.clone();
        let qid = query_id.clone();
        let qtext = query_text.clone();
        let client = Arc::clone(&client);
        let permit_sem = Arc::clone(&permits);
        set.spawn(async move {
            // WHY 在任务内取许可而非 spawn 前逐个 acquire:任务内 acquire 使
            // 排队等待发生在 JoinSet 任务里,deadline 超时 abort 时可一并终止
            let _permit = match permit_sem.acquire_owned().await {
                Ok(p) => p,
                Err(_closed) => {
                    // Semaphore 永不 close（本模块无 close 调用点）,此臂不可达;
                    // 按 fail-safe 处理为该 server 查询失败,不以 panic 破坏聚合
                    return QueryResult::err(
                        qid,
                        sid,
                        0,
                        "fanout semaphore closed (unreachable)",
                    );
                }
            };
            let start = std::time::Instant::now();
            let result = client.query(&server, &qid, &qtext).await;
            let latency_ms = start.elapsed().as_millis() as u64;
            match result {
                Ok(payload) => QueryResult::ok(qid, sid, latency_ms, payload),
                // 传输失败如实标记(网络错误/Nack),不伪造成功载荷
                Err(e) => QueryResult::err(qid, sid, latency_ms, e.to_string()),
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
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use crate::config::MeshConfig;
    use crate::quantum::participant_client::InProcessClient;
    use crate::server_registry::{MeshServer, ServerRegistry};
    use chrono::Utc;
    use std::future::Future;
    use std::pin::Pin;

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

    /// 默认进程内客户端(测试既有语义路径)
    fn in_process_client() -> Arc<dyn ParticipantClient> {
        Arc::new(InProcessClient::new())
    }

    /// 带标记 payload 的桩客户端 — 验证 fanout 走注入抽象而非进程内伪造
    #[derive(Debug, Default)]
    struct TaggedQueryClient;

    impl ParticipantClient for TaggedQueryClient {
        fn prepare<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
            _op: &'a str,
            _args: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn commit<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn query<'a>(
            &'a self,
            server: &'a MeshServer,
            query_id: &'a str,
            query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
            Box::pin(async move { Ok(format!("tagged::{query}::{query_id}@{}", server.server_id)) })
        }
    }

    /// 单服务器注入失败的桩 — 验证传输失败如实标记 success=false
    #[derive(Debug, Default)]
    struct FailQueryClient;

    impl ParticipantClient for FailQueryClient {
        fn prepare<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
            _op: &'a str,
            _args: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn commit<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn query<'a>(
            &'a self,
            server: &'a MeshServer,
            _query_id: &'a str,
            _query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
            let sid = server.server_id.clone();
            Box::pin(async move {
                Err(McpError::NetworkError {
                    server_id: sid,
                    endpoint: String::new(),
                    reason: "injected transport failure".into(),
                })
            })
        }
    }

    #[tokio::test]
    async fn test_superposition_single_server() {
        let registry = make_registry_with_servers(1);
        let query = SuperpositionQuery::new("test", vec!["s-0".into()], 100);
        let results = execute_superposition_query(
                &query,
                &registry,
                5000,
                in_process_client(),
                DEFAULT_MAX_QUERY_FANOUT,
            )
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
        let results = execute_superposition_query(
                &query,
                &registry,
                5000,
                in_process_client(),
                DEFAULT_MAX_QUERY_FANOUT,
            )
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| r.success));
    }

    /// 接线验收(Task 0.7.10 闭环):fanout 必须经注入的 `ParticipantClient`
    /// 抽象发查询,payload 来自传输层而非进程内伪造
    #[tokio::test]
    async fn test_superposition_uses_injected_participant_client() {
        let registry = make_registry_with_servers(2);
        let query = SuperpositionQuery::new("probe", vec!["s-0".into(), "s-1".into()], 200);
        let client: Arc<dyn ParticipantClient> = Arc::new(TaggedQueryClient);
        let results = execute_superposition_query(&query, &registry, 5000, client, DEFAULT_MAX_QUERY_FANOUT)
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
        for r in &results {
            assert!(
                r.payload.starts_with("tagged::"),
                "payload 必须来自注入的传输抽象,实际: {}",
                r.payload
            );
            // 桩格式 tagged::{query}::{query_id}@{server_id} — 三个关联字段都应到达
            assert!(r.payload.contains(&format!("@{}", r.server_id)));
            assert!(r.payload.contains(&query.query_id));
        }
    }

    /// 传输失败必须如实标记 `success=false`,不得伪造成功载荷
    #[tokio::test]
    async fn test_superposition_marks_transport_failure() {
        let registry = make_registry_with_servers(2);
        let query = SuperpositionQuery::new("probe", vec!["s-0".into(), "s-1".into()], 200);
        let client: Arc<dyn ParticipantClient> = Arc::new(FailQueryClient);
        let results = execute_superposition_query(&query, &registry, 5000, client, DEFAULT_MAX_QUERY_FANOUT)
            .await
            .expect("校验通过即不返回 Err(失败以 QueryResult 承载)");
        assert_eq!(results.len(), 2);
        assert!(
            results.iter().all(|r| !r.success),
            "注入传输失败后所有结果应标记失败"
        );
        assert!(results
            .iter()
            .all(|r| r.payload.contains("injected transport failure")));
    }

    #[tokio::test]
    async fn test_superposition_unregistered_server_rejected() {
        let registry = make_registry_with_servers(1);
        let query = SuperpositionQuery::new("test", vec!["s-0".into(), "unknown".into()], 100);
        let err = execute_superposition_query(
                &query,
                &registry,
                5000,
                in_process_client(),
                DEFAULT_MAX_QUERY_FANOUT,
            )
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
        let err = execute_superposition_query(
            &query,
            &registry,
            1000,
            in_process_client(),
            DEFAULT_MAX_QUERY_FANOUT,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, McpError::ServerUnreachable { .. }));
    }

    /// 并发探测桩 — 记录在飞查询峰值（验证 Semaphore 上界生效）
    ///
    /// 进入 query 时在飞 +1、退出 -1，用 `fetch_max` 留存历史峰值。
    /// WHY fetch_max 而非退出后读：退出重置为 0 会丢失峰值观测。
    #[derive(Debug, Default)]
    struct ConcurrencyProbeClient {
        in_flight: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }

    impl ConcurrencyProbeClient {
        fn peak(&self) -> usize {
            self.peak.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl ParticipantClient for ConcurrencyProbeClient {
        fn prepare<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
            _op: &'a str,
            _args: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn commit<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback<'a>(
            &'a self,
            _server: &'a MeshServer,
            _transaction_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn query<'a>(
            &'a self,
            _server: &'a MeshServer,
            _query_id: &'a str,
            _query: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
            Box::pin(async move {
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                // 让并发窗口可观测（否则任务过快完成，峰值恒 1）
                tokio::time::sleep(Duration::from_millis(30)).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok("probe".into())
            })
        }
    }

    /// fanout 并发上界 — 15 servers + 上界 4 ⇒ 峰值 ≤ 4 且全部完成
    ///
    /// WHY 锁定：转正前 `JoinSet::spawn` 扇出无上界（工单 T2 登记的资源风险），
    /// 本测试锁定 Semaphore 排队语义（排队而非拒绝：全部 server 都有结果）。
    #[tokio::test]
    async fn test_superposition_fanout_concurrency_bounded() {
        let registry = make_registry_with_servers(15);
        let servers: Vec<String> = (0..15).map(|i| format!("s-{i}")).collect();
        let query = SuperpositionQuery::new("probe", servers, 5_000);
        let client = Arc::new(ConcurrencyProbeClient::default());
        let results = execute_superposition_query(
            &query,
            &registry,
            5000,
            client.clone(),
            4, // 上界 4（刻意小于 server 数，逼出排队）
        )
        .await
        .expect("查询失败");
        assert_eq!(results.len(), 15, "排队语义：全部 server 都必须完成");
        assert!(results.iter().all(|r| r.success));
        let peak = client.peak();
        assert!(
            peak <= 4,
            "在飞峰值 {peak} 超过上界 4 — Semaphore 未生效"
        );
        assert!(peak > 1, "峰值 {peak} == 1 说明并发完全没发生，测试失真");
    }

    /// max_concurrency = 0 按 1 处理（防御钳制）
    #[tokio::test]
    async fn test_superposition_zero_concurrency_clamped_to_one() {
        let registry = make_registry_with_servers(3);
        let query =
            SuperpositionQuery::new("probe", vec!["s-0".into(), "s-1".into(), "s-2".into()], 5_000);
        let client = Arc::new(ConcurrencyProbeClient::default());
        let results = execute_superposition_query(&query, &registry, 5000, client.clone(), 0)
            .await
            .expect("查询失败");
        assert_eq!(results.len(), 3);
        assert!(
            client.peak() <= 1,
            "0 上界必须钳制为 1，实测峰值 {}",
            client.peak()
        );
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

    /// O-4 wire 向后兼容：旧序列化负载（无 trace_id 键）必须仍可反序列化 → None
    #[test]
    fn legacy_wire_without_trace_id_deserializes_to_none() {
        let legacy = r#"{
            "query_id": "q-legacy",
            "query": "old",
            "fanout_servers": ["s-1"],
            "deadline_ms": 100
        }"#;
        let q: SuperpositionQuery = serde_json::from_str(legacy).expect("旧 wire 必须兼容反序列化");
        assert_eq!(q.query_id, "q-legacy");
        assert_eq!(q.trace_id, None, "缺键必须落 None(历史 fresh 行为)");
    }

    /// O-4 贯传：with_trace 经序列化往返保真；trace_meta 延续上游 trace 而非新种根
    #[test]
    fn with_trace_survives_roundtrip_and_continues_chain_in_meta() {
        let q =
            SuperpositionQuery::new("q", vec!["s-1".into()], 100).with_trace("upstream-root-trace");
        // 序列化→反序列化往返（模拟跨进程 wire）trace_id 保真
        let json = serde_json::to_string(&q).expect("serialize");
        let back: SuperpositionQuery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.trace_id.as_deref(), Some("upstream-root-trace"));
        // 网格侧元数据延续同一 trace（不另种根）
        let meta = back.trace_meta();
        assert_eq!(
            meta.trace_id.as_deref(),
            Some("upstream-root-trace"),
            "携上游 trace 时必须延续而非新种"
        );
        // 无上游 trace 时保持历史 fresh（trace_id 仍 None，由下游 ensure/种根逻辑接管）
        let plain = SuperpositionQuery::new("q", vec!["s-1".into()], 100);
        assert_eq!(plain.trace_meta().trace_id, None);
    }
}
