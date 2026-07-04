//! MTPE 错误类型 — 库层错误定义(§4.1:库层用自定义 thiserror enum)
//!
//! 对应架构层:L7 Execution
//!
//! # 错误分类
//! - `InvalidN`:N 值超出 [1, max_n] 范围,调用方参数错误
//! - `PredictionFailed`:预测执行失败(如模型不可用、上下文异常)
//! - `RollbackFailed`:回退到单步预测失败,通常意味着单步预测也不可用

use thiserror::Error;

/// MTPE 执行器错误类型
///
/// WHY 使用 thiserror:库层错误需要被上层 anyhow 自动转换,
/// thiserror 提供 `#[from]` 与 Display 派生,符合 §4.1 规范
#[derive(Debug, Error)]
pub enum MtpeError {
    /// N 值超出允许范围 [1, max]
    ///
    /// WHY:N=0 无意义,N>10 预测成功率过低(架构决策:上限 10)
    #[error("invalid N: {n}, must be in [1, {max}]")]
    InvalidN {
        /// 调用方传入的 N 值
        n: usize,
        /// 配置允许的最大 N 值
        max: usize,
    },

    /// 预测执行失败
    #[error("prediction failed: {reason}")]
    PredictionFailed {
        /// 失败原因描述
        reason: String,
    },

    /// 回退到单步预测失败
    ///
    /// WHY:回退失败意味着单步预测也不可用,调用方应中止当前 Quest
    #[error("rollback failed: {reason}")]
    RollbackFailed {
        /// 失败原因描述
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_n_display() {
        let err = MtpeError::InvalidN { n: 0, max: 10 };
        assert!(err.to_string().contains("invalid N"));
        assert!(err.to_string().contains("0"));
    }

    #[test]
    fn test_prediction_failed_display() {
        let err = MtpeError::PredictionFailed {
            reason: "model unavailable".into(),
        };
        assert!(err.to_string().contains("model unavailable"));
    }

    #[test]
    fn test_rollback_failed_display() {
        let err = MtpeError::RollbackFailed {
            reason: "single-step also failed".into(),
        };
        assert!(err.to_string().contains("single-step also failed"));
    }
}
