//! FaaE 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 3 个变体覆盖 FaaE + EDSB 的全生命周期:专家查找 → 路由 → 熵计算
//! - 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// FaaE 路由器错误类型 — 覆盖专家管理与路由的全生命周期失败场景
#[derive(Debug, Error)]
pub enum FaaeError {
    /// 专家未找到(指定的 tool_id 未注册或已注销)
    #[error("专家未找到: {tool_id}")]
    ExpertNotFound {
        /// 未找到的工具 ID
        tool_id: String,
    },

    /// 路由失败(候选集为空、相似度计算异常等)
    #[error("路由失败: {reason}")]
    RoutingFailed {
        /// 失败原因描述
        reason: String,
    },

    /// 熵计算失败(工具数为 0、计数溢出等)
    #[error("熵计算失败: {reason}")]
    EntropyCalculationFailed {
        /// 失败原因描述
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expert_not_found_display() {
        let err = FaaeError::ExpertNotFound {
            tool_id: "tool-999".into(),
        };
        assert!(err.to_string().contains("tool-999"));
    }

    #[test]
    fn test_routing_failed_display() {
        let err = FaaeError::RoutingFailed {
            reason: "无候选工具".into(),
        };
        assert!(err.to_string().contains("无候选工具"));
    }

    #[test]
    fn test_entropy_calculation_failed_display() {
        let err = FaaeError::EntropyCalculationFailed {
            reason: "除零错误".into(),
        };
        assert!(err.to_string().contains("除零错误"));
    }
}
