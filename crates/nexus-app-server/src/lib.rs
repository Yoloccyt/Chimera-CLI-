//! NEXUS-OMEGA L10 宿主层协议门面 — 核心-表面分离（WI-01）
//!
//! 对应架构层: **L10 Interface**（第 39 crate，v4.0 48/53 预算内）
//! 对应工作项: **WI-01 核心-表面分离：nexus-app-server**（v4.0 §6.1/§6.2/§13）
//! 对应设计源: Codex CLI app-server"核心不知道自己在哪种表面层中运行"；
//!             OpenCode serve/attach；DSH headless 五形态
//!
//! # 核心职责
//!
//! 对外提供**稳定外部协议（JSON-RPC v1）**，对内以 `CoreOp/CoreEvent`
//! 单向驱动核心——实现"核心-表面分离"：
//!
//! ```text
//! TUI/CLI/ACP/未来宿主 → App 协议 → nexus-app-server（每 Thread 一 actor）→ 核心
//! ```
//!
//! # 设计纪律（WI-01 + T6 内闭外开）
//!
//! - **NexusEvent 永不进外部协议**: 内部事件经 EventBus 广播（内闭），
//!   外部只经 AppOp/AppEvent（外开）；转译在 server 层完成
//! - **协议 v1 冻结 ≥3 个月**: 扩展走 `extras` 逃逸舱（L0 app.rs 契约）
//! - **断线恢复**: 客户端持 last_item_id，重连后回放增量（Item 为
//!   最小 I/O 单元，状态机 started → in_progress → completed/failed）
//! - **每 Thread 一 actor**: 会话状态归 actor 独占，外界只经消息交互
//!
//! # 快速示例
//! ```
//! use nexus_app_server::{AppServer, AppServerConfig};
//!
//! let server = AppServer::new(AppServerConfig::default());
//! assert!(server.session_count() == 0);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod protocol;
pub mod server;
pub mod transport;
// P3-T5: 真实核心接入（QuestBackend）+ 审批仲裁 + SSE 传输（T6 遗留三项）
pub mod approval;
pub mod backend;
pub mod sse;
/// P4-T3: SubAgent × Quest 执行引擎组合根（D-P5 遗留接线②）
pub mod subagent_engine;

pub use approval::{ApprovalArbiter, VoteOutcome};
pub use backend::QuestBackend;
pub use protocol::{JsonRpcError, RpcCodec, RpcNotification, RpcRequest, RpcResponse};
pub use server::{AppServer, AppServerConfig, CoreBackend, SessionSnapshot};
pub use sse::{SseConnection, SseError, SseServer};
pub use subagent_engine::SubAgentQuestEngine;
pub use transport::{AppTransport, StdinTransport, TransportError};

/// 预导入模块 — 常用类型便捷导入
pub mod prelude {
    pub use crate::protocol::{JsonRpcError, RpcCodec, RpcNotification, RpcRequest, RpcResponse};
    pub use crate::server::{AppServer, AppServerConfig, CoreBackend, SessionSnapshot};
    pub use crate::transport::{AppTransport, StdinTransport, TransportError};
}
