//! MCP Mesh 协议适配层
//!
//! 提供 JSON-RPC 2.0 协议实现,将 2PC 事务与超位置查询的网络交互标准化。
//!
//! ## 模块结构
//! - `json_rpc`: JSON-RPC 2.0 请求/响应类型与 HTTP 传输客户端

pub mod json_rpc;
