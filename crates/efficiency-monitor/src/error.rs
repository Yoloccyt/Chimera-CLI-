//! 错误类型定义 — efficiency-monitor 库层错误
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖监控器在事件订阅、告警发布与配置校验过程中的失败场景。

use thiserror::Error;

/// 监控器错误枚举
///
/// 每个变体对应一种监控失败模式。
#[derive(Debug, Clone, Error)]
pub enum MonitorError {
    /// 事件总线错误:订阅或发布失败
    ///
    /// 可能原因:EventBus 未绑定、publish_blocking 失败。
    #[error("事件总线错误: {0}")]
    EventBus(String),

    /// 配置错误:无效的配置参数或缺少必要依赖
    ///
    /// 可能原因:未绑定 EventBus 就调用 start_event_subscriber。
    #[error("配置错误: {reason}")]
    Config {
        /// 错误原因(人类可读描述)
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_event_bus() {
        let e = MonitorError::EventBus("channel closed".into());
        let msg = format!("{e}");
        assert!(msg.contains("事件总线错误"));
        assert!(msg.contains("channel closed"));
    }

    #[test]
    fn test_error_display_config() {
        let e = MonitorError::Config {
            reason: "未绑定 EventBus".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("配置错误"));
        assert!(msg.contains("未绑定 EventBus"));
    }

    #[test]
    fn test_error_clone() {
        let e = MonitorError::Config {
            reason: "test".into(),
        };
        let _cloned = e.clone();
    }
}
