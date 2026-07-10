//! MCP Mesh 主入口 — 量子事务执行与 EventBus 集成
//!
//! 对应架构层:L10 Interface
//!
//! ## 核心职责
//! - `execute_transaction`:2PC 占位实现,跨多服务器原子提交
//! - `superposition_query`:委托至 `quantum::superposition` 模块
//! - `register_server` / `unregister_server` / `heartbeat`:委托至 `ServerRegistry`
//! - 发布 `McpMeshTransactionCompleted` 事件
//! - 订阅 `ChtcToolCallReceived` 事件(后台 spawn 处理)
//!
//! ## Week 6 教训:broadcast 时序
//! `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用,不能在 async 块内订阅。
//! WHY:`tokio::broadcast` 仅投递给发布时已存在的 receiver;若在 spawn 的 async
//! block 内 subscribe,后台任务调度时机不确定,可能晚于 publish 导致事件静默丢失。

use std::sync::Arc;
use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::MeshConfig;
use crate::error::McpError;
use crate::protocol::json_rpc::JsonRpcClient;
use crate::quantum::superposition::{execute_superposition_query, QueryResult, SuperpositionQuery};
use crate::quantum::transaction::{QuantumTransaction, TransactionState};
use crate::server_registry::{MeshServer, ServerRegistry};
use crate::types::TransactionResult;

/// MCP Mesh — 量子网格的核心入口
///
/// 持有服务器注册表(`Arc<ServerRegistry>`)、可选的 EventBus 与配置。
/// 所有方法通过 `&self` 调用,内部状态基于 `Arc` + `DashMap`,线程安全。
pub struct McpMesh {
    /// Mesh 配置(事务超时、心跳阈值等)
    config: MeshConfig,
    /// 服务器注册表(Arc 共享,后台订阅任务可 clone)
    registry: Arc<ServerRegistry>,
    /// 可选事件总线(事务完成时发布事件)
    event_bus: Option<EventBus>,
    /// JSON-RPC 客户端(真实网络或 Mock 模式)
    rpc_client: JsonRpcClient,
}

impl McpMesh {
    /// 创建 MCP Mesh(无 EventBus,不发布事件)
    pub fn new(config: MeshConfig) -> Self {
        let registry = Arc::new(ServerRegistry::new(config.registry_capacity));
        let rpc_client = if config.json_rpc_mock {
            JsonRpcClient::mock()
        } else {
            JsonRpcClient::new(config.json_rpc_timeout_ms)
        };
        Self {
            config,
            registry,
            event_bus: None,
            rpc_client,
        }
    }

    /// 创建 MCP Mesh 并绑定 EventBus
    ///
    /// 绑定后,`execute_transaction` 成功完成会发布 `McpMeshTransactionCompleted` 事件。
    /// 调用 `start_event_subscriber` 可订阅 `ChtcToolCallReceived` 处理 IDE 工具调用。
    pub fn with_event_bus(config: MeshConfig, bus: EventBus) -> Self {
        let registry = Arc::new(ServerRegistry::new(config.registry_capacity));
        let rpc_client = if config.json_rpc_mock {
            JsonRpcClient::mock()
        } else {
            JsonRpcClient::new(config.json_rpc_timeout_ms)
        };
        Self {
            config,
            registry,
            event_bus: Some(bus),
            rpc_client,
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &MeshConfig {
        &self.config
    }

    /// 获取服务器注册表引用
    pub fn registry(&self) -> &ServerRegistry {
        &self.registry
    }

    /// 注册服务器
    pub fn register_server(&self, server: MeshServer) -> Result<(), McpError> {
        self.registry.register(server)
    }

    /// 注销服务器
    pub fn unregister_server(&self, server_id: &str) -> Result<(), McpError> {
        self.registry.unregister(server_id)
    }

    /// 更新服务器心跳
    pub fn heartbeat(&self, server_id: &str) -> Result<(), McpError> {
        self.registry.heartbeat(server_id)
    }

    /// 执行量子事务 — 2PC 占位实现
    ///
    /// # 流程
    /// 1. 校验 participants:已注册且 alive
    /// 2. 创建 `QuantumTransaction`(Init)
    /// 3. `tokio::time::timeout` 包装整体执行,超时即 Abort+Rollback
    /// 4. Prepare 阶段:并发向所有参与者发 prepare(sleep 模拟网络往返)
    /// 5. 全部 ACK → Commit 阶段 → 返回 success=true
    /// 6. 任一失败 → Abort + Rollback 阶段 → 返回 success=false
    /// 7. 发布 `McpMeshTransactionCompleted` 事件
    ///
    /// # 错误
    /// - `TooManyParticipants`:参与者数量超过 `max_participants`
    /// - `ServerNotFound` / `ServerUnreachable`:参与者未注册或心跳超时
    /// - `TransactionTimeout`:整体超时(已自动回滚)
    pub async fn execute_transaction(
        &self,
        participants: Vec<String>,
        op: String,
    ) -> Result<TransactionResult, McpError> {
        // 1. 校验参与者数量
        if participants.len() > self.config.max_participants {
            return Err(McpError::TooManyParticipants {
                actual: participants.len(),
                limit: self.config.max_participants,
            });
        }

        // 2. 校验所有参与者已注册且 alive
        for sid in &participants {
            let server = self
                .registry
                .get(sid)
                .ok_or_else(|| McpError::ServerNotFound {
                    server_id: sid.clone(),
                })?;
            if !server.is_alive(self.config.heartbeat_timeout_ms) {
                return Err(McpError::ServerUnreachable {
                    server_id: sid.clone(),
                });
            }
        }

        // 3. 创建事务
        let tx_id = Uuid::now_v7().to_string();
        let mut tx = QuantumTransaction::with_id(tx_id.clone(), participants.clone());
        let start = Instant::now();

        // 4. timeout 包装整体执行,超时即触发 Abort+Rollback
        let deadline = Duration::from_millis(self.config.transaction_timeout_ms);
        let outcome = tokio::time::timeout(deadline, self.run_2pc(&mut tx, &op)).await;

        // 用单独标志位记录超时,避免 outcome 部分移动后再借用
        let mut timed_out = false;
        let result = match outcome {
            // 内部 2PC 完成(成功 Commit 或失败 Rollback)
            Ok(Ok(committed)) => TransactionResult::new(
                tx_id.clone(),
                committed.is_some(),
                start.elapsed().as_millis() as u64,
                committed.unwrap_or_default(),
            ),
            // 内部 2PC 返回错误(应已 Rollback,此处兜底)
            Ok(Err(e)) => {
                warn!(transaction_id = %tx_id, error = %e, "2PC 内部错误,事务失败");
                TransactionResult::failed(tx_id.clone(), start.elapsed().as_millis() as u64)
            }
            // 整体超时:确保 Abort+Rollback
            Err(_) => {
                warn!(
                    transaction_id = %tx_id,
                    timeout_ms = self.config.transaction_timeout_ms,
                    "事务超时,触发回滚"
                );
                timed_out = true;
                let _ = self.rollback_phase(&participants, &tx_id).await;
                TransactionResult::failed(tx_id.clone(), start.elapsed().as_millis() as u64)
            }
        };

        // 5. 发布事务完成事件(best-effort)
        self.publish_transaction_completed(&result).await;

        // 6. 整体超时也返回 TransactionTimeout 错误(让调用方知晓)
        if timed_out {
            return Err(McpError::TransactionTimeout {
                transaction_id: tx_id,
                timeout_ms: self.config.transaction_timeout_ms,
            });
        }

        Ok(result)
    }

    /// 2PC 内部执行 — 返回 Some(committed_servers) 表示 Commit,None 表示 Rollback
    ///
    /// 状态机路径:Init → Prepare → (Commit | Abort → Rollback)
    async fn run_2pc(
        &self,
        tx: &mut QuantumTransaction,
        op: &str,
    ) -> Result<Option<Vec<String>>, McpError> {
        // Init → Prepare
        tx.transition(TransactionState::Prepare)?;

        // Prepare 阶段:并发向所有参与者发 prepare
        match self
            .prepare_phase(&tx.participant_servers, op, &tx.transaction_id)
            .await
        {
            Ok(()) => {
                // 全部 ACK → Commit
                tx.transition(TransactionState::Commit)?;
                self.commit_phase(&tx.participant_servers, &tx.transaction_id)
                    .await?;
                Ok(Some(tx.participant_servers.clone()))
            }
            Err(e) => {
                // 任一失败 → Abort → Rollback
                warn!(
                    transaction_id = %tx.transaction_id,
                    error = %e,
                    "Prepare 阶段失败,触发回滚"
                );
                tx.transition(TransactionState::Abort)?;
                self.rollback_phase(&tx.participant_servers, &tx.transaction_id)
                    .await?;
                tx.transition(TransactionState::Rollback)?;
                Ok(None)
            }
        }
    }

    /// Prepare 阶段 — 并发向所有参与者发送 JSON-RPC prepare 请求
    ///
    /// 真实模式:通过 `JsonRpcClient` 向每个参与者的 endpoint 发送 `mcp.prepare`。
    /// Mock 模式:保留 `tokio::time::sleep` 模拟网络往返(1-2ms/服务器),始终成功。
    /// 任一参与者失败则整体失败。
    async fn prepare_phase(
        &self,
        participants: &[String],
        op: &str,
        tx_id: &str,
    ) -> Result<(), McpError> {
        use tokio::task::JoinSet;
        let mut set: JoinSet<Result<(), McpError>> = JoinSet::new();
        for sid in participants {
            let sid = sid.clone();
            let op = op.to_string();
            let tx_id = tx_id.to_string();
            let client = self.rpc_client.clone();
            let endpoint = self.registry.get(&sid).map(|s| s.endpoint.clone());
            set.spawn(async move {
                match endpoint {
                    Some(ep) => {
                        let result = client.prepare(&ep, &tx_id, &op, &sid).await?;
                        if result.ack {
                            Ok(())
                        } else {
                            Err(McpError::ServerUnreachable {
                                server_id: sid.clone(),
                            })
                        }
                    }
                    None => Err(McpError::ServerNotFound {
                        server_id: sid.clone(),
                    }),
                }
            });
        }
        // 收集所有结果,任一失败则返回错误
        while let Some(res) = set.join_next().await {
            if let Ok(Err(e)) = res {
                return Err(e);
            }
        }
        Ok(())
    }

    /// Commit 阶段 — 并发向所有参与者发送 JSON-RPC commit 请求
    async fn commit_phase(&self, participants: &[String], tx_id: &str) -> Result<(), McpError> {
        use tokio::task::JoinSet;
        let mut set: JoinSet<Result<(), McpError>> = JoinSet::new();
        for sid in participants {
            let sid = sid.clone();
            let tx_id = tx_id.to_string();
            let client = self.rpc_client.clone();
            let endpoint = self.registry.get(&sid).map(|s| s.endpoint.clone());
            set.spawn(async move {
                match endpoint {
                    Some(ep) => {
                        let result = client.commit(&ep, &tx_id).await?;
                        if result.ack {
                            Ok(())
                        } else {
                            Err(McpError::ServerUnreachable {
                                server_id: sid.clone(),
                            })
                        }
                    }
                    None => Err(McpError::ServerNotFound {
                        server_id: sid.clone(),
                    }),
                }
            });
        }
        while let Some(res) = set.join_next().await {
            if let Ok(Err(e)) = res {
                return Err(e);
            }
        }
        Ok(())
    }

    /// Rollback 阶段 — 并发向所有参与者发送 JSON-RPC rollback 请求(best-effort,失败仅告警)
    async fn rollback_phase(&self, participants: &[String], tx_id: &str) -> Result<(), McpError> {
        use tokio::task::JoinSet;
        let mut set: JoinSet<()> = JoinSet::new();
        for sid in participants {
            let sid = sid.clone();
            let tx_id = tx_id.to_string();
            let client = self.rpc_client.clone();
            let endpoint = self.registry.get(&sid).map(|s| s.endpoint.clone());
            set.spawn(async move {
                if let Some(ep) = endpoint {
                    if let Err(e) = client.rollback(&ep, &tx_id, Some("2PC abort")).await {
                        warn!(server_id = %sid, error = %e, "Rollback 请求失败(best-effort)");
                    }
                }
            });
        }
        // 等待所有回滚完成(忽略错误)
        while set.join_next().await.is_some() {}
        Ok(())
    }

    /// 执行超位置查询 — 委托至 `quantum::superposition` 模块
    pub async fn superposition_query(
        &self,
        query: SuperpositionQuery,
    ) -> Result<Vec<QueryResult>, McpError> {
        execute_superposition_query(
            &query,
            &self.registry,
            self.config.heartbeat_timeout_ms,
            Some(&self.rpc_client),
        )
        .await
    }

    /// 发布 `McpMeshTransactionCompleted` 事件(best-effort,失败仅告警)
    async fn publish_transaction_completed(&self, result: &TransactionResult) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::McpMeshTransactionCompleted {
                metadata: EventMetadata::new("mcp-mesh"),
                transaction_id: result.transaction_id.clone(),
                participant_count: if result.success {
                    result.committed_servers.len() as u32
                } else {
                    0
                },
                latency_ms: result.latency_ms,
                success: result.success,
            };
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, "McpMeshTransactionCompleted 事件发布失败");
            }
        }
    }

    /// 启动后台订阅任务,处理 `ChtcToolCallReceived` 事件
    ///
    /// 收到事件后记录日志(模拟工具调用分发)。
    ///
    /// # Week 6 教训:broadcast 时序
    /// `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用,否则可能错过事件。
    ///
    /// # 返回
    /// `Some(JoinHandle)` 表示订阅任务已启动;`None` 表示未绑定 EventBus。
    pub fn start_event_subscriber(&self) -> Option<JoinHandle<()>> {
        let bus = self.event_bus.clone()?;

        // 关键:在 spawn 之前同步订阅,确保不遗漏后续事件
        // WHY: tokio::broadcast 仅投递给发布时已存在的 receiver;
        // 若在 spawn 的 async block 内 subscribe,后台任务调度时机不确定,
        // 可能晚于 publish 导致事件静默丢失(broadcast 不缓存历史给新订阅者)
        let mut rx = bus.subscribe();

        Some(tokio::spawn(async move {
            info!("McpMesh 后台订阅任务启动,监听 ChtcToolCallReceived");
            while let Ok(event) = rx.recv().await {
                if let NexusEvent::ChtcToolCallReceived {
                    call_id,
                    tool_id,
                    ide_source,
                    parameters_hash,
                    ..
                } = &event
                {
                    // 模拟工具调用分发:记录日志,实际工具调用由下层路由组件执行
                    info!(
                        call_id = %call_id,
                        tool_id = %tool_id,
                        ide_source = %ide_source,
                        parameters_hash = %parameters_hash,
                        "McpMesh 收到 IDE 工具调用,分发至下层路由"
                    );
                }
            }
            info!("McpMesh 后台订阅任务退出");
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mesh_with_servers(n: usize) -> McpMesh {
        let mesh = McpMesh::new(MeshConfig::default());
        for i in 0..n {
            let sid = format!("s-{i}");
            // 使用 RFC 5737 TEST-NET-3 地址,绕过 SSRF 校验
            mesh.register_server(MeshServer::new(sid, format!("203.0.113.1:{i}"), vec![]))
                .expect("注册失败");
        }
        mesh
    }

    #[tokio::test]
    async fn test_execute_transaction_single_server() {
        let mesh = make_mesh_with_servers(1);
        let result = mesh
            .execute_transaction(vec!["s-0".into()], "test".into())
            .await
            .expect("事务失败");
        assert!(result.success);
        assert_eq!(result.committed_servers.len(), 1);
        assert!(result.latency_ms < 200, "单服务器事务应在 200ms 内完成");
    }

    #[tokio::test]
    async fn test_execute_transaction_five_servers() {
        let mesh = make_mesh_with_servers(5);
        let participants: Vec<String> = (0..5).map(|i| format!("s-{i}")).collect();
        let result = mesh
            .execute_transaction(participants.clone(), "test".into())
            .await
            .expect("事务失败");
        assert!(result.success);
        assert_eq!(result.committed_servers.len(), 5);
    }

    #[tokio::test]
    async fn test_execute_transaction_too_many_participants() {
        let mesh = McpMesh::new(MeshConfig::default());
        // 注册 33 个服务器(max_participants=32)
        for i in 0..33 {
            mesh.register_server(MeshServer::new(format!("s-{i}"), "203.0.113.1", vec![]))
                .expect("注册失败");
        }
        let participants: Vec<String> = (0..33).map(|i| format!("s-{i}")).collect();
        let err = mesh
            .execute_transaction(participants, "test".into())
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::TooManyParticipants { .. }));
    }

    #[tokio::test]
    async fn test_execute_transaction_unregistered_server() {
        let mesh = make_mesh_with_servers(1);
        let err = mesh
            .execute_transaction(vec!["s-0".into(), "unknown".into()], "test".into())
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }

    #[tokio::test]
    async fn test_publishes_mcp_mesh_transaction_completed() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mesh = McpMesh::with_event_bus(MeshConfig::default(), bus);
        mesh.register_server(MeshServer::new("s-1", "203.0.113.1", vec![]))
            .expect("注册失败");

        let result = mesh
            .execute_transaction(vec!["s-1".into()], "test".into())
            .await
            .expect("事务失败");

        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::McpMeshTransactionCompleted {
                transaction_id,
                participant_count,
                latency_ms,
                success,
                ..
            } => {
                assert_eq!(transaction_id, result.transaction_id);
                assert_eq!(participant_count, 1);
                assert_eq!(latency_ms, result.latency_ms);
                assert!(success);
            }
            _ => panic!(
                "期望 McpMeshTransactionCompleted 事件,得到 {:?}",
                event.type_name()
            ),
        }
    }

    #[tokio::test]
    async fn test_no_event_bus_does_not_panic() {
        let mesh = make_mesh_with_servers(1);
        // 无 EventBus,事务应正常完成,不 panic
        let result = mesh
            .execute_transaction(vec!["s-0".into()], "test".into())
            .await
            .expect("事务失败");
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_superposition_query_via_mesh() {
        let mesh = make_mesh_with_servers(3);
        let query =
            SuperpositionQuery::new("test", (0..3).map(|i| format!("s-{i}")).collect(), 100);
        let results = mesh.superposition_query(query).await.expect("查询失败");
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn test_event_subscriber_handles_chtc_tool_call() {
        let bus = EventBus::new();
        let mesh = McpMesh::with_event_bus(MeshConfig::default(), bus.clone());

        // 启动后台订阅任务
        let handle = mesh.start_event_subscriber().expect("应启动订阅");

        // 发布 ChtcToolCallReceived 事件
        bus.publish(NexusEvent::ChtcToolCallReceived {
            metadata: EventMetadata::new("chtc-bridge"),
            call_id: "call-1".into(),
            tool_id: "vscode.command".into(),
            ide_source: "VSCode".into(),
            parameters_hash: "abc123".into(),
        })
        .await
        .expect("发布失败");

        // 等待后台任务处理
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 关闭订阅任务
        handle.abort();
    }

    #[tokio::test]
    async fn test_heartbeat_lifecycle() {
        let mesh = make_mesh_with_servers(1);
        // 注册后立即事务应成功
        let r1 = mesh
            .execute_transaction(vec!["s-0".into()], "op1".into())
            .await
            .expect("事务失败");
        assert!(r1.success);

        // 心跳应成功
        mesh.heartbeat("s-0").expect("心跳失败");

        // 注销后事务应失败
        mesh.unregister_server("s-0").expect("注销失败");
        let err = mesh
            .execute_transaction(vec!["s-0".into()], "op2".into())
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }
}
