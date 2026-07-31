//! 2PC 参与者客户端 — 抽象跨服务器网络通信
//!
//! 对应架构层:L10 Interface
//! 对应任务:P1-6(mcp-mesh 2PC 占位实现替换)
//!
//! # 设计决策(WHY)
//!
//! ## 1. trait 抽象 + 多实现
//!
//! 原 `prepare_phase` / `commit_phase` / `rollback_phase` 直接用 `tokio::time::sleep`
//! 模拟网络往返(in-process mock),无法支持真实跨进程部署。本模块引入
//! `ParticipantClient` trait,将"如何与参与者通信"从 2PC 状态机中解耦:
//!
//! - `InProcessClient`:保持原 sleep-based 行为(默认,向后兼容)
//! - `TcpParticipantClient`:真实 TCP 通信(生产路径)
//! - `MockParticipantClient`:内存 mock,可注入失败(测试用)
//!
//! ## 2. boxed Future(与 JudgeClient / CiGate 模式一致)
//!
//! 项目未引入 `async-trait` 依赖(保持依赖最小化)。`Pin<Box<dyn Future>>`
//! 是兼容 `dyn Trait` 的标准模式,与 `auto-dpo::JudgeClient`、
//! `gsoe-evolution::CiGate` trait 模式一致,降低认知负担。
//!
//! ## 3. FuturesUnordered 并发 fanout
//!
//! 阶段内并发向所有参与者发请求,用 `FuturesUnordered` 收集结果(§4.1 规范)。
//! 相比 `JoinSet::spawn`,`FuturesUnordered` 允许非 `'static` future,
//! 直接持有对 `&self.participant_client` 和局部 `MeshServer` 的借用,无需 Arc clone。
//!
//! ## 4. 长度前缀帧协议
//!
//! TCP 是字节流,需 framing 区分消息边界。采用 4 字节大端长度前缀 + JSON 载荷:
//! - 简单、广泛使用(如 gRPC 前 5 字节、Redis RESP)
//! - 4 字节最大支持 4GiB 载荷,远超 2PC 消息需求
//! - `MAX_FRAME_SIZE` 上限(16 MiB)防止恶意参与者谎报超大长度导致 OOM
//!
//! ## 5. 单阶段超时独立于事务总超时
//!
//! - 事务总超时(`transaction_timeout_ms`):覆盖整个 2PC 流程,由 `McpMesh::execute_transaction` 的 `tokio::time::timeout` 控制
//! - 单阶段超时(`phase_timeout_ms`):覆盖单次 TCP 往返,由 `TcpParticipantClient` 控制
//!
//! 两者层次不同:单阶段超时让快速失败(单个参与者无响应)触发 Abort,
//! 避免等待事务总超时才回滚。

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::error::McpError;
use crate::server_registry::{extract_host, is_reserved_ip, MeshServer};

// ============================================================
// 2PC 协议消息类型
// ============================================================

/// 2PC 协议请求 — 协调者发给参与者的三种消息
///
/// 序列化为 JSON 后通过长度前缀帧发送(见 `TcpParticipantClient`)。
/// 协议版本隐含在 JSON 字段名中,未来版本可通过新增字段向后兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TwoPcRequest {
    /// Prepare 请求:询问参与者是否可以提交
    Prepare {
        /// 事务 ID(UUIDv7)
        transaction_id: String,
        /// 操作描述(参与者据此判断能否提交,如 "write file X")
        op: String,
    },
    /// Commit 请求:通知参与者正式提交
    Commit {
        /// 事务 ID
        transaction_id: String,
    },
    /// Rollback 请求:通知参与者回滚
    Rollback {
        /// 事务 ID
        transaction_id: String,
    },
}

/// 2PC 协议响应 — 参与者返回给协调者的应答
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TwoPcResponse {
    /// ACK:同意 prepare / 确认 commit / 确认 rollback
    Ack,
    /// NACK:拒绝 prepare(携带原因);commit/rollback 不应返回 Nack
    Nack {
        /// 拒绝原因(如 "资源锁定失败" / "约束冲突")
        reason: String,
    },
}

/// 查询请求 — 非事务只读操作(Task 0.7 v2.9.0-omega SubTask 0.7.10)
///
/// 与 `TwoPcRequest` 分离,因查询不参与 2PC 状态机,无需 prepare/commit。
/// 用于超位置查询(superposition)的 fanout,复用 TCP 连接池。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryRequest {
    /// 查询 ID(与 SuperpositionQuery.query_id 对应)
    pub query_id: String,
    /// 查询语句(语义由具体 MCP 服务器解释)
    pub query: String,
}

/// 查询响应 — 参与者返回查询结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryResponse {
    /// 响应负载(成功时为查询产出,失败时由 McpError 传递错误)
    pub payload: String,
}

// ============================================================
// ParticipantClient trait
// ============================================================

/// 参与者客户端 — 抽象 2PC 各阶段的网络通信
///
/// 生产环境使用 `TcpParticipantClient`(真实 TCP 通信),
/// 测试环境使用 `InProcessClient`(in-process mock,保持向后兼容)。
///
/// # trait 不提供默认实现
///
/// 强制实现者显式提供通信逻辑,避免忘记实现导致空 2PC(静默成功)。
///
/// # 调用方约束
///
/// - 调用方应在 async 上下文中 `.await` 返回的 Future
/// - 同一 `ParticipantClient` 实例可被并发调用(实现需保证 `Send + Sync`)
/// - 实现不应 panic(可能导致 `McpMesh` 不可用)
///
/// # 错误语义
///
/// - `Ok(())`:参与者 ACK,阶段成功
/// - `Err(McpError::NetworkError)`:网络层失败(连接拒绝 / 超时 / RST)
/// - `Err(McpError::ProtocolError)`:协议层失败(反序列化失败 / Nack / 超长帧)
///
/// 调用方(`McpMesh::prepare_phase` 等)根据阶段不同处理错误:
/// - Prepare 阶段:任何 Err 触发 Abort+Rollback
/// - Commit 阶段:Err 记录告警(需人工补偿,数据可能不一致)
/// - Rollback 阶段:Err 记录告警(best-effort,不阻塞事务终结)
pub trait ParticipantClient: Send + Sync {
    /// 向指定参与者发送 prepare 请求
    ///
    /// # 参数
    /// - `server`:目标参与者(含 endpoint)
    /// - `transaction_id`:事务 ID
    /// - `op`:操作描述
    ///
    /// # 返回
    /// - `Ok(())`:参与者 ACK,可以进入 Commit
    /// - `Err`:参与者 Nack 或网络/协议错误,应触发 Abort
    fn prepare<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
        op: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;

    /// 向指定参与者发送 commit 请求
    ///
    /// # 参数
    /// - `server`:目标参与者
    /// - `transaction_id`:事务 ID
    fn commit<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;

    /// 向指定参与者发送 rollback 请求
    ///
    /// # 参数
    /// - `server`:目标参与者
    /// - `transaction_id`:事务 ID
    fn rollback<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;

    /// 向指定参与者发送查询请求(非事务只读操作)
    ///
    /// WHY Task 0.7 v2.9.0-omega SubTask 0.7.10:
    /// 超位置查询(superposition)需通过统一抽象访问参与者,而非直接 in-process 模拟。
    /// 此方法不参与 2PC 状态机,仅做只读查询,可复用 TCP 连接池(SubTask 0.7.11)。
    ///
    /// # 参数
    /// - `server`:目标参与者
    /// - `query_id`:查询 ID(用于关联 SuperpositionQuery)
    /// - `query`:查询语句
    ///
    /// # 返回
    /// - `Ok(payload)`:查询成功,payload 为响应数据
    /// - `Err`:网络/协议错误
    fn query<'a>(
        &'a self,
        server: &'a MeshServer,
        query_id: &'a str,
        query: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>>;
}

// ============================================================
// InProcessClient — 默认实现,保持向后兼容
// ============================================================

/// 进程内 mock 客户端 — 保持与原 sleep-based 占位实现一致的行为
///
/// 用 `tokio::time::sleep` 模拟网络往返延迟(1-2ms,基于 `server_id` 哈希)。
/// prepare/commit 始终成功,rollback 始终成功(best-effort)。
///
/// # 使用场景
///
/// - `McpMesh::new` / `McpMesh::with_event_bus` 默认使用此客户端(向后兼容)
/// - 单元测试中无需真实网络
/// - 开发环境快速验证 2PC 状态机逻辑
///
/// # 延迟模拟
///
/// 延迟基于 `server_id` 字节哈希(1-2ms),与原 `prepare_phase` 实现一致:
/// ```text
/// delay_ms = 1 + (server_id.bytes().fold(0, |acc, b| acc + b as u64) % 2)
/// ```
/// 这确保不同服务器有确定性但不同的延迟,模拟真实网络异构性。
#[derive(Debug, Clone, Default)]
pub struct InProcessClient;

impl InProcessClient {
    /// 创建进程内 mock 客户端
    pub fn new() -> Self {
        Self
    }

    /// 基于 server_id 哈希计算模拟延迟(1-2ms,确定性)
    ///
    /// WHY 哈希:与原 `prepare_phase` 占位实现一致,保持向后兼容。
    /// 不同 server_id 产生不同延迟,模拟网络异构性,但不引入随机性(测试确定性)。
    fn simulated_delay_ms(server_id: &str) -> u64 {
        1 + (server_id.bytes().fold(0u64, |acc, b| acc + b as u64) % 2)
    }
}

impl ParticipantClient for InProcessClient {
    fn prepare<'a>(
        &'a self,
        server: &'a MeshServer,
        _transaction_id: &'a str,
        _op: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let delay_ms = Self::simulated_delay_ms(&server.server_id);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(())
        })
    }

    fn commit<'a>(
        &'a self,
        server: &'a MeshServer,
        _transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let delay_ms = Self::simulated_delay_ms(&server.server_id);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(())
        })
    }

    fn rollback<'a>(
        &'a self,
        server: &'a MeshServer,
        _transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let delay_ms = Self::simulated_delay_ms(&server.server_id);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(())
        })
    }

    fn query<'a>(
        &'a self,
        server: &'a MeshServer,
        query_id: &'a str,
        query: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
        let delay_ms = Self::simulated_delay_ms(&server.server_id);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            // 模拟查询响应(与 superposition.rs 原 in-process mock 一致的格式)
            Ok(format!("result@{query}@{query_id}@{}", server.server_id))
        })
    }
}

// ============================================================
// TcpParticipantClient — 生产路径真实网络通信
// ============================================================

/// TCP 帧最大载荷(16 MiB),防止恶意参与者谎报超大长度导致 OOM
///
/// 2PC 消息通常 < 200 字节(transaction_id 36 字节 + op ~100 字节 + JSON 开销),
/// 16 MiB 余量 80,000×,足以容纳未来扩展(如携带操作上下文)。
const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// TCP 参与者客户端 — 生产环境真实网络通信
///
/// 通过 TCP 连接参与者的 endpoint,发送 JSON 编码的 2PC 消息,
/// 等待 ACK/NACK 响应。每个阶段有独立超时控制。
///
/// # 协议格式(长度前缀帧)
///
/// ```text
/// +-------------------+---------------------------+
/// | 4 字节大端长度    | JSON 载荷(TwoPcRequest)  |
/// +-------------------+---------------------------+
/// ```
///
/// 响应同理:`4 字节大端长度 + JSON 载荷(TwoPcResponse)`。
///
/// # 超时层次
///
/// - **单阶段超时**(`phase_timeout_ms`):单次 TCP 往返超时,默认 50ms
///   - 超过则返回 `McpError::NetworkError`,让 Prepare 阶段快速失败触发 Abort
/// - **事务总超时**(`MeshConfig::transaction_timeout_ms`):覆盖整个 2PC 流程
///   - 由 `McpMesh::execute_transaction` 的 `tokio::time::timeout` 控制
///
/// # 错误处理
///
/// | 错误类型 | 触发条件 | 调用方处理 |
/// |---------|---------|-----------|
/// | `NetworkError` | 连接拒绝 / 读写超时 / RST | Prepare→Abort;Commit→告警 |
/// | `ProtocolError` | JSON 解析失败 / Nack / 超长帧 | Prepare→Abort;Commit→告警 |
/// | `DnsRebindingBlocked` | DNS 解析后 IP 为保留地址 | Prepare→Abort;Commit→告警 |
///
/// # SSRF 安全 + DNS rebinding 防御(Task 0.7 v2.9.0-omega SubTask 0.7.12)
///
/// `MeshServer::endpoint` 在 `register` 阶段已通过 SSRF 校验(拒绝内网/保留地址)。
/// Task 0.7 引入 DNS rebinding 二次校验:若 endpoint 含域名(非 IP 字面量),
/// `send_request_inner` 在 `connect` 前会通过 `tokio::net::lookup_host` 解析域名,
/// 对解析出的每个 IP 调用 `is_reserved_ip` 校验。任一 IP 为保留地址则拒绝连接,
/// 返回 `McpError::DnsRebindingBlocked`。
///
/// WHY 二次校验:`register` 阶段只校验字面量 IP/已知内网域名,实际 connect 时
/// DNS 可能返回不同的 IP(DNS rebinding 攻击)。二次校验在 connect 前拦截,
/// 彻底切断 DNS rebinding 路径。
pub struct TcpParticipantClient {
    /// 单阶段单服务器超时(毫秒),默认 50ms
    phase_timeout_ms: u64,
    /// TCP 连接池 — 复用已建立的连接,降低 TCP 握手开销
    ///
    /// WHY Task 0.7 v2.9.0-omega SubTask 0.7.11:
    /// 2PC 多阶段(prepare/commit/rollback)对同一参与者连续请求,每次新建 TCP
    /// 连接会引入额外 RTT(握手 ~1ms LAN / ~30ms WAN)。连接池按 endpoint 缓存
    /// TcpStream,后续请求直接复用。
    ///
    /// WHY `Arc<Mutex<HashMap>>`:`TcpStream` 非 `Clone`,且 `tokio::net::TcpStream`
    /// 的并发读写需互斥(同一 stream 不能同时 write_all 和 read_exact)。
    /// `Mutex` 保证同一 endpoint 的 stream 串行使用,避免数据混乱。
    /// 替代方案 `deadpool-tokio` 引入额外依赖,自维护 HashMap 足够轻量。
    connection_pool: Arc<std::sync::Mutex<HashMap<String, TcpStream>>>,
}

impl TcpParticipantClient {
    /// 创建 TCP 客户端,默认单阶段超时 50ms
    ///
    /// WHY 50ms:典型数据中心 RTT < 1ms,跨区域 < 30ms;
    /// 50ms 留 1.5-50× 余量,超时即认为网络分区或参与者故障。
    pub fn new() -> Self {
        Self {
            phase_timeout_ms: 50,
            connection_pool: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 自定义单阶段超时
    ///
    /// # 参数
    /// - `phase_timeout_ms`:单次 TCP 往返超时(毫秒),建议 20-500ms
    pub fn with_phase_timeout(phase_timeout_ms: u64) -> Self {
        Self {
            phase_timeout_ms,
            connection_pool: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// 发送 2PC 请求并等待响应(核心网络通信方法)
    ///
    /// 封装 TCP 连接 → 序列化 → 发送 → 接收 → 反序列化 全流程,
    /// 由 `prepare` / `commit` / `rollback` 方法调用。
    ///
    /// # 参数
    /// - `server`:目标参与者(含 endpoint)
    /// - `request`:2PC 请求消息
    ///
    /// # 返回
    /// - `Ok(())`:参与者返回 `Ack`
    /// - `Err(NetworkError)`:TCP 连接/读写失败或超时
    /// - `Err(ProtocolError)`:JSON 解析失败、超长帧、参与者返回 `Nack`
    async fn send_request(
        &self,
        server: &MeshServer,
        request: TwoPcRequest,
    ) -> Result<(), McpError> {
        let deadline = Duration::from_millis(self.phase_timeout_ms);
        let result = timeout(deadline, self.send_request_inner(server, request)).await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(McpError::NetworkError {
                server_id: server.server_id.clone(),
                endpoint: server.endpoint.clone(),
                reason: format!("单阶段超时({}ms,参与者无响应)", self.phase_timeout_ms),
            }),
        }
    }

    /// 发送查询请求并等待响应(带超时包装)
    ///
    /// Task 0.7 v2.9.0-omega SubTask 0.7.10
    ///
    /// 与 `send_request` 对称,但使用 `QueryRequest`/`QueryResponse` 协议消息,
    /// 返回查询负载字符串。超时语义与 `send_request` 一致(单阶段超时)。
    async fn send_query(
        &self,
        server: &MeshServer,
        query_request: QueryRequest,
    ) -> Result<String, McpError> {
        let deadline = Duration::from_millis(self.phase_timeout_ms);
        let result = timeout(deadline, self.send_query_inner(server, &query_request)).await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(McpError::NetworkError {
                server_id: server.server_id.clone(),
                endpoint: server.endpoint.clone(),
                reason: format!("查询单阶段超时({}ms,参与者无响应)", self.phase_timeout_ms),
            }),
        }
    }

    /// `send_request` 的内部实现(无超时包装,由 `send_request` 统一包装)
    ///
    /// Task 0.7 v2.9.0-omega 新增:
    /// - DNS rebinding 防御(SubTask 0.7.12):connect 前对域名解析结果做 is_reserved_ip 校验
    /// - TCP 连接池(SubTask 0.7.11):优先复用缓存的 TcpStream,失败时新建
    async fn send_request_inner(
        &self,
        server: &MeshServer,
        request: TwoPcRequest,
    ) -> Result<(), McpError> {
        // 1. DNS rebinding 防御:若 endpoint 含域名,解析后校验 IP
        // WHY: register 阶段只校验字面量 IP,connect 时 DNS 可能返回内网 IP
        self.validate_endpoint_dns(&server.server_id, &server.endpoint)
            .await?;

        // 2. 序列化请求为 JSON
        let payload = serde_json::to_vec(&request).map_err(|e| McpError::ProtocolError {
            server_id: server.server_id.clone(),
            reason: format!("请求序列化失败: {e}"),
        })?;

        // 3. 发送请求并接收响应(复用连接池)
        let resp_data = self.send_frame_and_recv(server, &payload).await?;

        // 4. 反序列化响应
        let response: TwoPcResponse =
            serde_json::from_slice(&resp_data).map_err(|e| McpError::ProtocolError {
                server_id: server.server_id.clone(),
                reason: format!("响应反序列化失败: {e}"),
            })?;

        // 5. 根据响应类型返回
        match response {
            TwoPcResponse::Ack => {
                debug!(
                    server_id = %server.server_id,
                    "2PC 请求获 ACK"
                );
                Ok(())
            }
            TwoPcResponse::Nack { reason } => {
                warn!(
                    server_id = %server.server_id,
                    reason = %reason,
                    "2PC 请求获 Nack"
                );
                Err(McpError::ProtocolError {
                    server_id: server.server_id.clone(),
                    reason: format!("参与者 Nack: {reason}"),
                })
            }
        }
    }

    /// 发送查询请求并等待响应
    ///
    /// 与 `send_request_inner` 类似,但使用 `QueryRequest`/`QueryResponse` 协议消息,
    /// 返回查询负载字符串。
    async fn send_query_inner(
        &self,
        server: &MeshServer,
        query_request: &QueryRequest,
    ) -> Result<String, McpError> {
        // 1. DNS rebinding 防御
        self.validate_endpoint_dns(&server.server_id, &server.endpoint)
            .await?;

        // 2. 序列化查询请求
        let payload = serde_json::to_vec(query_request).map_err(|e| McpError::ProtocolError {
            server_id: server.server_id.clone(),
            reason: format!("查询请求序列化失败: {e}"),
        })?;

        // 3. 发送请求并接收响应
        let resp_data = self.send_frame_and_recv(server, &payload).await?;

        // 4. 反序列化查询响应
        let response: QueryResponse =
            serde_json::from_slice(&resp_data).map_err(|e| McpError::ProtocolError {
                server_id: server.server_id.clone(),
                reason: format!("查询响应反序列化失败: {e}"),
            })?;

        Ok(response.payload)
    }

    /// 发送帧并接收响应(复用连接池)
    ///
    /// Task 0.7 SubTask 0.7.11: 优先从连接池取已建立的 TcpStream,
    /// 若无或复用失败则新建连接。请求完成后将 stream 归还连接池。
    ///
    /// WHY 短锁策略:锁内仅做 HashMap get/remove(微秒级),不持锁跨 await。
    /// 这遵循 §4.4 反模式 #1(禁止持锁跨 .await)。
    ///
    /// WHY 连接池失败回退:对端可能在上次请求后关闭连接(如 HTTP/1.1 keep-alive
    /// 超时),复用的 stream 首次 write/read 会失败。此时丢弃旧 stream,
    /// 用新连接重试一次,避免因连接池缓存过期连接而误报网络错误。
    async fn send_frame_and_recv(
        &self,
        server: &MeshServer,
        payload: &[u8],
    ) -> Result<Vec<u8>, McpError> {
        // 尝试从连接池取已建立的 stream
        let pooled = self
            .connection_pool
            .lock()
            .ok()
            .and_then(|mut pool| pool.remove(&server.endpoint).map(Some).unwrap_or(None));

        // 1. 若有 pooled stream,先尝试复用
        if let Some(stream) = pooled {
            debug!(
                server_id = %server.server_id,
                endpoint = %server.endpoint,
                "复用连接池中的 TCP 连接"
            );
            match self.try_send_on_stream(server, stream, payload).await {
                Ok((resp_data, stream)) => {
                    // 成功:将 stream 归还连接池
                    if let Ok(mut pool) = self.connection_pool.lock() {
                        pool.insert(server.endpoint.clone(), stream);
                    }
                    return Ok(resp_data);
                }
                Err(e) => {
                    // 复用失败:丢弃旧 stream,回退到新建连接
                    debug!(
                        server_id = %server.server_id,
                        endpoint = %server.endpoint,
                        error = %e,
                        "连接池复用失败,回退到新连接"
                    );
                }
            }
        }

        // 2. 新建 TCP 连接(无 pooled 或 pooled 失败后)
        let stream = TcpStream::connect(&server.endpoint)
            .await
            .map_err(|e| Self::io_to_network_error(&server.server_id, &server.endpoint, &e))?;
        debug!(
            server_id = %server.server_id,
            endpoint = %server.endpoint,
            "TCP 连接建立成功(新连接)"
        );

        // 3. 在新连接上发送/接收(失败直接返回,不再重试)
        let (resp_data, stream) = self.try_send_on_stream(server, stream, payload).await?;

        // 4. 成功:将 stream 归还连接池
        if let Ok(mut pool) = self.connection_pool.lock() {
            pool.insert(server.endpoint.clone(), stream);
        }

        Ok(resp_data)
    }

    /// 在给定 stream 上发送帧并接收响应
    ///
    /// 返回 `(响应数据, stream)` — 成功时 stream 可归还连接池;
    /// 失败时 stream 已损坏,调用方应丢弃。
    async fn try_send_on_stream(
        &self,
        server: &MeshServer,
        mut stream: TcpStream,
        payload: &[u8],
    ) -> Result<(Vec<u8>, TcpStream), McpError> {
        // 发送长度前缀帧
        let frame_len = payload.len() as u32;
        stream
            .write_all(&frame_len.to_be_bytes())
            .await
            .map_err(|e| Self::io_to_network_error(&server.server_id, &server.endpoint, &e))?;
        stream
            .write_all(payload)
            .await
            .map_err(|e| Self::io_to_network_error(&server.server_id, &server.endpoint, &e))?;

        // 接收响应长度前缀(4 字节大端)
        let mut resp_len_buf = [0u8; 4];
        stream
            .read_exact(&mut resp_len_buf)
            .await
            .map_err(|e| Self::io_to_network_error(&server.server_id, &server.endpoint, &e))?;
        let resp_len = u32::from_be_bytes(resp_len_buf) as usize;

        // 防御:超长帧拦截
        if resp_len > MAX_FRAME_SIZE {
            return Err(McpError::ProtocolError {
                server_id: server.server_id.clone(),
                reason: format!(
                    "响应帧超长: {resp_len} bytes > MAX_FRAME_SIZE({MAX_FRAME_SIZE} bytes)"
                ),
            });
        }

        // 接收响应 JSON 载荷
        let mut resp_data = vec![0u8; resp_len];
        stream
            .read_exact(&mut resp_data)
            .await
            .map_err(|e| Self::io_to_network_error(&server.server_id, &server.endpoint, &e))?;

        Ok((resp_data, stream))
    }

    /// DNS rebinding 防御 — 解析 endpoint 中的域名,校验解析后的 IP
    ///
    /// Task 0.7 v2.9.0-omega SubTask 0.7.12
    ///
    /// # 流程
    /// 1. 从 endpoint 提取 host(剥离 scheme/port)
    /// 2. 若 host 为 IP 字面量,跳过(register 阶段已校验)
    /// 3. 若 host 为域名,通过 `tokio::net::lookup_host` 解析
    /// 4. 对每个解析出的 IP 调用 `is_reserved_ip`,任一保留则拒绝
    ///
    /// # WHY 同步解析改为异步
    /// `tokio::net::lookup_host` 是异步 DNS 解析,不阻塞 runtime 工作线程。
    /// 标准库 `std::net::ToSocketAddrs` 是同步阻塞,会阻塞 async runtime。
    async fn validate_endpoint_dns(&self, server_id: &str, endpoint: &str) -> Result<(), McpError> {
        // 提取 host(复用 server_registry 的 extract_host 逻辑)
        // WHY `?`:`extract_host` 对格式异常的 endpoint 返回 `SsrfBlocked`,
        // 此处直接传播(格式异常的 endpoint 不应到达 connect 阶段)
        let host = extract_host(endpoint)?;

        // 若 host 可解析为 IP 字面量,register 阶段已校验,跳过
        if host.parse::<std::net::IpAddr>().is_ok() {
            return Ok(());
        }

        // host 为域名:异步 DNS 解析
        // WHY lookup_host:tokio 异步 DNS 解析,不阻塞 runtime
        let target = format!("{}:0", host); // lookup_host 需要 host:port 格式
        let resolved =
            tokio::net::lookup_host(&target)
                .await
                .map_err(|e| McpError::NetworkError {
                    server_id: server_id.to_string(),
                    endpoint: endpoint.to_string(),
                    reason: format!("DNS 解析失败: {e}"),
                })?;

        for addr in resolved {
            let ip = addr.ip();
            if is_reserved_ip(ip) {
                return Err(McpError::DnsRebindingBlocked {
                    hostname: host.clone(),
                    resolved_ip: ip.to_string(),
                });
            }
        }

        Ok(())
    }

    /// 将 `io::Error` 转换为 `McpError::NetworkError`
    fn io_to_network_error(server_id: &str, endpoint: &str, e: &io::Error) -> McpError {
        McpError::NetworkError {
            server_id: server_id.to_string(),
            endpoint: endpoint.to_string(),
            reason: e.to_string(),
        }
    }
}

impl Default for TcpParticipantClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticipantClient for TcpParticipantClient {
    fn prepare<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
        op: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let request = TwoPcRequest::Prepare {
            transaction_id: transaction_id.to_string(),
            op: op.to_string(),
        };
        Box::pin(async move { self.send_request(server, request).await })
    }

    fn commit<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let request = TwoPcRequest::Commit {
            transaction_id: transaction_id.to_string(),
        };
        Box::pin(async move { self.send_request(server, request).await })
    }

    fn rollback<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let request = TwoPcRequest::Rollback {
            transaction_id: transaction_id.to_string(),
        };
        Box::pin(async move { self.send_request(server, request).await })
    }

    /// Task 0.7 v2.9.0-omega SubTask 0.7.10
    /// 超位置查询接入 ParticipantClient,复用 TCP 连接池与 DNS rebinding 防御。
    fn query<'a>(
        &'a self,
        server: &'a MeshServer,
        query_id: &'a str,
        query: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
        let request = QueryRequest {
            query_id: query_id.to_string(),
            query: query.to_string(),
        };
        Box::pin(async move { self.send_query(server, request).await })
    }
}

// ============================================================
// MockParticipantClient — 测试用,可注入失败
// ============================================================

/// Mock 参与者客户端 — 测试用,可注入失败场景
///
/// 与 `InProcessClient` 不同,`MockParticipantClient`:
/// - **无延迟**:立即返回(加速测试,无需 sleep)
/// - **可注入失败**:按 `server_id` 配置哪些参与者在哪个阶段失败
/// - **记录调用历史**:验证 2PC 各阶段的调用顺序与参数
///
/// # 失败注入
///
/// ```rust,ignore
/// use mcp_mesh::MockParticipantClient;
///
/// let mock = MockParticipantClient::new()
///     .fail_prepare("s-3")       // s-3 在 prepare 阶段返回 Err
///     .fail_commit("s-2");       // s-2 在 commit 阶段返回 Err
/// ```
///
/// # 调用记录
///
/// 每次 `prepare` / `commit` / `rollback` 调用后,`call_log()` 返回调用历史,
/// 用于验证 2PC 流程是否正确触发了回滚等。
#[derive(Debug, Clone, Default)]
pub struct MockParticipantClient {
    /// 在 prepare 阶段失败的 server_id 集合
    fail_prepare_on: std::collections::HashSet<String>,
    /// 在 commit 阶段失败的 server_id 集合
    fail_commit_on: std::collections::HashSet<String>,
    /// 在 rollback 阶段失败的 server_id 集合(best-effort,失败仅记录)
    fail_rollback_on: std::collections::HashSet<String>,
    /// 调用日志(按时间顺序记录)
    call_log: Arc<std::sync::Mutex<Vec<MockCall>>>,
}

/// Mock 调用记录 — 用于验证 2PC 流程
#[derive(Debug, Clone, PartialEq)]
pub struct MockCall {
    /// 调用的阶段
    pub phase: MockPhase,
    /// 目标服务器 ID
    pub server_id: String,
    /// 事务 ID
    pub transaction_id: String,
    /// 操作描述(仅 prepare 阶段有值)
    pub op: Option<String>,
}

/// Mock 调用阶段标识
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockPhase {
    /// Prepare 阶段
    Prepare,
    /// Commit 阶段
    Commit,
    /// Rollback 阶段
    Rollback,
}

impl MockParticipantClient {
    /// 创建 Mock 客户端(默认全部成功)
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入 prepare 阶段失败(链式调用)
    pub fn fail_prepare(mut self, server_id: impl Into<String>) -> Self {
        self.fail_prepare_on.insert(server_id.into());
        self
    }

    /// 注入 commit 阶段失败(链式调用)
    pub fn fail_commit(mut self, server_id: impl Into<String>) -> Self {
        self.fail_commit_on.insert(server_id.into());
        self
    }

    /// 注入 rollback 阶段失败(链式调用)
    pub fn fail_rollback(mut self, server_id: impl Into<String>) -> Self {
        self.fail_rollback_on.insert(server_id.into());
        self
    }

    /// 获取调用日志(可用于验证 2PC 流程)
    pub fn call_log(&self) -> Vec<MockCall> {
        self.call_log
            .lock()
            .expect("MockParticipantClient call_log Mutex poison")
            .clone()
    }

    /// 记录一次调用
    fn record_call(
        &self,
        phase: MockPhase,
        server_id: &str,
        transaction_id: &str,
        op: Option<&str>,
    ) {
        self.call_log
            .lock()
            .expect("MockParticipantClient call_log Mutex poison")
            .push(MockCall {
                phase,
                server_id: server_id.to_string(),
                transaction_id: transaction_id.to_string(),
                op: op.map(|s| s.to_string()),
            });
    }

    /// 构造协议错误(用于失败注入)
    fn protocol_error(server_id: &str, reason: &str) -> McpError {
        McpError::ProtocolError {
            server_id: server_id.to_string(),
            reason: reason.to_string(),
        }
    }
}

impl ParticipantClient for MockParticipantClient {
    fn prepare<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
        op: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let should_fail = self.fail_prepare_on.contains(&server.server_id);
        Box::pin(async move {
            self.record_call(
                MockPhase::Prepare,
                &server.server_id,
                transaction_id,
                Some(op),
            );
            if should_fail {
                Err(Self::protocol_error(
                    &server.server_id,
                    "Mock 注入: prepare 失败",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn commit<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let should_fail = self.fail_commit_on.contains(&server.server_id);
        Box::pin(async move {
            self.record_call(MockPhase::Commit, &server.server_id, transaction_id, None);
            if should_fail {
                Err(Self::protocol_error(
                    &server.server_id,
                    "Mock 注入: commit 失败",
                ))
            } else {
                Ok(())
            }
        })
    }

    fn rollback<'a>(
        &'a self,
        server: &'a MeshServer,
        transaction_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        let should_fail = self.fail_rollback_on.contains(&server.server_id);
        Box::pin(async move {
            self.record_call(MockPhase::Rollback, &server.server_id, transaction_id, None);
            if should_fail {
                Err(Self::protocol_error(
                    &server.server_id,
                    "Mock 注入: rollback 失败",
                ))
            } else {
                Ok(())
            }
        })
    }

    /// Task 0.7 v2.9.0-omega SubTask 0.7.10
    /// Mock 查询实现 — 立即返回模拟结果,不注入失败(查询为只读操作,无事务状态)
    fn query<'a>(
        &'a self,
        server: &'a MeshServer,
        query_id: &'a str,
        query: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
        Box::pin(async move {
            // 查询不记录到 call_log(非 2PC 阶段),仅返回模拟结果
            Ok(format!(
                "mock_result@{query}@{query_id}@{}",
                server.server_id
            ))
        })
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_server(id: &str) -> MeshServer {
        // 使用 RFC 5737 TEST-NET-3 地址,绕过 SSRF 校验
        MeshServer::new(id, format!("203.0.113.1:{id}"), vec!["cap-1".into()])
    }

    // === 协议消息测试 ===

    #[test]
    fn test_two_pc_request_serde_roundtrip() {
        let req = TwoPcRequest::Prepare {
            transaction_id: "tx-001".into(),
            op: "write file".into(),
        };
        let json = serde_json::to_string(&req).expect("序列化失败");
        let restored: TwoPcRequest = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(req, restored);
    }

    #[test]
    fn test_two_pc_response_serde_roundtrip() {
        let resp = TwoPcResponse::Nack {
            reason: "resource locked".into(),
        };
        let json = serde_json::to_string(&resp).expect("序列化失败");
        let restored: TwoPcResponse = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(resp, restored);
    }

    #[test]
    fn test_two_pc_request_variant_tags() {
        // 验证 JSON 中包含变体标签(用于协议版本兼容性检查)
        let prepare = TwoPcRequest::Prepare {
            transaction_id: "t1".into(),
            op: "op".into(),
        };
        let commit = TwoPcRequest::Commit {
            transaction_id: "t1".into(),
        };
        let rollback = TwoPcRequest::Rollback {
            transaction_id: "t1".into(),
        };

        assert!(serde_json::to_string(&prepare).unwrap().contains("Prepare"));
        assert!(serde_json::to_string(&commit).unwrap().contains("Commit"));
        assert!(serde_json::to_string(&rollback)
            .unwrap()
            .contains("Rollback"));
    }

    // === InProcessClient 测试 ===

    #[tokio::test]
    async fn test_in_process_client_prepare_success() {
        let client = InProcessClient::new();
        let server = make_server("s-1");
        let result = client.prepare(&server, "tx-1", "op").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_in_process_client_commit_success() {
        let client = InProcessClient::new();
        let server = make_server("s-1");
        let result = client.commit(&server, "tx-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_in_process_client_rollback_success() {
        let client = InProcessClient::new();
        let server = make_server("s-1");
        let result = client.rollback(&server, "tx-1").await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_in_process_client_simulated_delay_deterministic() {
        // 相同 server_id 产生相同延迟(确定性)
        let d1 = InProcessClient::simulated_delay_ms("s-1");
        let d2 = InProcessClient::simulated_delay_ms("s-1");
        assert_eq!(d1, d2);
        assert!((1..=2).contains(&d1));
    }

    // === MockParticipantClient 测试 ===

    #[tokio::test]
    async fn test_mock_client_default_all_success() {
        let client = MockParticipantClient::new();
        let server = make_server("s-1");

        assert!(client.prepare(&server, "tx-1", "op").await.is_ok());
        assert!(client.commit(&server, "tx-1").await.is_ok());
        assert!(client.rollback(&server, "tx-1").await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_client_fail_prepare() {
        let client = MockParticipantClient::new().fail_prepare("s-1");
        let server = make_server("s-1");

        let err = client.prepare(&server, "tx-1", "op").await.unwrap_err();
        assert!(matches!(err, McpError::ProtocolError { .. }));
    }

    #[tokio::test]
    async fn test_mock_client_fail_commit() {
        let client = MockParticipantClient::new().fail_commit("s-2");
        let server = make_server("s-2");

        let err = client.commit(&server, "tx-1").await.unwrap_err();
        assert!(matches!(err, McpError::ProtocolError { .. }));
    }

    #[tokio::test]
    async fn test_mock_client_fail_rollback() {
        let client = MockParticipantClient::new().fail_rollback("s-3");
        let server = make_server("s-3");

        let err = client.rollback(&server, "tx-1").await.unwrap_err();
        assert!(matches!(err, McpError::ProtocolError { .. }));
    }

    #[tokio::test]
    async fn test_mock_client_call_log_records_all_phases() {
        let client = MockParticipantClient::new();
        let server = make_server("s-1");

        client.prepare(&server, "tx-1", "write").await.unwrap();
        client.commit(&server, "tx-1").await.unwrap();
        client.rollback(&server, "tx-1").await.unwrap();

        let log = client.call_log();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].phase, MockPhase::Prepare);
        assert_eq!(log[0].op, Some("write".into()));
        assert_eq!(log[1].phase, MockPhase::Commit);
        assert_eq!(log[1].op, None);
        assert_eq!(log[2].phase, MockPhase::Rollback);
    }

    #[tokio::test]
    async fn test_mock_client_selective_failure() {
        // s-1 成功,s-2 失败
        let client = MockParticipantClient::new().fail_prepare("s-2");
        let s1 = make_server("s-1");
        let s2 = make_server("s-2");

        assert!(client.prepare(&s1, "tx-1", "op").await.is_ok());
        assert!(client.prepare(&s2, "tx-1", "op").await.is_err());
    }

    // === TcpParticipantClient 测试(无真实网络,仅测试错误路径) ===

    #[tokio::test]
    async fn test_tcp_client_connection_refused() {
        // 连接一个未监听的端口(203.0.113.1:1 — TEST-NET-3,不会被 SSRF 拦截,
        // 但实际上无法建立连接,因为 203.0.113.0/24 是文档地址,不会路由)
        // 注意:此测试在 CI 中可能因网络栈差异而表现不同,但连接失败是预期行为
        let client = TcpParticipantClient::with_phase_timeout(100);
        let server = MeshServer::new("s-1", "203.0.113.1:1", vec![]);

        let result = client.prepare(&server, "tx-1", "op").await;
        assert!(result.is_err());
        // 应该是 NetworkError(连接失败或超时)
        match result.unwrap_err() {
            McpError::NetworkError { server_id, .. } => {
                assert_eq!(server_id, "s-1");
            }
            McpError::ProtocolError { .. } => {
                // 某些平台可能在连接阶段返回协议错误(不太可能但允许)
            }
            other => panic!("期望 NetworkError,得到 {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_tcp_client_with_real_echo_server() {
        // 启动一个本地 TCP 服务器,模拟 2PC 参与者
        // 使用 127.0.0.1 绕过 SSRF 校验(localhost 在 SSRF 黑名单中,
        // 但此处直接测试 TcpParticipantClient,不经 ServerRegistry)
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 失败");
        let addr = listener.local_addr().expect("获取地址失败");
        let port = addr.port();

        // 后台任务:接受连接,读取请求,返回 Ack
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        // 读取长度前缀
                        let mut len_buf = [0u8; 4];
                        if sock.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut data = vec![0u8; len];
                        if sock.read_exact(&mut data).await.is_err() {
                            return;
                        }
                        // 返回 Ack
                        let ack = serde_json::to_vec(&TwoPcResponse::Ack).unwrap();
                        let ack_len = ack.len() as u32;
                        let _ = sock.write_all(&ack_len.to_be_bytes()).await;
                        let _ = sock.write_all(&ack).await;
                    });
                }
            }
        });

        // 使用 TcpParticipantClient 发送 prepare(绕过 SSRF 校验,直接构造 MeshServer)
        let client = TcpParticipantClient::with_phase_timeout(500);
        // 注意:这里直接构造 MeshServer 绕过 register 的 SSRF 校验,
        // 因为 127.0.0.1 在 SSRF 黑名单中。这是测试专用,生产环境不应绕过校验。
        let server = MeshServer {
            server_id: "test-echo".into(),
            endpoint: format!("127.0.0.1:{port}"),
            capabilities: vec![],
            last_heartbeat: chrono::Utc::now(),
        };

        let result = client.prepare(&server, "tx-echo-1", "test-op").await;
        assert!(result.is_ok(), "prepare 应成功: {result:?}");

        let result = client.commit(&server, "tx-echo-1").await;
        assert!(result.is_ok(), "commit 应成功: {result:?}");

        let result = client.rollback(&server, "tx-echo-1").await;
        assert!(result.is_ok(), "rollback 应成功: {result:?}");
    }

    #[tokio::test]
    async fn test_tcp_client_nack_response() {
        // 测试参与者返回 Nack 时的处理
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 失败");
        let addr = listener.local_addr().expect("获取地址失败");
        let port = addr.port();

        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut len_buf = [0u8; 4];
                        if sock.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut data = vec![0u8; len];
                        if sock.read_exact(&mut data).await.is_err() {
                            return;
                        }
                        // 返回 Nack
                        let nack = serde_json::to_vec(&TwoPcResponse::Nack {
                            reason: "resource locked".into(),
                        })
                        .unwrap();
                        let nack_len = nack.len() as u32;
                        let _ = sock.write_all(&nack_len.to_be_bytes()).await;
                        let _ = sock.write_all(&nack).await;
                    });
                }
            }
        });

        let client = TcpParticipantClient::with_phase_timeout(500);
        let server = MeshServer {
            server_id: "test-nack".into(),
            endpoint: format!("127.0.0.1:{port}"),
            capabilities: vec![],
            last_heartbeat: chrono::Utc::now(),
        };

        let err = client
            .prepare(&server, "tx-nack-1", "op")
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::ProtocolError { .. }));
        if let McpError::ProtocolError { reason, .. } = err {
            assert!(reason.contains("Nack"));
            assert!(reason.contains("resource locked"));
        }
    }

    #[tokio::test]
    async fn test_tcp_client_phase_timeout() {
        // 测试单阶段超时:参与者接受连接但不响应
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 失败");
        let addr = listener.local_addr().expect("获取地址失败");
        let port = addr.port();

        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        // 读取请求但不响应
                        let mut len_buf = [0u8; 4];
                        let _ = sock.read_exact(&mut len_buf).await;
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut data = vec![0u8; len];
                        let _ = sock.read_exact(&mut data).await;
                        // 故意不响应,让调用方超时
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    });
                }
            }
        });

        let client = TcpParticipantClient::with_phase_timeout(50);
        let server = MeshServer {
            server_id: "test-slow".into(),
            endpoint: format!("127.0.0.1:{port}"),
            capabilities: vec![],
            last_heartbeat: chrono::Utc::now(),
        };

        let start = std::time::Instant::now();
        let err = client
            .prepare(&server, "tx-slow-1", "op")
            .await
            .unwrap_err();
        let elapsed = start.elapsed();

        assert!(matches!(err, McpError::NetworkError { .. }));
        // 应在 ~50ms 超时(允许一些余量)
        assert!(
            elapsed < Duration::from_millis(500),
            "超时应快速返回,实际 {:?}",
            elapsed
        );
    }

    // === Arc<dyn ParticipantClient> 兼容性测试 ===

    #[tokio::test]
    async fn test_dyn_participant_client_works() {
        // 验证 Arc<dyn ParticipantClient> 可以正常使用
        // 这是 McpMesh 将持有的字段类型
        let client: Arc<dyn ParticipantClient> = Arc::new(InProcessClient::new());
        let server = make_server("s-1");

        let result = client.prepare(&server, "tx-1", "op").await;
        assert!(result.is_ok());

        let result = client.commit(&server, "tx-1").await;
        assert!(result.is_ok());

        let result = client.rollback(&server, "tx-1").await;
        assert!(result.is_ok());
    }
}
