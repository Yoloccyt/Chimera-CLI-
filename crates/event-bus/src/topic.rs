//! 事件主题过滤订阅 — 基于 `EventTopic` 的订阅级 topic 过滤(运行时面)
//!
//! 对应架构层:L1 Core
//! 设计决策（2026-07-09）：采用 10 类 EventTopic 分类方案，架构纯净度优先
//!
//! # 迁出边界(ADR-322)
//! `EventTopic` 本体（纯 serde 类型）已迁至 `nexus-events::topic`，本模块经
//! [`pub use`](EventTopic) 保持 `event_bus::topic::EventTopic` 路径可用。
//! 留在这里的只有 [`FilteredSubscriber`]——它持有 `EventReceiver`（广播通道
//! 接收端），属运行时面，与"零 IO 零运行时"的事件目录无关。
//!
//! # 10 类分类理据
//! 按十层架构的功能域划分，每个 topic 对应一组职责相关的 NexusEvent 变体。
//! `EventTopic::all()` 返回全部 10 个 topic。
//! FilteredSubscriber 订阅指定 topic 集合后，仅接收匹配事件，避免无关事件
//! 占用消费者缓冲区。既有 `subscribe()` 保持全量广播，向后兼容。
//!
//! # 与 recv_matching 的区别
//! - `recv_matching(FnMut)`：基于谓词的临时过滤，每次调用都要传闭包
//! - `FilteredSubscriber`：基于 topic 集合的订阅级过滤，构造时确定，
//!   后续 recv 自动跳过不匹配事件，更适合长期订阅场景

use std::collections::HashSet;

pub use nexus_events::topic::EventTopic;

use crate::types::NexusEvent;

/// 事件主题过滤订阅者 — 仅接收指定 topic 集合的事件
///
/// 包装 `EventReceiver`，内部跳过不匹配 topic 的事件。
/// 不匹配的事件从接收缓冲区移除（消费但不返回），与 `recv_matching` 语义一致。
///
/// # 使用场景
/// - TTG 仲裁层只需 Parliament + Budget 事件，无需接收全部事件变体
///   （变体计数以 `tests/variant_count_test.rs` 的 `VARIANT_COUNT_LOCK`
///   为权威，散文不登记数值；主题分类仍覆盖全部 155 个 NexusEvent 变体）
/// - N9 PrerequisiteChecker 只需 Routing 事件
/// - 减少无关事件对消费者缓冲区的占用
///
/// # 向后兼容
/// 既有 `EventBus::subscribe()` 返回 `EventReceiver`（全量广播）不受影响。
/// FilteredSubscriber 是独立的类型，通过 `EventBus::subscribe_filtered()` 创建。
pub struct FilteredSubscriber {
    /// 内部接收者（复用 EventReceiver 的日志与背压能力）
    inner: crate::bus::EventReceiver,
    /// 订阅的 topic 集合
    topics: HashSet<EventTopic>,
}

impl FilteredSubscriber {
    /// 内部构造函数（crate 内可见，由 EventBus::subscribe_filtered 调用）
    ///
    /// WHY pub(crate)：避免外部直接拼装 FilteredSubscriber 绕过 EventBus 的
    /// 订阅者计数与日志埋点；同时允许 bus.rs 在同 crate 内构造。
    pub(crate) fn new(inner: crate::bus::EventReceiver, topics: HashSet<EventTopic>) -> Self {
        Self { inner, topics }
    }

    /// 接收下一个匹配 topic 的事件
    ///
    /// 内部循环调用 `inner.recv()`，跳过 topic 不匹配的事件。
    /// 不匹配的事件被消费（从缓冲区移除），确保缓冲区不被无关事件占满。
    ///
    /// # 错误
    /// 透传 `EventReceiver::recv()` 的错误（ChannelClosed / SlowConsumerDropped）。
    pub async fn recv(&mut self) -> Result<NexusEvent, crate::error::EventBusError> {
        loop {
            let event = self.inner.recv().await?;
            if self.topics.contains(&event.topic()) {
                return Ok(event);
            }
            // 不匹配的事件被消费并丢弃
        }
    }

    /// 带超时的接收
    ///
    /// 透传 `EventReceiver::recv_timeout()` 的超时与错误处理。
    pub async fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<NexusEvent, crate::error::EventBusError> {
        loop {
            let event = self.inner.recv_timeout(timeout).await?;
            if self.topics.contains(&event.topic()) {
                return Ok(event);
            }
        }
    }

    /// 尝试非阻塞接收
    ///
    /// 扫描当前缓冲区，返回第一个匹配 topic 的事件。
    /// 不匹配的事件被消费（从缓冲区移除），与 `try_recv_matching` 语义一致。
    ///
    /// # 返回值
    /// - `Ok(Some(event))`：找到匹配事件
    /// - `Ok(None)`：缓冲区为空（可能还有后续事件，但当前无可用）
    /// - `Err`：通道关闭或 lag 超限
    pub fn try_recv(&mut self) -> Result<Option<NexusEvent>, crate::error::EventBusError> {
        loop {
            match self.inner.try_recv()? {
                Some(event) if self.topics.contains(&event.topic()) => return Ok(Some(event)),
                Some(_) => continue, // 不匹配，消费并继续
                None => return Ok(None),
            }
        }
    }

    /// 获取订阅的 topic 集合
    pub fn topics(&self) -> &HashSet<EventTopic> {
        &self.topics
    }

    /// 获取订阅者标识（委托给内部 EventReceiver）
    pub fn subscriber_id(&self) -> &str {
        self.inner.subscriber_id()
    }
}
