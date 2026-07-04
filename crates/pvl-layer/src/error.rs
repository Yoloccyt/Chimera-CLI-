//! PVL 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 PVL 在生产验证闭环中可能遇到的所有失败场景。

use thiserror::Error;

/// PVL 执行错误枚举
///
/// 每个变体对应一种生产验证失败模式。
/// 所有错误均不可恢复(需调用者决定重试或降级)。
///
/// # 对应尸检教训
/// Claude Code 5.4% 孤儿调用(void Promise 无 await)的根因之一是
/// 通道关闭未被正确处理。PVL 通过 `ChannelClosed` 变体显式暴露此场景,
/// 使调用者能感知 Producer/Verifier 通道断开并采取恢复措施。
#[derive(Debug, Clone, Error)]
pub enum PvlError {
    /// 通道关闭:Producer→Verifier 或 Verifier→Feedback 通道已断开。
    ///
    /// 对应尸检教训:通道关闭未被处理导致操作丢失。
    /// PVL 通过显式错误使调用者能感知通道状态并恢复。
    #[error("通道已关闭")]
    ChannelClosed,

    /// 验证失败:Verifier 在验证过程中遇到错误(非操作被拒绝)。
    ///
    /// 区别于操作被拒绝(正常验证结果),此变体表示验证过程本身出错
    /// (如验证规则加载失败、内部状态异常)。
    #[error("验证失败: {reason}")]
    VerificationFailed {
        /// 失败原因(人类可读描述)
        reason: String,
    },

    /// 策略调整失败:FeedbackChannel 在调整 Producer 策略时出错。
    ///
    /// 可能原因:Producer 策略锁获取失败、事件发布失败等。
    #[error("策略调整失败: {reason}")]
    StrategyAdjustmentFailed {
        /// 失败原因
        reason: String,
    },

    /// 生产失败:Producer 在生成操作时出错。
    ///
    /// 可能原因:通道发送失败、内容生成异常等。
    #[error("生产失败: {reason}")]
    ProduceFailed {
        /// 失败原因
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_channel_closed() {
        let e = PvlError::ChannelClosed;
        let msg = format!("{e}");
        assert!(msg.contains("通道已关闭"));
    }

    #[test]
    fn test_error_display_verification_failed() {
        let e = PvlError::VerificationFailed {
            reason: "规则加载失败".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("验证失败"));
        assert!(msg.contains("规则加载失败"));
    }

    #[test]
    fn test_error_display_strategy_adjustment_failed() {
        let e = PvlError::StrategyAdjustmentFailed {
            reason: "锁竞争超时".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("策略调整失败"));
        assert!(msg.contains("锁竞争超时"));
    }

    #[test]
    fn test_error_display_produce_failed() {
        let e = PvlError::ProduceFailed {
            reason: "通道发送失败".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("生产失败"));
        assert!(msg.contains("通道发送失败"));
    }

    #[test]
    fn test_error_clone() {
        // 部分场景需要 Clone(如错误收集)
        let e = PvlError::VerificationFailed {
            reason: "test".into(),
        };
        let _cloned = e.clone();
    }
}
