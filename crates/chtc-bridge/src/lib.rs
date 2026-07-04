//! 跨平台工具兼容桥 — 5 大 IDE 的工具调用兼容适配层
//!
//! 对应架构层:L10 Interface
//! 对应创新点:CHTC(Cross-Harness Tool Compatibility,设计来源 Qwen 3.7 + ADR-020)
//!
//! # 核心机制
//! - 5 大 IDE 适配器(VSCode/IntelliJ/Vim/Emacs/Zed),使用 enum dispatch 静态分发
//! - 统一工具调用协议(UnifiedToolCall),归一化异构 IDE 原生格式
//! - 通过 EventBus 发布 `ChtcToolCallReceived` 事件,实现 L10→下层解耦(§2.2)
//!
//! # 架构约束
//! 本 crate 仅依赖 L1(event-bus、nexus-core),不直接依赖 L2-L9 任何 crate。
//! 跨层通信只走 EventBus,违反此约束需有 ADR 记录特批。
//!
//! # 快速示例
//! ```
//! use chtc_bridge::{ChtcBridge, ChtcConfig, IdeSource};
//! use serde_json::json;
//!
//! let bridge = ChtcBridge::new(ChtcConfig::default());
//! let call = bridge.receive(json!({ "command": "open", "args": {} }), IdeSource::vscode())?;
//! let result = bridge.execute(&call)?;
//! assert!(result.success);
//! # Ok::<(), chtc_bridge::ChtcError>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod adapters;
pub mod bridge;
pub mod config;
pub mod error;
pub mod protocol;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use adapters::{IdeAdapter, IdeAdapterKind};
pub use bridge::ChtcBridge;
pub use config::ChtcConfig;
pub use error::ChtcError;
pub use protocol::ProtocolConverter;
pub use types::{IdeSource, ToolCallResult, UnifiedToolCall};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::adapters::IdeAdapterKind;
    pub use crate::bridge::ChtcBridge;
    pub use crate::config::ChtcConfig;
    pub use crate::error::ChtcError;
    pub use crate::protocol::ProtocolConverter;
    pub use crate::types::{IdeSource, ToolCallResult, UnifiedToolCall};
}
