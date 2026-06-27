//! GQEP 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 GQEP 在聚集执行生命周期中可能遇到的所有失败场景。

use thiserror::Error;

/// GQEP 执行错误枚举
///
/// 每个变体对应一种聚集执行失败模式,所有错误均不可恢复
/// (需调用者决定重试或降级)。
///
/// # 对应尸检教训
/// Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因是:
/// - 异步操作 spawn 后,JoinHandle 未被 await
/// - future 被 drop 但无运行时检测
/// - 无超时控制导致永久挂起
///
/// GQEP 通过 `OperationTimeout` 与 `OrphanCallDetected` 两个变体
/// 显式暴露这些问题,从机制上杜绝此类隐患。
#[derive(Debug, Clone, Error)]
pub enum GqepError {
    /// 操作超时:单个操作在超时窗口内未完成。
    ///
    /// 对应尸检教训:5.4% 孤儿调用中,部分源于无超时控制导致永久挂起。
    /// GQEP 通过 `tokio::time::timeout` 强制每个操作有超时上限。
    #[error("操作超时: operation_id={operation_id}, timeout_ms={timeout_ms}")]
    OperationTimeout {
        /// 超时操作 ID
        operation_id: String,
        /// 超时阈值(毫秒)
        timeout_ms: u64,
    },

    /// 操作失败:future 返回错误或执行异常。
    ///
    /// 区别于 `OperationTimeout`(超时)与 `OrphanCallDetected`(孤儿),
    /// 此变体表示 future 正常完成但返回错误。
    #[error("操作失败: operation_id={operation_id}, reason={reason}")]
    OperationFailed {
        /// 失败操作 ID
        operation_id: String,
        /// 失败原因(人类可读描述)
        reason: String,
    },

    /// 批量原子性失败:批量操作中某个失败,触发回滚。
    ///
    /// `failed_index` 标识失败操作在批次中的位置,调用者据此决定
    /// 是否重试或降级。回滚操作本身也经 GQEP 聚集(避免孤儿调用)。
    #[error("批量原子性失败: failed_index={failed_index}, reason={reason}")]
    BatchAtomicFailure {
        /// 失败操作在批次中的索引(从 0 开始)
        failed_index: usize,
        /// 失败原因
        reason: String,
    },

    /// 孤儿调用检测:future 被 drop 但未完成(void Promise 无 await)。
    ///
    /// 对应 Claude Code 尸检 5.4% 孤儿调用教训。
    /// 此错误由 QEEP `OrphanGuard` 在 future drop 时检测并报告,
    /// GQEP 聚集后通过 `OrphanCallDetected` 事件广播(Critical 级别)。
    #[error("孤儿调用检测: operation_id={operation_id}, spawn_location={spawn_location}")]
    OrphanCallDetected {
        /// 孤儿操作 ID
        operation_id: String,
        /// spawn 位置(文件:行号),用于定位代码中的未 await 调用
        spawn_location: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_timeout() {
        let e = GqepError::OperationTimeout {
            operation_id: "op-1".into(),
            timeout_ms: 5000,
        };
        let msg = format!("{e}");
        assert!(msg.contains("op-1"));
        assert!(msg.contains("5000"));
    }

    #[test]
    fn test_error_display_failed() {
        let e = GqepError::OperationFailed {
            operation_id: "op-2".into(),
            reason: "disk full".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("op-2"));
        assert!(msg.contains("disk full"));
    }

    #[test]
    fn test_error_display_batch() {
        let e = GqepError::BatchAtomicFailure {
            failed_index: 3,
            reason: "rollback needed".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("3"));
        assert!(msg.contains("rollback needed"));
    }

    #[test]
    fn test_error_display_orphan() {
        let e = GqepError::OrphanCallDetected {
            operation_id: "op-4".into(),
            spawn_location: "gatherer.rs:42".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("op-4"));
        assert!(msg.contains("gatherer.rs:42"));
    }

    #[test]
    fn test_error_clone() {
        // GatherResult.errors 需要 Vec<GqepError>,故 GqepError 必须实现 Clone
        let e = GqepError::OperationTimeout {
            operation_id: "op-1".into(),
            timeout_ms: 1000,
        };
        let _cloned = e.clone();
    }
}
