//! 聚集查询执行协议 — 并发异步操作的聚集汇聚与超时治理
//!
//! 对应架构层:L6 Router
//! 对应创新点:GQEP(Gather-Query Execution Protocol)
//!
//! ## 核心职责
//! - 使用 `FuturesUnordered` 流式聚集并发异步操作(对应 A.2 设计决策)
//! - 单操作超时 + 全局超时治理,杜绝永久挂起(对应尸检教训:5.4% 孤儿调用)
//! - 批量原子性保证:任一失败触发回滚,回滚本身也经 GQEP 聚集
//! - 集成 QEEP `OrphanDetector`,检测孤儿调用并发布 Critical 事件
//!
//! ## 对应尸检教训
//! Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
//! - 异步操作 spawn 后,JoinHandle 未被 await
//! - future 被 drop 但无运行时检测
//!
//! GQEP 通过 `FuturesUnordered` 强制聚集所有 future 的结果,
//! 并集成 QEEP `OrphanGuard` 从机制上杜绝此类问题:
//! 每个 future 经 `QeepProtocol::entangle` 包裹,drop 时若未完成则报告孤儿。
//!
//! ## 快速示例
//! ```no_run
//! use gqep_executor::{GqepConfig, GqepExecutor, GqepFuture, GqepError};
//! use event_bus::EventBus;
//!
//! # async fn run() {
//! let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
//! let futures: Vec<GqepFuture<String>> = vec![
//!     Box::pin(async { Ok("op-1".to_string()) }),
//!     Box::pin(async { Ok("op-2".to_string()) }),
//! ];
//! let result = executor.gather(futures).await;
//! assert_eq!(result.succeeded, 2);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod batch;
pub mod config;
pub mod error;
pub mod gatherer;
pub mod timeout;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use batch::RollbackFn;
pub use config::GqepConfig;
pub use error::GqepError;
pub use gatherer::GqepExecutor;
pub use timeout::with_timeout;
pub use types::{GatherResult, GqepFuture, OperationId};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::batch::RollbackFn;
    pub use crate::config::GqepConfig;
    pub use crate::error::GqepError;
    pub use crate::gatherer::GqepExecutor;
    pub use crate::timeout::with_timeout;
    pub use crate::types::{GatherResult, GqepFuture, OperationId};
}
