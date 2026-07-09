//! 结构化日志埋点 — 事件总线全链路可观测性
//!
//! 对应架构:L1 Core,为所有跨层通信提供 JSON 结构化日志,
//! 确保网络波动/连接异常时可完整追溯问题时间线与根因。
//!
//! # 日志字段规范
//! 每条日志包含:module、level、timestamp、context_id、operation、
//! 以及操作特定字段(如 subscriber_count、event_type、payload_size 等)。
//!
//! # Prometheus 指标(Phase V Task V-8)
//! BusLogger 内置 Prometheus Registry,在 `log_publish` 时自动采集:
//! - `nexus_event_total{topic="..."}`:按 EventTopic 分区的事件发布计数器
//! - `nexus_critical_event_total`:Critical 事件发布计数器
//! - `nexus_event_publish_duration_seconds`:发布耗时直方图(秒)
//!
//! 通过 `render_metrics()` 获取 Prometheus 文本格式输出,用于 /metrics 端点导出。
//!
//! # 使用方式
//! ```no_run
//! # use std::time::Duration;
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
//! logger.log_publish(&event, 0, Duration::ZERO);
//! # }
//! ```

use crate::topic::EventTopic;
use crate::types::{EventSeverity, NexusEvent};
use prometheus_client::encoding::{text::encode, EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{error, info, warn};

/// 可观测性上下文 ID — 用于关联同一操作链路上的所有日志
///
/// 对 publish→recv 链路,使用 event_id 作为 context_id;
/// 对订阅生命周期,使用 subscriber_id。
pub type ContextId = String;

// ============================================================
// Prometheus 标签类型 — topic 标签分区(Phase V Task V-8)
// ============================================================

/// Prometheus 标签值 — EventTopic 的 9 类映射
///
/// WHY 单独定义而非直接复用 EventTopic:
/// 1. prometheus-client 的 `EncodeLabelValue` derive 要求标签类型实现
///    `Hash + Eq + Clone`,枚举变体名直接编码为标签值字符串(如 "Routing")
/// 2. 隔离标签类型与领域类型,避免 EventTopic 未来变更(如新增变体)
///    破坏 Prometheus 指标的向后兼容性
/// 3. `From<EventTopic>` 转换确保两处枚举变体一一对应,编译期可检测遗漏
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
enum TopicLabel {
    Routing,
    Memory,
    Security,
    Execution,
    Parliament,
    Quest,
    System,
    Knowledge,
    Storage,
}

impl From<EventTopic> for TopicLabel {
    fn from(topic: EventTopic) -> Self {
        match topic {
            EventTopic::Routing => TopicLabel::Routing,
            EventTopic::Memory => TopicLabel::Memory,
            EventTopic::Security => TopicLabel::Security,
            EventTopic::Execution => TopicLabel::Execution,
            EventTopic::Parliament => TopicLabel::Parliament,
            EventTopic::Quest => TopicLabel::Quest,
            EventTopic::System => TopicLabel::System,
            EventTopic::Knowledge => TopicLabel::Knowledge,
            EventTopic::Storage => TopicLabel::Storage,
        }
    }
}

/// Prometheus 标签集 — `{topic="..."}` 单标签结构
///
/// `EncodeLabelSet` derive 将字段名 `topic` 作为标签名,
/// 字段值 `TopicLabel` 作为标签值(通过 `EncodeLabelValue` 编码)。
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct TopicLabelSet {
    topic: TopicLabel,
}

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
///
/// # Prometheus 指标(Phase V Task V-8)
/// 内置 Prometheus Registry,在 `log_publish` 时自动采集指标,
/// 通过 `render_metrics()` 导出 Prometheus 文本格式,支持 /metrics 端点。
pub struct BusLogger {
    /// 发布者模块标识(如 "chimera-cli"、"osa-coordinator")
    module_id: String,
    /// 累计发布事件数(原子计数器,跨任务安全)
    total_published: AtomicU64,
    /// 累计接收事件数
    total_received: AtomicU64,
    /// 累计错误数
    total_errors: AtomicU64,
    // --- Prometheus 指标字段(Phase V Task V-8)---
    // WHY 补齐可观测性:既有 AtomicU64 计数器仅通过 tracing 日志输出,
    // 无法被 Prometheus 抓取。新增 Registry + Counter + Histogram 后,
    // 运维可通过 /metrics 端点实时监控事件总线吞吐与延迟。
    /// Prometheus 指标注册表(存储所有已注册指标)
    registry: Registry,
    /// 按 topic 分区的事件发布计数器(Family 自动管理 label set)
    event_total: Family<TopicLabelSet, Counter>,
    /// Critical 事件发布计数器(无标签,总数统计)
    critical_event_total: Counter,
    /// 发布耗时直方图(秒),桶覆盖 100µs ~ 3.3s
    publish_duration: Histogram,
}

impl BusLogger {
    /// 创建日志记录器
    ///
    /// `module_id` 用于标识日志来源,建议使用 crate 名。
    ///
    /// # Prometheus 指标初始化(Phase V Task V-8)
    /// 在构造时注册 3 个指标到内部 Registry,后续 `log_publish` 自动采集:
    /// - `nexus_event_total{topic="..."}`:按 EventTopic 分区的发布计数器
    /// - `nexus_critical_event_total`:Critical 级事件发布计数器
    /// - `nexus_event_publish_duration_seconds`:发布耗时直方图(秒)
    ///
    /// WHY 注册名不带 `_total` 后缀:prometheus-client 对 Counter 类型在
    /// 文本编码时自动追加 `_total`(见 Registry::register 文档),手动追加
    /// 会变成 `_total_total`。注册名 `"nexus_event"` → 输出名 `nexus_event_total`。
    pub fn new(module_id: impl Into<String>) -> Self {
        let mut registry = Registry::default();

        // 1. 按 topic 分区的事件计数器(Family 按需创建 label set)
        let event_total: Family<TopicLabelSet, Counter> = Family::default();
        registry.register(
            "nexus_event",
            "Total events published, partitioned by EventTopic",
            event_total.clone(),
        );

        // 2. Critical 事件计数器(无标签,总数统计)
        let critical_event_total: Counter = Counter::default();
        registry.register(
            "nexus_critical_event",
            "Total critical-severity events published",
            critical_event_total.clone(),
        );

        // 3. 发布耗时直方图(秒)
        // WHY exponential_buckets(0.0001, 2.0, 16):
        // 桶边界 100µs → 200µs → 400µs → ... → ~3.3s。
        // 事件总线发布通常在微秒级(仅 broadcast::send + mpsc::send),
        // 上限 3.3s 覆盖异常慢路径(如 mpsc 积压阻塞)。
        let publish_duration = Histogram::new(exponential_buckets(0.0001, 2.0, 16));
        registry.register(
            "nexus_event_publish_duration_seconds",
            "Time spent publishing an event in seconds",
            publish_duration.clone(),
        );

        Self {
            module_id: module_id.into(),
            total_published: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            registry,
            event_total,
            critical_event_total,
            publish_duration,
        }
    }

    /// 导出 Prometheus 文本格式指标
    ///
    /// 返回符合 Prometheus exposition format 的字符串,
    /// 可直接作为 `/metrics` 端点的 HTTP 响应体。
    /// 输出包含所有已注册指标的 `# HELP`、`# TYPE` 与数据行。
    ///
    /// WHY 返回 String 而非 Vec<u8>:event-bus 是 L1 核心层,
    /// 上层 HTTP 服务器(chimera-cli / mcp-mesh)负责最终字节编码,
    /// 此处返回 String 便于测试断言与日志打印。
    pub fn render_metrics(&self) -> String {
        let mut buf = String::new();
        // encode 失败仅在 fmt::Write 写入失败时发生,String 的 Write 实现
        // 不会返回 Err(内存分配失败会 abort),因此安全忽略。
        let _ = encode(&mut buf, &self.registry);
        buf
    }

    /// 采集 Prometheus 指标(内部辅助方法)
    ///
    /// 在 `log_publish` / `log_publish_with_size` 内部调用,记录:
    /// - `nexus_event_total`:按 EventTopic 分区递增(Family 按需创建标签集)
    /// - `nexus_critical_event_total`:仅 Critical 级事件递增
    /// - `nexus_event_publish_duration_seconds`:观察发布耗时(秒)
    ///
    /// WHY 私有方法:指标采集逻辑与日志埋点强耦合(同一事件需同时记录),
    /// 暴露为公开 API 会导致调用方重复采集或遗漏,封装在 log_* 内部保证一致性。
    fn record_publish_metrics(&self, event: &NexusEvent, duration: Duration) {
        // 1. 按 topic 分区计数:get_or_create 按需创建对应标签集的 Counter
        self.event_total
            .get_or_create(&TopicLabelSet {
                topic: event.topic().into(),
            })
            .inc();

        // 2. Critical 事件计数:仅 Critical 级递增(见 NexusEvent::severity)
        if event.severity() == EventSeverity::Critical {
            self.critical_event_total.inc();
        }

        // 3. 发布耗时直方图(秒):Duration → f64 秒
        self.publish_duration.observe(duration.as_secs_f64());
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
    /// `publish_duration`:本次发布操作的耗时(由调用方 `Instant::now()` 测量),
    /// 用于 Prometheus 直方图 `nexus_event_publish_duration_seconds`。
    /// 传入 `Duration::ZERO` 表示跳过耗时统计(如单元测试或非 publish 路径)。
    ///
    /// # Prometheus 指标采集(Phase V Task V-8)
    /// 调用此方法会自动递增以下指标:
    /// - `nexus_event_total{topic="..."}`:按事件所属 EventTopic 分区
    /// - `nexus_critical_event_total`:仅 Critical 级事件递增
    /// - `nexus_event_publish_duration_seconds`:观察 `publish_duration`
    pub fn log_publish(
        &self,
        event: &NexusEvent,
        subscriber_count: usize,
        publish_duration: Duration,
    ) {
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

        // WHY 指标采集在日志输出之后:日志失败(tracing 层级过滤)不应
        // 阻断指标采集;而指标采集失败(prometheus 内部 infallible)不影响日志。
        // 两者独立,采集放在日志后保持"日志优先"的失败语义。
        self.record_publish_metrics(event, publish_duration);
    }

    /// 记录事件发布(带序列化大小估算)
    ///
    /// 与 `log_publish` 功能相同,额外计算 payload 的 MessagePack 序列化大小,
    /// 用于传输流量统计。仅在非热路径上调用。
    ///
    /// `publish_duration`:同 `log_publish`,本次发布操作的耗时。
    pub fn log_publish_with_size(
        &self,
        event: &NexusEvent,
        subscriber_count: usize,
        publish_duration: Duration,
    ) {
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

        self.record_publish_metrics(event, publish_duration);
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

impl Default for BusLogger {
    /// 默认日志记录器,module_id = "default"
    ///
    /// WHY 提供 Default:`EventBus` 未来可能支持 `with_logger(Default::default())`
    /// 链式构造,且 `#[derive(Default)]` 无法自动派生(含 Registry 等非 Default 字段)。
    /// 显式实现保证构造逻辑与 `new` 一致(注册相同指标集)。
    fn default() -> Self {
        Self::new("default")
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

        // 发布计数(Duration::ZERO:单元测试不测量真实耗时)
        logger.log_publish(&event, 1, Duration::ZERO);
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
        logger.log_publish(&make_test_event(), 0, Duration::ZERO);
        logger.log_recv(&make_test_event());
        logger.log_stats_summary();
        // 统计摘要不应 panic
    }
}
