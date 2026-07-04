//! ACB 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
//! 所有变体携带足够上下文,便于调用方定位问题。

use thiserror::Error;

/// ACB 治理器错误类型
///
/// WHY:ACB 作为预算治理器,需要在预算超限、级别非法、配置错误等场景
/// 向调用方传递结构化错误信息。每个变体携带足够上下文用于审计与日志。
#[derive(Debug, Error)]
pub enum AcbError {
    /// 预算超限 — 当前请求消耗超过当前级别预算上限
    ///
    /// WHY:BudgetExceeded 是预算治理的核心信号,携带请求 ID、当前消耗与上限,
    /// 便于上层(L8 Parliament)决策是否拒绝新 Quest 或触发降级。
    #[error("budget exceeded: quest={quest_id} current={current} limit={limit}")]
    BudgetExceeded {
        /// 触发超限的 Quest ID
        quest_id: String,
        /// 当前请求消耗量
        current: u64,
        /// 当前级别预算上限
        limit: u64,
    },

    /// 预算级别非法 — 值不在合法范围或层级不存在
    ///
    /// WHY:BudgetTier 在构造时校验,此错误用于防御外部传入的非法级别
    /// (如反序列化后的脏数据)
    #[error("invalid budget tier: {tier} ({reason})")]
    InvalidTier {
        /// 非法的级别值
        tier: u8,
        /// 越界原因(人类可读)
        reason: String,
    },

    /// 降级模式拒绝 — L0 级别下仍超预算,拒绝新请求
    ///
    /// WHY:L0 是最低级别,无法继续降级。此时新请求应被拒绝,
    /// 携带 quest_id 便于上层追踪哪个请求被拒绝
    #[error("L0 degraded mode rejected quest: {quest_id} ({reason})")]
    DegradedModeRejected {
        /// 被拒绝的 Quest ID
        quest_id: String,
        /// 拒绝原因(人类可读)
        reason: String,
    },

    /// 配置错误 — 配置项非法(如阈值为负、级别上限倒挂等)
    #[error("config error: {detail}")]
    ConfigError {
        /// 配置错误详情
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_exceeded_display() {
        let err = AcbError::BudgetExceeded {
            quest_id: "quest-1".into(),
            current: 1500,
            limit: 1000,
        };
        assert!(err.to_string().contains("quest-1"));
        assert!(err.to_string().contains("1500"));
        assert!(err.to_string().contains("1000"));
    }

    #[test]
    fn test_invalid_tier_display() {
        let err = AcbError::InvalidTier {
            tier: 5,
            reason: "out of [0, 3]".into(),
        };
        assert!(err.to_string().contains("5"));
        assert!(err.to_string().contains("out of"));
    }

    #[test]
    fn test_degraded_mode_rejected_display() {
        let err = AcbError::DegradedModeRejected {
            quest_id: "quest-1".into(),
            reason: "budget exhausted".into(),
        };
        assert!(err.to_string().contains("quest-1"));
        assert!(err.to_string().contains("budget exhausted"));
    }

    #[test]
    fn test_config_error_display() {
        let err = AcbError::ConfigError {
            detail: "threshold inverted".into(),
        };
        assert!(err.to_string().contains("threshold inverted"));
    }
}
