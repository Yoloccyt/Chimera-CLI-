//! 在线学习框架错误类型

use thiserror::Error;

/// 在线学习框架错误枚举
#[derive(Debug, Error)]
pub enum LearningError {
    /// 参数未找到
    #[error("parameter not found: {0}")]
    ParameterNotFound(String),

    /// 参数类型不匹配
    #[error("parameter type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// 期望类型
        expected: String,
        /// 实际类型
        actual: String,
    },

    /// 序列化/反序列化失败
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// 参数更新失败
    #[error("parameter update failed: {0}")]
    UpdateFailed(String),
}

impl From<serde_json::Error> for LearningError {
    fn from(e: serde_json::Error) -> Self {
        Self::SerializationError(e.to_string())
    }
}
