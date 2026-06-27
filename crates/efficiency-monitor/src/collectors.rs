//! 指标采集器 — 订阅 NexusEvent 并统计发布次数
//!
//! 对应架构层:L9 Quest
//!
//! ## 核心机制
//! - `MetricCollector` trait 定义统一的指标采集接口
//! - `EventMetricCollector` 订阅全部 NexusEvent 变体,按 type_name 分桶统计发布次数
//! - 基于 `DashMap` 实现并发安全的计数器,O(1) 分片查找,Lock-free 读取
//! - `collect()` 产出 `Vec<MetricSample>`,供告警规则引擎与 Prometheus 渲染消费
//!
//! ## 线程安全
//! 内部计数器基于 `Arc<DashMap>`,`Clone` 廉价(仅 Arc 引用计数),
//! 可在 `tokio::spawn` 的后台订阅任务与主线程间自由共享。

use chrono::Utc;
use dashmap::DashMap;
use event_bus::{EventSeverity, NexusEvent};
use std::sync::Arc;

use crate::types::MetricSample;

/// 指标采集器 trait — 统一的指标采集接口
///
/// 实现者负责从特定数据源(如事件流、系统资源)采集指标,
/// 并以 `MetricSample` 列表形式返回当前快照。
pub trait MetricCollector: Send + Sync {
    /// 采集当前指标快照,返回所有指标样本
    fn collect(&self) -> Vec<MetricSample>;
}

/// 事件指标采集器 — 订阅 NexusEvent 并统计发布次数
///
/// 维护三个维度的计数器:
/// - `event_counts`:按事件类型(type_name)统计发布次数
/// - `critical_counts`:按事件类型统计 Critical 严重级别事件次数
/// - `alert_counts`:按告警严重级别统计告警触发次数
///
/// 所有计数器基于 `Arc<DashMap>`,`Clone` 后共享同一份数据,
/// 适合在 `start_event_subscriber` 的后台任务与主线程间共享。
#[derive(Clone)]
pub struct EventMetricCollector {
    /// 按事件类型统计发布次数:type_name -> count
    event_counts: Arc<DashMap<&'static str, u64>>,
    /// 按 Critical 严重级别事件类型统计次数:type_name -> count
    ///
    /// WHY 单独维护:Critical 事件(NexusEvent::severity() == Critical)代表
    /// 系统健康告警,需在 /metrics 输出中单独暴露,便于运维监控。
    critical_counts: Arc<DashMap<&'static str, u64>>,
    /// 按告警严重级别统计触发次数:severity -> count
    alert_counts: Arc<DashMap<&'static str, u64>>,
}

impl EventMetricCollector {
    /// 创建新的事件指标采集器(所有计数器初始化为空)
    pub fn new() -> Self {
        Self {
            event_counts: Arc::new(DashMap::new()),
            critical_counts: Arc::new(DashMap::new()),
            alert_counts: Arc::new(DashMap::new()),
        }
    }

    /// 记录一个事件,更新对应类型的计数器
    ///
    /// 若事件 `severity()` 为 Critical,同时更新 `critical_counts`。
    /// 该方法是同步的,可在 `tokio::spawn` 的后台任务中直接调用。
    pub fn record_event(&self, event: &NexusEvent) {
        let type_name = event.type_name();
        // entry().or_insert().modify() 保证原子性
        let mut entry = self.event_counts.entry(type_name).or_insert(0);
        *entry += 1;
        drop(entry); // 显式释放 shard lock

        if event.severity() == EventSeverity::Critical {
            let mut crit_entry = self.critical_counts.entry(type_name).or_insert(0);
            *crit_entry += 1;
        }
    }

    /// 记录一次告警触发,按严重级别更新计数器
    ///
    /// `severity` 应为 `AlertSeverity::as_str()` 的返回值("info"/"warning"/"critical")。
    pub fn record_alert(&self, severity: &'static str) {
        let mut entry = self.alert_counts.entry(severity).or_insert(0);
        *entry += 1;
    }

    /// 获取所有事件的总发布次数(所有类型求和)
    pub fn total_events(&self) -> u64 {
        self.event_counts.iter().map(|e| *e.value()).sum()
    }

    /// 获取特定类型事件的发布次数
    pub fn event_count(&self, type_name: &str) -> u64 {
        self.event_counts
            .get(type_name)
            .map(|e| *e.value())
            .unwrap_or(0)
    }

    /// 获取 Critical 事件总次数(所有类型求和)
    pub fn total_critical_events(&self) -> u64 {
        self.critical_counts.iter().map(|e| *e.value()).sum()
    }

    /// 获取特定严重级别的告警触发次数
    pub fn alert_count(&self, severity: &str) -> u64 {
        self.alert_counts
            .get(severity)
            .map(|e| *e.value())
            .unwrap_or(0)
    }
}

impl Default for EventMetricCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricCollector for EventMetricCollector {
    fn collect(&self) -> Vec<MetricSample> {
        let now = Utc::now();
        let mut samples = Vec::new();

        // 采集事件计数(nexus_event_total)
        for entry in self.event_counts.iter() {
            samples.push(MetricSample {
                name: "nexus_event_total".to_string(),
                value: *entry.value() as f64,
                labels: vec![("type".to_string(), entry.key().to_string())],
                timestamp: now,
            });
        }

        // 采集 Critical 事件计数(nexus_critical_event_total)
        for entry in self.critical_counts.iter() {
            samples.push(MetricSample {
                name: "nexus_critical_event_total".to_string(),
                value: *entry.value() as f64,
                labels: vec![("type".to_string(), entry.key().to_string())],
                timestamp: now,
            });
        }

        // 采集告警触发计数(nexus_alert_triggered_total)
        for entry in self.alert_counts.iter() {
            samples.push(MetricSample {
                name: "nexus_alert_triggered_total".to_string(),
                value: *entry.value() as f64,
                labels: vec![("severity".to_string(), entry.key().to_string())],
                timestamp: now,
            });
        }

        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn make_event(type_name: &str) -> NexusEvent {
        let meta = EventMetadata::new("test-source");
        match type_name {
            "SkepticVeto" => NexusEvent::SkepticVeto {
                metadata: meta,
                quest_id: "q-1".into(),
                veto_reason: "test".into(),
                frozen_capabilities: vec![],
            },
            "RedTeamAudit" => NexusEvent::RedTeamAudit {
                metadata: meta,
                vulnerability_type: "test".into(),
                failed_probes: 1,
                total_probes: 10,
                detection_rate: 0.1,
                remediation_suggestion: "fix".into(),
            },
            "CacheHit" => NexusEvent::CacheHit {
                metadata: meta,
                cache_key: "k-1".into(),
            },
            "BudgetExceeded" => NexusEvent::BudgetExceeded {
                metadata: meta,
                budget_type: "token".into(),
                current: 100,
                limit: 50,
            },
            _ => panic!("未知事件类型: {type_name}"),
        }
    }

    #[test]
    fn test_record_event_increments_count() {
        let collector = EventMetricCollector::new();
        collector.record_event(&make_event("CacheHit"));
        collector.record_event(&make_event("CacheHit"));
        collector.record_event(&make_event("CacheHit"));

        assert_eq!(collector.event_count("CacheHit"), 3);
        assert_eq!(collector.total_events(), 3);
    }

    #[test]
    fn test_record_critical_event_updates_critical_counts() {
        let collector = EventMetricCollector::new();
        // SkepticVeto 是 Critical 事件
        collector.record_event(&make_event("SkepticVeto"));
        collector.record_event(&make_event("RedTeamAudit"));

        assert_eq!(collector.event_count("SkepticVeto"), 1);
        assert_eq!(collector.event_count("RedTeamAudit"), 1);
        assert_eq!(collector.total_critical_events(), 2);
    }

    #[test]
    fn test_normal_event_does_not_update_critical_counts() {
        let collector = EventMetricCollector::new();
        collector.record_event(&make_event("CacheHit"));
        assert_eq!(collector.total_critical_events(), 0);
    }

    #[test]
    fn test_record_alert_increments_alert_count() {
        let collector = EventMetricCollector::new();
        collector.record_alert("critical");
        collector.record_alert("critical");
        collector.record_alert("warning");

        assert_eq!(collector.alert_count("critical"), 2);
        assert_eq!(collector.alert_count("warning"), 1);
    }

    #[test]
    fn test_collect_returns_samples() {
        let collector = EventMetricCollector::new();
        collector.record_event(&make_event("CacheHit"));
        collector.record_alert("critical");

        let samples = collector.collect();
        // 至少包含 nexus_event_total 和 nexus_alert_triggered_total
        assert!(samples.iter().any(|s| s.name == "nexus_event_total"));
        assert!(samples
            .iter()
            .any(|s| s.name == "nexus_alert_triggered_total"));
    }

    #[test]
    fn test_collect_event_total_sample_has_correct_labels() {
        let collector = EventMetricCollector::new();
        collector.record_event(&make_event("CacheHit"));

        let samples = collector.collect();
        let event_sample = samples
            .iter()
            .find(|s| s.name == "nexus_event_total")
            .expect("应有 nexus_event_total 样本");
        assert_eq!(
            event_sample.labels,
            vec![("type".to_string(), "CacheHit".to_string())]
        );
        assert!((event_sample.value - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_clone_shares_state() {
        let collector = EventMetricCollector::new();
        let cloned = collector.clone();
        // 通过 clone 记录事件,原 collector 应可见
        cloned.record_event(&make_event("CacheHit"));
        assert_eq!(collector.event_count("CacheHit"), 1);
    }

    #[test]
    fn test_budget_exceeded_is_normal_in_event_bus_but_critical_in_monitor() {
        // BudgetExceeded 在 NexusEvent::severity() 中返回 Normal,
        // 但在 efficiency-monitor 中是 Critical 告警事件。
        // 这里验证 record_event 按 severity() 统计 critical_counts,
        // BudgetExceeded 不会进入 critical_counts(因为 severity() == Normal)
        let collector = EventMetricCollector::new();
        collector.record_event(&make_event("BudgetExceeded"));
        // event_counts 应记录
        assert_eq!(collector.event_count("BudgetExceeded"), 1);
        // critical_counts 不应记录(severity() == Normal)
        assert_eq!(collector.total_critical_events(), 0);
    }
}
