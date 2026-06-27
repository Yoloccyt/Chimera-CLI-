//! AutoDPO 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块)
//!
//! WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
//! 所有变体携带足够上下文,便于调用方定位问题。

use thiserror::Error;

/// AutoDPO 错误类型
///
/// WHY:AutoDPO 作为偏好对生成器,需要在样本不足、质量过低、配置错误等
/// 场景向调用方传递结构化错误信息。每个变体携带足够上下文用于审计与日志。
#[derive(Debug, Error)]
pub enum AutoDpoError {
    /// 样本不足 — 输入候选数少于 2,无法构造偏好对
    ///
    /// WHY:DPO 至少需要 2 个候选(一个 chosen,一个 rejected),
    /// 少于 2 个无法构造偏好对,携带实际数量便于定位
    #[error("insufficient samples: need at least 2, got {actual}")]
    InsufficientSamples {
        /// 实际输入的候选数
        actual: usize,
    },

    /// 样本质量过低 — 所有候选质量分数均低于阈值
    ///
    /// WHY:低质量样本会污染训练集,必须过滤。携带阈值与最高分便于调参
    #[error("all samples below quality threshold: threshold={threshold}, best_score={best_score}")]
    QualityTooLow {
        /// 质量阈值
        threshold: f32,
        /// 当前批次最高质量分
        best_score: f32,
    },

    /// 偏好对生成失败 — 内部逻辑错误
    ///
    /// WHY:携带原因,便于定位生成逻辑 bug
    #[error("pair generation failed: {reason}")]
    GenerationFailed {
        /// 失败原因(人类可读)
        reason: String,
    },

    /// 配置错误 — 配置项非法(如阈值为负、样本数为 0 等)
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
    fn test_insufficient_samples_display() {
        let err = AutoDpoError::InsufficientSamples { actual: 1 };
        assert!(err.to_string().contains("1"));
        assert!(err.to_string().contains("2"));
    }

    #[test]
    fn test_quality_too_low_display() {
        let err = AutoDpoError::QualityTooLow {
            threshold: 0.5,
            best_score: 0.3,
        };
        assert!(err.to_string().contains("0.5"));
        assert!(err.to_string().contains("0.3"));
    }

    #[test]
    fn test_generation_failed_display() {
        let err = AutoDpoError::GenerationFailed {
            reason: "no valid pair".into(),
        };
        assert!(err.to_string().contains("no valid pair"));
    }

    #[test]
    fn test_config_error_display() {
        let err = AutoDpoError::ConfigError {
            detail: "threshold negative".into(),
        };
        assert!(err.to_string().contains("threshold negative"));
    }
}
