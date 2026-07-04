//! SSRA 错误类型定义
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 SSRA 在模板管理与融合过程中可能遇到的所有失败场景。

use thiserror::Error;

/// SSRA 执行错误枚举
///
/// 每个变体对应一种适配融合失败模式。
/// 所有错误均不可恢复(需调用者决定重试或降级)。
#[derive(Debug, Clone, Error)]
pub enum SsraError {
    /// 模板未找到:指定的 capability_id 在 registry 中不存在
    ///
    /// 融合请求的 source_adapters 中存在未注册的能力 ID。
    /// 调用者应先预编译注册缺失的模板,或从源列表中移除。
    #[error("模板未找到: {capability_id}")]
    TemplateNotFound {
        /// 未找到的能力 ID
        capability_id: String,
    },

    /// 融合超时:融合操作超过截止时间
    ///
    /// 对应架构红线:所有异步操作必须有超时处理。
    /// 超时后调用方应降级为直接使用最强单模板,避免阻塞执行流。
    #[error("融合超时: 截止时间 {deadline_ms}ms")]
    FusionTimeout {
        /// 截止时间(毫秒)
        deadline_ms: u64,
    },

    /// 配置错误:无效的配置参数或模板缓存已满
    ///
    /// 可能原因:缓存容量为 0、注册时缓存已满未驱逐等。
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因(人类可读描述)
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_template_not_found() {
        let e = SsraError::TemplateNotFound {
            capability_id: "shell-exec".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("模板未找到"));
        assert!(msg.contains("shell-exec"));
    }

    #[test]
    fn test_error_display_fusion_timeout() {
        let e = SsraError::FusionTimeout { deadline_ms: 20 };
        let msg = format!("{e}");
        assert!(msg.contains("融合超时"));
        assert!(msg.contains("20"));
    }

    #[test]
    fn test_error_display_config_error() {
        let e = SsraError::ConfigError {
            reason: "缓存已满".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("配置错误"));
        assert!(msg.contains("缓存已满"));
    }

    #[test]
    fn test_error_clone() {
        let e = SsraError::FusionTimeout { deadline_ms: 20 };
        let _cloned = e.clone();
    }
}
