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
use crate::quantum::superposition::{
    execute_superposition_query, QueryResult, SuperpositionQuery, DEFAULT_MAX_QUERY_FANOUT,
};
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

/// 指数退避毫秒数：`base * 2^min(attempt, 6)`。
///
/// WHY 位移封顶 6：不封顶时 `1 << 64` 在 debug panic、release 回绕；
/// 且默认 `transaction_timeout_ms=200` 下，超预算的退避只会把「可重试」
/// 变成「必超时」。base=50 时单次上限 3200ms（50 × 2^6）。
fn backoff_ms(base: u64, attempt: u32) -> u64 {
    base * (1u64 << attempt.min(6))
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
    /// 4. Prepare 阶段:并发向所有参与者发 prepare(通过 `ParticipantClient`,
    ///    op 与 args 全量透传至协议帧)
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
        args: Vec<String>,
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
        let outcome = tokio::time::timeout(deadline, self.run_2pc(&mut tx, &op, &args)).await;

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
        args: &[String],
    ) -> Result<Option<Vec<String>>, McpError> {
        // Init → Prepare
        tx.transition(TransactionState::Prepare)?;

        // Prepare 阶段:并发向所有参与者发 prepare
        match self.prepare_phase(tx, op, args).await {
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
    /// 首次失败(NetworkError)的参与者各自拥有独立的并行重试 future(独立指数退避
    /// 50ms / 100ms),重试墙钟 ≈ 最慢一臂而非各臂之和;Nack(ProtocolError)立即
    /// fail-fast;重试耗尽仍失败则整体失败,触发 Abort+Rollback。
    ///
    /// Task 0.7 v2.9.0-omega 新增:
    /// - 指数退避重试(`max_retries` 控制,默认 2,退避 50ms / 100ms)
    /// - Prepare 成功后写 WAL `Prepare` entry(含已 ACK 参与者列表)
    ///
    /// # 参数
    /// - `tx`:量子事务(提供 transaction_id 和 participant_servers)
    /// - `op`:操作描述
    /// - `args`:操作参数(全量透传至协议帧,不在此层裁剪)
    async fn prepare_phase(
        &self,
        tx: &QuantumTransaction,
        op: &str,
        args: &[String],
    ) -> Result<(), McpError> {
        let max_retries = self.config.max_retries;
        let mut acked: Vec<String> = Vec::with_capacity(tx.participant_servers.len());

        // WP-A:重试并行化 — 首次并发 fanout 后,每个失败 sid 拥有独立的并行重试
        // future(独立退避计数,见下方 retry_futures)。旧实现"并发重试需复杂状态机"
        // 的理由不成立:per-sid 重试各管各的退避计数,无需全局状态机。
        let mut futures: FuturesUnordered<_> =
            tx.participant_servers
                .iter()
                .map(|sid| {
                    let sid = sid.clone();
                    async move {
                        let sid_for_result = sid.clone();
                        let result = async {
                            let server = self.registry.get(&sid).ok_or_else(|| {
                                McpError::ServerNotFound {
                                    server_id: sid.clone(),
                                }
                            })?;
                            self.participant_client
                                .prepare(&server, &tx.transaction_id, op, args)
                                .await
                        }
                        .await;
                        // 错误必须携带真实 sid（P3-4）：旧实现 `map(|_| sid.clone())`
                        // 在 Err 时丢掉 sid，调用方只能 `find(|(_, _)| true)` 恒取首错。
                        (sid_for_result, result)
                    }
                })
                .collect();

        // 收集首次失败的参与者(网络错误才重试;Nack 协议错误直接放弃)
        let mut failed_participants: Vec<(String, McpError)> = Vec::new();
        while let Some((sid, result)) = futures.next().await {
            match result {
                Ok(()) => acked.push(sid),
                Err(e) => {
                    // ProtocolError(Nack)不重试 — 参与者明确拒绝,重试无意义
                    if matches!(e, McpError::ProtocolError { .. }) {
                        return Err(e);
                    }
                    // NetworkError 收集后重试（携带真实 sid）
                    failed_participants.push((sid, e));
                }
            }
        }

        // WP-A 重试并行化:每个失败 sid 一个独立重试 future(独立退避计数),用
        // FuturesUnordered 并发驱动 — 重试墙钟 ≈ max(各臂) 而非 Σ(各臂)。
        // WHY 并行安全:prepare 是 2PC 的幂等询问,允许重发;每 sid 退避互不干扰,
        // 无需全局状态机;收集循环收到 ProtocolError 即 return,未完成 future 随
        // FuturesUnordered 一起 drop(零 tokio::spawn,无孤儿任务,drop 即取消)。
        // failed_participants 已携带 (sid, 首次错误),无需再按 acked 差集重算 sid 列表。
        let mut retry_futures: FuturesUnordered<_> = failed_participants
            .into_iter()
            .map(|(sid, first_err)| {
                async move {
                    // first_err 兜底:全部重试失败时返回最后一次错(至少有首次错)
                    let mut last_err = first_err;
                    for attempt in 0..max_retries {
                        // 指数退避(默认预算可完成一次重试):50ms * 2^min(attempt, 6)
                        // (50ms / 100ms / 200ms / 400ms / 800ms / 1600ms / 3200ms 封顶)
                        let backoff_ms = backoff_ms(50, attempt);
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                        let server =
                            self.registry
                                .get(&sid)
                                .ok_or_else(|| McpError::ServerNotFound {
                                    server_id: sid.clone(),
                                })?;

                        match self
                            .participant_client
                            .prepare(&server, &tx.transaction_id, op, args)
                            .await
                        {
                            Ok(()) => return Ok(sid),
                            Err(e) => {
                                warn!(
                                    transaction_id = %tx.transaction_id,
                                    server_id = %sid,
                                    attempt = attempt + 1,
                                    max_retries,
                                    error = %e,
                                    "Prepare 重试失败"
                                );
                                // ProtocolError(Nack)上抛交由收集循环 fail-fast,
                                // 其余参与者的重试 future 随 return 被 drop(无孤儿任务)
                                if matches!(e, McpError::ProtocolError { .. }) {
                                    return Err(e);
                                }
                                last_err = e;
                            }
                        }
                    }
                    // 重试耗尽:返回本 sid 最后一次的错误
                    Err(last_err)
                }
            })
            .collect();

        // 收集重试结果:ProtocolError 立即 fail-fast(return 即 drop 其余臂);
        // 其他最终失败先记录、继续等剩余臂跑完,全部结束后统一裁决(最终一致性)。
        let mut final_err: Option<McpError> = None;
        while let Some(result) = retry_futures.next().await {
            match result {
                Ok(sid) => acked.push(sid),
                Err(e) => {
                    if matches!(e, McpError::ProtocolError { .. }) {
                        return Err(e);
                    }
                    // 多 sid 失败时返回首个最终失败者的 last_err(具体是哪个 sid 与
                    // 串行版可能不同,可接受:run_2pc 仅用于日志 + Abort 判定,不区分 sid)
                    final_err.get_or_insert(e);
                }
            }
        }
        if let Some(e) = final_err {
            return Err(e);
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
    /// 性能重构并行化(P1-1)：旧实现虽文档称「并发」但实为串行 `for`（每参与者
    /// 内嵌 `sleep` 退避），commit 阶段墙钟 ≈ Σ 参与者 RTT。现与 `prepare`/`rollback`
    /// 一致真并发扇出，墙钟降为 ≈ max RTT。保留：
    /// - **失败范围最小化**（不立即 return，先试完所有参与者，最后统一报 CompensationFailed）；
    /// - **输入序可观测顺序**：committed/failed 按 `participant_servers` 输入序展开（与旧串行一致）；
    /// - 回滚开关：`CHIMERA_MCP_SERIAL_COMMIT=1` 走串行分支（含 `ServerNotFound`
    ///   循环层短路，与旧行为**逐位等价**）；并发分支把 `ServerNotFound` 归一为
    ///   `CompensationFailed` 并入待补偿台账（集合语义，失败范围最小化）。
    ///
    /// Task 0.7 v2.9.0-omega 新增：指数退避重试 / 失败入待补偿队列 / Commit 成功后写 WAL。
    async fn commit_phase(&self, tx: &QuantumTransaction) -> Result<(), McpError> {
        let serial = std::env::var("CHIMERA_MCP_SERIAL_COMMIT").as_deref() == Ok("1");
        self.commit_phase_with_mode(tx, serial).await
    }

    /// `commit_phase` 的模式显式版本（`serial` 由环境开关或测试注入）。
    ///
    /// WHY 拆出参数而非常直接在测试里设 env：`CHIMERA_MCP_SERIAL_COMMIT` 是
    /// 进程级变量，同二进制内的其它事务测试会被并行改成串行模式。
    async fn commit_phase_with_mode(
        &self,
        tx: &QuantumTransaction,
        serial: bool,
    ) -> Result<(), McpError> {
        let tx_id = tx.transaction_id.clone();
        let n = tx.participant_servers.len();

        // 每参与者一个结果槽（None = 未回收，兼作防御）。按输入序 idx 存放，
        // 使 committed/failed 重建时仍为输入序（与旧串行 for 的可观测顺序一致）。
        let mut outcomes: Vec<Option<Result<(), McpError>>> = vec![None; n];
        if serial {
            for (idx, sid) in tx.participant_servers.iter().enumerate() {
                // P1-3 逐位等价：旧实现在 **循环层** `registry.get(sid)?` 短路——
                // 未注册（含校验后被并发摘除的竞态）立即 `ServerNotFound`，
                // 且**不写**待补偿台账。此处显式复刻该语义。
                // 纯存在性判断走零 clone 的 `contains`（`get` 会 clone 整条记录）
                if !self.registry.contains(sid) {
                    return Err(McpError::ServerNotFound {
                        server_id: sid.clone(),
                    });
                }
                outcomes[idx] = Some(self.commit_one(sid, &tx_id).await);
            }
        } else {
            // FuturesUnordered 并发扇出：每个 future 内嵌指数退避重试（复用 commit_one）。
            let mut futures: FuturesUnordered<_> = tx
                .participant_servers
                .iter()
                .enumerate()
                .map(|(idx, sid)| {
                    let sid = sid.clone();
                    // 捕获 `&str`（Copy）而非 move 外层 `tx_id` String，避开 FnMut 闭包
                    // 的所有权冲突；`tx_id` 在 commit_phase 作期间一直有效。
                    let tx_ref = tx_id.as_str();
                    async move { (idx, self.commit_one(&sid, tx_ref).await) }
                })
                .collect();
            while let Some((idx, res)) = futures.next().await {
                outcomes[idx] = Some(res);
            }
        }

        // 按输入序展开为 committed / failed（集合语义，顺序与旧实现一致）。
        let mut committed: Vec<String> = Vec::with_capacity(n);
        let mut failed_participants: Vec<String> = Vec::new();
        for (idx, sid) in tx.participant_servers.iter().enumerate() {
            match outcomes[idx].take() {
                Some(Ok(())) => committed.push(sid.clone()),
                // None = 理论上不可达（每个 idx 必被回收）；防御性归为失败，不谎报成功。
                Some(Err(_)) | None => failed_participants.push(sid.clone()),
            }
        }

        if !failed_participants.is_empty() {
            // 入待补偿队列（Commit 阶段失败需人工介入或后续 reconcile），按输入序。
            self.pending_compensations
                .entry(tx_id.clone())
                .or_insert_with(|| PendingCompensation {
                    transaction_id: tx_id.clone(),
                    failed_participants: Vec::new(),
                    retries: 0,
                    created_at: chrono::Utc::now(),
                })
                .failed_participants
                .extend(failed_participants.iter().cloned());
            return Err(McpError::CompensationFailed {
                transaction_id: tx_id.clone(),
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

    /// 单个参与者的 commit（含指数退避重试），供 [`commit_phase`](Self::commit_phase) 并发/串行共用。
    ///
    /// WHY 拆出：串行与并发分支共享同一重试语义，避免两份潜在分叉（单一真值源）。
    /// `ServerNotFound`（registry 未命中）立即返回不重试——该参与者不可达，重试无意义。
    /// 串行分支在 `commit_phase_with_mode` 已做循环层短路（与旧行为逐位等价）；
    /// 并发分支会把该错误收进 `outcomes`，最终归一为 `CompensationFailed`。
    async fn commit_one(&self, sid: &str, tx_id: &str) -> Result<(), McpError> {
        let max_retries = self.config.max_retries;
        let mut last_err: Option<McpError> = None;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                // 指数退避(默认 transaction_timeout_ms=200 内可完成首次重试):
                // 50ms * 2^(attempt-1)，位移封顶 6 → 单次 ≤ 3200ms。
                // WHY 基数 50ms 而非旧 200ms：旧值 ≥ 默认超时预算，重试在生产是死码。
                let backoff_ms = backoff_ms(50, attempt - 1);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            let server = self
                .registry
                .get(sid)
                .ok_or_else(|| McpError::ServerNotFound {
                    server_id: sid.to_string(),
                })?;
            match self.participant_client.commit(&server, tx_id).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!(
                        transaction_id = %tx_id,
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
        // 循环结束仍有错误 = 重试耗尽（至少执行过一次 commit，last_err 必为 Some）。
        Err(last_err.unwrap_or_else(|| McpError::ServerNotFound {
            server_id: sid.to_string(),
        }))
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
        // WS-4A: 收包成功路径 — 消息到达网格即发布 McpMessageReceived(同层通信)
        // source_node 取查询消息源标识(query_id),msg_type 标记消息类型。
        // 消息进入网格即发布(best-effort,失败仅告警),不阻塞查询执行。
        // O-4(深审 2026-09-25): 携带上游 trace_id 时延续同一追踪链跨进程贯传
        self.publish_message_received(query.query_id.clone(), "superposition_query", &query)
            .await;

        execute_superposition_query(
            &query,
            &self.registry,
            self.config.heartbeat_timeout_ms,
            // 传输抽象统一注入(Task 0.7.10 闭环):默认 InProcessClient 保持
            // 进程内模拟语义;--network 时走真实 QueryRequest TCP 帧
            Arc::clone(&self.participant_client),
            // WP-H/ADR-284:fanout 并发上界(排队语义),消除无上界扇出资源风险
            DEFAULT_MAX_QUERY_FANOUT,
        )
        .await
    }

    /// 发布 `McpMessageReceived` 事件(best-effort,失败仅告警)
    ///
    /// WS-4A:"消息到达网格"的发布入口。镜像 `publish_transaction_completed`
    /// 的既有模式(event-bus `with_event_bus` 注入的 `Option<EventBus>` +
    /// `publish().await`),未绑定 EventBus 时静默跳过。
    async fn publish_message_received(
        &self,
        source_node: String,
        msg_type: &str,
        query: &SuperpositionQuery,
    ) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::McpMessageReceived {
                // O-4 wire:请求携上游 trace 则延续追踪链,未携则保持历史 fresh 行为
                metadata: query.trace_meta(),
                source_node,
                msg_type: msg_type.to_string(),
            };
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, "McpMessageReceived 事件发布失败");
            }
        }
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// 首次 commit 失败、后续成功的客户端（P2-1 预算验证用）。
    #[derive(Default)]
    struct FlakyOnceClient {
        failed: Mutex<HashSet<String>>,
    }

    impl ParticipantClient for FlakyOnceClient {
        fn prepare<'a>(
            &'a self,
            _s: &'a MeshServer,
            _t: &'a str,
            _op: &'a str,
            _args: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn commit<'a>(
            &'a self,
            s: &'a MeshServer,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            // insert 返回 true = 首次调用
            let first = {
                let mut guard = self.failed.lock().unwrap_or_else(|e| e.into_inner());
                guard.insert(s.server_id.clone())
            };
            if first {
                let sid = s.server_id.clone();
                Box::pin(async move {
                    Err(McpError::NetworkError {
                        server_id: sid,
                        endpoint: String::new(),
                        reason: "flaky first commit".into(),
                    })
                })
            } else {
                Box::pin(async { Ok(()) })
            }
        }

        fn rollback<'a>(
            &'a self,
            _s: &'a MeshServer,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn query<'a>(
            &'a self,
            _s: &'a MeshServer,
            _q: &'a str,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
    }

    /// P2-1：默认预算（200ms）内首次失败 + 50ms 退避重试必须能成功。
    ///
    /// 改前基数 200ms ≥ 预算 → 首次退避即被外层 timeout 取消（重试是死码）。
    #[tokio::test]
    async fn commit_retry_within_default_budget() {
        let client = Arc::new(FlakyOnceClient::default());
        let mesh = McpMesh::with_participant_client(
            MeshConfig::default(),
            None,
            Arc::clone(&client) as Arc<dyn ParticipantClient>,
        );
        mesh.register_server(MeshServer::new("s-0", "203.0.113.1:80", vec![]))
            .expect("注册失败");

        let result = mesh
            .execute_transaction(vec!["s-0".into()], "op".into(), Vec::new())
            .await
            .expect("默认预算内应能完成首次失败 + 重试（改前必超时）");
        assert!(result.success, "重试后 commit 应成功");
    }

    /// P2-1：位移封顶 6，极大 `max_retries` 不 panic/回绕，单次退避 ≤ 3200ms。
    #[test]
    fn backoff_is_bounded_for_large_max_retries() {
        assert_eq!(backoff_ms(50, 0), 50);
        assert_eq!(backoff_ms(50, 1), 100);
        assert_eq!(backoff_ms(50, 6), 3200);
        assert_eq!(backoff_ms(50, 70), 3200, "位移超 6 必被封印");
        assert_eq!(backoff_ms(50, u32::MAX), 3200);
    }

    // === WP-A: prepare 重试并行化(失败者串行指数退避 → per-sid 并行重试) ===

    /// prepare 单步脚本 — `ScriptedPrepareClient` 按 sid 按调用序消费
    ///
    /// WHY 枚举而非直接存 `Result`:错误需按目标 sid 构造(携带真实 server_id /
    /// endpoint),脚本只描述"行为形状",错误构造放在 future 内。
    #[derive(Clone, Debug)]
    enum PrepareScriptStep {
        /// sleep 指定时长后返回 NetworkError(模拟网络故障;ZERO = 立即失败)
        NetworkErrAfter(Duration),
        /// sleep 指定时长后返回 Ok(模拟慢参与者重试后恢复)
        OkAfter(Duration),
        /// 立即返回 ProtocolError(模拟 Nack — 协议层明确拒绝,重试无意义)
        ProtocolErr,
    }

    /// 测试专用脚本化 Mock:按 server_id 配置 prepare 逐次调用行为
    ///
    /// 与 `MockParticipantClient`(静态 per-sid 失败注入)不同,本 Mock:
    /// - **按调用序消费脚本**:每 sid 第 n 次调用取脚本第 n 步,耗尽后重复末步
    ///   ("持续失败"脚本只需写一步)
    /// - **可注入延迟**:步内 tokio sleep 模拟慢参与者(真实计时,延迟取短值控制测试时长)
    /// - **记录每 sid 调用次数**:验证 fail-fast 截断与重试次数
    struct ScriptedPrepareClient {
        scripts: HashMap<String, Vec<PrepareScriptStep>>,
        prepare_calls: Mutex<HashMap<String, usize>>,
    }

    impl ScriptedPrepareClient {
        fn new(scripts: Vec<(&str, Vec<PrepareScriptStep>)>) -> Self {
            Self {
                scripts: scripts
                    .into_iter()
                    .map(|(sid, steps)| (sid.to_string(), steps))
                    .collect(),
                prepare_calls: Mutex::new(HashMap::new()),
            }
        }

        fn prepare_calls(&self, sid: &str) -> usize {
            self.prepare_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(sid)
                .copied()
                .unwrap_or(0)
        }
    }

    impl ParticipantClient for ScriptedPrepareClient {
        fn prepare<'a>(
            &'a self,
            server: &'a MeshServer,
            _transaction_id: &'a str,
            _op: &'a str,
            _args: &'a [String],
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            // 锁内快照本步脚本并计数,锁外 await(§4.4 反模式 #1:禁止持锁跨 .await)
            let step = {
                let mut calls = self.prepare_calls.lock().unwrap_or_else(|e| e.into_inner());
                let n = calls.entry(server.server_id.clone()).or_insert(0);
                let idx = *n;
                *n += 1;
                self.scripts.get(&server.server_id).and_then(|script| {
                    let last = script.len().saturating_sub(1);
                    script.get(idx.min(last)).cloned()
                })
            };
            Box::pin(async move {
                match step {
                    Some(PrepareScriptStep::OkAfter(delay)) => {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    }
                    Some(PrepareScriptStep::NetworkErrAfter(delay)) => {
                        tokio::time::sleep(delay).await;
                        Err(McpError::NetworkError {
                            server_id: server.server_id.clone(),
                            endpoint: server.endpoint.clone(),
                            reason: "Scripted 注入: prepare 网络失败".into(),
                        })
                    }
                    Some(PrepareScriptStep::ProtocolErr) => Err(McpError::ProtocolError {
                        server_id: server.server_id.clone(),
                        reason: "Scripted 注入: prepare 被拒绝(Nack)".into(),
                    }),
                    // 未配置脚本的 sid 默认成功(与 MockParticipantClient 默认全成功一致)
                    None => Ok(()),
                }
            })
        }

        fn commit<'a>(
            &'a self,
            _s: &'a MeshServer,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback<'a>(
            &'a self,
            _s: &'a MeshServer,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn query<'a>(
            &'a self,
            _s: &'a MeshServer,
            _q: &'a str,
            _t: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
            Box::pin(async { Ok("ok".to_string()) })
        }
    }

    /// 构造注入 `ScriptedPrepareClient` 的 Mesh。
    ///
    /// WHY `durable: false`:避免测试向磁盘写 WAL 文件(纯内存行为,更干净)。
    /// WHY `transaction_timeout_ms: 30_000`:被测预算是重试墙钟而非事务超时,
    /// 大预算消除 16 线程并行下 200ms 默认值的 flaky(v2.21.0 同款教训)。
    fn make_scripted_mesh(client: Arc<ScriptedPrepareClient>, sids: &[&str]) -> McpMesh {
        let config = MeshConfig {
            transaction_timeout_ms: 30_000,
            max_retries: 2,
            durable: false,
            ..Default::default()
        };
        let mesh =
            McpMesh::with_participant_client(config, None, client as Arc<dyn ParticipantClient>);
        for (i, sid) in sids.iter().enumerate() {
            // RFC 5737 TEST-NET-3 地址绕过 SSRF 校验
            mesh.register_server(MeshServer::new(
                (*sid).to_string(),
                format!("203.0.113.1:{i}"),
                vec![],
            ))
            .expect("注册失败");
        }
        mesh
    }

    /// WP-A:两个慢参与者的重试必须重叠执行(墙钟 ≈ max(各臂) 而非 Σ)。
    ///
    /// 场景:两参与者首次 prepare 立即 NetworkError,重试调用各自耗时 60ms 后成功。
    /// 并行版墙钟 ≈ 50ms(退避) + 60ms(重试 RTT) = 110ms;
    /// 串行版(旧实现)≈ (50 + 60) × 2 = 220ms。断言 < 150ms 只在两臂重叠时成立。
    ///
    /// WHY 真实计时而非 `start_paused`:start_paused 需 tokio `test-util` feature,
    /// 为本测试给 mcp-mesh 加 feature 超出 WP-A 只改 mesh.rs 的约束;真实 sleep
    /// 取 60ms 让并行余量(≈110ms → 阈值 150ms)吸收 Windows timer 精度与调度抖动。
    #[tokio::test]
    async fn prepare_retry_parallel_wallclock() {
        let client = Arc::new(ScriptedPrepareClient::new(vec![
            (
                "s-0",
                vec![
                    PrepareScriptStep::NetworkErrAfter(Duration::ZERO),
                    PrepareScriptStep::OkAfter(Duration::from_millis(60)),
                ],
            ),
            (
                "s-1",
                vec![
                    PrepareScriptStep::NetworkErrAfter(Duration::ZERO),
                    PrepareScriptStep::OkAfter(Duration::from_millis(60)),
                ],
            ),
        ]));
        let mesh = make_scripted_mesh(Arc::clone(&client), &["s-0", "s-1"]);

        let start = tokio::time::Instant::now();
        let result = mesh
            .execute_transaction(vec!["s-0".into(), "s-1".into()], "op".into(), Vec::new())
            .await
            .expect("首次失败 + 重试成功后事务应完成");
        let elapsed = start.elapsed();

        assert!(result.success, "两参与者重试成功后应 Commit");
        assert_eq!(result.committed_servers.len(), 2);
        assert!(
            elapsed < Duration::from_millis(150),
            "两臂重试应重叠(并行墙钟 ≈110ms),实际 {elapsed:?} — 若 ≈220ms 说明退化为串行"
        );
        // 每参与者恰 2 次 prepare:首次失败 + 1 次重试成功
        assert_eq!(client.prepare_calls("s-0"), 2);
        assert_eq!(client.prepare_calls("s-1"), 2);
    }

    /// WP-A:任一臂返回 ProtocolError(Nack)必须立即 fail-fast,截断其余臂的重试。
    ///
    /// s-0 排参与者首位且持续 NetworkError:旧串行实现会先耗尽它的 2 次重试
    /// (3 次调用)并返回 NetworkError;并行实现中 s-1 的 ProtocolError 在
    /// t≈50ms(退避后)先到,立即整体失败,s-0 的重试被截断(≤2 次调用)。
    #[tokio::test]
    async fn prepare_retry_parallel_fail_fast_on_protocol_error() {
        let client = Arc::new(ScriptedPrepareClient::new(vec![
            // s-0:持续 NetworkError(脚本耗尽后重复末步)
            (
                "s-0",
                vec![PrepareScriptStep::NetworkErrAfter(Duration::ZERO)],
            ),
            // s-1:首次 NetworkError,重试返回 ProtocolError(Nack)
            (
                "s-1",
                vec![
                    PrepareScriptStep::NetworkErrAfter(Duration::ZERO),
                    PrepareScriptStep::ProtocolErr,
                ],
            ),
        ]));
        let mesh = make_scripted_mesh(Arc::clone(&client), &["s-0", "s-1"]);
        let tx = QuantumTransaction::with_id(
            "tx-fail-fast".to_string(),
            vec!["s-0".into(), "s-1".into()],
        );

        // 直接观测 prepare_phase 的错误变体(execute_transaction 会吞掉 McpError)
        let err = mesh
            .prepare_phase(&tx, "op", &[])
            .await
            .expect_err("ProtocolError 必须 fail-fast 整体失败");
        assert!(
            matches!(err, McpError::ProtocolError { .. }),
            "应上抛 ProtocolError 而非其它错误,实际 {err:?}"
        );
        let s0_calls = client.prepare_calls("s-0");
        assert!(
            s0_calls <= 2,
            "s-0 的重试应被 fail-fast 截断(≤2 次),实际 {s0_calls} 次 — 3 次说明未截断"
        );
        assert_eq!(client.prepare_calls("s-1"), 2, "s-1 首次 + 1 次重试(Nack)");
    }

    /// WP-A:重试耗尽必须返回 last_err(NetworkError)且 Abort+Rollback 正常终结。
    ///
    /// 行为守恒测试(新旧实现均应通过):单参与者持续 NetworkError,
    /// max_retries=2 → 首次 + 2 次重试 = 3 次调用后整体失败;
    /// `execute_transaction` 全路径走 Abort → Rollback,事务以 success=false 终结。
    #[tokio::test]
    async fn prepare_retry_exhausted_returns_error() {
        let client = Arc::new(ScriptedPrepareClient::new(vec![(
            "s-0",
            vec![PrepareScriptStep::NetworkErrAfter(Duration::ZERO)],
        )]));
        let mesh = make_scripted_mesh(Arc::clone(&client), &["s-0"]);

        // 1) 直接观测 prepare_phase 的错误变体(execute_transaction 会吞掉 McpError)
        let tx = QuantumTransaction::with_id("tx-exhausted".to_string(), vec!["s-0".into()]);
        let err = mesh
            .prepare_phase(&tx, "op", &[])
            .await
            .expect_err("重试耗尽应返回 last_err");
        assert!(
            matches!(err, McpError::NetworkError { .. }),
            "重试耗尽应返回最后一次 NetworkError,实际 {err:?}"
        );
        assert_eq!(client.prepare_calls("s-0"), 3, "首次 + 2 次重试");

        // 2) 全路径:prepare 失败 → Abort → Rollback,事务正常终结(success=false)
        let result = mesh
            .execute_transaction(vec!["s-0".into()], "op".into(), Vec::new())
            .await
            .expect("prepare 失败应走 Abort+Rollback 正常返回,而非 Err 或挂起");
        assert!(!result.success, "prepare 重试耗尽的事务必须失败");
    }

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

    /// P1-3：serial 2PC 必须与旧实现的**循环层 `?` 短路**逐位等价——
    /// 未注册参与者立即 `ServerNotFound`，且**不写**待补偿台账。
    #[tokio::test]
    async fn serial_commit_short_circuits_unregistered_participant() {
        let mesh = make_mesh_with_servers(1); // 只注册 s-0
        let tx = QuantumTransaction::with_id(
            "tx-serial-short-circuit".to_string(),
            vec!["s-0".into(), "s-missing".into()],
        );

        let err = mesh
            .commit_phase_with_mode(&tx, true)
            .await
            .expect_err("未注册参与者应短路返回错误");
        assert!(
            matches!(err, McpError::ServerNotFound { .. }),
            "serial 分支必须与旧短路一致（ServerNotFound），实际 {err:?}"
        );
        assert_eq!(
            mesh.pending_compensations.len(),
            0,
            "短路路径不得写待补偿台账（旧行为零台账）"
        );
    }

    /// 并发分支的差异必须被显式钉住：同一未注册场景归一为
    /// `CompensationFailed` 并入台账（集合语义、失败范围最小化）。
    #[tokio::test]
    async fn concurrent_commit_normalizes_unregistered_to_compensation_failed() {
        let mesh = make_mesh_with_servers(1);
        let tx = QuantumTransaction::with_id(
            "tx-concurrent-normalize".to_string(),
            vec!["s-0".into(), "s-missing".into()],
        );

        let err = mesh
            .commit_phase_with_mode(&tx, false)
            .await
            .expect_err("未注册参与者应使事务失败");
        assert!(
            matches!(err, McpError::CompensationFailed { .. }),
            "并发分支应将 ServerNotFound 归一为 CompensationFailed，实际 {err:?}"
        );
        assert_eq!(
            mesh.pending_compensations.len(),
            1,
            "并发分支须登记待补偿台账（可 reconcile）"
        );
    }

    #[tokio::test]
    async fn test_execute_transaction_single_server() {
        let mesh = make_mesh_with_servers(1);
        let result = mesh
            .execute_transaction(vec!["s-0".into()], "test".into(), Vec::new())
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
            .execute_transaction(participants.clone(), "test".into(), Vec::new())
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
            .execute_transaction(participants, "test".into(), Vec::new())
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::TooManyParticipants { .. }));
    }

    #[tokio::test]
    async fn test_execute_transaction_unregistered_server() {
        let mesh = make_mesh_with_servers(1);
        let err = mesh
            .execute_transaction(
                vec!["s-0".into(), "unknown".into()],
                "test".into(),
                Vec::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }

    #[tokio::test]
    async fn test_publishes_mcp_mesh_transaction_completed() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        // C-114/M78：本行原先用 `MeshConfig::default()`（事务超时 200ms），是全仓
        // 已经收过的那类「把调度延迟当成被测属性」的漏网——16 线程满并行下 2PC 可以
        // 跑过 200ms ⇒ 直接报 `TransactionTimeout{timeout_ms:200}`（实测并跑红、单跑绿）。
        // 同文件 `make_mesh_with_servers` 已因 v2.21.0 同款教训改用 30_000ms，本用例
        // 当时没跟着改。本测试的被测对象是**事件发布**，不是 200ms 预算；
        // 生产默认 200ms 的语义与超时机制本身均不在本行调整范围内
        // （`config.rs` 的默认值断言照引）。
        let mesh = McpMesh::with_event_bus(
            MeshConfig {
                transaction_timeout_ms: 30_000,
                ..Default::default()
            },
            bus,
        );
        mesh.register_server(MeshServer::new("s-1", "203.0.113.1", vec![]))
            .expect("注册失败");

        let result = mesh
            .execute_transaction(vec!["s-1".into()], "test".into(), Vec::new())
            .await
            .expect("事务失败");

        // WHY timeout:全量并行(跨 crate)时资源竞争曾导致本测试偶发失败
        // (事件延迟/断言时序);无超时 recv 在事件丢失时永久挂起,超时保护
        // 将 hang 转为确定性失败并携带诊断信息。
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("5s 内未收到事务完成事件(资源竞争或事件丢失)")
            .expect("应收到事件");
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
            .execute_transaction(vec!["s-0".into()], "test".into(), Vec::new())
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
    async fn test_superposition_query_publishes_mcp_message_received() {
        // WS-4A: 消息到达网格即发布 McpMessageReceived(同层通信)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let mesh = McpMesh::with_event_bus(MeshConfig::default(), bus);
        for i in 0..3 {
            mesh.register_server(MeshServer::new(
                format!("s-{i}"),
                format!("203.0.113.1:{i}"),
                vec![],
            ))
            .expect("注册失败");
        }

        let query = SuperpositionQuery::with_id(
            "query-node-1",
            "test",
            (0..3).map(|i| format!("s-{i}")).collect(),
            100,
        );
        let _ = mesh.superposition_query(query).await.expect("查询失败");

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("5s 内未收到 McpMessageReceived(资源竞争或事件丢失)")
            .expect("应收到事件");
        match event {
            NexusEvent::McpMessageReceived {
                metadata,
                source_node,
                msg_type,
            } => {
                assert_eq!(metadata.source, "mcp-mesh");
                assert_eq!(source_node, "query-node-1");
                assert_eq!(msg_type, "superposition_query");
            }
            _ => panic!("期望 McpMessageReceived 事件,得到 {:?}", event.type_name()),
        }
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
            .execute_transaction(vec!["s-0".into()], "op1".into(), Vec::new())
            .await
            .expect("事务失败");
        assert!(r1.success);

        // 心跳应成功
        mesh.heartbeat("s-0").expect("心跳失败");

        // 注销后事务应失败
        mesh.unregister_server("s-0").expect("注销失败");
        let err = mesh
            .execute_transaction(vec!["s-0".into()], "op2".into(), Vec::new())
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ServerNotFound { .. }));
    }
}
