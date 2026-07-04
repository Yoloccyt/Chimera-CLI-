//! 效率监控与告警 — 实时采集执行指标并触发告警
//!
//! 对应架构层:L9 Quest
//! 对应创新点:无(任务层监控基础设施)
//!
//! ## 核心职责
//! - 订阅全部 NexusEvent 变体,按 type_name 统计发布次数
//! - Critical 事件(SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded)立即告警
//! - 配置化 AlertRule 阈值检测,cooldown 防抖
//! - 输出 Prometheus 文本格式 /metrics 端点
//! - 触发告警时发布 EfficiencyAlertTriggered 事件
//!
//! ## 快速示例
//! ```no_run
//! use efficiency_monitor::{
//!     EfficiencyMonitor, MonitorConfig, AlertRule, Comparison, AlertSeverity,
//! };
//! use event_bus::{EventBus, EventMetadata, NexusEvent};
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());
//!
//! // 添加告警规则:Critical 事件 > 0 时告警
//! monitor.add_alert_rule(AlertRule::new(
//!     "critical-alert",
//!     "nexus_critical_event_total",
//!     0.0,
//!     Comparison::GreaterThan,
//!     AlertSeverity::Critical,
//! ));
//!
//! // 同步记录事件
//! let event = NexusEvent::CacheHit {
//!     metadata: EventMetadata::new("test"),
//!     cache_key: "k-1".into(),
//! };
//! monitor.record_event(&event);
//!
//! // 检查告警
//! let alerts = monitor.check_alerts();
//!
//! // 渲染 Prometheus /metrics 输出
//! let metrics_output = monitor.render_metrics();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod alerts;
pub mod collectors;
pub mod config;
pub mod dashboard;
pub mod error;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use alerts::AlertRuleEngine;
pub use collectors::{EventMetricCollector, MetricCollector};
pub use config::MonitorConfig;
pub use error::MonitorError;
pub use types::{AlertEvent, AlertRule, AlertSeverity, Comparison, MetricSample};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;

/// efficiency-monitor 的 source 标识(用于 EventMetadata)
const MONITOR_SOURCE: &str = "efficiency-monitor";

/// 判断事件是否为 Critical 告警事件(必须立即告警)
///
/// 注意:这与 `NexusEvent::severity()` 部分重叠但语义不同。
/// - `NexusEvent::severity()` 是事件总线的背压级别:SkepticVeto/RedTeamAudit/
///   BudgetExceeded 为 Critical(F-001 修复后),AsaIntervention 仍为 Normal
/// - `is_critical_alert_event` 是 efficiency-monitor 的告警级别(4 个事件均为 Critical)
///
/// WHY 单独定义:AsaIntervention 在 event-bus 中返回 Normal
/// (因为 severity() 是同步函数不依赖运行时值),但在 efficiency-monitor 中
/// 代表安全红线,必须立即告警。F-001 修复后 BudgetExceeded 在两层都是 Critical,
/// 此处保留匹配是出于对称性与稳定性——即使未来 event-bus 的 severity 分类变化,
/// efficiency-monitor 的告警语义也不受影响。
fn is_critical_alert_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::AsaIntervention { .. }
            | NexusEvent::BudgetExceeded { .. }
    )
}

/// 效率监控器 — 整合采集器、告警引擎与事件总线
///
/// 持有四个核心组件:
/// - `config`:监控配置(采集间隔、cooldown、Critical 立即告警开关)
/// - `collectors`:事件指标采集器(按 type_name 统计发布次数)
/// - `alert_engine`:告警规则引擎(配置化阈值检测 + cooldown 防抖)
/// - `event_bus`:可选事件总线(订阅 NexusEvent + 发布 EfficiencyAlertTriggered)
pub struct EfficiencyMonitor {
    /// 监控配置
    config: MonitorConfig,
    /// 事件指标采集器(Clone 廉价,基于 `Arc<DashMap>`)
    collectors: EventMetricCollector,
    /// 告警规则引擎(Clone 廉价,基于 `Arc<DashMap>`)
    alert_engine: AlertRuleEngine,
    /// 可选事件总线(订阅事件 + 发布告警)
    event_bus: Option<EventBus>,
}

impl EfficiencyMonitor {
    /// 创建效率监控器(无 EventBus,不订阅事件也不发布告警)
    ///
    /// 适用场景:单元测试、仅需要同步记录事件与渲染 /metrics 的场景。
    pub fn new(config: MonitorConfig) -> Self {
        Self {
            config,
            collectors: EventMetricCollector::new(),
            alert_engine: AlertRuleEngine::new(),
            event_bus: None,
        }
    }

    /// 创建效率监控器并绑定 EventBus
    ///
    /// 绑定后:
    /// - `record_event` 中触发的 Critical 告警会发布 `EfficiencyAlertTriggered` 事件
    /// - `check_alerts` 触发的规则告警会发布 `EfficiencyAlertTriggered` 事件
    /// - `start_event_subscriber` 可启动后台订阅循环
    pub fn with_event_bus(config: MonitorConfig, bus: EventBus) -> Self {
        Self {
            config,
            collectors: EventMetricCollector::new(),
            alert_engine: AlertRuleEngine::new(),
            event_bus: Some(bus),
        }
    }

    /// 同步记录一个事件,更新指标计数器
    ///
    /// 若配置启用 `critical_instant_alert` 且事件为 Critical 告警事件
    /// (SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded),
    /// 将立即记录 Critical 告警计数并发布 `EfficiencyAlertTriggered` 事件。
    ///
    /// 该方法是同步的,适合在不便 await 的场景调用。
    pub fn record_event(&self, event: &NexusEvent) {
        // 记录事件指标
        self.collectors.record_event(event);

        // Critical 事件立即告警(绕过规则引擎,直接触发)
        if self.config.critical_instant_alert && is_critical_alert_event(event) {
            self.collectors
                .record_alert(AlertSeverity::Critical.as_str());
            self.publish_critical_alert(event);
        }
    }

    /// 添加一条告警规则
    ///
    /// 添加后,`check_alerts` 会按规则阈值检查指标样本。
    pub fn add_alert_rule(&self, rule: AlertRule) {
        self.alert_engine.add_rule(rule);
    }

    /// 检查所有告警规则,返回触发的告警事件
    ///
    /// # 流程
    /// 1. 从采集器收集当前指标快照
    /// 2. 用告警规则引擎检查快照(考虑 cooldown)
    /// 3. 对每个触发的告警,记录告警计数并发布 `EfficiencyAlertTriggered` 事件
    ///
    /// 返回的 `Vec<AlertEvent>` 不包含 Critical 立即告警(那些在 `record_event` 中处理)。
    pub fn check_alerts(&self) -> Vec<AlertEvent> {
        let samples = self.collectors.collect();
        let alerts = self.alert_engine.check(&samples);

        // 对每个触发的告警,记录计数并发布事件
        for alert in &alerts {
            if let Some((metric_name, severity)) = self.alert_engine.get_rule_info(&alert.rule_id) {
                self.collectors.record_alert(severity.as_str());
                self.publish_rule_alert(
                    &alert.rule_id,
                    &metric_name,
                    alert.triggered_value,
                    alert.threshold,
                );
            }
        }

        alerts
    }

    /// 渲染 Prometheus 文本格式的 /metrics 输出
    ///
    /// 输出格式遵循 Prometheus exposition format,包含:
    /// - `nexus_event_total`:按事件类型分桶的发布次数
    /// - `nexus_critical_event_total`:按事件类型分桶的 Critical 事件次数
    /// - `nexus_alert_triggered_total`:按严重级别分桶的告警触发次数
    pub fn render_metrics(&self) -> String {
        dashboard::render_metrics(&self.collectors)
    }

    /// 启动后台事件订阅循环
    ///
    /// 在 `tokio::spawn` 之前同步调用 `bus.subscribe()` 与
    /// `bus.subscribe_critical_events()`,确保不会错过后续发布的事件
    /// (Week 6 教训:broadcast 时序;§4.4 反模式 3)。
    ///
    /// # 双通道消费(§6.2 红线,2026-06-29)
    /// 后台任务通过 `tokio::select!` 同时消费两个通道,职责互斥避免 double-count:
    /// - **broadcast 主通道**:接收全部事件,**仅记录事件指标**(不触发告警)
    /// - **critical mpsc 旁路**:接收 4 类 Critical 安全告警事件
    ///   (SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded),
    ///   **仅触发 Critical 告警**(不记录事件指标)。
    ///   mpsc Unbounded 不会 Lagged,broadcast 丢弃时仍确保告警触发。
    ///
    /// WHY 职责拆分:同一 Critical 事件会双投递到两个通道(broadcast + mpsc),
    /// 若两条通道都触发告警会导致 double-count。拆分后 broadcast 负责指标、
    /// mpsc 负责告警,即使 broadcast Lagged 仅损失指标,告警必达(§6.2 红线)。
    ///
    /// # 错误
    /// 返回 `MonitorError::Config` 若未绑定 EventBus。
    ///
    /// # 注意
    /// 调用方必须在 tokio runtime 上下文中调用此方法。
    pub fn start_event_subscriber(&self) -> Result<(), MonitorError> {
        let bus = self.event_bus.clone().ok_or_else(|| MonitorError::Config {
            reason: "未绑定 EventBus,无法启动事件订阅".into(),
        })?;

        let collectors = self.collectors.clone();
        let bus_for_alerts = bus.clone();
        let critical_enabled = self.config.critical_instant_alert;

        // 在 spawn 之前同步订阅两个通道,确保不会错过后续发布的事件
        // WHY: tokio::broadcast 仅投递给发布时已存在的 receiver;
        // 若在 spawn 的 async block 内 subscribe,后台任务调度时机不确定,
        // 可能晚于 publish 导致事件静默丢失(broadcast 不缓存历史消息给新订阅者)
        // WHY mpsc 旁路同步订阅:§4.4 反模式 3,与 broadcast 同理;
        // Critical 安全告警事件必须确保投递,不能因 spawn 时序丢失
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical_events();

        // WHY fire-and-forget(B-Min-2 评估):事件订阅器为应用生命周期任务,
        // 随进程退出自动终止。panic 时 tokio 运行时回收资源,不影响监控数据完整性
        // (collectors 为 Arc 共享,下一轮订阅周期会重新记录)。
        tokio::spawn(async move {
            // 双通道消费循环:broadcast 主流 + mpsc 旁路兜底
            // WHY tokio::select!:同时 await broadcast recv 与 mpsc recv,
            // 哪个先就绪就处理哪个,实现双通道并行消费。select! 是 tokio 标准
            // 模式,无需额外依赖,符合 §4.1 workspace 依赖规范。
            // WHY mpsc 旁路处理:即使在 broadcast Lagged 场景下,Critical 事件
            // 仍需触发告警记录与 EfficiencyAlertTriggered 发布,确保运维感知
            loop {
                tokio::select! {
                    // mpsc 旁路:Critical 安全告警事件(broadcast Lagged 兜底)
                    Some(critical_event) = critical_rx.recv() => {
                        handle_critical_event(
                            &collectors,
                            &bus_for_alerts,
                            critical_enabled,
                            &critical_event,
                        );
                    }
                    // broadcast 主流:全部事件(含 Critical,向后兼容)
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                handle_broadcast_event(
                                    &collectors,
                                    &bus_for_alerts,
                                    critical_enabled,
                                    &event,
                                );
                            }
                            Err(e) => {
                                // SlowConsumerDropped/RecvTimeout:继续循环等新事件
                                // ChannelClosed:所有 Sender 已 drop,退出循环
                                if matches!(e, event_bus::EventBusError::ChannelClosed) {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// 获取配置引用
    pub fn config(&self) -> &MonitorConfig {
        &self.config
    }

    /// 获取采集器引用(用于直接查询计数)
    pub fn collectors(&self) -> &EventMetricCollector {
        &self.collectors
    }

    /// 获取告警引擎引用(用于直接管理规则)
    pub fn alert_engine(&self) -> &AlertRuleEngine {
        &self.alert_engine
    }

    /// 发布 Critical 事件立即告警(同步,使用 publish_blocking)
    ///
    /// 构造 `EfficiencyAlertTriggered` 事件并通过 `publish_blocking` 发布。
    /// 发布失败仅记录日志,不阻塞调用方。
    fn publish_critical_alert(&self, event: &NexusEvent) {
        let Some(bus) = &self.event_bus else {
            return;
        };

        let type_name = event.type_name();
        let alert_event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new(MONITOR_SOURCE),
            rule_id: format!("critical-{type_name}"),
            metric_name: type_name.to_string(),
            triggered_value: 1.0,
            threshold: 1.0,
        };

        if let Err(e) = bus.publish_blocking(alert_event) {
            warn!(error = %e, event_type = type_name, "发布 Critical 告警事件失败");
        }
    }

    /// 发布规则触发的告警(同步,使用 publish_blocking)
    ///
    /// 构造 `EfficiencyAlertTriggered` 事件并通过 `publish_blocking` 发布。
    /// 发布失败仅记录日志,不阻塞调用方。
    fn publish_rule_alert(
        &self,
        rule_id: &str,
        metric_name: &str,
        triggered_value: f64,
        threshold: f64,
    ) {
        let Some(bus) = &self.event_bus else {
            return;
        };

        let alert_event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new(MONITOR_SOURCE),
            rule_id: rule_id.to_string(),
            metric_name: metric_name.to_string(),
            triggered_value,
            threshold,
        };

        if let Err(e) = bus.publish_blocking(alert_event) {
            warn!(error = %e, rule_id = rule_id, "发布规则告警事件失败");
        }
    }
}

impl Default for EfficiencyMonitor {
    fn default() -> Self {
        Self::new(MonitorConfig::default())
    }
}

/// 在后台任务中处理 broadcast 主流事件(全部事件)
///
/// 记录事件指标(所有事件,含 Critical)。**不触发告警** — Critical 告警
/// 逻辑委托给 [`handle_critical_event`] 通过 mpsc 旁路处理,避免同一
/// Critical 事件被 double-count(broadcast + mpsc 旁路双投递)。
///
/// WHY 拆分职责:broadcast 主流负责事件指标记录(所有事件),
/// mpsc 旁路负责 Critical 告警触发(仅 4 类事件)。两条通道职责互斥,
/// 即使 broadcast Lagged 导致事件指标缺失,Critical 告警仍由 mpsc 旁路
/// 确保触发(§6.2 红线:Critical 安全事件必须确保送达)。
fn handle_broadcast_event(
    collectors: &EventMetricCollector,
    _bus: &EventBus,
    _critical_enabled: bool,
    event: &NexusEvent,
) {
    // 仅记录事件指标,告警逻辑委托给 handle_critical_event(mpsc 旁路)
    collectors.record_event(event);
}

/// 在后台任务中处理 mpsc 旁路 Critical 事件(Critical 告警触发)
///
/// §6.2 红线:Critical 安全告警事件通过 mpsc 旁路确保投递。此函数是
/// Critical 告警的**唯一触发点** — broadcast 主流不再触发告警,
/// 避免同一事件被 double-count。mpsc 旁路 Unbounded 不会 Lagged,
/// 确保 broadcast 丢弃时 Critical 告警仍能触发。
///
/// WHY 不调用 record_event:event 指标已由 broadcast 主流的
/// `handle_broadcast_event` 记录(若未 Lagged);若 broadcast Lagged,
/// event 指标缺失但告警仍触发(可接受取舍:告警优先于指标)。
fn handle_critical_event(
    collectors: &EventMetricCollector,
    bus: &EventBus,
    critical_enabled: bool,
    event: &NexusEvent,
) {
    // mpsc 旁路仅投递 4 类 Critical 安全告警事件,无需再判 is_critical_alert_event
    // WHY 不调用 record_event:避免与 broadcast 主流 double-count event 指标
    if critical_enabled {
        collectors.record_alert(AlertSeverity::Critical.as_str());
        publish_critical_alert_blocking(bus, event);
    }
}

/// 在后台任务中发布 Critical 事件立即告警(异步上下文,使用 publish_blocking)
///
/// WHY 使用 publish_blocking:后台订阅循环中不便 await(会影响后续事件接收),
/// publish_blocking 是同步发送,不会阻塞事件循环。
fn publish_critical_alert_blocking(bus: &EventBus, event: &NexusEvent) {
    let type_name = event.type_name();
    let alert_event = NexusEvent::EfficiencyAlertTriggered {
        metadata: EventMetadata::new(MONITOR_SOURCE),
        rule_id: format!("critical-{type_name}"),
        metric_name: type_name.to_string(),
        triggered_value: 1.0,
        threshold: 1.0,
    };

    if let Err(e) = bus.publish_blocking(alert_event) {
        warn!(error = %e, event_type = type_name, "后台任务发布 Critical 告警事件失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn make_skeptic_veto() -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            veto_reason: "test".into(),
            frozen_capabilities: vec![],
        }
    }

    fn make_red_team_audit() -> NexusEvent {
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("parliament"),
            vulnerability_type: "test".into(),
            failed_probes: 1,
            total_probes: 10,
            detection_rate: 0.1,
            remediation_suggestion: "fix".into(),
        }
    }

    fn make_asa_intervention() -> NexusEvent {
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("seccore"),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("unsafe".into()),
            alternative_suggestion: None,
        }
    }

    fn make_budget_exceeded() -> NexusEvent {
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 100,
            limit: 50,
        }
    }

    fn make_cache_hit() -> NexusEvent {
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k-1".into(),
        }
    }

    #[test]
    fn test_is_critical_alert_event_skeptic_veto() {
        assert!(is_critical_alert_event(&make_skeptic_veto()));
    }

    #[test]
    fn test_is_critical_alert_event_red_team_audit() {
        assert!(is_critical_alert_event(&make_red_team_audit()));
    }

    #[test]
    fn test_is_critical_alert_event_asa_intervention() {
        assert!(is_critical_alert_event(&make_asa_intervention()));
    }

    #[test]
    fn test_is_critical_alert_event_budget_exceeded() {
        assert!(is_critical_alert_event(&make_budget_exceeded()));
    }

    #[test]
    fn test_is_critical_alert_event_normal_event() {
        assert!(!is_critical_alert_event(&make_cache_hit()));
    }

    #[test]
    fn test_new_without_event_bus() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        assert!(monitor.event_bus.is_none());
    }

    #[test]
    fn test_with_event_bus_binds_bus() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
        assert!(monitor.event_bus.is_some());
    }

    #[test]
    fn test_record_event_updates_collectors() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());
        monitor.record_event(&make_cache_hit());

        assert_eq!(monitor.collectors().event_count("CacheHit"), 2);
    }

    #[test]
    fn test_record_critical_event_records_alert_count() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_skeptic_veto());

        // Critical 事件应记录 critical 告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 1);
    }

    #[test]
    fn test_record_normal_event_does_not_record_alert() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());

        // Normal 事件不应记录告警
        assert_eq!(monitor.collectors().alert_count("critical"), 0);
        assert_eq!(monitor.collectors().alert_count("warning"), 0);
    }

    #[test]
    fn test_record_critical_event_with_disabled_instant_alert() {
        let config = MonitorConfig {
            critical_instant_alert: false,
            ..MonitorConfig::default()
        };
        let monitor = EfficiencyMonitor::new(config);
        monitor.record_event(&make_skeptic_veto());

        // 禁用立即告警后,Critical 事件不应记录告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 0);
        // 但事件计数仍应记录
        assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    }

    #[test]
    fn test_add_alert_rule() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
        assert_eq!(monitor.alert_engine().rule_count(), 1);
    }

    #[test]
    fn test_check_alerts_returns_triggered() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        // 记录 6 次 CacheHit 事件
        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let alerts = monitor.check_alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "r-1");
    }

    #[test]
    fn test_check_alerts_records_alert_count() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let _ = monitor.check_alerts();
        // 应记录 warning 告警计数
        assert_eq!(monitor.collectors().alert_count("warning"), 1);
    }

    #[test]
    fn test_render_metrics_contains_event_total() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());

        let output = monitor.render_metrics();
        assert!(output.contains("nexus_event_total"));
        assert!(output.contains(r#"type="CacheHit""#));
    }

    #[test]
    fn test_render_metrics_contains_alert_triggered() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_skeptic_veto()); // 触发 Critical 告警

        let output = monitor.render_metrics();
        assert!(output.contains("nexus_alert_triggered_total"));
        assert!(output.contains(r#"severity="critical""#));
    }

    #[test]
    fn test_start_event_subscriber_without_bus_returns_error() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let result = monitor.start_event_subscriber();
        assert!(matches!(result, Err(MonitorError::Config { .. })));
    }

    #[tokio::test]
    async fn test_record_critical_event_publishes_alert_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

        // 记录 Critical 事件,应发布 EfficiencyAlertTriggered
        monitor.record_event(&make_skeptic_veto());

        // 接收并验证事件
        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::EfficiencyAlertTriggered {
                rule_id,
                metric_name,
                triggered_value,
                threshold,
                ..
            } => {
                assert!(rule_id.contains("critical"));
                assert_eq!(metric_name, "SkepticVeto");
                assert!((triggered_value - 1.0).abs() < f64::EPSILON);
                assert!((threshold - 1.0).abs() < f64::EPSILON);
            }
            _ => panic!("期望 EfficiencyAlertTriggered 事件,收到 {:?}", event),
        }
    }

    #[tokio::test]
    async fn test_check_alerts_publishes_alert_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let alerts = monitor.check_alerts();
        assert_eq!(alerts.len(), 1);

        // 接收告警事件
        let event = rx.recv().await.expect("应收到告警事件");
        match event {
            NexusEvent::EfficiencyAlertTriggered { rule_id, .. } => {
                assert_eq!(rule_id, "r-1");
            }
            _ => panic!("期望 EfficiencyAlertTriggered 事件"),
        }
    }

    #[tokio::test]
    async fn test_start_event_subscriber_receives_events() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布事件
        bus.publish(make_cache_hit()).await.expect("发布失败");

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证事件被记录
        assert_eq!(monitor.collectors().event_count("CacheHit"), 1);
    }

    #[tokio::test]
    async fn test_start_event_subscriber_critical_alert() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布 Critical 事件
        bus.publish(make_skeptic_veto()).await.expect("发布失败");

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证事件被记录
        assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
        // 验证 Critical 告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 1);
    }

    #[test]
    fn test_default_uses_default_config() {
        let monitor = EfficiencyMonitor::default();
        assert_eq!(monitor.config().collect_interval_ms, 1000);
        assert!(monitor.config().critical_instant_alert);
    }
}
