//! Prometheus 文本格式渲染 — /metrics 端点输出
//!
//! 对应架构层:L9 Quest
//!
//! ## Grafana 仪表盘
//! 对应的 Grafana 仪表盘配置文件位于 `docs/grafana/dashboard.json`,
//! 部署说明参见 `docs/grafana/README.md`。
//! 指标名称与本模块 `render_metrics()` 输出的 Prometheus 文本格式一一对应。
//!
//! ## 输出格式
//! 遵循 Prometheus exposition format:
//! ```text
//! # HELP nexus_event_total Total NexusEvent published by type
//! # TYPE nexus_event_total counter
//! nexus_event_total{type="SkepticVeto"} 5
//! nexus_event_total{type="RedTeamAudit"} 3
//! # HELP nexus_alert_triggered_total Total alerts triggered
//! # TYPE nexus_alert_triggered_total counter
//! nexus_alert_triggered_total{severity="critical"} 2
//! ```
//!
//! ## 设计决策
//! 手动实现文本格式渲染而非使用 prometheus-client 的 encoder,因为:
//! 1. 任务要求的指标类型简单(仅 counter),手动渲染更清晰可控
//! 2. 避免引入 prometheus-client Family 泛型类型的复杂性
//! 3. 输出格式完全可控,便于测试与调试
//!
//! prometheus-client 依赖保留在 Cargo.toml 中,供未来扩展(如 histogram)使用。

use std::collections::BTreeMap;

use crate::collectors::MetricCollector;
use crate::types::MetricSample;

/// 指标元数据:HELP 文本与 TYPE 类型
struct MetricMeta {
    help: &'static str,
    metric_type: &'static str,
}

/// 获取指标名对应的元数据
///
/// 返回已知指标的 (help, type),未知指标返回默认元数据。
fn get_metric_meta(name: &str) -> MetricMeta {
    match name {
        "nexus_event_total" => MetricMeta {
            help: "Total NexusEvent published by type",
            metric_type: "counter",
        },
        "nexus_critical_event_total" => MetricMeta {
            help: "Total Critical NexusEvent published by type",
            metric_type: "counter",
        },
        "nexus_alert_triggered_total" => MetricMeta {
            help: "Total alerts triggered",
            metric_type: "counter",
        },
        _ => MetricMeta {
            help: "Unknown metric",
            metric_type: "untyped",
        },
    }
}

/// 转义标签值中的特殊字符
///
/// Prometheus 文本格式要求转义:`\` -> `\\`,`"` -> `\"`,`\n` -> `\n`
fn escape_label_value(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 渲染标签列表为 Prometheus 文本格式
///
/// 输入:`[("type", "SkepticVeto"), ("severity", "critical")]`
/// 输出:`{type="SkepticVeto",severity="critical"}`
///
/// 无标签时返回空字符串。
fn render_labels(labels: &[(String, String)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", escape_label_value(v)))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

/// 将样本列表渲染为 Prometheus 文本格式
///
/// 按 metric name 分组输出,每组包含 HELP、TYPE 行与若干数据行。
/// 数据行格式:`{name}{labels} {value}`
pub fn render_samples(samples: &[MetricSample]) -> String {
    let mut output = String::new();

    // 按 metric name 分组(BTreeMap 保证输出顺序稳定,便于测试)
    let mut groups: BTreeMap<&str, Vec<&MetricSample>> = BTreeMap::new();
    for sample in samples {
        groups.entry(sample.name.as_str()).or_default().push(sample);
    }

    for (name, group) in &groups {
        let meta = get_metric_meta(name);
        // 输出 HELP 与 TYPE 行
        output.push_str(&format!("# HELP {name} {}\n", meta.help));
        output.push_str(&format!("# TYPE {name} {}\n", meta.metric_type));

        // 输出每个样本的数据行
        for sample in group {
            let labels_str = render_labels(&sample.labels);
            // counter 类型输出整数(符合 Prometheus 惯例)
            let value_str = if sample.value.fract() == 0.0 {
                format!("{}", sample.value as u64)
            } else {
                format!("{}", sample.value)
            };
            output.push_str(&format!("{name}{labels_str} {value_str}\n"));
        }
    }

    output
}

/// 从 MetricCollector 采集并渲染 Prometheus 文本格式
///
/// 便捷方法:等价于 `render_samples(&collector.collect())`。
pub fn render_metrics(collector: &dyn MetricCollector) -> String {
    let samples = collector.collect();
    render_samples(&samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_sample(name: &str, value: f64, labels: Vec<(&str, &str)>) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            value,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_escape_label_value_plain() {
        assert_eq!(escape_label_value("SkepticVeto"), "SkepticVeto");
    }

    #[test]
    fn test_escape_label_value_with_quotes() {
        assert_eq!(escape_label_value(r#"a"b"#), r#"a\"b"#);
    }

    #[test]
    fn test_escape_label_value_with_backslash() {
        assert_eq!(escape_label_value(r"a\b"), r"a\\b");
    }

    #[test]
    fn test_escape_label_value_with_newline() {
        assert_eq!(escape_label_value("a\nb"), r"a\nb");
    }

    #[test]
    fn test_render_labels_empty() {
        assert_eq!(render_labels(&[]), "");
    }

    #[test]
    fn test_render_labels_single() {
        let labels = vec![("type".to_string(), "SkepticVeto".to_string())];
        assert_eq!(render_labels(&labels), r#"{type="SkepticVeto"}"#);
    }

    #[test]
    fn test_render_labels_multiple() {
        let labels = vec![
            ("type".to_string(), "SkepticVeto".to_string()),
            ("severity".to_string(), "critical".to_string()),
        ];
        assert_eq!(
            render_labels(&labels),
            r#"{type="SkepticVeto",severity="critical"}"#
        );
    }

    #[test]
    fn test_render_samples_empty() {
        let output = render_samples(&[]);
        assert!(output.is_empty());
    }

    #[test]
    fn test_render_samples_single_metric() {
        let samples = vec![make_sample(
            "nexus_event_total",
            5.0,
            vec![("type", "SkepticVeto")],
        )];
        let output = render_samples(&samples);
        assert!(output.contains("# HELP nexus_event_total Total NexusEvent published by type"));
        assert!(output.contains("# TYPE nexus_event_total counter"));
        assert!(output.contains(r#"nexus_event_total{type="SkepticVeto"} 5"#));
    }

    #[test]
    fn test_render_samples_multiple_metrics_grouped() {
        let samples = vec![
            make_sample("nexus_event_total", 5.0, vec![("type", "SkepticVeto")]),
            make_sample("nexus_event_total", 3.0, vec![("type", "RedTeamAudit")]),
            make_sample(
                "nexus_alert_triggered_total",
                2.0,
                vec![("severity", "critical")],
            ),
        ];
        let output = render_samples(&samples);

        // 验证分组:同一 metric name 的样本应连续输出
        // BTreeMap 按 metric name 字母序输出:nexus_alert_triggered_total < nexus_event_total
        let alert_pos = output
            .find("nexus_alert_triggered_total{severity=\"critical\"}")
            .unwrap();
        let event_pos = output
            .find("nexus_event_total{type=\"SkepticVeto\"}")
            .unwrap();
        let event_pos2 = output
            .find("nexus_event_total{type=\"RedTeamAudit\"}")
            .unwrap();

        // alert 组在 event 组之前(字母序:'a' < 'e')
        assert!(alert_pos < event_pos);
        // 同组内样本连续(event_pos < event_pos2)
        assert!(event_pos < event_pos2);
        // 同组样本之间不应混入其他 metric 的数据行
        let event_section = &output[event_pos..event_pos2];
        assert!(
            !event_section.contains("nexus_alert_triggered_total{"),
            "同组样本应连续,不应混入其他 metric 数据行"
        );
    }

    #[test]
    fn test_render_samples_integer_value() {
        let samples = vec![make_sample(
            "nexus_event_total",
            10.0,
            vec![("type", "CacheHit")],
        )];
        let output = render_samples(&samples);
        assert!(output.contains("nexus_event_total{type=\"CacheHit\"} 10\n"));
    }

    #[test]
    fn test_render_samples_float_value() {
        let samples = vec![make_sample("custom_metric", 10.5, vec![])];
        let output = render_samples(&samples);
        assert!(output.contains("custom_metric 10.5"));
    }

    #[test]
    fn test_render_samples_unknown_metric_untyped() {
        let samples = vec![make_sample("unknown_metric", 1.0, vec![])];
        let output = render_samples(&samples);
        assert!(output.contains("# TYPE unknown_metric untyped"));
    }

    #[test]
    fn test_render_metrics_from_collector() {
        use crate::collectors::EventMetricCollector;
        use event_bus::{EventMetadata, NexusEvent};

        let collector = EventMetricCollector::new();
        let event = NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k-1".into(),
        };
        collector.record_event(&event);
        collector.record_alert("critical");

        let output = render_metrics(&collector);
        assert!(output.contains("nexus_event_total"));
        assert!(output.contains("nexus_alert_triggered_total"));
        assert!(output.contains(r#"severity="critical""#));
    }
}
