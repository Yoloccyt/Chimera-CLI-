//! SESA 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 SESA 在专家激活与稀疏化过程中可能遇到的所有失败场景。

use thiserror::Error;

/// SESA 执行错误枚举
///
/// 每个变体对应一种稀疏激活失败模式。
/// 所有错误均不可恢复(需调用者决定重试或降级)。
#[derive(Debug, Clone, Error)]
pub enum SesaError {
    /// 专家未找到:指定的 expert_id 在注册表中不存在
    ///
    /// 激活请求的候选专家中存在未注册的 ID。
    /// 调用者应先注册缺失的专家,或从候选列表中移除。
    #[error("专家未找到: {expert_id}")]
    ExpertNotFound {
        /// 未找到的专家 ID
        expert_id: String,
    },

    /// 激活超时:激活操作超过截止时间
    ///
    /// 对应架构红线:所有异步操作必须有超时处理。
    /// 超时后调用方应降级为使用空掩码(不激活任何专家),避免阻塞执行流。
    #[error("激活超时: 截止时间 {deadline_ms}ms")]
    ActivationTimeout {
        /// 截止时间(毫秒)
        deadline_ms: u64,
    },

    /// 专家索引越界:激活的专家索引超过 256-bit 掩码容量
    ///
    /// WHY:256-bit 掩码最多表示 256 个专家,超过应通过 KVBSR 粗筛降至 256 以内。
    /// 此错误表示上游 KVBSR 未正确粗筛,调用者应检查 KVBSR 配置。
    #[error("专家索引越界: {index} >= {capacity}")]
    IndexOutOfBounds {
        /// 越界的索引值
        index: usize,
        /// 掩码容量(256)
        capacity: usize,
    },

    /// 配置错误:无效的配置参数
    ///
    /// 可能原因:max_sparsity_ratio 为 0、top_k 为 0、mask_width 不为 256 等。
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因(人类可读描述)
        reason: String,
    },

    /// 空专家池:注册表中没有任何专家,无法激活
    ///
    /// 调用者应先注册至少一个专家再发起激活请求。
    #[error("空专家池: 无法激活")]
    EmptyExpertPool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_expert_not_found() {
        let e = SesaError::ExpertNotFound {
            expert_id: "expert-1".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("专家未找到"));
        assert!(msg.contains("expert-1"));
    }

    #[test]
    fn test_error_display_activation_timeout() {
        let e = SesaError::ActivationTimeout { deadline_ms: 5 };
        let msg = format!("{e}");
        assert!(msg.contains("激活超时"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_error_display_index_out_of_bounds() {
        let e = SesaError::IndexOutOfBounds {
            index: 300,
            capacity: 256,
        };
        let msg = format!("{e}");
        assert!(msg.contains("专家索引越界"));
        assert!(msg.contains("300"));
        assert!(msg.contains("256"));
    }

    #[test]
    fn test_error_display_config_error() {
        let e = SesaError::ConfigError {
            reason: "top_k 为 0".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("配置错误"));
        assert!(msg.contains("top_k 为 0"));
    }

    #[test]
    fn test_error_display_empty_expert_pool() {
        let e = SesaError::EmptyExpertPool;
        let msg = format!("{e}");
        assert!(msg.contains("空专家池"));
    }

    #[test]
    fn test_error_clone() {
        let e = SesaError::ActivationTimeout { deadline_ms: 5 };
        let _cloned = e.clone();
    }
}
