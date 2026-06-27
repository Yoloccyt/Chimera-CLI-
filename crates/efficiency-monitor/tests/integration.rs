//! 集成测试 — 4 个 Critical 事件 → 告警 → /metrics 全链路验证
//!
//! 对应 SubTask 4.6:集成测试
//!
//! # 验证场景
//! 1. SkepticVeto → 立即告警 → EfficiencyAlertTriggered → /metrics 输出
//! 2. RedTeamAudit → 立即告警 → EfficiencyAlertTriggered → /metrics 输出
//! 3. AsaIntervention → 立即告警 → EfficiencyAlertTriggered → /metrics 输出
//! 4. BudgetExceeded → 立即告警 → EfficiencyAlertTriggered → /metrics 输出
//! 5. 4 个 Critical 事件全覆盖(4/4)
//! 6. 后台订阅模式:start_event_subscriber + EventBus.publish 全链路
//! 7. 规则引擎:AlertRule 阈值检测 → EfficiencyAlertTriggered 发布
//! 8. Prometheus /metrics 输出格式正确性

#![forbid(unsafe_code)]

use efficiency_monitor::{AlertRule, AlertSeverity, Comparison, EfficiencyMonitor, MonitorConfig};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use std::time::Duration;

// ============================================================
// 辅助函数:构造 4 个 Critical 事件
// ============================================================

fn make_skeptic_veto() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        veto_reason: "unsafe shell injection detected".into(),
        frozen_capabilities: vec!["shell_exec".into()],
    }
}

fn make_red_team_audit() -> NexusEvent {
    NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 5,
        total_probes: 20,
        detection_rate: 0.25,
        remediation_suggestion: "add input sanitization".into(),
    }
}

fn make_asa_intervention() -> NexusEvent {
    NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        operation_id: "op-1".into(),
        action: "Block".into(),
        safety_score: 0.2,
        block_reason: Some("unsafe operation".into()),
        alternative_suggestion: Some("use sandboxed tool".into()),
    }
}

fn make_budget_exceeded() -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "token".into(),
        current: 15000,
        limit: 10000,
    }
}

/// 辅助:验证事件为 EfficiencyAlertTriggered 且字段正确
fn assert_efficiency_alert_event(event: NexusEvent, expected_type_name: &str) {
    match event {
        NexusEvent::EfficiencyAlertTriggered {
            rule_id,
            metric_name,
            triggered_value,
            threshold,
            metadata,
            ..
        } => {
            assert!(
                rule_id.starts_with("critical-"),
                "rule_id 应以 'critical-' 开头,实际: {rule_id}"
            );
            assert_eq!(
                metric_name, expected_type_name,
                "metric_name 应为事件类型名"
            );
            assert!((triggered_value - 1.0).abs() < f64::EPSILON);
            assert!((threshold - 1.0).abs() < f64::EPSILON);
            assert_eq!(metadata.source, "efficiency-monitor");
        }
        _ => panic!("期望 EfficiencyAlertTriggered 事件,收到 {:?}", event),
    }
}

// ============================================================
// 1. SkepticVeto → 立即告警 → EfficiencyAlertTriggered
// ============================================================

#[tokio::test]
async fn test_critical_skeptic_veto_triggers_alert() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    monitor.record_event(&make_skeptic_veto());

    let event = rx
        .recv()
        .await
        .expect("应收到 EfficiencyAlertTriggered 事件");
    assert_efficiency_alert_event(event, "SkepticVeto");

    // 验证指标计数
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 1);
}

// ============================================================
// 2. RedTeamAudit → 立即告警 → EfficiencyAlertTriggered
// ============================================================

#[tokio::test]
async fn test_critical_red_team_audit_triggers_alert() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    monitor.record_event(&make_red_team_audit());

    let event = rx
        .recv()
        .await
        .expect("应收到 EfficiencyAlertTriggered 事件");
    assert_efficiency_alert_event(event, "RedTeamAudit");

    assert_eq!(monitor.collectors().event_count("RedTeamAudit"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 1);
}

// ============================================================
// 3. AsaIntervention → 立即告警 → EfficiencyAlertTriggered
// ============================================================

#[tokio::test]
async fn test_critical_asa_intervention_triggers_alert() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    monitor.record_event(&make_asa_intervention());

    let event = rx
        .recv()
        .await
        .expect("应收到 EfficiencyAlertTriggered 事件");
    assert_efficiency_alert_event(event, "AsaIntervention");

    assert_eq!(monitor.collectors().event_count("AsaIntervention"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 1);
}

// ============================================================
// 4. BudgetExceeded → 立即告警 → EfficiencyAlertTriggered
// ============================================================

#[tokio::test]
async fn test_critical_budget_exceeded_triggers_alert() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    monitor.record_event(&make_budget_exceeded());

    let event = rx
        .recv()
        .await
        .expect("应收到 EfficiencyAlertTriggered 事件");
    assert_efficiency_alert_event(event, "BudgetExceeded");

    assert_eq!(monitor.collectors().event_count("BudgetExceeded"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 1);
}

// ============================================================
// 5. 4 个 Critical 事件全覆盖(4/4)
// ============================================================

#[test]
fn test_all_four_critical_events_covered() {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // 记录全部 4 个 Critical 事件
    monitor.record_event(&make_skeptic_veto());
    monitor.record_event(&make_red_team_audit());
    monitor.record_event(&make_asa_intervention());
    monitor.record_event(&make_budget_exceeded());

    // 验证每个事件的计数
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().event_count("RedTeamAudit"), 1);
    assert_eq!(monitor.collectors().event_count("AsaIntervention"), 1);
    assert_eq!(monitor.collectors().event_count("BudgetExceeded"), 1);

    // 验证 Critical 告警计数 = 4
    assert_eq!(monitor.collectors().alert_count("critical"), 4);

    // 验证总事件数 = 4
    assert_eq!(monitor.collectors().total_events(), 4);
}

// ============================================================
// 6. 后台订阅模式:start_event_subscriber + EventBus.publish
// ============================================================

#[tokio::test]
async fn test_background_subscriber_full_chain() {
    let bus = EventBus::new();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

    // 启动后台订阅
    monitor.start_event_subscriber().expect("启动订阅失败");

    // 给后台任务时间启动
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 通过 EventBus 发布 4 个 Critical 事件
    bus.publish(make_skeptic_veto()).await.expect("发布失败");
    bus.publish(make_red_team_audit()).await.expect("发布失败");
    bus.publish(make_asa_intervention())
        .await
        .expect("发布失败");
    bus.publish(make_budget_exceeded()).await.expect("发布失败");

    // 给后台任务时间处理
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证所有事件被记录
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().event_count("RedTeamAudit"), 1);
    assert_eq!(monitor.collectors().event_count("AsaIntervention"), 1);
    assert_eq!(monitor.collectors().event_count("BudgetExceeded"), 1);

    // 验证 Critical 告警计数 = 4(每个 Critical 事件触发一次立即告警)
    assert_eq!(monitor.collectors().alert_count("critical"), 4);
}

#[tokio::test]
async fn test_background_subscriber_normal_event_no_alert() {
    let bus = EventBus::new();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

    monitor.start_event_subscriber().expect("启动订阅失败");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 发布 Normal 事件
    let normal_event = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-1".into(),
    };
    bus.publish(normal_event).await.expect("发布失败");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Normal 事件应记录但不应触发告警
    assert_eq!(monitor.collectors().event_count("CacheHit"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 0);
    assert_eq!(monitor.collectors().alert_count("warning"), 0);
}

// ============================================================
// 7. 规则引擎:AlertRule 阈值检测 → EfficiencyAlertTriggered
// ============================================================

#[tokio::test]
async fn test_rule_engine_triggers_alert_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    // 添加规则:nexus_event_total >= 3 时触发 Warning 告警
    monitor.add_alert_rule(AlertRule::new(
        "event-count-rule",
        "nexus_event_total",
        3.0,
        Comparison::GreaterOrEqual,
        AlertSeverity::Warning,
    ));

    // 记录 3 次 CacheHit 事件
    let cache_hit = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-1".into(),
    };
    monitor.record_event(&cache_hit);
    monitor.record_event(&cache_hit);
    monitor.record_event(&cache_hit);

    // 检查告警(应触发规则)
    let alerts = monitor.check_alerts();
    assert_eq!(alerts.len(), 1, "应触发 1 条规则告警");
    assert_eq!(alerts[0].rule_id, "event-count-rule");

    // 接收并验证 EfficiencyAlertTriggered 事件
    let event = rx.recv().await.expect("应收到规则告警事件");
    match event {
        NexusEvent::EfficiencyAlertTriggered {
            rule_id,
            metric_name,
            triggered_value,
            threshold,
            ..
        } => {
            assert_eq!(rule_id, "event-count-rule");
            assert_eq!(metric_name, "nexus_event_total");
            assert!((triggered_value - 3.0).abs() < f64::EPSILON);
            assert!((threshold - 3.0).abs() < f64::EPSILON);
        }
        _ => panic!("期望 EfficiencyAlertTriggered 事件"),
    }

    // 验证 warning 告警计数
    assert_eq!(monitor.collectors().alert_count("warning"), 1);
}

#[tokio::test]
async fn test_rule_engine_cooldown_prevents_repeat() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    // 添加规则:cooldown=60 秒
    monitor.add_alert_rule(
        AlertRule::new(
            "cooldown-rule",
            "nexus_event_total",
            1.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        )
        .with_cooldown(60),
    );

    let cache_hit = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-1".into(),
    };
    monitor.record_event(&cache_hit);

    // 第一次检查:应触发
    let alerts = monitor.check_alerts();
    assert_eq!(alerts.len(), 1);

    // 接收第一次告警事件
    let _ = rx.recv().await.expect("应收到第一次告警");

    // 第二次检查:在 cooldown 期内,不应触发
    let alerts = monitor.check_alerts();
    assert!(alerts.is_empty(), "cooldown 期内不应重复触发");
}

// ============================================================
// 8. Prometheus /metrics 输出格式正确性
// ============================================================

#[test]
fn test_metrics_output_format() {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // 记录事件
    monitor.record_event(&make_skeptic_veto());
    monitor.record_event(&make_red_team_audit());

    let output = monitor.render_metrics();

    // 验证 Prometheus 文本格式关键元素
    assert!(output.contains("# HELP nexus_event_total Total NexusEvent published by type"));
    assert!(output.contains("# TYPE nexus_event_total counter"));
    assert!(output.contains(r#"nexus_event_total{type="SkepticVeto"} 1"#));
    assert!(output.contains(r#"nexus_event_total{type="RedTeamAudit"} 1"#));

    assert!(output.contains("# HELP nexus_alert_triggered_total Total alerts triggered"));
    assert!(output.contains("# TYPE nexus_alert_triggered_total counter"));
    assert!(output.contains(r#"nexus_alert_triggered_total{severity="critical"} 2"#));
}

#[test]
fn test_metrics_output_contains_critical_event_total() {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // SkepticVeto 和 RedTeamAudit 在 event-bus 中是 Critical severity
    monitor.record_event(&make_skeptic_veto());
    monitor.record_event(&make_red_team_audit());

    let output = monitor.render_metrics();

    // 验证 nexus_critical_event_total 指标
    assert!(output
        .contains("# HELP nexus_critical_event_total Total Critical NexusEvent published by type"));
    assert!(output.contains("# TYPE nexus_critical_event_total counter"));
    assert!(output.contains(r#"nexus_critical_event_total{type="SkepticVeto"} 1"#));
    assert!(output.contains(r#"nexus_critical_event_total{type="RedTeamAudit"} 1"#));
}

#[test]
fn test_metrics_output_after_background_subscriber() {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // 模拟后台订阅记录的事件
    monitor.record_event(&make_skeptic_veto());
    monitor.record_event(&make_asa_intervention());
    monitor.record_event(&make_budget_exceeded());

    let output = monitor.render_metrics();

    // 验证所有 3 个事件类型都被记录
    assert!(output.contains(r#"type="SkepticVeto""#));
    assert!(output.contains(r#"type="AsaIntervention""#));
    assert!(output.contains(r#"type="BudgetExceeded""#));

    // 验证 Critical 告警计数 = 3
    assert!(output.contains(r#"nexus_alert_triggered_total{severity="critical"} 3"#));
}

// ============================================================
// 9. 全链路集成:事件 → 告警 → /metrics
// ============================================================

#[tokio::test]
async fn test_full_chain_critical_event_to_metrics() {
    let bus = EventBus::new();
    // subscribe 必须在 with_event_bus 之前调用(Week 6 教训:broadcast 不缓存历史消息)
    // WHY: with_event_bus 消费 bus,subscribe 借用 &self,所以先订阅再移动
    let mut rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    // 记录 Critical 事件(触发立即告警 + 发布 EfficiencyAlertTriggered)
    monitor.record_event(&make_skeptic_veto());

    // 验证 EfficiencyAlertTriggered 被发布
    let event = rx
        .recv()
        .await
        .expect("应收到 EfficiencyAlertTriggered 事件");
    assert_efficiency_alert_event(event, "SkepticVeto");

    // 验证指标计数
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 1);

    // 验证 /metrics 输出
    let output = monitor.render_metrics();
    assert!(output.contains(r#"nexus_event_total{type="SkepticVeto"} 1"#));
    assert!(output.contains(r#"nexus_alert_triggered_total{severity="critical"} 1"#));
}

#[tokio::test]
async fn test_full_chain_rule_engine_to_metrics() {
    let bus = EventBus::new();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

    // 添加规则
    monitor.add_alert_rule(AlertRule::new(
        "high-event-rate",
        "nexus_event_total",
        5.0,
        Comparison::GreaterThan,
        AlertSeverity::Critical,
    ));

    // 记录 6 次事件(超过阈值)
    let cache_hit = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-1".into(),
    };
    for _ in 0..6 {
        monitor.record_event(&cache_hit);
    }

    // 检查告警
    let alerts = monitor.check_alerts();
    assert_eq!(alerts.len(), 1);

    // 验证 /metrics 输出
    let output = monitor.render_metrics();
    assert!(output.contains(r#"nexus_event_total{type="CacheHit"} 6"#));
    // 规则告警应记录 critical 计数(规则 severity 为 Critical)
    assert!(output.contains(r#"nexus_alert_triggered_total{severity="critical"} 1"#));
}

// ============================================================
// 10. 配置化测试:critical_instant_alert 开关
// ============================================================

#[tokio::test]
async fn test_disabled_critical_instant_alert() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = MonitorConfig {
        critical_instant_alert: false,
        ..MonitorConfig::default()
    };
    let monitor = EfficiencyMonitor::with_event_bus(config, bus);

    // 记录 Critical 事件
    monitor.record_event(&make_skeptic_veto());

    // 禁用立即告警后,不应发布 EfficiencyAlertTriggered
    // 给一点时间确保没有事件到达
    let result = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(result.is_err(), "禁用立即告警后不应发布事件");

    // 但事件计数仍应记录
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().alert_count("critical"), 0);
}

// ============================================================
// 11. 多事件混合场景
// ============================================================

#[tokio::test]
async fn test_mixed_critical_and_normal_events() {
    let bus = EventBus::new();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

    monitor.start_event_subscriber().expect("启动订阅失败");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 混合发布 Critical 与 Normal 事件
    bus.publish(make_skeptic_veto()).await.expect("发布失败");
    bus.publish(NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-1".into(),
    })
    .await
    .expect("发布失败");
    bus.publish(make_budget_exceeded()).await.expect("发布失败");
    bus.publish(NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k-2".into(),
    })
    .await
    .expect("发布失败");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证事件计数
    assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    assert_eq!(monitor.collectors().event_count("CacheHit"), 2);
    assert_eq!(monitor.collectors().event_count("BudgetExceeded"), 1);

    // Critical 告警计数 = 2(SkepticVeto + BudgetExceeded)
    assert_eq!(monitor.collectors().alert_count("critical"), 2);

    // /metrics 输出应包含所有事件类型
    let output = monitor.render_metrics();
    assert!(output.contains(r#"type="SkepticVeto""#));
    assert!(output.contains(r#"type="CacheHit""#));
    assert!(output.contains(r#"type="BudgetExceeded""#));
    assert!(output.contains(r#"nexus_alert_triggered_total{severity="critical"} 2"#));
}
