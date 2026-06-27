//! DECB 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
//! 所有变体携带足够上下文,便于调用方定位问题。

use thiserror::Error;

/// DECB 治理器错误类型
///
/// WHY:DECB 作为预算治理器,需要在预算超限、系数非法、降级拒绝等场景
/// 向调用方传递结构化错误信息。每个变体携带足够上下文用于审计与日志。
#[derive(Debug, Error)]
pub enum DecbError {
    /// 预算超限 — 当前消耗超过预算上限
    ///
    /// WHY:BudgetExceeded 是溢出检测的核心信号,携带预算类型、当前值与上限,
    /// 便于上层(L8 Parliament)决策是否拒绝新 Quest 或触发降级链路。
    /// TODO(Week 5 Task 37):集成到 event-bus,发布 `BudgetExceeded` 事件
    #[error("budget exceeded: {budget_type} current={current} limit={limit}")]
    BudgetExceeded {
        /// 预算类型(如 "token"、"tool_call"、"total_cost")
        budget_type: String,
        /// 当前消耗量
        current: u64,
        /// 预算上限
        limit: u64,
    },

    /// 预算系数非法 — 值不在 [0.0, 1.0] 区间或为 NaN
    ///
    /// WHY:BudgetCoefficient newtype 在构造时校验,此错误用于防御外部
    /// 传入的预计算系数越界(如序列化反序列化后的脏数据)
    #[error("invalid budget coefficient: {value} ({reason})")]
    InvalidCoefficient {
        /// 越界的系数值
        value: f32,
        /// 越界原因(人类可读)
        reason: String,
    },

    /// 降级模式拒绝 — Degraded 模式下仍超预算,拒绝新 Quest
    ///
    /// WHY:Degraded 是最低档位,无法继续降级。此时新 Quest 应被拒绝,
    /// 携带 quest_id 便于上层追踪哪个 Quest 被拒绝
    #[error("degraded mode rejected quest: {quest_id} ({reason})")]
    DegradedModeRejected {
        /// 被拒绝的 Quest ID
        quest_id: String,
        /// 拒绝原因(人类可读)
        reason: String,
    },

    /// 配置错误 — 配置项非法(如阈值倒挂、预算为负等)
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
        let err = DecbError::BudgetExceeded {
            budget_type: "token".into(),
            current: 1500,
            limit: 1000,
        };
        assert!(err.to_string().contains("token"));
        assert!(err.to_string().contains("1500"));
        assert!(err.to_string().contains("1000"));
    }

    #[test]
    fn test_invalid_coefficient_display() {
        let err = DecbError::InvalidCoefficient {
            value: 1.5,
            reason: "out of [0.0, 1.0]".into(),
        };
        assert!(err.to_string().contains("1.5"));
        assert!(err.to_string().contains("out of"));
    }

    #[test]
    fn test_degraded_mode_rejected_display() {
        let err = DecbError::DegradedModeRejected {
            quest_id: "quest-1".into(),
            reason: "budget exhausted".into(),
        };
        assert!(err.to_string().contains("quest-1"));
        assert!(err.to_string().contains("budget exhausted"));
    }

    #[test]
    fn test_config_error_display() {
        let err = DecbError::ConfigError {
            detail: "threshold inverted".into(),
        };
        assert!(err.to_string().contains("threshold inverted"));
    }
}
