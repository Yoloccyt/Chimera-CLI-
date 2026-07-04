//! 结构化日志埋点 — 事件总线全链路可观测性
//!
//! 对应架构:L1 Core,为所有跨层通信提供 JSON 结构化日志,
//! 确保网络波动/连接异常时可完整追溯问题时间线与根因。
//!
//! # 日志字段规范
//! 每条日志包含:module、level、timestamp、context_id、operation、
//! 以及操作特定字段(如 subscriber_count、event_type、payload_size 等)。
//!
//! # 使用方式
//! ```no_run
//! # use event_bus::logging::BusLogger;
//! # use event_bus::{EventBus, NexusEvent, EventMetadata};
//! # async fn example() {
//! let bus = EventBus::new();
//! let logger = BusLogger::new("chimera-cli");
//! let event = NexusEvent::QuestCreated {
//!     metadata: EventMetadata::new("test"),
//!     quest_id: "q-1".into(),
//!     title: "示例".into(),
//!     task_count: 1,
//! };
//! logger.log_publish(&event, 0);
//! # }
//! ```

use crate::types::NexusEvent;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{error, info, warn};

/// 可观测性上下文 ID — 用于关联同一操作链路上的所有日志
///
/// 对 publish→recv 链路,使用 event_id 作为 context_id;
/// 对订阅生命周期,使用 subscriber_id。
pub type ContextId = String;

/// 事件总线日志记录器 — 提供全链路结构化日志埋点
///
/// 每个 `EventBus` 实例配套一个 `BusLogger`,记录:
/// - 订阅者连接/断开时间戳
/// - 事件发布/接收统计(吞吐量、字节数)
/// - 错误码与错误描述
/// - 通道状态变化(关闭、lag 告警)
/// - 关键协议交互过程
///
/// 内部使用 `tracing` 宏输出 JSON 结构化日志,
/// 上层通过 `tracing-subscriber` 配置输出目标(文件/stdout/OTLP)。
pub struct BusLogger {
    /// 发布者模块标识(如 "chimera-cli"、"osa-coordinator")
    module_id: String,
    /// 累计发布事件数(原子计数器,跨任务安全)
    total_published: AtomicU64,
    /// 累计接收事件数
    total_received: AtomicU64,
    /// 累计错误数
    total_errors: AtomicU64,
}

impl BusLogger {
    /// 创建日志记录器
    ///
    /// `module_id` 用于标识日志来源,建议使用 crate 名。
    pub fn new(module_id: impl Into<String>) -> Self {
        Self {
            module_id: module_id.into(),
            total_published: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        }
    }

    // ============================================================
    // 订阅者生命周期 — 连接建立/断开时间戳
    // ============================================================

    /// 记录订阅者连接建立
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=subscriber_id、
    /// operation="subscriber_connected"、subscriber_count、capacity
    pub fn log_subscriber_connected(
        &self,
        subscriber_id: &str,
        subscriber_count: usize,
        capacity: usize,
    ) {
        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %subscriber_id,
            operation = "subscriber_connected",
            subscriber_count = subscriber_count,
            capacity = capacity,
            "订阅者已连接: subscriber_id={subscriber_id} total_subscribers={subscriber_count} capacity={capacity}",
            subscriber_id = subscriber_id,
            subscriber_count = subscriber_count,
            capacity = capacity,
        );
    }

    /// 记录订阅者断开连接(Receiver drop)
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=subscriber_id、
    /// operation="subscriber_disconnected"、remaining_subscribers
    pub fn log_subscriber_disconnected(&self, subscriber_id: &str, remaining_subscribers: usize) {
        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %subscriber_id,
            operation = "subscriber_disconnected",
            remaining_subscribers = remaining_subscribers,
            "订阅者已断开: subscriber_id={subscriber_id} remaining={remaining_subscribers}",
            subscriber_id = subscriber_id,
            remaining_subscribers = remaining_subscribers,
        );
    }

    // ============================================================
    // 事件发布 — 数据传输量统计、协议交互记录
    // ============================================================

    /// 记录事件发布
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=event_id、
    /// operation="event_published"、event_type、event_severity、
    /// subscriber_count、total_published、payload_size_estimate
    ///
    /// `payload_size_estimate`:事件序列化后字节数估算(用于流量统计),
    /// 传入 0 表示跳过大小估算(性能敏感路径)。
    pub fn log_publish(&self, event: &NexusEvent, subscriber_count: usize) {
        let event_id = event.metadata().event_id.to_string();
        let event_type = event.type_name();
        let severity = format!("{:?}", event.severity());
        let total = self.total_published.fetch_add(1, Ordering::Relaxed) + 1;

        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %event_id,
            operation = "event_published",
            event_type = %event_type,
            event_severity = %severity,
            subscriber_count = subscriber_count,
            total_published = total,
            "事件已发布: type={event_type} severity={severity} context_id={event_id} subscribers={subscriber_count} total_published={total}",
            event_type = event_type,
            severity = severity,
            event_id = event_id,
            subscriber_count = subscriber_count,
            total = total,
        );
    }

    /// 记录事件发布(带序列化大小估算)
    ///
    /// 与 `log_publish` 功能相同,额外计算 payload 的 MessagePack 序列化大小,
    /// 用于传输流量统计。仅在非热路径上调用。
    pub fn log_publish_with_size(&self, event: &NexusEvent, subscriber_count: usize) {
        let event_id = event.metadata().event_id.to_string();
        let event_type = event.type_name();
        let severity = format!("{:?}", event.severity());
        let total = self.total_published.fetch_add(1, Ordering::Relaxed) + 1;

        // 估算序列化后大小(用于流量统计,失败时回退到 0)
        let payload_size = rmp_serde::to_vec_named(event).map(|v| v.len()).unwrap_or(0);

        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %event_id,
            operation = "event_published",
            event_type = %event_type,
            event_severity = %severity,
            subscriber_count = subscriber_count,
            total_published = total,
            payload_size_bytes = payload_size,
            "事件已发布: type={event_type} severity={severity} context_id={event_id} subscribers={subscriber_count} size={payload_size}B total_published={total}",
            event_type = event_type,
            severity = severity,
            event_id = event_id,
            subscriber_count = subscriber_count,
            payload_size = payload_size,
            total = total,
        );
    }

    // ============================================================
    // 事件接收 — 数据传输量统计
    // ============================================================

    /// 记录事件接收成功
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=event_id、
    /// operation="event_received"、event_type、total_received
    pub fn log_recv(&self, event: &NexusEvent) {
        let event_id = event.metadata().event_id.to_string();
        let event_type = event.type_name();
        let total = self.total_received.fetch_add(1, Ordering::Relaxed) + 1;

        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %event_id,
            operation = "event_received",
            event_type = %event_type,
            total_received = total,
            "事件已接收: type={event_type} context_id={event_id} total_received={total}",
            event_type = event_type,
            event_id = event_id,
            total = total,
        );
    }

    /// 记录接收超时
    ///
    /// 日志级别:WARN(超时可能是网络波动信号,非致命错误)
    ///
    /// 日志字段:module、level=WARN、timestamp、context_id=subscriber_id、
    /// operation="recv_timeout"、timeout_ms、error_code="RECV_TIMEOUT"
    pub fn log_recv_timeout(&self, subscriber_id: &str, timeout_ms: u64) {
        warn!(
            module = %self.module_id,
            level = "WARN",
            context_id = %subscriber_id,
            operation = "recv_timeout",
            timeout_ms = timeout_ms,
            error_code = "RECV_TIMEOUT",
            error_description = "在指定时间内未收到事件,可能因上游无事件或网络波动",
            "接收超时: subscriber_id={subscriber_id} timeout_ms={timeout_ms}",
            subscriber_id = subscriber_id,
            timeout_ms = timeout_ms,
        );
    }

    // ============================================================
    // 错误记录 — 错误码及错误描述
    // ============================================================

    /// 记录通道关闭错误
    ///
    /// 日志级别:ERROR(通道关闭意味着所有 Sender 已 drop,属于严重状态变化)
    ///
    /// 日志字段:module、level=ERROR、timestamp、context_id=subscriber_id、
    /// operation="channel_closed"、error_code="CHANNEL_CLOSED"、
    /// error_description、total_errors
    pub fn log_channel_closed(&self, subscriber_id: &str) {
        let total = self.total_errors.fetch_add(1, Ordering::Relaxed) + 1;

        error!(
            module = %self.module_id,
            level = "ERROR",
            context_id = %subscriber_id,
            operation = "channel_closed",
            error_code = "CHANNEL_CLOSED",
            error_description = "所有 EventBus Sender 已 drop,通道关闭,无法继续接收事件",
            total_errors = total,
            "通道已关闭: subscriber_id={subscriber_id} total_errors={total}",
            subscriber_id = subscriber_id,
            total = total,
        );
    }

    /// 记录慢消费者被丢弃
    ///
    /// 日志级别:WARN(lag 超限是系统健康信号,需关注但非致命)
    ///
    /// 日志字段:module、level=WARN、timestamp、context_id=subscriber_id、
    /// operation="slow_consumer_dropped"、error_code="SLOW_CONSUMER"、
    /// lag、dropped_count、total_errors
    pub fn log_slow_consumer_dropped(&self, subscriber_id: &str, lag: u64, dropped_count: u64) {
        let total = self.total_errors.fetch_add(1, Ordering::Relaxed) + 1;

        warn!(
            module = %self.module_id,
            level = "WARN",
            context_id = %subscriber_id,
            operation = "slow_consumer_dropped",
            error_code = "SLOW_CONSUMER",
            error_description = "订阅者消费速度过慢,lag 超过阈值,已被强制断开。建议增大通道容量或优化消费者逻辑",
            lag = lag,
            dropped_count = dropped_count,
            total_errors = total,
            "慢消费者被丢弃: subscriber_id={subscriber_id} lag={lag} dropped_count={dropped_count} total_errors={total}",
            subscriber_id = subscriber_id,
            lag = lag,
            dropped_count = dropped_count,
            total = total,
        );
    }

    /// 记录序列化错误
    ///
    /// 日志级别:ERROR(序列化失败通常是数据损坏,需立即排查)
    ///
    /// 日志字段:module、level=ERROR、timestamp、context_id、
    /// operation="serialization_error"、error_code="SERIALIZATION_ERROR"、
    /// error_description、total_errors
    pub fn log_serialization_error(&self, context_id: &str, error_detail: &str) {
        let total = self.total_errors.fetch_add(1, Ordering::Relaxed) + 1;

        error!(
            module = %self.module_id,
            level = "ERROR",
            context_id = %context_id,
            operation = "serialization_error",
            error_code = "SERIALIZATION_ERROR",
            error_description = %error_detail,
            total_errors = total,
            "序列化失败: context_id={context_id} detail={error_detail} total_errors={total}",
            context_id = context_id,
            error_detail = error_detail,
            total = total,
        );
    }

    // ============================================================
    // 通道状态变化 — 网络/通道状态变化记录
    // ============================================================

    /// 记录通道状态变化
    ///
    /// 日志字段:module、level、timestamp、context_id=channel_id、
    /// operation="channel_state_change"、previous_state、new_state、reason
    pub fn log_channel_state_change(
        &self,
        channel_id: &str,
        previous_state: &str,
        new_state: &str,
        reason: &str,
    ) {
        let level = if new_state == "error" || new_state == "closed" {
            "WARN"
        } else {
            "INFO"
        };

        match level {
            "WARN" => warn!(
                module = %self.module_id,
                level = "WARN",
                context_id = %channel_id,
                operation = "channel_state_change",
                previous_state = %previous_state,
                new_state = %new_state,
                change_reason = %reason,
                "通道状态变化: channel={channel_id} {previous_state}→{new_state} reason={reason}",
                channel_id = channel_id,
                previous_state = previous_state,
                new_state = new_state,
                reason = reason,
            ),
            _ => info!(
                module = %self.module_id,
                level = "INFO",
                context_id = %channel_id,
                operation = "channel_state_change",
                previous_state = %previous_state,
                new_state = %new_state,
                change_reason = %reason,
                "通道状态变化: channel={channel_id} {previous_state}→{new_state} reason={reason}",
                channel_id = channel_id,
                previous_state = previous_state,
                new_state = new_state,
                reason = reason,
            ),
        }
    }

    // ============================================================
    // 重连/重订阅 — 重连尝试次数及间隔
    // ============================================================

    /// 记录重订阅尝试(慢消费者恢复后重新订阅)
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=subscriber_id、
    /// operation="resubscribe_attempt"、attempt_number、retry_interval_ms、
    /// reason
    pub fn log_resubscribe_attempt(
        &self,
        subscriber_id: &str,
        attempt_number: u32,
        retry_interval_ms: u64,
        reason: &str,
    ) {
        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %subscriber_id,
            operation = "resubscribe_attempt",
            attempt_number = attempt_number,
            retry_interval_ms = retry_interval_ms,
            reason = %reason,
            "重订阅尝试: subscriber_id={subscriber_id} attempt={attempt_number} interval_ms={retry_interval_ms} reason={reason}",
            subscriber_id = subscriber_id,
            attempt_number = attempt_number,
            retry_interval_ms = retry_interval_ms,
            reason = reason,
        );
    }

    /// 记录重订阅成功
    ///
    /// 日志字段:module、level=INFO、timestamp、context_id=subscriber_id、
    /// operation="resubscribe_success"、total_attempts、total_recovery_time_ms
    pub fn log_resubscribe_success(
        &self,
        subscriber_id: &str,
        total_attempts: u32,
        total_recovery_time_ms: u64,
    ) {
        info!(
            module = %self.module_id,
            level = "INFO",
            context_id = %subscriber_id,
            operation = "resubscribe_success",
            total_attempts = total_attempts,
            total_recovery_time_ms = total_recovery_time_ms,
            "重订阅成功: subscriber_id={subscriber_id} attempts={total_attempts} recovery_time_ms={total_recovery_time_ms}",
            subscriber_id = subscriber_id,
            total_attempts = total_attempts,
            total_recovery_time_ms = total_recovery_time_ms,
        );
    }

    /// 记录重订阅失败(超过最大重试次数)
    ///
    /// 日志级别:ERROR
    ///
    /// 日志字段:module、level=ERROR、timestamp、context_id=subscriber_id、
    /// operation="resubscribe_failed"、max_attempts、error_code="RESUBSCRIBE_FAILED"
    pub fn log_resubscribe_failed(&self, subscriber_id: &str, max_attempts: u32, reason: &str) {
        let total = self.total_errors.fetch_add(1, Ordering::Relaxed) + 1;

        error!(
            module = %self.module_id,
            level = "ERROR",
            context_id = %subscriber_id,
            operation = "resubscribe_failed",
            error_code = "RESUBSCRIBE_FAILED",
            error_description = "重订阅超过最大重试次数,订阅者已永久断开",
            max_attempts = max_attempts,
            reason = %reason,
            total_errors = total,
            "重订阅失败: subscriber_id={subscriber_id} max_attempts={max_attempts} reason={reason} total_errors={total}",
            subscriber_id = subscriber_id,
            max_attempts = max_attempts,
            reason = reason,
            total = total,
        );
    }

    // ============================================================
    // 统计查询 — 数据传输量汇总
    // ============================================================

    /// 获取累计发布事件数
    pub fn total_published(&self) -> u64 {
        self.total_published.load(Ordering::Relaxed)
    }

    /// 获取累计接收事件数
    pub fn total_received(&self) -> u64 {
        self.total_received.load(Ordering::Relaxed)
    }

    /// 获取累计错误数
    pub fn total_errors(&self) -> u64 {
        self.total_errors.load(Ordering::Relaxed)
    }

    /// 记录统计摘要(Debug 级别,用于周期性健康检查)
    ///
    /// 日志字段:module、level=DEBUG、operation="stats_summary"、
    /// total_published、total_received、total_errors、throughput_estimate
    pub fn log_stats_summary(&self) {
        let published = self.total_published();
        let received = self.total_received();
        let errors = self.total_errors();

        tracing::debug!(
            module = %self.module_id,
            level = "DEBUG",
            operation = "stats_summary",
            total_published = published,
            total_received = received,
            total_errors = errors,
            "事件总线统计: published={published} received={received} errors={errors}",
            published = published,
            received = received,
            errors = errors,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    fn make_test_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test-module"),
            quest_id: "q-test".into(),
            title: "测试".into(),
            task_count: 1,
        }
    }

    #[test]
    fn test_logger_creation() {
        let logger = BusLogger::new("test-module");
        assert_eq!(logger.total_published(), 0);
        assert_eq!(logger.total_received(), 0);
        assert_eq!(logger.total_errors(), 0);
    }

    #[test]
    fn test_logger_counters() {
        let logger = BusLogger::new("test-module");
        let event = make_test_event();

        // 发布计数
        logger.log_publish(&event, 1);
        assert_eq!(logger.total_published(), 1);

        // 接收计数
        logger.log_recv(&event);
        assert_eq!(logger.total_received(), 1);

        // 错误计数
        logger.log_channel_closed("sub-1");
        assert_eq!(logger.total_errors(), 1);
    }

    #[test]
    fn test_logger_stats_summary() {
        let logger = BusLogger::new("test-module");
        logger.log_publish(&make_test_event(), 0);
        logger.log_recv(&make_test_event());
        logger.log_stats_summary();
        // 统计摘要不应 panic
    }
}
