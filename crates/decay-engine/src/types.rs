//! 衰减引擎核心类型定义
//!
//! 连续 [0.0, 1.0] 权限流体模型(非离散 0/1)
//! 对应 ADR-002:能力衰减模型设计
//!
//! 设计动机:Claude Code 尸检发现权限离散 0/1 导致权限提升攻击
//! 连续流体模型使权限可渐进衰减,避免"全有或全无"的安全风险

use std::time::Instant;

use crate::error::DecayError;

/// 连续权限流体值,范围 [0.0, 1.0]
///
/// - 0.0 表示完全冻结(无权限)
/// - 1.0 表示满权限
/// - (0.0, 1.0) 表示部分权限(流体模型,非离散)
///
/// 使用 newtype 包装 f32,确保构造时校验范围,
/// 避免非法值(如负数或 >1.0)进入系统。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapabilityLevel(f32);

impl CapabilityLevel {
    /// 创建新的能力等级,校验范围 [0.0, 1.0]
    ///
    /// # 错误
    /// 返回 [`DecayError::InvalidLevel`] 当 value 超出 [0.0, 1.0]。
    pub fn new(value: f32) -> Result<Self, DecayError> {
        if !(0.0..=1.0).contains(&value) {
            return Err(DecayError::InvalidLevel(value));
        }
        Ok(Self(value))
    }

    /// 获取原始 f32 值
    pub fn value(&self) -> f32 {
        self.0
    }

    /// 是否冻结(level <= 0.0)
    pub fn is_frozen(&self) -> bool {
        self.0 <= 0.0
    }

    /// 是否满权限(level >= 1.0)
    pub fn is_full(&self) -> bool {
        self.0 >= 1.0
    }
}

/// 能力项:单个能力的当前运行时状态
#[derive(Debug, Clone)]
pub struct Capability {
    /// 能力唯一标识(如 "file_write"、"shell_exec")
    pub id: String,
    /// 能力名称(人类可读,用于审计日志)
    pub name: String,
    /// 当前权限流体等级
    pub level: CapabilityLevel,
    /// 是否被显式冻结(对应 Skeptic 否决权)
    pub frozen: bool,
    /// 上次衰减时间戳(用于计算时间驱动衰减的 elapsed)
    pub last_decay_at: Instant,
}

/// 衰减配置
#[derive(Debug, Clone)]
pub struct DecayConfig {
    /// 时间驱动衰减速率(每秒衰减比例)
    /// 如 0.001 表示每秒减 0.1%,生产环境推荐值
    pub time_decay_rate: f32,
    /// 违规事件衰减惩罚基数
    /// 实际惩罚 = penalty × severity
    pub event_decay_penalty: f32,
    /// 最低权限下限(衰减不会低于此值,除非冻结)
    pub min_level: f32,
    /// 自动冻结阈值(低于此值自动冻结,防止权限过低仍可操作)
    pub freeze_threshold: f32,
    /// 恢复速率(每秒恢复比例,解冻后逐步恢复权限)
    pub restore_rate: f32,
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            time_decay_rate: 0.001,
            event_decay_penalty: 0.1,
            min_level: 0.0,
            freeze_threshold: 0.05,
            restore_rate: 0.01,
        }
    }
}

/// 衰减事件类型
///
/// 双驱动衰减模型:
/// - 时间驱动(TimeDecay):随时间自然递减
/// - 事件驱动(ViolationPenalty):违规触发惩罚性衰减
#[derive(Debug, Clone)]
pub enum DecayEvent {
    /// 时间驱动衰减
    /// 按 last_decay_at 至今的 elapsed 计算:level -= elapsed × time_decay_rate
    TimeDecay,
    /// 违规事件惩罚
    ViolationPenalty {
        /// 能力 ID(event 自包含,便于广播到 event-bus)
        capability_id: String,
        /// 严重程度(1.0 为标准,>1.0 加重,<1.0 减轻)
        severity: f32,
    },
    /// 冻结能力(对应 Skeptic 否决权)
    Freeze {
        /// 能力 ID
        capability_id: String,
        /// 冻结原因(审计用)
        reason: String,
    },
    /// 恢复能力(解冻后逐步恢复)
    Restore {
        /// 能力 ID
        capability_id: String,
    },
}
