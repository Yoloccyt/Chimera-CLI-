//! 告警规则引擎 — 配置化阈值检测 + cooldown 防抖
//!
//! 对应架构层:L9 Quest
//!
//! ## 核心机制
//! - `AlertRuleEngine` 持有 `AlertRule` 列表,对 `MetricSample` 进行阈值比较
//! - `cooldown_secs` 防止同一规则在短时间内重复触发告警风暴
//! - 触发记录以 `AlertEvent` 形式返回,调用方据此发布 `EfficiencyAlertTriggered`
//!
//! ## 线程安全
//! 规则列表与触发时间戳基于 `Arc<DashMap>`,`Clone` 廉价,可在多任务间共享。

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;

use crate::types::{AlertEvent, AlertRule, AlertSeverity, MetricSample};

/// 告警规则引擎 — 配置化阈值检测 + cooldown 防抖
///
/// 维护两个映射:
/// - `rules`:规则 ID -> AlertRule
/// - `last_triggered`:规则 ID -> 上次触发时间戳(用于 cooldown 计算)
///
/// `check()` 方法对一批 `MetricSample` 进行规则匹配,返回触发的 `AlertEvent` 列表。
#[derive(Clone)]
pub struct AlertRuleEngine {
    /// 告警规则列表:rule_id -> AlertRule
    rules: Arc<DashMap<String, AlertRule>>,
    /// 上次触发时间戳:rule_id -> last_triggered_at
    last_triggered: Arc<DashMap<String, DateTime<Utc>>>,
}

impl AlertRuleEngine {
    /// 创建新的告警规则引擎(无规则)
    pub fn new() -> Self {
        Self {
            rules: Arc::new(DashMap::new()),
            last_triggered: Arc::new(DashMap::new()),
        }
    }

    /// 添加一条告警规则
    ///
    /// 若 `rule_id` 已存在,将覆盖原规则。
    pub fn add_rule(&self, rule: AlertRule) {
        self.rules.insert(rule.rule_id.clone(), rule);
    }

    /// 移除一条告警规则
    ///
    /// 返回被移除的规则(若存在)。
    pub fn remove_rule(&self, rule_id: &str) -> Option<AlertRule> {
        self.last_triggered.remove(rule_id);
        self.rules.remove(rule_id).map(|(_, v)| v)
    }

    /// 获取当前规则数量
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 获取指定规则的指标名与严重级别(用于发布 EfficiencyAlertTriggered 事件)
    ///
    /// 返回 `(metric_name, severity)`,若规则不存在返回 `None`。
    pub fn get_rule_info(&self, rule_id: &str) -> Option<(String, AlertSeverity)> {
        self.rules
            .get(rule_id)
            .map(|r| (r.metric_name.clone(), r.severity))
    }

    /// 检查所有规则,返回触发的告警事件(考虑 cooldown)
    ///
    /// # 流程
    /// 1. 收集所有规则快照(避免持有 DashMap 的引用导致死锁)
    /// 2. 对每条规则,查找匹配的 MetricSample
    /// 3. 若指标值满足比较条件且不在 cooldown 期内,触发告警
    /// 4. 更新 `last_triggered` 时间戳
    ///
    /// # 性能
    /// - 规则数 R × 样本数 S,复杂度 O(R × S)
    /// - 典型场景 R < 20, S < 100,单次 check < 1ms
    pub fn check(&self, samples: &[MetricSample]) -> Vec<AlertEvent> {
        let now = Utc::now();
        let mut triggered = Vec::new();

        // 先收集所有规则快照,避免在循环中持有 DashMap 的借用
        // WHY:DashMap::iter() 返回 RefMulti,持有 shard lock;
        // 若在循环内向 last_triggered 插入(可能命中同一 shard),会死锁。
        let rules_snapshot: Vec<AlertRule> = self.rules.iter().map(|r| r.clone()).collect();

        for rule in rules_snapshot {
            for sample in samples {
                // 指标名不匹配,跳过
                if sample.name != rule.metric_name {
                    continue;
                }

                // 阈值比较
                if !rule.comparison.compare(sample.value, rule.threshold) {
                    continue;
                }

                // 检查 cooldown:若上次触发距今不足 cooldown_secs,跳过
                if self.is_in_cooldown(&rule.rule_id, now, rule.cooldown_secs) {
                    continue;
                }

                // 触发告警
                let event = AlertEvent::new(rule.rule_id.clone(), sample.value, rule.threshold);
                triggered.push(event);

                // 更新触发时间戳
                self.last_triggered.insert(rule.rule_id.clone(), now);
                break; // 一条规则只匹配一个样本,跳出内层循环
            }
        }

        triggered
    }

    /// 判断指定规则是否在 cooldown 期内
    ///
    /// 返回 true 表示在 cooldown 期内(应跳过触发)。
    fn is_in_cooldown(&self, rule_id: &str, now: DateTime<Utc>, cooldown_secs: u64) -> bool {
        if let Some(last) = self.last_triggered.get(rule_id) {
            let elapsed = now.signed_duration_since(*last).num_seconds();
            // elapsed < 0 处理时钟回拨(返回 false,允许触发)
            elapsed >= 0 && (elapsed as u64) < cooldown_secs
        } else {
            false
        }
    }

    /// 清除指定规则的 cooldown 记录(用于测试或手动重置)
    pub fn clear_cooldown(&self, rule_id: &str) {
        self.last_triggered.remove(rule_id);
    }

    /// 清除所有规则的 cooldown 记录
    pub fn clear_all_cooldowns(&self) {
        self.last_triggered.clear();
    }
}

impl Default for AlertRuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Comparison;
    use chrono::Duration;

    fn make_sample(name: &str, value: f64, labels: Vec<(String, String)>) -> MetricSample {
        MetricSample::new(name, value, labels)
    }

    #[test]
    fn test_add_rule_increments_count() {
        let engine = AlertRuleEngine::new();
        let rule = AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        );
        engine.add_rule(rule);
        assert_eq!(engine.rule_count(), 1);
    }

    #[test]
    fn test_remove_rule_decrements_count() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
        assert_eq!(engine.rule_count(), 1);

        let removed = engine.remove_rule("r-1");
        assert!(removed.is_some());
        assert_eq!(engine.rule_count(), 0);
    }

    #[test]
    fn test_check_triggers_when_threshold_exceeded() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));

        let samples = vec![make_sample(
            "nexus_event_total",
            15.0,
            vec![("type".into(), "SkepticVeto".into())],
        )];

        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "r-1");
        assert!((alerts[0].triggered_value - 15.0).abs() < f64::EPSILON);
        assert!((alerts[0].threshold - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_check_no_trigger_when_threshold_not_exceeded() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));

        let samples = vec![make_sample("nexus_event_total", 5.0, vec![])];
        let alerts = engine.check(&samples);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_check_respects_comparison_operator() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::LessThan,
            AlertSeverity::Info,
        ));

        // value=5 < threshold=10,应触发
        let samples = vec![make_sample("metric", 5.0, vec![])];
        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 1);

        // value=15 > threshold=10,不应触发
        engine.clear_all_cooldowns();
        let samples = vec![make_sample("metric", 15.0, vec![])];
        let alerts = engine.check(&samples);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_check_cooldown_prevents_repeat_trigger() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(
            AlertRule::new(
                "r-1",
                "metric",
                10.0,
                Comparison::GreaterThan,
                AlertSeverity::Warning,
            )
            .with_cooldown(60),
        );

        let samples = vec![make_sample("metric", 15.0, vec![])];

        // 第一次检查:应触发
        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 1);

        // 第二次检查:在 cooldown 期内,不应触发
        let alerts = engine.check(&samples);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_clear_cooldown_allows_retrigger() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(
            AlertRule::new(
                "r-1",
                "metric",
                10.0,
                Comparison::GreaterThan,
                AlertSeverity::Warning,
            )
            .with_cooldown(60),
        );

        let samples = vec![make_sample("metric", 15.0, vec![])];

        // 第一次触发
        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 1);

        // 清除 cooldown 后可再次触发
        engine.clear_cooldown("r-1");
        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn test_check_multiple_rules_independent() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "metric_a",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
        engine.add_rule(AlertRule::new(
            "r-2",
            "metric_b",
            5.0,
            Comparison::GreaterThan,
            AlertSeverity::Critical,
        ));

        let samples = vec![
            make_sample("metric_a", 15.0, vec![]),
            make_sample("metric_b", 8.0, vec![]),
        ];

        let alerts = engine.check(&samples);
        assert_eq!(alerts.len(), 2);
        let rule_ids: Vec<&str> = alerts.iter().map(|a| a.rule_id.as_str()).collect();
        assert!(rule_ids.contains(&"r-1"));
        assert!(rule_ids.contains(&"r-2"));
    }

    #[test]
    fn test_get_rule_info() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Critical,
        ));

        let info = engine.get_rule_info("r-1");
        assert!(info.is_some());
        let (metric_name, severity) = info.unwrap();
        assert_eq!(metric_name, "nexus_event_total");
        assert_eq!(severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_get_rule_info_nonexistent() {
        let engine = AlertRuleEngine::new();
        assert!(engine.get_rule_info("nonexistent").is_none());
    }

    #[test]
    fn test_is_in_cooldown_with_manual_timestamp() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(
            AlertRule::new(
                "r-1",
                "metric",
                10.0,
                Comparison::GreaterThan,
                AlertSeverity::Warning,
            )
            .with_cooldown(60),
        );

        // 手动设置上次触发时间为 30 秒前(在 cooldown 期内)
        let thirty_secs_ago = Utc::now() - Duration::seconds(30);
        engine.last_triggered.insert("r-1".into(), thirty_secs_ago);

        let now = Utc::now();
        assert!(engine.is_in_cooldown("r-1", now, 60));

        // 手动设置上次触发时间为 120 秒前(超过 cooldown 期)
        let two_mins_ago = Utc::now() - Duration::seconds(120);
        engine.last_triggered.insert("r-1".into(), two_mins_ago);
        assert!(!engine.is_in_cooldown("r-1", now, 60));
    }

    #[test]
    fn test_clone_shares_state() {
        let engine = AlertRuleEngine::new();
        engine.add_rule(AlertRule::new(
            "r-1",
            "metric",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));

        let cloned = engine.clone();
        // 通过 clone 添加规则,原 engine 应可见
        cloned.add_rule(AlertRule::new(
            "r-2",
            "metric",
            20.0,
            Comparison::GreaterThan,
            AlertSeverity::Critical,
        ));

        assert_eq!(engine.rule_count(), 2);
    }
}
