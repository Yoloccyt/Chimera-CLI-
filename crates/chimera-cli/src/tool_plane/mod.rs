//! L10 tool-plane adapters (providers + env configuration).
//!
//! The execution runtime itself lives in L7 `gqep-executor::tool_plane`; this
//! module only supplies concrete providers so L7 never depends on L10 crates.

pub mod config;
pub mod echo;
pub mod mcp;
pub mod process;

pub use config::{ToolPlaneConfig, ToolPlaneMode};
pub use echo::EchoToolProvider;
pub use mcp::McpToolProvider;
pub use process::ProcessToolProvider;
