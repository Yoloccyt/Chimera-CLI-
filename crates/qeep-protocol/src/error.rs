//! QEEP 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 QEEP 协议在纠缠调用生命周期中可能遇到的所有失败场景。

use thiserror::Error;

/// QEEP 协议错误枚举
///
/// 每个变体对应一种纠缠调用失败模式,所有错误均不可恢复(需调用者决定重试或降级)。
#[derive(Debug, Clone, Error)]
pub enum QeepError {
    /// 调用超时:future 在 `timeout` 窗口内未完成。
    /// 对应尸检教训:5.4% 孤儿调用中,部分源于无超时控制导致永久挂起。
    #[error("调用超时:future 在超时窗口内未完成")]
    Timeout,

    /// 调用被取消:执行单元主动 abort 或外部 cancel。
    #[error("调用被取消")]
    Cancelled,

    /// 孤儿调用:future 被 drop 但未完成(void Promise 无 await)。
    /// 这是 QEEP 的核心检测目标,对应 Claude Code 尸检中 5.4% 孤儿调用。
    #[error("孤儿调用:future 被 drop 但未完成")]
    Orphaned,

    /// 调用已完成,不能重复操作(防止重复 await 或重复回执)。
    #[error("调用已完成,不能重复操作")]
    AlreadyCompleted,

    /// 缺少确认(Ack):执行单元未在预期时间内回送 Ack。
    #[error("缺少确认(Ack)")]
    AckMissing,

    /// 缺少回执(Receipt):执行单元未在预期时间内回送 Receipt。
    #[error("缺少回执(Receipt)")]
    ReceiptMissing,

    /// 序列化/反序列化失败(用于 Event Bus 广播时的 MessagePack 编解码)。
    #[error("序列化/反序列化失败: {0}")]
    SerializationError(String),
}

// 注:QeepError 实现 Clone,因为 Receipt<T> 需要存储 Result<T, QeepError>。
