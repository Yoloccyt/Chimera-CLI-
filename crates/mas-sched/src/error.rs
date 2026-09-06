//! 调度器错误 — 库层 thiserror 标准（§4.1）

use crate::types::TaskId;
use thiserror::Error;

/// mas-sched 错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchedError {
    /// 任务不存在（claim 前 renew/handoff/should_run）
    #[error("task not found: {0}")]
    TaskNotFound(TaskId),
    /// 租约已存在（重复 claim）
    #[error("task already claimed: {0}")]
    AlreadyClaimed(TaskId),
    /// 租约归属错误（handoff 目标不符 / renew 主体不符）
    #[error("lease holder mismatch: task={0} holder={1} caller={2}")]
    LeaseHolderMismatch(TaskId, String, String),
    /// 影子日志序列化失败
    #[error("shadow log encode/decode error: {0}")]
    ShadowLog(String),
}
