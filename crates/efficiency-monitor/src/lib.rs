//! 效率监控与告警 — 实时采集执行指标并触发告警
//!
//! 对应架构层：L9 Quest
//! 对应创新点：无（任务层监控基础设施）
//!
//! ## 核心职责
//! - 订阅全部 NexusEvent 变体，按 type_name 统计发布次数
//! - Critical 事件（SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded）立即告警
//! - 配置化 AlertRule 阈值检测，cooldown 防抖
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
//! // 添加告警规则：Critical 事件 > 0 时告警
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

/// MCA A5 亲和指标采集 — 体验对等不变量(E1-E5)度量(ADR-065/066)
pub mod affinity_metrics;
pub mod alerts;
/// polish-v2.7 P1-1: RuntimeAuditor 运行时自我评估(ADR-049 决策 1)
///
/// 证据纪律 + 五维度 Harness 报告,只读观察者零执行路径侵入。
pub mod auditor;
pub mod collectors;
pub mod config;
pub mod dashboard;
pub mod error;
pub mod monitor;
pub mod oscillation_detector;
pub mod types;

// === 关键类型重导出，简化外部导入 ===
pub use affinity_metrics::AffinityMetrics;
pub use alerts::AlertRuleEngine;
// polish-v2.7 P1-1: RuntimeAuditor 公开 API 重导出
pub use auditor::{
    EvidenceKind, Finding, FindingCategory, FindingSeverity, HarnessReport, RuntimeAuditor,
};
pub use collectors::{EventMetricCollector, MetricCollector};
pub use config::MonitorConfig;
pub use error::MonitorError;
pub use monitor::{EfficiencyMonitor, CRITICAL_DROPPED_METRIC_NAME};
pub use oscillation_detector::{
    OscillationConfig, OscillationReport, PolicyOscillationDetector, DEFAULT_HIGH_FREQ_THRESHOLD,
    DEFAULT_OSCILLATION_THRESHOLD, DEFAULT_SEVERITY_ALERT_THRESHOLD, DEFAULT_WINDOW_SECS,
};
pub use types::{AlertEvent, AlertRule, AlertSeverity, Comparison, MetricSample};
