//! L10 tool-plane adapters (providers + env configuration).
//!
//! The execution runtime itself lives in L7 `gqep-executor::tool_plane`; this
//! module only supplies concrete providers so L7 never depends on L10 crates.

pub mod config;
pub mod echo;
pub mod fs_tools;
pub mod mcp;
pub mod mcp_connect;
pub mod process;
pub mod unified_adapter;

pub use config::{ToolPlaneConfig, ToolPlaneMode};
pub use echo::EchoToolProvider;
pub use fs_tools::FsToolsProvider;
pub use mcp::McpToolProvider;
pub use mcp_connect::{assemble as assemble_mcp, FenceGate, McpPlane};
pub use process::ProcessToolProvider;
pub use unified_adapter::UnifiedExecutorAdapter;
