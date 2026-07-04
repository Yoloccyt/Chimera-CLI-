//! 事件总线错误类型 — 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// 事件总线错误
#[derive(Debug, Error)]
pub enum EventBusError {
    /// 序列化/反序列化失败 — MessagePack 或 JSON 编解码错误
    #[error("事件序列化失败: {0}")]
    SerializationError(String),

    /// 通道已关闭 — 所有 Sender 已 drop,无法继续通信
    #[error("事件通道已关闭")]
    ChannelClosed,

    /// 接收超时 — 在指定时间内未收到事件
    #[error("接收事件超时: {0}ms")]
    RecvTimeout(u64),

    /// 慢消费者被丢弃 — lag 超过阈值,接收者被强制断开
    ///
    /// WHY:broadcast 通道默认行为是返回 Lagged 错误,
    /// 这里包装为显式错误以便上层决定重订阅或告警
    #[error("慢消费者被丢弃: subscriber={subscriber_id} lag={lag}")]
    SlowConsumerDropped {
        /// 被丢弃的订阅者标识
        subscriber_id: String,
        /// 滞后事件数
        lag: u64,
    },
}

/// 从 broadcast 接收错误转换
impl From<tokio::sync::broadcast::error::RecvError> for EventBusError {
    fn from(err: tokio::sync::broadcast::error::RecvError) -> Self {
        match err {
            tokio::sync::broadcast::error::RecvError::Closed => Self::ChannelClosed,
            tokio::sync::broadcast::error::RecvError::Lagged(lag) => Self::SlowConsumerDropped {
                subscriber_id: "unknown".into(),
                lag,
            },
        }
    }
}

/// 从 MessagePack 序列化错误转换
impl From<rmp_serde::encode::Error> for EventBusError {
    fn from(err: rmp_serde::encode::Error) -> Self {
        Self::SerializationError(format!("msgpack encode: {err}"))
    }
}

/// 从 MessagePack 反序列化错误转换
impl From<rmp_serde::decode::Error> for EventBusError {
    fn from(err: rmp_serde::decode::Error) -> Self {
        Self::SerializationError(format!("msgpack decode: {err}"))
    }
}

/// 从 JSON 序列化错误转换(降级通道)
impl From<serde_json::Error> for EventBusError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(format!("json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let e = EventBusError::ChannelClosed;
        assert_eq!(e.to_string(), "事件通道已关闭");
    }

    #[test]
    fn test_slow_consumer_error_fields() {
        let e = EventBusError::SlowConsumerDropped {
            subscriber_id: "sub-1".into(),
            lag: 42,
        };
        let msg = e.to_string();
        assert!(msg.contains("sub-1"));
        assert!(msg.contains("42"));
    }
}
