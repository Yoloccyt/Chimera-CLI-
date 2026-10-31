//! LLM 工具循环（Phase 7，ADR-232）
//!
//! 对应架构层: L10 Interface（chimera-cli）
//!
//! 默认关闭：只有 `chimera run --llm` 或 `CHIMERA_LLM_LOOP=apply` 才进入本模块；
//! 旧 `run` / `chat` / TUI `agent.chat` 路径逐位不变（见 `llm_off_legacy` 测试）。
//!
//! - [`client`] —— `LlmClient` 契约与循环数据类型
//! - [`tools`] —— `ToolDescriptor` → `ToolDecl`（模型可见面）
//! - [`approval`] —— 写工具准入闸门（三档策略）
//! - [`tool_loop`] —— 多轮闭环本体
//! - [`mock`] —— 脚本化模型（无网络、无 API key）
//! - [`mca_client`] —— `--features mca` 下的真实厂商适配（只做装配，不发网络）

pub mod approval;
pub mod client;
pub mod mock;
pub mod tool_loop;
pub mod tools;

#[cfg(feature = "mca")]
pub mod mca_client;

pub use approval::{ApproveAll, CliApproval, DenyWrites, ToolApproval};
pub use client::{LlmClient, LlmError, LlmLoopError, LlmLoopOutcome, LlmLoopStop};
pub use mock::MockLlmClient;
pub use tool_loop::{LlmToolLoop, LlmToolLoopConfig, ASK_REQUIRED_MARKER};
pub use tools::{tool_decls, FALLBACK_OBJECT_SCHEMA};
