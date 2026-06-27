//! CSN 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 CSN 在能力注册、替代查询与降级链管理中的所有失败场景。

use thiserror::Error;

/// CSN 执行错误枚举
///
/// 每个变体对应一种能力替代失败模式。
/// 所有错误均不可恢复(需调用者决定重试或降级)。
#[derive(Debug, Clone, Error)]
pub enum CsnError {
    /// 能力未找到:指定的 capability_id 在注册表中不存在
    ///
    /// 替代查询的目标能力未注册。调用者应先注册缺失能力,
    /// 或检查 capability_id 拼写。
    #[error("能力未找到: {capability_id}")]
    CapabilityNotFound {
        /// 未找到的能力 ID
        capability_id: String,
    },

    /// 无可用替代候选:注册表中无与目标能力相似的候选
    ///
    /// 可能原因:注册表为空、目标能力是唯一注册项、
    /// 或所有候选相似度低于阈值。
    #[error("无可用替代候选: {capability_id}")]
    NoSubstituteFound {
        /// 找不到替代的能力 ID
        capability_id: String,
    },

    /// 无效能力:能力描述符字段不合法
    ///
    /// 可能原因:语义向量维度与 `vector_dimension` 不匹配、
    /// capability_id 为空。
    #[error("无效能力: {reason}")]
    InvalidCapability {
        /// 错误原因(人类可读描述)
        reason: String,
    },

    /// 注册表已满:已达容量上限且 key 不存在
    ///
    /// 调用者应先驱逐冷能力腾出空间,或增大 `registry_capacity`。
    #[error("注册表已满(capacity={capacity})")]
    RegistryFull {
        /// 当前容量上限
        capacity: usize,
    },

    /// 降级链未找到:指定的 chain_id 不存在
    ///
    /// 调用者应先调用 `trigger_substitution` 创建降级链,
    /// 或检查 chain_id 拼写。
    #[error("降级链未找到: {chain_id}")]
    ChainNotFound {
        /// 未找到的降级链 ID
        chain_id: String,
    },

    /// 降级链已耗尽:已到达末端层级,无法继续推进
    ///
    /// 调用者应终止依赖该能力的操作,或重置降级链从头开始。
    #[error("降级链已耗尽: {chain_id}(共 {total_levels} 级)")]
    ChainExhausted {
        /// 耗尽的降级链 ID
        chain_id: String,
        /// 总层级数
        total_levels: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_capability_not_found() {
        let e = CsnError::CapabilityNotFound {
            capability_id: "shell-exec".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("能力未找到"));
        assert!(msg.contains("shell-exec"));
    }

    #[test]
    fn test_error_display_no_substitute_found() {
        let e = CsnError::NoSubstituteFound {
            capability_id: "shell-exec".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("无可用替代候选"));
        assert!(msg.contains("shell-exec"));
    }

    #[test]
    fn test_error_display_invalid_capability() {
        let e = CsnError::InvalidCapability {
            reason: "向量维度不匹配".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("无效能力"));
        assert!(msg.contains("向量维度不匹配"));
    }

    #[test]
    fn test_error_display_registry_full() {
        let e = CsnError::RegistryFull { capacity: 100 };
        let msg = format!("{e}");
        assert!(msg.contains("注册表已满"));
        assert!(msg.contains("100"));
    }

    #[test]
    fn test_error_display_chain_not_found() {
        let e = CsnError::ChainNotFound {
            chain_id: "c-1".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("降级链未找到"));
        assert!(msg.contains("c-1"));
    }

    #[test]
    fn test_error_display_chain_exhausted() {
        let e = CsnError::ChainExhausted {
            chain_id: "c-1".into(),
            total_levels: 3,
        };
        let msg = format!("{e}");
        assert!(msg.contains("降级链已耗尽"));
        assert!(msg.contains("c-1"));
        assert!(msg.contains("3"));
    }

    #[test]
    fn test_error_clone() {
        let e = CsnError::ChainExhausted {
            chain_id: "c-1".into(),
            total_levels: 3,
        };
        let _cloned = e.clone();
    }
}
