//! 衰减引擎错误类型
//!
//! 遵循 §4.1 规范:库层错误使用 thiserror enum(而非 anyhow),
//! 以便调用方按错误类型精确匹配处理策略。

use thiserror::Error;

/// 衰减引擎错误类型
#[derive(Debug, Error)]
pub enum DecayError {
    /// 能力等级超出 [0.0, 1.0] 范围
    #[error("无效的能力等级 {0}:必须在 [0.0, 1.0] 范围内")]
    InvalidLevel(f32),

    /// 指定 ID 的能力未找到
    #[error("能力未找到: {0}")]
    CapabilityNotFound(String),

    /// 能力已被冻结,无法再次冻结(幂等保护)
    #[error("能力已被冻结: {0}")]
    AlreadyFrozen(String),

    /// 能力未冻结,无法解冻
    #[error("能力未冻结: {0}")]
    NotFrozen(String),

    /// 配置错误(如重复注册、非法配置值)
    #[error("配置错误: {0}")]
    ConfigError(String),
}
