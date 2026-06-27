//! 事件总线实现 — 基于 tokio::broadcast 的 typed broadcast bus
//!
//! 对应架构:L1 Core,所有跨层通信的唯一通道(§2.2 依赖铁律)
//!
//! # 设计要点
//! - 封装 `tokio::broadcast::channel`,提供类型安全的发布订阅
//! - 序列化采用 MessagePack(ADR-004),提供跨进程投递能力
//! - 关键事件标注 Critical,背压策略据此保护(见 backpressure 模块)
//! - 所有 async fn 满足 Send 约束,可被 tokio::spawn

use crate::error::EventBusError;
use crate::logging::BusLogger;
use crate::types::{EventSeverity, NexusEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::warn;

/// 默认广播通道容量
///
/// WHY:1024 平衡内存占用与突发流量。每个 NexusEvent 约 200-500 字节,
/// 1024 容量约占 0.5MB,可吸收短时突发;持续高吞吐应增大容量或加背压策略。
pub const DEFAULT_CAPACITY: usize = 1024;

/// 事件总线 — 跨层通信的唯一通道
///
/// 基于 `tokio::broadcast::Sender<NexusEvent>`,支持多订阅者广播。
/// Clone 廉价(仅 Arc 引用计数),可在任务间自由传递。
///
/// 可选配备 `BusLogger` 实现全链路结构化日志埋点,
/// 记录订阅者连接/断开、事件发布/接收、错误码、重连尝试等关键信息。
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<NexusEvent>,
    /// 通道容量(创建时固定,broadcast::Sender 不暴露 capacity())
    capacity: usize,
    /// 可选日志记录器(Arc 共享,跨 Clone 共享同一计数器)
    logger: Option<Arc<BusLogger>>,
}

impl EventBus {
    /// 创建事件总线,使用默认容量(1024),不启用日志埋点
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// 创建事件总线,指定通道容量,不启用日志埋点
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            logger: None,
        }
    }

    /// 创建事件总线,指定通道容量并启用日志埋点
    ///
    /// `logger` 会被包装在 Arc 中,Clone 时共享同一计数器。
    pub fn with_logger(capacity: usize, logger: BusLogger) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            logger: Some(Arc::new(logger)),
        }
    }

    /// 为已有总线设置日志记录器(仅在未设置时生效)
    ///
    /// 返回 self 以便链式调用。
    /// 若已设置 logger,此调用无效果(保留第一个 logger)。
    pub fn set_logger(&mut self, logger: BusLogger) {
        if self.logger.is_none() {
            self.logger = Some(Arc::new(logger));
        }
    }

    /// 获取日志记录器的引用(若已设置)
    pub fn logger(&self) -> Option<&BusLogger> {
        self.logger.as_deref()
    }

    /// 发布事件到所有订阅者
    ///
    /// 若无订阅者,事件被丢弃但不视为错误(返回 Ok(()))。
    /// 慢消费者导致的丢弃由接收端的 `recv()` 以 `SlowConsumerDropped` 错误暴露。
    ///
    /// WHY async 签名:当前内部为同步 send,但保留 async 以保证 API 稳定性 —
    /// 未来引入跨进程投递(MCP Mesh)或异步序列化时无需破坏调用方。
    ///
    /// # SubTask 17.2:Critical 事件无订阅者告警
    /// 当 `subscriber_count == 0` 且事件为 Critical 级时,记录 `warn` 日志。
    /// WHY:CheckpointSaved/ConsensusReached 等关键事件丢失会导致系统状态不一致
    /// (如 Quest 无法恢复),无订阅者时必须告警。
    /// Normal 级事件保持静默丢弃,避免日志噪声。
    #[allow(clippy::unused_async)]
    pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count);
        }

        // SubTask 17.2:Critical 事件无订阅者告警
        // WHY:关键事件(CheckpointSaved/ConsensusReached/SlowConsumerDropped)丢失
        // 会导致系统状态不一致,无订阅者时必须告警;Normal 级静默丢弃避免日志噪声
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃"
            );
        }

        // broadcast::Sender::send 仅在无活跃接收者时返回 Err;
        // 按设计无订阅者时事件被静默丢弃,不视为错误。
        let _ = self.sender.send(event);
        Ok(())
    }

    /// 同步发布 — 用于不便 await 的场景(如 Drop 实现)
    ///
    /// WHY:某些回调场景无法 await,提供同步版本避免阻塞
    ///
    /// # SubTask 17.2:Critical 事件无订阅者告警
    /// 与 `publish` 保持一致:无订阅者且 Critical 级时记录 `warn` 日志。
    pub fn publish_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count);
        }

        // SubTask 17.2:Critical 事件无订阅者告警(与 publish 保持一致)
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃(同步发布)"
            );
        }

        // 与 publish 保持一致:无订阅者时静默丢弃事件。
        let _ = self.sender.send(event);
        Ok(())
    }

    /// 订阅事件流,返回新的接收者
    ///
    /// 每次调用创建独立接收者,从订阅时刻开始接收新事件(不回放历史)。
    /// 接收者会继承总线的日志记录器,用于记录接收端事件。
    pub fn subscribe(&self) -> EventReceiver {
        let subscriber_id = format!("sub-{}", uuid::Uuid::now_v7());
        let subscriber_count = self.sender.receiver_count() + 1; // +1 包含即将创建的

        // 记录订阅者连接日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_subscriber_connected(&subscriber_id, subscriber_count, self.capacity);
        }

        EventReceiver {
            inner: self.sender.subscribe(),
            subscriber_id,
            logger: self.logger.clone(),
        }
    }

    /// 获取当前订阅者数量
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// 获取通道容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 事件接收者 — 包装 broadcast::Receiver
///
/// 每个接收者独立维护读取位置,慢消费者会收到 Lagged 错误。
/// 持有总线日志记录器的引用,用于记录接收/超时/错误事件。
pub struct EventReceiver {
    inner: broadcast::Receiver<NexusEvent>,
    /// 订阅者唯一标识,用于日志关联
    subscriber_id: String,
    /// 日志记录器(与总线共享)
    logger: Option<Arc<BusLogger>>,
}

impl EventReceiver {
    /// 接收下一个事件
    ///
    /// 错误处理:
    /// - `ChannelClosed`:所有 Sender 已 drop,流结束
    /// - `SlowConsumerDropped`:lag 超限,需决定重订阅或告警
    pub async fn recv(&mut self) -> Result<NexusEvent, EventBusError> {
        match self.inner.recv().await {
            Ok(event) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(event)
            }
            Err(e) => {
                let eb_err = EventBusError::from(e);
                if let Some(logger) = &self.logger {
                    match &eb_err {
                        EventBusError::ChannelClosed => {
                            logger.log_channel_closed(&self.subscriber_id);
                        }
                        EventBusError::SlowConsumerDropped {
                            subscriber_id: _,
                            lag,
                        } => {
                            logger.log_slow_consumer_dropped(&self.subscriber_id, *lag, *lag);
                        }
                        _ => {}
                    }
                }
                Err(eb_err)
            }
        }
    }

    /// 带超时的接收
    ///
    /// WHY:架构红线要求所有异步操作有超时处理,避免孤儿调用
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Result<NexusEvent, EventBusError> {
        match tokio::time::timeout(timeout, self.inner.recv()).await {
            Ok(Ok(event)) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(event)
            }
            Ok(Err(e)) => {
                let eb_err = EventBusError::from(e);
                if let Some(logger) = &self.logger {
                    match &eb_err {
                        EventBusError::ChannelClosed => {
                            logger.log_channel_closed(&self.subscriber_id);
                        }
                        EventBusError::SlowConsumerDropped {
                            subscriber_id: _,
                            lag,
                        } => {
                            logger.log_slow_consumer_dropped(&self.subscriber_id, *lag, *lag);
                        }
                        _ => {}
                    }
                }
                Err(eb_err)
            }
            Err(_) => {
                let timeout_ms = timeout.as_millis() as u64;
                if let Some(logger) = &self.logger {
                    logger.log_recv_timeout(&self.subscriber_id, timeout_ms);
                }
                Err(EventBusError::RecvTimeout(timeout_ms))
            }
        }
    }

    /// 尝试非阻塞接收
    ///
    /// 返回 Ok(Some(event)) 表示有事件,Ok(None) 表示暂无事件,Err 表示错误
    pub fn try_recv(&mut self) -> Result<Option<NexusEvent>, EventBusError> {
        use broadcast::error::TryRecvError;
        match self.inner.try_recv() {
            Ok(event) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Closed) => {
                if let Some(logger) = &self.logger {
                    logger.log_channel_closed(&self.subscriber_id);
                }
                Err(EventBusError::ChannelClosed)
            }
            Err(TryRecvError::Lagged(lag)) => {
                if let Some(logger) = &self.logger {
                    logger.log_slow_consumer_dropped(&self.subscriber_id, lag, lag);
                }
                Err(EventBusError::SlowConsumerDropped {
                    subscriber_id: self.subscriber_id.clone(),
                    lag,
                })
            }
        }
    }

    /// 获取订阅者标识
    pub fn subscriber_id(&self) -> &str {
        &self.subscriber_id
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        if let Some(logger) = &self.logger {
            // 记录订阅者断开连接
            // 注:广播通道的 receiver_count 在 drop 前已减 1,
            // 此处记录的是 drop 后的剩余数量
            logger.log_subscriber_disconnected(
                &self.subscriber_id,
                0, // 无法在 Drop 中获取精确剩余数,用 0 表示已断开
            );
        }
    }
}

// ============================================================
// 序列化工具 — 用于跨进程投递(MCP Mesh)与持久化
// ============================================================

/// 将事件序列化为 MessagePack 字节(ADR-004)
///
/// 跨进程通信(MCP Mesh)与事件日志持久化时使用。
pub fn serialize_msgpack(event: &NexusEvent) -> Result<Vec<u8>, EventBusError> {
    rmp_serde::to_vec_named(event).map_err(EventBusError::from)
}

/// 从 MessagePack 字节反序列化事件
pub fn deserialize_msgpack(bytes: &[u8]) -> Result<NexusEvent, EventBusError> {
    rmp_serde::from_slice(bytes).map_err(EventBusError::from)
}

/// 将事件序列化为 JSON 字符串(降级通道,调试与兼容场景)
///
/// WHY:MessagePack 不可读,调试时 JSON 更直观;
/// 部分 MCP 客户端可能仅支持 JSON
pub fn serialize_json(event: &NexusEvent) -> Result<String, EventBusError> {
    serde_json::to_string(event).map_err(EventBusError::from)
}

/// 从 JSON 字符串反序列化事件
pub fn deserialize_json(s: &str) -> Result<NexusEvent, EventBusError> {
    serde_json::from_str(s).map_err(EventBusError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    fn make_test_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-001".into(),
            title: "测试任务".into(),
            task_count: 3,
        }
    }

    #[tokio::test]
    async fn test_publish_subscribe_basic() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_no_subscribers_ok() {
        let bus = EventBus::new();
        // 无订阅者时发布应返回 Ok(()),非错误
        bus.publish(make_test_event()).await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        assert_eq!(rx1.recv().await.unwrap(), event);
        assert_eq!(rx2.recv().await.unwrap(), event);
    }

    #[tokio::test]
    async fn test_recv_timeout() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let result = rx.recv_timeout(Duration::from_millis(50)).await;
        assert!(matches!(result, Err(EventBusError::RecvTimeout(_))));
    }

    #[test]
    fn test_msgpack_roundtrip() {
        let event = make_test_event();
        let bytes = serialize_msgpack(&event).unwrap();
        let decoded = deserialize_msgpack(&bytes).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn test_json_roundtrip() {
        let event = make_test_event();
        let s = serialize_json(&event).unwrap();
        let decoded = deserialize_json(&s).unwrap();
        assert_eq!(decoded, event);
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }
}
