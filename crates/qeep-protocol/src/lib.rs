//! 量子纠缠执行协议 — 跨执行单元的零孤儿结果汇聚协议
//!
//! 对应架构层:L4 Security
//! 对应创新点:QEEP(Quantum-Entangled Execution Protocol)
//!
//! ## 核心机制
//!
//! 请求(Request) → 确认(Ack) → 回执(Receipt)三元组
//! 每个 async 操作必须经 `entangle()` 包裹,超时未回执则告警 + 重试。
//!
//! ## 对应尸检教训
//!
//! Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
//! - 异步操作 spawn 后,JoinHandle 未被 await
//! - future 被 drop 但无运行时检测
//!
//! QEEP 通过 `OrphanGuard` + `Drop` trait 从机制上杜绝此类问题。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod detector;
pub mod error;
pub mod protocol;
pub mod types;

pub use detector::OrphanDetector;
pub use error::QeepError;
pub use protocol::QeepProtocol;
pub use types::{Ack, CallState, EntangledCall, EntangledCallId, OrphanReport, Receipt, Request};

use std::time::Duration;

/// 默认超时时间:30 秒
///
/// 对应 QEEP 覆盖的 14 个 async 点(12 个 async 操作 + 自身 + Event Bus 广播),
/// 30 秒足够覆盖大多数 L4-L10 层的异步操作(如沙箱执行、模型调用、Wiki 沉淀)。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
