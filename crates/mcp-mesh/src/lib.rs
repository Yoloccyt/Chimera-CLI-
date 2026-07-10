//! MCP 量子网格 — Model Context Protocol 的量子化网格通信层
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(跨进程通信唯一通道,符合 §2.2 依赖铁律)
//!
//! ## 核心机制
//! - **量子事务(Quantum Transaction)**:2PC JSON-RPC 实现,跨多服务器原子提交
//!   - Prepare/Commit/Rollback 阶段通过 `JsonRpcClient` 发送真实 HTTP JSON-RPC 2.0 请求
//!   - Mock 模式(默认):跳过网络,直接返回成功,用于 CI/测试
//! - **超位置查询(Superposition Query)**:并发 fanout 至多服务器,聚合结果
//! - **纠缠链接(Entanglement Link)**:服务器间状态同步策略(Eager/Lazy/BestEffort)
//! - **服务器注册与心跳**:DashMap-based 注册表,周期性探活
//! - 通过 EventBus 发布 `McpMeshTransactionCompleted`,订阅 `ChtcToolCallReceived`
//!
//! ## 依赖方向铁律
//! 仅依赖 L1 Core(`event-bus`),不依赖 L2-L9 任何 crate。
//!
//! ## 快速示例
//! ```no_run
//! use mcp_mesh::{McpMesh, MeshConfig, MeshServer};
//!
//! # async fn run() {
//! let mesh = McpMesh::new(MeshConfig::default());
//! mesh.register_server(MeshServer::new("s-1", "203.0.113.1:8080", vec!["tool-a".into()])).unwrap();
//!
//! let result = mesh.execute_transaction(vec!["s-1".into()], "query".into()).await.unwrap();
//! assert!(result.success);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod mesh;
pub mod protocol;
pub mod quantum;
pub mod server_registry;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::MeshConfig;
pub use error::McpError;
pub use mesh::McpMesh;
pub use protocol::json_rpc::{
    JsonRpcClient, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
};
pub use quantum::entanglement::{EntanglementLink, EntanglementManager, SyncStrategy};
pub use quantum::superposition::{QueryResult, SuperpositionQuery};
pub use quantum::transaction::{QuantumTransaction, TransactionState};
pub use server_registry::{MeshServer, ServerRegistry};
pub use types::TransactionResult;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::MeshConfig;
    pub use crate::error::McpError;
    pub use crate::mesh::McpMesh;
    pub use crate::protocol::json_rpc::{
        JsonRpcClient, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    };
    pub use crate::quantum::entanglement::{EntanglementLink, EntanglementManager, SyncStrategy};
    pub use crate::quantum::superposition::{QueryResult, SuperpositionQuery};
    pub use crate::quantum::transaction::{QuantumTransaction, TransactionState};
    pub use crate::server_registry::{MeshServer, ServerRegistry};
    pub use crate::types::TransactionResult;
}
