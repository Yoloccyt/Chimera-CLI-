//! 错误类型 — GSOE 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// GSOE 进化引擎错误
#[derive(Debug, Error)]
pub enum GsoeError {
    /// 策略参数非法(超出范围或违反约束)
    #[error("非法策略参数: {reason}")]
    InvalidPolicy {
        /// 错误原因描述
        reason: String,
    },

    /// 变异失败(幅度越界或类型不匹配)
    #[error("变异失败: {reason}")]
    MutationFailed {
        /// 错误原因描述
        reason: String,
    },

    /// 配置错误(字段非法或缺失)
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因描述
        reason: String,
    },

    /// 达到最大世代数,进化已终止
    #[error("达到最大世代数 {max_generation},进化已终止")]
    MaxGenerationReached {
        /// 配置的最大世代数
        max_generation: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_policy_display() {
        let e = GsoeError::InvalidPolicy {
            reason: "mutation_rate out of range".into(),
        };
        assert!(e.to_string().contains("非法策略参数"));
        assert!(e.to_string().contains("mutation_rate out of range"));
    }

    #[test]
    fn test_mutation_failed_display() {
        let e = GsoeError::MutationFailed {
            reason: "magnitude overflow".into(),
        };
        assert!(e.to_string().contains("变异失败"));
    }

    #[test]
    fn test_config_error_display() {
        let e = GsoeError::ConfigError {
            reason: "bad value".into(),
        };
        assert!(e.to_string().contains("配置错误"));
    }

    #[test]
    fn test_max_generation_display() {
        let e = GsoeError::MaxGenerationReached {
            max_generation: 1000,
        };
        assert!(e.to_string().contains("1000"));
    }
}
