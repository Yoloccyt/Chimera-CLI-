//! GEA 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)

use thiserror::Error;

/// GEA 激活器错误类型
///
/// WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
/// 所有变体携带足够上下文,便于调用方定位问题。
#[derive(Debug, Error)]
pub enum GeaError {
    /// 门控值非法(不在 [0.0, 1.0] 区间)
    ///
    /// WHY:门控值经 sigmoid + clamp 后理论上必 ∈ [0, 1],
    /// 此变体用于防御外部传入的预计算门控值越界
    #[error("invalid gate value: {value} (expected [0.0, 1.0])")]
    InvalidGateValue {
        /// 越界的门控值
        value: f32,
    },

    /// 指定专家未在注册表中找到
    #[error("expert not found: {expert_id}")]
    ExpertNotFound {
        /// 未找到的专家 ID
        expert_id: String,
    },

    /// 冲突消解失败(如所有候选专家均被抑制)
    #[error("conflict resolution failed: {reason}")]
    ConflictResolutionFailed {
        /// 失败原因
        reason: String,
    },

    /// 配置错误(如权重和不为 1.0、阈值为负等)
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
    fn test_invalid_gate_value_display() {
        let err = GeaError::InvalidGateValue { value: 1.5 };
        assert!(err.to_string().contains("1.5"));
    }

    #[test]
    fn test_expert_not_found_display() {
        let err = GeaError::ExpertNotFound {
            expert_id: "e-1".into(),
        };
        assert!(err.to_string().contains("e-1"));
    }

    #[test]
    fn test_conflict_resolution_failed_display() {
        let err = GeaError::ConflictResolutionFailed {
            reason: "all suppressed".into(),
        };
        assert!(err.to_string().contains("all suppressed"));
    }

    #[test]
    fn test_config_error_display() {
        let err = GeaError::ConfigError {
            detail: "bad weights".into(),
        };
        assert!(err.to_string().contains("bad weights"));
    }
}
