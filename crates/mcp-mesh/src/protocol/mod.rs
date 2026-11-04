//! MCP（Model Context Protocol）语义层 —— JSON-RPC 2.0 编解码 + 传输接缝。
//!
//! # 与 mesh 自带协议的关系（注意别混）
//! `crate::quantum::participant_client` 里的 4 字节长度前缀 JSON 是**本项目自研的
//! 2PC/查询/工具调用协议**，不是 MCP。MCP 的 stdio 传输用**换行分隔 JSON**，
//! 两者的帧格式、消息形状与语义都不同，故本模块独立成层、不改造既有 2PC 路径
//! （ADR-235 D3）。共用点只有 `serde_json`。
//!
//! # 版本
//! 客户端请求 `MCP_PROTOCOL_VERSION`；服务器回不同版本时，只有落在
//! [`SUPPORTED_PROTOCOL_VERSIONS`] 内才接受，否则
//! [`crate::McpError::UnsupportedVersion`]（fail-closed，不降级猜测）。
//!
//! # 零新事件
//! 本层失败只通过 `McpError` 与 `tracing` 表达，不发布 `NexusEvent`
//! （ADR-229:70 / ADR-230:41 / ADR-223 七点位同批纪律）。

#![forbid(unsafe_code)]

pub mod jsonrpc;
pub mod transport;

pub use jsonrpc::{
    decode_incoming, encode_frame, Incoming, Notification, Request, RequestId, Response, RpcError,
};
pub use transport::{DuplexTransport, McpTransport, MAX_FRAME_BYTES};

/// 客户端在 `initialize` 里请求的协议版本。
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// 可接受的服务器回包版本集合（当前只钉一个；扩版本要同时扩解析与测试）。
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26"];

/// `clientInfo.name` —— 服务器侧日志里能认出调用方。
pub const CLIENT_NAME: &str = "chimera-cli";

/// `clientInfo.version` —— 跟 crate 版本走，发布时自动同步。
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
