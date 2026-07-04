//! 能力衰减引擎 — 连续 [0.0, 1.0] 权限流体衰减模型
//!
//! 对应架构层:L4 Security
//! 对应 ADR-002:能力衰减模型设计(连续权限流体)
//! 对应尸检教训:Claude 安全权限提升(权限不应离散 0/1)
//!
//! 双驱动衰减:
//! - 时间驱动:随时间自然递减(防止权限长期闲置累积)
//! - 事件驱动:违规事件触发惩罚性衰减
//!
//! 冻结/解冻 API 对应 Skeptic 否决权(Week 5 Parliament 实现)

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod engine;
pub mod error;
pub mod types;

pub use engine::DecayEngine;
pub use error::DecayError;
pub use types::{Capability, CapabilityLevel, DecayConfig, DecayEvent};

/// 默认衰减配置
///
/// 生产推荐值:
/// - time_decay_rate: 0.001(每秒衰减 0.1%)
/// - event_decay_penalty: 0.1(标准违规惩罚)
/// - freeze_threshold: 0.05(5% 以下自动冻结)
pub fn default_config() -> DecayConfig {
    DecayConfig::default()
}
