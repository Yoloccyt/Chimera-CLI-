//! efficiency-monitor 属性测试 — 监控指标聚合不变量
//!
//! 对应架构层:L9 Quest
//! 对应 SubTask 13.5:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. Comparison::compare 语义正确(严格/非严格比较)
//! 2. AlertRule::new 默认 cooldown=60s,with_cooldown 覆盖默认值
//! 3. EventMetricCollector:record_event N 次 → event_count == N(单调递增)
//! 4. EventMetricCollector:record_alert 累加 alert_count
//! 5. AlertSeverity::as_str 三个变体返回互不相同的字符串
//!
//! # 设计要点
//! - 使用 CacheHit(Normal 事件)避免触发 Critical 告警副作用
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use efficiency_monitor::{AlertRule, AlertSeverity, Comparison, EventMetricCollector};
use event_bus::{EventMetadata, NexusEvent};
use proptest::prelude::*;

/// 构造 CacheHit 事件(Normal 级别,不触发 Critical 告警副作用)
fn make_cache_hit(key: &str) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("test-source"),
        cache_key: key.into(),
    }
}

proptest! {
    /// 不变量 1:Comparison::compare 语义正确
    ///
    /// 验证四种比较运算符的语义:
    /// - GreaterThan: value > threshold(严格)
    /// - LessThan: value < threshold(严格)
    /// - GreaterOrEqual: value >= threshold(非严格)
    /// - LessOrEqual: value <= threshold(非严格)
    ///
    /// 使用整数策略(value_milli, threshold_milli)生成精确比较场景。
    #[test]
    fn prop_comparison_semantics(
        value_milli in 0u32..=2000,
        threshold_milli in 0u32..=2000,
    ) {
        let value = value_milli as f64 / 1000.0;
        let threshold = threshold_milli as f64 / 1000.0;

        // GreaterThan(严格大于)
        let gt = Comparison::GreaterThan.compare(value, threshold);
        prop_assert_eq!(gt, value > threshold, "GreaterThan 语义错误");

        // LessThan(严格小于)
        let lt = Comparison::LessThan.compare(value, threshold);
        prop_assert_eq!(lt, value < threshold, "LessThan 语义错误");

        // GreaterOrEqual(大于等于)
        let ge = Comparison::GreaterOrEqual.compare(value, threshold);
        prop_assert_eq!(ge, value >= threshold, "GreaterOrEqual 语义错误");

        // LessOrEqual(小于等于)
        let le = Comparison::LessOrEqual.compare(value, threshold);
        prop_assert_eq!(le, value <= threshold, "LessOrEqual 语义错误");
    }

    /// 不变量 2:AlertRule::new 默认 cooldown=60s,with_cooldown 覆盖默认值
    ///
    /// - new 创建的规则 cooldown_secs == 60(默认值)
    /// - with_cooldown(secs) 后 cooldown_secs == secs
    /// - with_cooldown 接受任意 secs(包括 0,表示无冷却)
    #[test]
    fn prop_alert_rule_cooldown(secs in 0u64..=3600) {
        let rule_default = AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        );
        prop_assert_eq!(
            rule_default.cooldown_secs, 60,
            "默认 cooldown 应为 60 秒"
        );

        let rule_custom = AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        )
        .with_cooldown(secs);
        prop_assert_eq!(
            rule_custom.cooldown_secs, secs,
            "with_cooldown({}) 后 cooldown 应为 {}", secs, secs
        );
    }

    /// 不变量 3:record_event N 次 → event_count == N(单调递增)
    ///
    /// 对同一事件类型记录 N 次,event_count 应精确等于 N。
    /// 验证 DashMap 计数器的原子性与单调性。
    #[test]
    fn prop_record_event_count_monotonic(n in 1u32..=100) {
        let collector = EventMetricCollector::new();
        let event = make_cache_hit("k-1");

        for _ in 0..n {
            collector.record_event(&event);
        }

        prop_assert_eq!(
            collector.event_count("CacheHit"),
            n as u64,
            "记录 {} 次后 event_count 应为 {}",
            n,
            n
        );
        prop_assert_eq!(
            collector.total_events(),
            n as u64,
            "total_events 应等于 {}",
            n
        );
    }

    /// 不变量 4:record_alert 累加 alert_count
    ///
    /// 对同一 severity 记录 N 次 alert_count,应精确等于 N。
    /// 验证告警计数的累加语义。
    #[test]
    fn prop_record_alert_count_accumulates(
        critical_count in 0u32..=50,
        warning_count in 0u32..=50,
        info_count in 0u32..=50,
    ) {
        let collector = EventMetricCollector::new();

        for _ in 0..critical_count {
            collector.record_alert(AlertSeverity::Critical.as_str());
        }
        for _ in 0..warning_count {
            collector.record_alert(AlertSeverity::Warning.as_str());
        }
        for _ in 0..info_count {
            collector.record_alert(AlertSeverity::Info.as_str());
        }

        prop_assert_eq!(
            collector.alert_count("critical"),
            critical_count as u64,
            "critical alert_count 应为 {}",
            critical_count
        );
        prop_assert_eq!(
            collector.alert_count("warning"),
            warning_count as u64,
            "warning alert_count 应为 {}",
            warning_count
        );
        prop_assert_eq!(
            collector.alert_count("info"),
            info_count as u64,
            "info alert_count 应为 {}",
            info_count
        );
    }
}

/// 辅助测试:AlertSeverity::as_str 三个变体返回互不相同的字符串(非属性测试)
///
/// WHY 独立测试:此不变量不依赖随机输入,是确定性的契约验证。
/// WHY 测试互不相同:Prometheus 标签使用 as_str() 作为 label value,
/// 若两个变体返回相同字符串会导致标签冲突与指标聚合错误。
#[test]
fn prop_severity_as_str_distinct() {
    let info = AlertSeverity::Info.as_str();
    let warning = AlertSeverity::Warning.as_str();
    let critical = AlertSeverity::Critical.as_str();

    assert_ne!(info, warning, "Info 与 Warning 的 as_str 不应相同");
    assert_ne!(info, critical, "Info 与 Critical 的 as_str 不应相同");
    assert_ne!(warning, critical, "Warning 与 Critical 的 as_str 不应相同");

    // 验证具体值(契约固定)
    assert_eq!(info, "info");
    assert_eq!(warning, "warning");
    assert_eq!(critical, "critical");
}
