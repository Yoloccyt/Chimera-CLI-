//! 聚集查询执行协议 — 并发异步操作的聚集汇聚与超时治理
//!
//! 对应架构层:L7 Execution
//! 对应创新点:GQEP(Gather-Query Execution Protocol)
//!
//! ## 核心职责
//! - 使用 `FuturesUnordered` 流式聚集并发异步操作(对应 A.2 设计决策)
//! - 双层超时治理,杜绝永久挂起(对应尸检教训:5.4% 孤儿调用):
//!   - **单操作超时**:`entangle` 内部 `tokio::time::timeout` 包裹每个 future
//!     (阈值 `GqepConfig.default_timeout_ms`),超时返回 `OperationTimeout`
//!   - **全局超时**:整个 `stream.next()` 循环用 `tokio::time::timeout` 包裹
//!     (阈值 `GqepConfig.gather_deadline_ms`),超时返回 `GlobalTimedOut` 并
//!     发布 `GatherTimedOut` 事件;`0` 禁用(向后兼容)。WHY 全局超时:大规模
//!     gather 时单操作超时累积可能导致整体执行时间失控,全局 deadline 为整批兜底
//! - 批量原子性保证:任一失败触发回滚,回滚本身也经 GQEP 聚集
//! - 集成 QEEP `OrphanDetector`,检测孤儿调用并发布 Critical 事件
//! - Phase 7 D-6 占位治理:`timeout_stats()` 从恒零占位改为双层超时真实
//!   计数（计数点收敛在 with_timeout 超时分支 + collect_with_deadline 全局分支）
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

use std::sync::atomic::Ordering;

pub mod batch;
pub mod config;
/// P2-T11: 神经符号一致性守护（v4.0 WI-27）
///
/// Invariant trait + ProjectCompilesInvariant（写文件后 cargo check,大仓库
/// 降级局部 check）;奖励信号映射（+3.0/−1.0）供 WI-30 Shadow 消费。
pub mod consistency_guardian;
pub mod error;
/// P4-T3: ToolExecutor × execpolicy 审批流水线接线（WI-16 安全不变量兑现）
pub mod exec_tool_executor;
pub mod gatherer;
/// P3-T7: 流式期间启动工具（v4.0 WI-17,增量解析 + 闭合即启动）
pub mod streaming_dispatch;
pub mod timeout;
/// P3-T8: 声明式 ToolPlan 解释执行（v4.0 WI-16,PTC 计划批编排）
pub mod toolplan_runner;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use batch::RollbackFn;
pub use config::GqepConfig;
pub use error::GqepError;
pub use gatherer::GqepExecutor;
pub use timeout::with_timeout;
// P3-T7: 流式派发公开 API（WI-17）
pub use streaming_dispatch::{DispatchOutcome, DispatchedCall, SideEffect, StreamingDispatcher};
// P3-T8: PTC 计划批编排公开 API（WI-16）
pub use toolplan_runner::{PlanGuards, PlanRunner, PlanSummary, ToolExecutor};
// P4-T3: execpolicy 接线公开 API
pub use exec_tool_executor::ExecPolicyToolExecutor;
pub use types::{GatherResult, GqepFuture, OperationId};

/// 双层超时防护统计快照（Task 3.7:L10 → L7 向下依赖）
///
/// 为 TUI MetricsDashboard 面板提供无需异步上下文的静态快照，
/// 展示 GQEP 双层超时治理（单操作超时 + 全局 gather 超时）的累计统计。
/// 真实 GQEP 数据由 `GqepExecutor::gather` 运行时动态更新，
/// 本函数为 TUI 面板提供占位快照，避免面板渲染阻塞。
///
/// TODO: v3.x 接入 RuntimeAuditor 实时采集后替换为真实 GQEP 统计。
#[derive(Debug, Clone, Copy)]
pub struct TimeoutStats {
    /// 单操作超时累计次数
    pub per_op_timeouts: u64,
    /// 全局 gather 超时累计次数
    pub global_timeouts: u64,
    /// 双层超时防护覆盖率（已防护操作数 / 总操作数）
    pub coverage: f32,
}

/// 返回 GQEP 双层超时防护统计的真实快照（Phase 7 D-6 占位治理）
///
/// 原恒零占位已替换为真实计数——计数点收敛在双层超时统一入口：
/// - 单操作超时: `timeout::with_timeout` 超时分支经 [`record_per_op_timeout`]
/// - 全局超时: `gatherer::collect_with_deadline` 超时分支经 [`record_global_timeout`]
///
/// coverage 语义: 经 `with_timeout` 包装且启用超时（timeout_ms > 0）的操作
/// 占比；零调用时返回 1.0（gather 路径恒受 entangle 单层超时保护）。
pub fn timeout_stats() -> TimeoutStats {
    let per_op = TIMEOUT_COUNTERS.per_op_timeouts.load(Ordering::Relaxed);
    let global = TIMEOUT_COUNTERS.global_timeouts.load(Ordering::Relaxed);
    let total_calls = TIMEOUT_COUNTERS.with_timeout_calls.load(Ordering::Relaxed);
    let protected = TIMEOUT_COUNTERS.protected_ops.load(Ordering::Relaxed);
    let coverage = if total_calls == 0 {
        1.0
    } else {
        protected as f32 / total_calls as f32
    };
    TimeoutStats {
        per_op_timeouts: per_op,
        global_timeouts: global,
        coverage,
    }
}

/// 双层超时真实计数器（Phase 7 D-6：全局函数无实例状态，静态计数器提供真实数据源）
static TIMEOUT_COUNTERS: TimeoutCounters = TimeoutCounters::new();

/// 双层超时计数器组（Relaxed 即可：统计计数无顺序依赖）
struct TimeoutCounters {
    /// 单操作超时累计次数
    per_op_timeouts: std::sync::atomic::AtomicU64,
    /// 全局 gather 超时累计次数
    global_timeouts: std::sync::atomic::AtomicU64,
    /// with_timeout 调用总次数（coverage 分母）
    with_timeout_calls: std::sync::atomic::AtomicU64,
    /// 启用超时的受保护操作数（coverage 分子）
    protected_ops: std::sync::atomic::AtomicU64,
}

impl TimeoutCounters {
    const fn new() -> Self {
        Self {
            per_op_timeouts: std::sync::atomic::AtomicU64::new(0),
            global_timeouts: std::sync::atomic::AtomicU64::new(0),
            with_timeout_calls: std::sync::atomic::AtomicU64::new(0),
            protected_ops: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

/// 记录一次单操作超时（Phase 7 D-6：timeout.rs 超时分支计数点）
pub fn record_per_op_timeout() {
    TIMEOUT_COUNTERS
        .per_op_timeouts
        .fetch_add(1, Ordering::Relaxed);
}

/// 记录一次全局 gather 超时（Phase 7 D-6：gatherer.rs 超时分支计数点）
pub fn record_global_timeout() {
    TIMEOUT_COUNTERS
        .global_timeouts
        .fetch_add(1, Ordering::Relaxed);
}

/// 记录一次 with_timeout 包装调用（Phase 7 D-6：coverage 统计）
pub(crate) fn record_with_timeout_call(timeout_enabled: bool) {
    TIMEOUT_COUNTERS
        .with_timeout_calls
        .fetch_add(1, Ordering::Relaxed);
    if timeout_enabled {
        TIMEOUT_COUNTERS
            .protected_ops
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::batch::RollbackFn;
    pub use crate::config::GqepConfig;
    pub use crate::error::GqepError;
    pub use crate::gatherer::GqepExecutor;
    pub use crate::timeout::with_timeout;
    pub use crate::types::{GatherResult, GqepFuture, OperationId};
}
