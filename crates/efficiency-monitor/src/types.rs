//! 核心类型定义 — 指标样本、告警规则与告警事件
//!
//! 对应架构层:L9 Quest
//!
//! ## 类型关系
//! - `MetricSample`:某个时刻的单个指标值快照,用于告警规则检查与 Prometheus 渲染
//! - `AlertRule`:配置化的告警阈值规则,携带比较运算符与 cooldown
//! - `AlertEvent`:规则触发后产生的告警事件,记录触发值与阈值

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 指标样本 — 某时刻的单个指标值
///
/// 由 `MetricCollector::collect()` 产生,供:
/// - `AlertRuleEngine::check()` 进行阈值比较
/// - `dashboard::render_metrics()` 渲染为 Prometheus 文本格式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricSample {
    /// 指标名(如 "nexus_event_total")
    pub name: String,
    /// 指标值
    pub value: f64,
    /// 标签列表(如 [("type", "SkepticVeto")])
    pub labels: Vec<(String, String)>,
    /// 采样时间戳(UTC)
    pub timestamp: DateTime<Utc>,
}

impl MetricSample {
    /// 创建新的指标样本,timestamp 默认当前 UTC 时间
    pub fn new(name: impl Into<String>, value: f64, labels: Vec<(String, String)>) -> Self {
        Self {
            name: name.into(),
            value,
            labels,
            timestamp: Utc::now(),
        }
    }
}

/// 比较运算符 — 用于告警规则阈值比较
///
/// 控制 `AlertRule` 如何将指标值与阈值比较以决定是否触发告警。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Comparison {
    /// 大于:value > threshold
    GreaterThan,
    /// 小于:value < threshold
    LessThan,
    /// 大于等于:value >= threshold
    GreaterOrEqual,
    /// 小于等于:value <= threshold
    LessOrEqual,
}

impl Comparison {
    /// 执行比较,返回是否满足条件
    pub fn compare(&self, value: f64, threshold: f64) -> bool {
        match self {
            Self::GreaterThan => value > threshold,
            Self::LessThan => value < threshold,
            Self::GreaterOrEqual => value >= threshold,
            Self::LessOrEqual => value <= threshold,
        }
    }

    /// 获取比较运算符的字符串表示(用于日志与调试)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GreaterThan => ">",
            Self::LessThan => "<",
            Self::GreaterOrEqual => ">=",
            Self::LessOrEqual => "<=",
        }
    }
}

/// 告警严重级别 — 控制告警的优先级与响应方式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AlertSeverity {
    /// 信息级:仅记录,无需立即响应
    Info,
    /// 警告级:需要关注,但不阻塞执行
    Warning,
    /// 关键级:必须立即响应(如安全/预算红线)
    Critical,
}

impl AlertSeverity {
    /// 获取严重级别的字符串表示(用于 Prometheus 标签与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// 告警规则 — 配置化阈值检测
///
/// 每条规则指定一个指标名的阈值与比较方式,当指标值满足比较条件时触发告警。
/// `cooldown_secs` 防止同一规则在短时间内重复触发告警风暴。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlertRule {
    /// 规则 ID(唯一标识,用于 cooldown 跟踪与日志关联)
    pub rule_id: String,
    /// 监控的指标名(匹配 MetricSample.name)
    pub metric_name: String,
    /// 阈值
    pub threshold: f64,
    /// 比较运算符
    pub comparison: Comparison,
    /// 冷却时间(秒),同一规则在此时间内不重复触发
    pub cooldown_secs: u64,
    /// 告警严重级别
    pub severity: AlertSeverity,
}

impl AlertRule {
    /// 创建新的告警规则,cooldown 默认 60 秒
    ///
    /// # 参数
    /// - `rule_id`:规则唯一标识
    /// - `metric_name`:监控的指标名
    /// - `threshold`:阈值
    /// - `comparison`:比较运算符
    /// - `severity`:告警严重级别
    pub fn new(
        rule_id: impl Into<String>,
        metric_name: impl Into<String>,
        threshold: f64,
        comparison: Comparison,
        severity: AlertSeverity,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            metric_name: metric_name.into(),
            threshold,
            comparison,
            cooldown_secs: 60,
            severity,
        }
    }

    /// 设置冷却时间(秒),返回 self 以便链式调用
    pub fn with_cooldown(mut self, secs: u64) -> Self {
        self.cooldown_secs = secs;
        self
    }
}

/// 告警事件 — 规则触发后产生
///
/// 由 `AlertRuleEngine::check()` 返回,记录触发时的值与阈值。
/// 调用方可据此发布 `EfficiencyAlertTriggered` 事件或执行其他响应动作。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlertEvent {
    /// 触发告警的规则 ID
    pub rule_id: String,
    /// 触发时的实际值
    pub triggered_value: f64,
    /// 规则阈值
    pub threshold: f64,
    /// 触发时间戳(UTC)
    pub timestamp: DateTime<Utc>,
}

impl AlertEvent {
    /// 创建新的告警事件,timestamp 默认当前 UTC 时间
    pub fn new(rule_id: impl Into<String>, triggered_value: f64, threshold: f64) -> Self {
        Self {
            rule_id: rule_id.into(),
            triggered_value,
            threshold,
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_sample_new() {
        let sample = MetricSample::new(
            "nexus_event_total",
            5.0,
            vec![("type".into(), "SkepticVeto".into())],
        );
        assert_eq!(sample.name, "nexus_event_total");
        assert!((sample.value - 5.0).abs() < f64::EPSILON);
        assert_eq!(sample.labels.len(), 1);
    }

    #[test]
    fn test_comparison_greater_than() {
        assert!(Comparison::GreaterThan.compare(5.0, 3.0));
        assert!(!Comparison::GreaterThan.compare(3.0, 5.0));
        assert!(!Comparison::GreaterThan.compare(5.0, 5.0));
    }

    #[test]
    fn test_comparison_less_than() {
        assert!(Comparison::LessThan.compare(3.0, 5.0));
        assert!(!Comparison::LessThan.compare(5.0, 3.0));
    }

    #[test]
    fn test_comparison_greater_or_equal() {
        assert!(Comparison::GreaterOrEqual.compare(5.0, 3.0));
        assert!(Comparison::GreaterOrEqual.compare(5.0, 5.0));
        assert!(!Comparison::GreaterOrEqual.compare(3.0, 5.0));
    }

    #[test]
    fn test_comparison_less_or_equal() {
        assert!(Comparison::LessOrEqual.compare(3.0, 5.0));
        assert!(Comparison::LessOrEqual.compare(5.0, 5.0));
        assert!(!Comparison::LessOrEqual.compare(5.0, 3.0));
    }

    #[test]
    fn test_comparison_as_str() {
        assert_eq!(Comparison::GreaterThan.as_str(), ">");
        assert_eq!(Comparison::LessThan.as_str(), "<");
        assert_eq!(Comparison::GreaterOrEqual.as_str(), ">=");
        assert_eq!(Comparison::LessOrEqual.as_str(), "<=");
    }

    #[test]
    fn test_alert_severity_as_str() {
        assert_eq!(AlertSeverity::Info.as_str(), "info");
        assert_eq!(AlertSeverity::Warning.as_str(), "warning");
        assert_eq!(AlertSeverity::Critical.as_str(), "critical");
    }

    #[test]
    fn test_alert_rule_new_default_cooldown() {
        let rule = AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        );
        assert_eq!(rule.rule_id, "r-1");
        assert_eq!(rule.metric_name, "nexus_event_total");
        assert!((rule.threshold - 10.0).abs() < f64::EPSILON);
        assert_eq!(rule.cooldown_secs, 60);
        assert_eq!(rule.severity, AlertSeverity::Warning);
    }

    #[test]
    fn test_alert_rule_with_cooldown() {
        let rule = AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Critical,
        )
        .with_cooldown(30);
        assert_eq!(rule.cooldown_secs, 30);
    }

    #[test]
    fn test_alert_event_new() {
        let event = AlertEvent::new("r-1", 15.0, 10.0);
        assert_eq!(event.rule_id, "r-1");
        assert!((event.triggered_value - 15.0).abs() < f64::EPSILON);
        assert!((event.threshold - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_types_serde_roundtrip() {
        let rule = AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Critical,
        )
        .with_cooldown(120);
        let json = serde_json::to_string(&rule).expect("序列化失败");
        let restored: AlertRule = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(rule, restored);
    }
}
