//! LSCT 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L3 Storage
//! 设计依据:§4.1 库层用自定义 thiserror enum

use thiserror::Error;

/// LSCT 错误类型
///
/// 覆盖三类失败场景:能力未注册、层级路径非法、配置阈值冲突。
#[derive(Debug, Error)]
pub enum LsctError {
    /// 能力未找到:指定 capability_id 未在 LSCT 注册
    #[error("能力未找到: {capability_id}")]
    CapabilityNotFound {
        /// 未找到的能力 ID
        capability_id: String,
    },

    /// 无效层级:升降温路径非法(跨级跳跃、方向错误、源目标相同)
    #[error("无效层级: {reason}")]
    InvalidTier {
        /// 错误原因
        reason: String,
    },

    /// 配置错误:阈值非法(promotion <= demotion 或超出 [0.0, 1.0])
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因
        reason: String,
    },
}
