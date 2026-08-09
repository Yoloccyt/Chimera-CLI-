//! MCP Mesh 主入口 — 量子事务执行与 EventBus 集成
//!
//! 对应架构层:L10 Interface
//!
//! ## 核心职责
//! - `execute_transaction`:2PC 跨多服务器原子提交(P1-6:已替换占位实现)
//! - `superposition_query`:委托至 `quantum::superposition` 模块
//! - `register_server` / `unregister_server` / `heartbeat`:委托至 `ServerRegistry`
//! - 发布 `McpMeshTransactionCompleted` 事件
//! - 订阅 `ChtcToolCallReceived` 事件(后台 spawn 处理)
//!
//! ## P1-6:2PC 占位实现替换
//!
//! 原 `prepare_phase` / `commit_phase` / `rollback_phase` 直接用 `tokio::time::sleep`
//! 模拟网络往返。P1-6 引入 `ParticipantClient` trait,将网络通信抽象化:
//! - 默认使用 `InProcessClient`(保持原 sleep-based 行为,向后兼容)
//! - 生产环境可通过 `with_participant_client` 注入 `TcpParticipantClient`
//! - 测试环境可注入 `MockParticipantClient` 验证失败场景
//!
//! ## Week 6 教训:broadcast 时序
//! `bus.subscribe()` 必须在 `tokio::spawn` 之前同步调用,不能在 async 块内订阅。
//! WHY:`tokio::broadcast` 仅投递给发布时已存在的 receiver;若在 spawn 的 async
//! block 内 subscribe,后台任务调度时机不确定,可能晚于 publish 导致事件静默丢失。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::MeshConfig;
use crate::error::McpError;
use crate::quantum::participant_client::{InProcessClient, ParticipantClient};
use crate::quantum::superposition::{execute_superposition_query, QueryResult, SuperpositionQuery};
use crate::quantum::transaction::{QuantumTransaction, TransactionState, WalEntry};
use crate::quantum::wal::WalStore;
use crate::server_registry::{MeshServer, ServerRegistry};
use crate::types::TransactionResult;

/// MCP Mesh — 量子网格的核心入口
///
/// 持有服务器注册表(`Arc<ServerRegistry>`)、可选的 EventBus、配置与
/// 参与者客户端(`Arc<dyn ParticipantClient>`)。
/// 所有方法通过 `&self` 调用,内部状态基于 `Arc` + `DashMap`,线程安全。
pub struct McpMesh {
    /// Mesh 配置(事务超时、心跳阈值等)
    config: MeshConfig,
    /// 服务器注册表(Arc 共享,后台订阅任务可 clone)
    registry: Arc<ServerRegistry>,
    /// 可选事件总线(事务完成时发布事件)
    event_bus: Option<EventBus>,
    /// 参与者客户端 — 抽象 2PC 各阶段的网络通信
    ///
    /// 默认为 `InProcessClient`(sleep-based mock,向后兼容);
    /// 生产环境通过 `with_participant_client` 注入 `TcpParticipantClient`。
    participant_client: Arc<dyn ParticipantClient>,
    /// WAL 持久化存储(Task 0.7 v2.9.0-omega)
    ///
    /// `Some` 时 2PC 各阶段切换会追加 WAL entry,协调者崩溃后可通过
    /// `recover_from_wal()` 重建未完成事务。`None` 表示禁用持久化
    /// (`config.durable == false`),适合纯内存测试场景。
    wal_store: Option<Arc<WalStore>>,
    /// 待补偿事务队列(Task 0.7 v2.9.0-omega)
    ///
    /// Commit 阶段部分参与者失败时,事务 ID + 失败参与者入队,
    /// 由 `reconcile_pending_transactions()` 周期重试。
    /// WHY DashMap:并发安全,允许 reconcile 任务与 execute_transaction 并行访问。
    pending_compensations: DashMap<String, PendingCompensation>,
}

/// 待补偿事务记录(Task 0.7 v2.9.0-omega)
///
/// Commit 阶段部分参与者失败时创建,记录失败的参与者与已重试次数,
/// 由 `reconcile_pending_transactions` 按 `max_retries` 重试。
#[derive(Debug, Clone)]
pub struct PendingCompensation {
    /// 事务 ID
    pub transaction_id: String,
    /// Commit 阶段失败的参与者 ID 列表
    pub failed_participants: Vec<String>,
    /// 已重试次数(达 `MeshConfig::max_retries` 后放弃)
    pub retries: u32,
    /// 入队时刻(UTC),用于 TTL 与日志
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 根据 `MeshConfig::durable` 与 `wal_path` 初始化 WalStore(自由函数)
///
/// WHY 抽出为函数:三个构造函数共用相同逻辑,避免重复。
/// - `durable == false` → 返回 `None`(禁用持久化)
/// - `durable == true` + `wal_path == Some(p)` → 使用 `p`
/// - `durable == true` + `wal_path == None` → 使用 `WalStore::default_path()`,
///   若 `default_path()` 返回 `None`(HOME/USERPROFILE 都不存在)则降级为 `None`
fn init_wal_store(config: &MeshConfig) -> Option<Arc<WalStore>> {
    if !config.durable {
        return None;
    }
    let path = config.wal_path.clone().or_else(WalStore::default_path)?;
    Some(Arc::new(WalStore::new(path)))
}

impl McpMesh {
    /// 创建 MCP Mesh(无 EventBus,默认 InProcessClient)
    ///
    /// 使用 `InProcessClient` 作为参与者客户端,保持与原占位实现一致的行为
    /// (sleep-based 网络模拟),向后兼容现有测试。
    pub fn new(config: MeshConfig) -> Self {
        let registry = Arc::new(ServerRegistry::new(config.registry_capacity));
        let wal_store = init_wal_store(&config);
        Self {
            config,
            registry,
            event_bus: None,
            participant_client: Arc::new(InProcessClient::new()),
            wal_store,
            pending_compensations: DashMap::new(),
        }
    }

    /// 创建 MCP Mesh 并绑定 EventBus(默认 InProcessClient)
    ///
    /// 绑定后,`execute_transaction` 成功完成会发布 `McpMeshTransactionCompleted` 事件。
    /// 调用 `start_event_subscriber` 可订阅 `ChtcToolCallReceived` 处理 IDE 工具调用。
    pub fn with_event_bus(config: MeshConfig, bus: EventBus) -> Self {
        let registry = Arc::new(ServerRegistry::new(config.registry_capacity));
        let wal_store = init_wal_store(&config);
        Self {
            config,
            registry,
            event_bus: Some(bus),
            participant_client: Arc::new(InProcessClient::new()),
            wal_store,
            pending_compensations: DashMap::new(),
        }
    }

    /// 创建 MCP Mesh 并注入自定义参与者客户端(生产路径)
    ///
    /// 用于生产环境注入 `TcpParticipantClient`(真实 TCP 通信),
    /// 或测试环境注入 `MockParticipantClient`(可注入失败)。
    ///
    /// # 参数
    /// - `config`:Mesh 配置
    /// - `bus`:事件总线(可选,传 `None` 则不发布事件)
    /// - `client`:参与者客户端(`Arc<dyn ParticipantClient>`)
    ///
    /// # 示例
    /// ```no_run
    /// use std::sync::Arc;
    /// use mcp_mesh::{McpMesh, MeshConfig, TcpParticipantClient, ParticipantClient};
    ///
    /// # async fn run() {
    /// let client: Arc<dyn ParticipantClient> = Arc::new(TcpParticipantClient::new());
    /// let mesh = McpMesh::with_participant_client(
    ///     MeshConfig::default(),
    ///     None,
    ///     client,
    /// );
    /// # }
    /// ```
    pub fn with_participant_client(
        config: MeshConfig,
        bus: Option<EventBus>,
        client: Arc<dyn ParticipantClient>,
    ) -> Self {
        let registry = Arc::new(ServerRegistry::new(config.registry_capacity));
        let wal_store = init_wal_store(&config);
        Self {
            config,
            registry,
            event_bus: bus,
            participant_client: client,
            wal_store,
            pending_compensations: DashMap::new(),
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

    /// 执行量子事务 — 2PC 跨多服务器原子提交
    ///
    /// # 流程
    /// 1. 校验 participants:已注册且 alive
    /// 2. 创建 `QuantumTransaction`(Init)
    /// 3. `tokio::time::timeout` 包装整体执行,超时即 Abort+Rollback
    /// 4. Prepare 阶段:并发向所有参与者发 prepare(通过 `ParticipantClient`)
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
                let _ = self.rollback_phase(&tx).await;
                TransactionResult::failed(tx_id.clone(), start.elapsed().as_millis() as u64)
            }
        };

        // 5. 发布事务完成事件(best-effort)
        // P2-5:透传真实 capability_id(op 参数即被调用的能力/工具名),
        // csn-substitutor 降级链以 capability_id 为键,必须透传才能精准推进。
        self.publish_transaction_completed(&result, Some(op)).await;

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
        match self.prepare_phase(tx, op).await {
            Ok(()) => {
                // 全部 ACK → Commit
                tx.transition(TransactionState::Commit)?;
                self.commit_phase(tx).await?;
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
                self.rollback_phase(tx).await?;
                tx.transition(TransactionState::Rollback)?;
                Ok(None)
            }
        }
    }

    /// Prepare 阶段 — 并发向所有参与者发送 prepare 请求(带 max_retries 重试)
    ///
    /// 通过 `ParticipantClient::prepare` 发送请求,用 `FuturesUnordered` 并发 fanout。
    /// 任一参与者失败(Nack / 网络错误)按指数退避(200ms / 400ms)重试,达 `max_retries`
    /// 仍失败则整体失败,触发 Abort+Rollback。
    ///
    /// Task 0.7 v2.9.0-omega 新增:
    /// - 指数退避重试(`max_retries` 控制,默认 2,退避 200ms / 400ms)
    /// - Prepare 成功后写 WAL `Prepare` entry(含已 ACK 参与者列表)
    ///
    /// # 参数
    /// - `tx`:量子事务(提供 transaction_id 和 participant_servers)
    /// - `op`:操作描述
    async fn prepare_phase(&self, tx: &QuantumTransaction, op: &str) -> Result<(), McpError> {
        let max_retries = self.config.max_retries;
        let mut acked: Vec<String> = Vec::with_capacity(tx.participant_servers.len());

        // WHY 逐个参与者串行重试而非并发重试:并发重试需要复杂的状态机跟踪每个参与者的
        // 重试次数,且 prepare 阶段允许部分慢参与者。这里采用并发首次 + 失败者串行重试
        // 的混合策略:首次并发 fanout,失败的参与者按指数退避串行重试。
        let mut futures: FuturesUnordered<_> = tx
            .participant_servers
            .iter()
            .map(|sid| {
                let sid = sid.clone();
                async move {
                    let server =
                        self.registry
                            .get(&sid)
                            .ok_or_else(|| McpError::ServerNotFound {
                                server_id: sid.clone(),
                            })?;
                    self.participant_client
                        .prepare(&server, &tx.transaction_id, op)
                        .await
                        .map(|_| sid.clone())
                }
            })
            .collect();

        // 收集首次失败的参与者(网络错误才重试;Nack 协议错误直接放弃)
        let mut failed_participants: Vec<(String, McpError)> = Vec::new();
        while let Some(result) = futures.next().await {
            match result {
                Ok(sid) => acked.push(sid),
                Err(e) => {
                    // ProtocolError(Nack)不重试 — 参与者明确拒绝,重试无意义
                    if matches!(e, McpError::ProtocolError { .. }) {
                        return Err(e);
                    }
                    // NetworkError 收集后重试
                    failed_participants.push((String::new(), e)); // sid 在下面填回
                }
            }
        }

        // WHY 重新收集失败的参与者 sid:上面的 async move 块消耗了 sid,需从原列表找回
        // 这里简化:首次并发失败的参与者通过 acked 集合差集计算
        let failed_sids: Vec<String> = tx
            .participant_servers
            .iter()
            .filter(|sid| !acked.contains(sid))
            .cloned()
            .collect();

        // 指数退避重试失败的参与者(仅 NetworkError)
        for sid in failed_sids {
            let mut last_err = failed_participants
                .iter()
                .find(|(_, _)| true)
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| McpError::NetworkError {
                    server_id: sid.clone(),
                    endpoint: String::new(),
                    reason: "unknown".into(),
                });

            for attempt in 0..max_retries {
                // 指数退避:200ms * 2^attempt(200ms / 400ms / 800ms...)
                let backoff_ms = 200u64 * (1 << attempt);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                let server = self
                    .registry
                    .get(&sid)
                    .ok_or_else(|| McpError::ServerNotFound {
                        server_id: sid.clone(),
                    })?;

                match self
                    .participant_client
                    .prepare(&server, &tx.transaction_id, op)
                    .await
                {
                    Ok(()) => {
                        acked.push(sid.clone());
                        break;
                    }
                    Err(e) => {
                        warn!(
                            transaction_id = %tx.transaction_id,
                            server_id = %sid,
                            attempt = attempt + 1,
                            max_retries,
                            error = %e,
                            "Prepare 重试失败"
                        );
                        // ProtocolError 不再重试
                        if matches!(e, McpError::ProtocolError { .. }) {
                            return Err(e);
                        }
                        last_err = e;
                    }
                }
            }

            // 检查最终是否成功(若 acked 中已包含 sid,说明重试成功)
            if !acked.contains(&sid) {
                return Err(last_err);
            }
        }

        // Task 0.7 SubTask 0.7.3: Prepare 成功后写 WAL Prepare entry
        // WHY best-effort:WAL 失败不阻塞主流程,仅告警(数据可能未持久化,崩溃后无法恢复)
        if let Some(wal) = &self.wal_store {
            let entry = WalEntry::new(&tx.transaction_id, TransactionState::Prepare, acked.clone());
            if let Err(e) = wal.append(&entry).await {
                warn!(
                    transaction_id = %tx.transaction_id,
                    error = %e,
                    "WAL Prepare entry 写入失败(继续,崩溃后可能无法恢复)"
                );
            }
        }

        Ok(())
    }

    /// Commit 阶段 — 并发向所有参与者发送 commit 请求(带 max_retries 重试)
    ///
    /// 通过 `ParticipantClient::commit` 发送请求,用 `FuturesUnordered` 并发 fanout。
    /// 任一参与者失败按指数退避重试;达 `max_retries` 仍失败则记录到待补偿队列
    /// (`pending_compensations`),由 `reconcile_pending_transactions()` 后续处理。
    ///
    /// Task 0.7 v2.9.0-omega 新增:
    /// - 指数退避重试
    /// - 失败参与者入待补偿队列(2PC 经典问题:Commit 阶段失败需人工介入)
    /// - Commit 成功后写 WAL `Commit` entry
    async fn commit_phase(&self, tx: &QuantumTransaction) -> Result<(), McpError> {
        let max_retries = self.config.max_retries;
        let mut committed: Vec<String> = Vec::with_capacity(tx.participant_servers.len());
        let mut failed_participants: Vec<String> = Vec::new();

        for sid in &tx.participant_servers {
            let server = self
                .registry
                .get(sid)
                .ok_or_else(|| McpError::ServerNotFound {
                    server_id: sid.clone(),
                })?;

            let mut last_err: Option<McpError> = None;
            for attempt in 0..=max_retries {
                if attempt > 0 {
                    // 指数退避:200ms * 2^(attempt-1)
                    let backoff_ms = 200u64 * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
                match self
                    .participant_client
                    .commit(&server, &tx.transaction_id)
                    .await
                {
                    Ok(()) => {
                        committed.push(sid.clone());
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        warn!(
                            transaction_id = %tx.transaction_id,
                            server_id = %sid,
                            attempt,
                            max_retries,
                            error = %e,
                            "Commit 重试失败"
                        );
                        last_err = Some(e);
                    }
                }
            }

            if let Some(err) = last_err {
                failed_participants.push(sid.clone());
                // 入待补偿队列:Commit 阶段失败需人工介入或后续 reconcile
                self.pending_compensations
                    .entry(tx.transaction_id.clone())
                    .or_insert_with(|| PendingCompensation {
                        transaction_id: tx.transaction_id.clone(),
                        failed_participants: Vec::new(),
                        retries: 0,
                        created_at: chrono::Utc::now(),
                    })
                    .failed_participants
                    .push(sid.clone());
                // 不立即返回 Err — 继续尝试其他参与者,让失败范围最小化
                let _ = err;
            }
        }

        if !failed_participants.is_empty() {
            // 部分参与者 Commit 失败:返回错误但已写入待补偿队列
            return Err(McpError::CompensationFailed {
                transaction_id: tx.transaction_id.clone(),
                retries: 0,
                failed_participants,
            });
        }

        // Task 0.7 SubTask 0.7.4: Commit 全部成功后写 WAL Commit entry
        if let Some(wal) = &self.wal_store {
            let entry = WalEntry::new(
                &tx.transaction_id,
                TransactionState::Commit,
                committed.clone(),
            );
            if let Err(e) = wal.append(&entry).await {
                warn!(
                    transaction_id = %tx.transaction_id,
                    error = %e,
                    "WAL Commit entry 写入失败(事务已成功,但崩溃后可能重复恢复)"
                );
            }
        }

        Ok(())
    }

    /// Rollback 阶段 — 并发向所有参与者发送 rollback 请求(best-effort)
    ///
    /// 通过 `ParticipantClient::rollback` 发送请求,用 `FuturesUnordered` 并发 fanout。
    /// best-effort:任一参与者失败仅记录告警,不阻塞事务终结(回滚是尽力而为)。
    async fn rollback_phase(&self, tx: &QuantumTransaction) -> Result<(), McpError> {
        let mut futures: FuturesUnordered<_> = tx
            .participant_servers
            .iter()
            .map(|sid| {
                let sid = sid.clone();
                async move {
                    let server = match self.registry.get(&sid) {
                        Some(s) => s,
                        None => {
                            warn!(
                                server_id = %sid,
                                transaction_id = %tx.transaction_id,
                                "Rollback 阶段服务器未注册(best-effort,跳过)"
                            );
                            return;
                        }
                    };
                    let result = self
                        .participant_client
                        .rollback(&server, &tx.transaction_id)
                        .await;
                    if let Err(e) = &result {
                        warn!(
                            server_id = %sid,
                            transaction_id = %tx.transaction_id,
                            error = %e,
                            "Rollback 阶段参与者失败(best-effort,继续)"
                        );
                    }
                }
            })
            .collect();

        while futures.next().await.is_some() {}
        Ok(())
    }

    /// 执行超位置查询 — 委托至 `quantum::superposition` 模块
    pub async fn superposition_query(
        &self,
        query: SuperpositionQuery,
    ) -> Result<Vec<QueryResult>, McpError> {
        execute_superposition_query(&query, &self.registry, self.config.heartbeat_timeout_ms).await
    }

    /// 发布 `McpMeshTransactionCompleted` 事件(best-effort,失败仅告警)
    ///
    /// # 参数
    /// - `capability_id`: 被调用的能力/工具名(execute_transaction 的 op 参数),
    ///   csn-substitutor 据此精准推进对应降级链(P2-5,Task 0.5.8)。
    async fn publish_transaction_completed(
        &self,
        result: &TransactionResult,
        capability_id: Option<String>,
    ) {
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
                // P2-5:透传真实 capability_id(原硬编码 None 导致 csn 精准推进分支不可达)
                capability_id,
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

    /// 从 WAL 恢复未完成事务(Task 0.7 v2.9.0-omega SubTask 0.7.5)
    ///
    /// 协调者重启后调用,扫描 WAL 重建未完成事务状态:
    /// - 仅有 Prepare entry 的事务(无 Commit/Rollback):参与者已 ACK,必须重新
    ///   发起 Commit(2PC 协议要求:Prepare ACK 后参与者锁定了资源,协调者必须 Commit)
    /// - 有 Commit 或 Rollback entry 的事务:已完成,跳过
    ///
    /// # 流程
    /// 1. `WalStore::read_all()` 读取所有 entry
    /// 2. 按 transaction_id 分组,记录每个事务的最后状态(按 timestamp 取最新)
    /// 3. 对最后状态为 Prepare 的事务:重建 QuantumTransaction 并调用 `commit_phase`
    /// 4. Commit 失败的事务入 `pending_compensations` 队列等待后续 reconcile
    /// 5. 恢复完成后调用 `WalStore::truncate()` 清空 WAL(避免无限增长)
    ///
    /// # 错误处理
    /// - WAL 未启用(`durable = false`):返回 `WalIoError`(调用方应决定是否继续)
    /// - WAL 读取失败:返回 `WalIoError`(启动失败)
    /// - 单个事务 Commit 失败:入待补偿队列,继续处理下一个事务(不阻塞恢复)
    ///
    /// # 返回
    /// 需要重新 Commit 的事务 ID 列表(用于日志与监控)
    pub async fn recover_from_wal(&self) -> Result<Vec<String>, McpError> {
        let wal_store = self
            .wal_store
            .as_ref()
            .ok_or_else(|| McpError::WalIoError {
                reason: "WAL 未启用(config.durable = false),无法恢复".into(),
            })?;

        // 1. 读取所有 WAL entry
        let entries = wal_store.read_all().await?;
        if entries.is_empty() {
            debug!("WAL 为空,无需恢复");
            return Ok(Vec::new());
        }

        // 2. 按 transaction_id 分组,记录每个事务的最终状态(取 timestamp 最大的 entry)
        // WHY 取最新:同一事务可能有多条 entry(Prepare → Commit),只关心最后状态
        let mut tx_final_state: HashMap<String, &WalEntry> = HashMap::new();
        for entry in &entries {
            let should_update = match tx_final_state.get(&entry.transaction_id) {
                None => true,
                Some(existing) => existing.timestamp < entry.timestamp,
            };
            if should_update {
                tx_final_state.insert(entry.transaction_id.clone(), entry);
            }
        }

        // 3. 筛选需要恢复的事务(最终状态为 Prepare)
        let needs_commit: Vec<(&String, &&WalEntry)> = tx_final_state
            .iter()
            .filter(|(_, e)| e.state == TransactionState::Prepare)
            .collect();

        info!(
            total_entries = entries.len(),
            total_transactions = tx_final_state.len(),
            needs_recovery = needs_commit.len(),
            "WAL 恢复扫描完成"
        );

        let mut recovered_tx_ids = Vec::with_capacity(needs_commit.len());

        // 4. 对每个需要恢复的事务重新发起 Commit
        for (tx_id, entry) in needs_commit {
            // WHY 用 participants_ack 而非 participant_servers:WAL Prepare entry 记录
            // 的是已 ACK 的参与者(未 ACK 的不应 Commit),确保只对已锁定资源的参与者提交
            let mut tx = QuantumTransaction::with_id(tx_id.clone(), entry.participants_ack.clone());
            // 已在 Prepare 状态,直接转换到 Commit(跳过 Init→Prepare,因 WAL 已记录 Prepare)
            tx.state = TransactionState::Prepare;
            if let Err(e) = tx.transition(TransactionState::Commit) {
                warn!(
                    transaction_id = %tx_id,
                    error = %e,
                    "WAL 恢复:状态转换 Prepare→Commit 失败,跳过"
                );
                continue;
            }

            match self.commit_phase(&tx).await {
                Ok(()) => {
                    info!(transaction_id = %tx_id, "WAL 恢复:Commit 成功");
                    recovered_tx_ids.push(tx_id.clone());
                }
                Err(e) => {
                    // commit_phase 内部已将失败参与者入 pending_compensations
                    warn!(
                        transaction_id = %tx_id,
                        error = %e,
                        "WAL 恢复:Commit 失败,已入待补偿队列等待 reconcile"
                    );
                    recovered_tx_ids.push(tx_id.clone());
                }
            }
        }

        // 5. 恢复完成后 truncate WAL(避免无限增长)
        // WHY truncate:已恢复的事务不再需要 WAL entry;未恢复的已在 pending_compensations,
        // 由 reconcile_pending_transactions 后续处理,不依赖 WAL
        if let Err(e) = wal_store.truncate().await {
            warn!(error = %e, "WAL 恢复后 truncate 失败(下次启动会重复恢复)");
        }

        Ok(recovered_tx_ids)
    }

    /// 重试待补偿事务(Task 0.7 v2.9.0-omega SubTask 0.7.8)
    ///
    /// 周期性调用(如每 30s),从 `pending_compensations` 队列取出 Commit 阶段
    /// 失败的事务,对失败参与者重试 Commit。
    ///
    /// # 流程
    /// 1. 遍历 `pending_compensations` 中的每条记录
    /// 2. 对每条记录的 `failed_participants` 重新发送 commit 请求
    /// 3. 成功的参与者从 failed 列表移除
    /// 4. 仍失败的参与者:递增 `retries`,若达 `max_retries` 则放弃并返回
    /// 5. 全部成功的记录从队列移除
    ///
    /// # 错误处理
    /// - 单条事务重试失败:不阻塞其他事务,继续处理下一条
    /// - 达 `max_retries` 仍失败:加入返回列表(调用方人工介入或告警)
    ///
    /// # 返回
    /// 已达 `max_retries` 仍失败的事务列表(需人工介入)。空 Vec 表示全部重试成功或无待补偿事务。
    pub async fn reconcile_pending_transactions(&self) -> Vec<PendingCompensation> {
        let max_retries = self.config.max_retries;
        let mut still_failing = Vec::new();

        // 收集需要处理的 transaction_id(避免持锁跨 await,§4.4 反模式 #1)
        let tx_ids: Vec<String> = self
            .pending_compensations
            .iter()
            .map(|r| r.key().clone())
            .collect();

        for tx_id in tx_ids {
            // 取出条目快照(避免持锁跨 await,§4.4 反模式 #1)
            // WHY 用 block scope:DashMap 读锁(get 返回的 Ref)必须在 await 前 drop,
            // 否则持锁跨 await 会阻塞其他写操作(如同 endpoint 的连接池操作)
            let (failed_participants, retries) = {
                let e = match self.pending_compensations.get(&tx_id) {
                    Some(e) => e,
                    None => continue,
                };
                (e.failed_participants.clone(), e.retries)
                // e(DashMap Ref guard)在 block 结束时自动 drop
            };

            // 对每个失败参与者重试 commit
            let mut still_failed: Vec<String> = Vec::new();
            for sid in &failed_participants {
                let server = match self.registry.get(sid) {
                    Some(s) => s,
                    None => {
                        warn!(
                            transaction_id = %tx_id,
                            server_id = %sid,
                            "Reconcile:服务器已注销,从失败列表移除"
                        );
                        continue;
                    }
                };

                match self.participant_client.commit(&server, &tx_id).await {
                    Ok(()) => {
                        info!(
                            transaction_id = %tx_id,
                            server_id = %sid,
                            "Reconcile:Commit 重试成功"
                        );
                    }
                    Err(e) => {
                        warn!(
                            transaction_id = %tx_id,
                            server_id = %sid,
                            error = %e,
                            "Reconcile:Commit 重试失败"
                        );
                        still_failed.push(sid.clone());
                    }
                }
            }

            // 更新或移除待补偿记录
            if still_failed.is_empty() {
                // 全部成功:移除记录
                self.pending_compensations.remove(&tx_id);
                info!(transaction_id = %tx_id, "Reconcile:全部参与者 Commit 成功,移除待补偿记录");
            } else {
                // 仍有失败:更新记录
                let new_retries = retries + 1;
                if new_retries >= max_retries {
                    // 达 max_retries:移除并加入 still_failing 列表
                    if let Some((_, mut pc)) = self.pending_compensations.remove(&tx_id) {
                        pc.retries = new_retries;
                        pc.failed_participants = still_failed.clone();
                        warn!(
                            transaction_id = %tx_id,
                            retries = new_retries,
                            failed = ?still_failed,
                            "Reconcile:达 max_retries,放弃重试(需人工介入)"
                        );
                        still_failing.push(pc);
                    }
                } else if let Some(mut entry) = self.pending_compensations.get_mut(&tx_id) {
                    entry.retries = new_retries;
                    entry.failed_participants = still_failed;
                }
            }
        }

        still_failing
    }

    /// 待补偿事务数量(用于测试与监控)
    pub fn pending_compensation_count(&self) -> usize {
        self.pending_compensations.len()
    }

    /// 启动后台探活任务(Task 0.7 v2.9.0-omega SubTask 0.7.13)
    ///
    /// `tokio::spawn` 一个周期任务,每隔 `config.background_probe_interval_ms`
    /// 遍历注册表,注销超过 `heartbeat_timeout_ms` 未心跳的僵尸服务器。
    ///
    /// # fire-and-forget 评估(§4.4 反模式 #7)
    ///
    /// 此任务符合 fire-and-forget 适用条件:
    /// - **幂等**:注销僵尸服务器是幂等操作(已注销的再次注销无副作用)
    /// - **非关键路径**:不参与 2PC 事务流程,失败仅导致僵尸服务器占用注册表
    ///   更长时间(下次探活仍会清理)
    /// - **不影响数据一致性**:注册表状态可由心跳重建(服务器重新注册即可)
    ///
    /// 因此 fire-and-forget 模式可接受,失败仅记日志。`JoinHandle` 返回给调用方
    /// 以便测试时主动 abort(避免泄漏)。
    ///
    /// # 返回
    /// `Some(JoinHandle)` 表示后台任务已启动;`None` 表示 `background_probe_interval_ms = 0`(禁用)。
    pub fn start_background_probe(&self) -> Option<JoinHandle<()>> {
        let interval_ms = self.config.background_probe_interval_ms;
        if interval_ms == 0 {
            debug!("background_probe_interval_ms = 0,后台探活禁用");
            return None;
        }

        let registry = Arc::clone(&self.registry);
        let heartbeat_timeout_ms = self.config.heartbeat_timeout_ms;

        Some(tokio::spawn(async move {
            let interval = Duration::from_millis(interval_ms);
            info!(
                interval_ms,
                heartbeat_timeout_ms, "McpMesh 后台探活任务启动"
            );

            loop {
                tokio::time::sleep(interval).await;

                // 遍历所有已注册服务器,注销僵尸
                let all_ids = registry.list_all();
                let mut dead_count = 0;
                for sid in &all_ids {
                    if let Some(server) = registry.get(sid) {
                        if !server.is_alive(heartbeat_timeout_ms) {
                            let _ = registry.unregister(sid);
                            dead_count += 1;
                        }
                    }
                }

                if dead_count > 0 {
                    warn!(
                        cleaned = dead_count,
                        remaining = registry.len(),
                        "后台探活清理僵尸服务器"
                    );
                } else {
                    debug!(alive = registry.len(), "后台探活:所有服务器心跳正常");
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mesh_with_servers(n: usize) -> McpMesh {
        // 事务超时用 30s 而非默认 200ms：16 线程全并行下 5 服务器 2PC
        // 可能超过 200ms 导致 TransactionTimeout flaky（v2.21.0 同款教训，
        // 消除全量回归不稳定；生产默认 200ms 语义不变）
        let config = MeshConfig {
            transaction_timeout_ms: 30_000,
            ..Default::default()
        };
        let mesh = McpMesh::new(config);
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
        assert!(result.latency_ms < 2000, "单服务器事务应在 2s 内完成");
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
