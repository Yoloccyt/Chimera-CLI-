//! 衰减引擎错误类型
//!
//! 遵循 §4.1 规范:库层错误使用 thiserror enum(而非 anyhow),
//! 以便调用方按错误类型精确匹配处理策略。
//!
//! # P4-W14.5 扩展
//!
//! 新增 3 个 CapabilityToken 相关错误变体:
//! - `TokenNotFound`: 接缝未注册 token
//! - `TokenFrozen`: token 已冻结,无法操作
//! - `CooldownActive`: token 处于冷却期,无法操作

use nexus_contracts::SeamId;
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

    /// 能力令牌未找到 — 指定接缝未注册 token（P4-W14.5）
    ///
    /// WHY 单独错误而非复用 CapabilityNotFound: token 操作语义独立,
    /// 调用方需要区分"能力未注册"与"令牌未注册"
    #[error("能力令牌未找到: 接缝 {0:?}")]
    TokenNotFound(SeamId),

    /// 能力令牌已冻结,无法提升或操作（P4-W14.5）
    ///
    /// 触发场景:
    /// - `maybe_promote_token` 在 Frozen 状态下调用
    /// - `record_token_outcome` 在 Frozen 状态下调用
    #[error("能力令牌已冻结: 接缝 {0:?}")]
    TokenFrozen(SeamId),

    /// 能力令牌处于冷却期,无法提升（P4-W14.5）
    ///
    /// 触发场景:
    /// - `maybe_promote_token` 在 Cooldown 状态下调用
    #[error("能力令牌处于冷却期: 接缝 {0:?}")]
    CooldownActive(SeamId),
}
