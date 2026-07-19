//! Phase V Task V-8: event-bus Prometheus 指标导出测试
//!
//! 验证 BusLogger 的 Prometheus Registry + Counter + Histogram 集成,
//! 确保 /metrics 端点可导出标准 Prometheus 文本格式指标。
//!
//! # 测试覆盖
//! - test_prometheus_metrics_registered: 指标名注册正确性
//! - test_event_total_counter_increments: 计数器递增正确性(普通+Critical)
//! - test_topic_label_partition: topic 标签分区计数
//! - test_render_metrics_prometheus_format: 输出符合 Prometheus 文本格式
//! - test_no_logger_no_metrics: 无 BusLogger 时向后兼容

use std::time::Duration;

use event_bus::{BusLogger, EventBus, EventMetadata, EventTopic, NexusEvent};

/// 构造 QuestCreated 事件(Quest topic, Normal severity)
fn make_quest_event(id: u32) -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: format!("q-{id}"),
        title: format!("Quest {id}"),
        task_count: 1,
    }
}

/// 构造 CacheHit 事件(Storage topic, Normal severity)
fn make_cache_hit_event(id: u32) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("key-{id}"),
    }
}

/// 构造 SkepticVeto 事件(Security topic, Critical severity)
fn make_skeptic_veto_event() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-veto".into(),
        veto_reason: "unsafe shell injection".into(),
        frozen_capabilities: vec!["shell-exec".into()],
    }
}

// ============================================================
// 测试 1: Prometheus 指标注册正确性
// ============================================================

#[test]
fn test_prometheus_metrics_registered() {
    let logger = BusLogger::new("test-module");
    // 发布几个事件以触发指标采集
    logger.log_publish(&make_quest_event(1), 1, Duration::from_micros(50));
    logger.log_publish(&make_skeptic_veto_event(), 1, Duration::from_micros(80));

    let output = logger.render_metrics();

    // 验证三个核心指标名均出现在输出中
    // 注意:prometheus-client 对 Counter 类型自动添加 _total 后缀
    assert!(
        output.contains("nexus_event_total"),
        "输出应包含 nexus_event_total 指标,实际输出:\n{output}"
    );
    assert!(
        output.contains("nexus_critical_event_total"),
        "输出应包含 nexus_critical_event_total 指标,实际输出:\n{output}"
    );
    assert!(
        output.contains("nexus_event_publish_duration_seconds"),
        "输出应包含 nexus_event_publish_duration_seconds 指标,实际输出:\n{output}"
    );
}

// ============================================================
// 测试 2: 计数器递增正确性(普通事件 + Critical 事件)
// ============================================================

#[test]
fn test_event_total_counter_increments() {
    let logger = BusLogger::new("test-module");

    // 发布 3 个普通事件(QuestCreated, Quest topic, Normal severity)
    for i in 0..3 {
        logger.log_publish(&make_quest_event(i), 1, Duration::from_micros(10));
    }
    // 发布 1 个 Critical 事件(SkepticVeto, Security topic, Critical severity)
    logger.log_publish(&make_skeptic_veto_event(), 1, Duration::from_micros(20));

    let output = logger.render_metrics();

    // nexus_event_total 应有 Quest=3, Security=1(总计 4)
    assert!(
        output.contains(r#"nexus_event_total{topic="Quest"} 3"#),
        "Quest topic 应计数为 3,实际输出:\n{output}"
    );
    assert!(
        output.contains(r#"nexus_event_total{topic="Security"} 1"#),
        "Security topic 应计数为 1,实际输出:\n{output}"
    );

    // nexus_critical_event_total 应为 1(仅 SkepticVeto 是 Critical)
    assert!(
        output.contains("nexus_critical_event_total 1"),
        "Critical 事件计数应为 1,实际输出:\n{output}"
    );
}

// ============================================================
// 测试 3: topic 标签分区计数
// ============================================================

#[test]
fn test_topic_label_partition() {
    let logger = BusLogger::new("test-module");

    // 发布不同 topic 的事件
    logger.log_publish(&make_quest_event(1), 1, Duration::from_micros(10)); // Quest
    logger.log_publish(&make_cache_hit_event(1), 1, Duration::from_micros(10)); // Storage
    logger.log_publish(&make_skeptic_veto_event(), 1, Duration::from_micros(10)); // Security

    let output = logger.render_metrics();

    // 验证 nexus_event_total 按 topic 分区计数
    assert!(
        output.contains(r#"nexus_event_total{topic="Quest"} 1"#),
        "Quest topic 应计数为 1,实际输出:\n{output}"
    );
    assert!(
        output.contains(r#"nexus_event_total{topic="Storage"} 1"#),
        "Storage topic 应计数为 1,实际输出:\n{output}"
    );
    assert!(
        output.contains(r#"nexus_event_total{topic="Security"} 1"#),
        "Security topic 应计数为 1,实际输出:\n{output}"
    );
}

// ============================================================
// 测试 4: render_metrics 输出符合 Prometheus 文本格式
// ============================================================

#[test]
fn test_render_metrics_prometheus_format() {
    let logger = BusLogger::new("test-module");
    logger.log_publish(&make_quest_event(1), 2, Duration::from_micros(100));

    let output = logger.render_metrics();

    // 验证 Prometheus 文本格式:每类指标都有 # HELP 和 # TYPE 行
    assert!(
        output.contains("# HELP nexus_event "),
        "应包含 nexus_event 的 HELP 行,实际输出:\n{output}"
    );
    assert!(
        output.contains("# TYPE nexus_event counter"),
        "应包含 nexus_event 的 TYPE 行(counter),实际输出:\n{output}"
    );

    // 验证 histogram 也有 HELP 和 TYPE
    assert!(
        output.contains("# HELP nexus_event_publish_duration_seconds "),
        "应包含 histogram 的 HELP 行,实际输出:\n{output}"
    );
    assert!(
        output.contains("# TYPE nexus_event_publish_duration_seconds histogram"),
        "应包含 histogram 的 TYPE 行,实际输出:\n{output}"
    );

    // 验证数据行格式:metric_name{labels} value
    // Counter 自动加 _total 后缀,格式如:nexus_event_total{topic="Quest"} 1
    assert!(
        output.contains(r#"nexus_event_total{topic="Quest"} 1"#),
        "应包含格式正确的数据行,实际输出:\n{output}"
    );

    // 验证 histogram 数据行包含 _bucket / _sum / _count
    assert!(
        output.contains("nexus_event_publish_duration_seconds_bucket"),
        "应包含 histogram bucket 行,实际输出:\n{output}"
    );
    assert!(
        output.contains("nexus_event_publish_duration_seconds_count"),
        "应包含 histogram count 行,实际输出:\n{output}"
    );
}

// ============================================================
// 测试 5: 无 BusLogger 时 EventBus 向后兼容
// ============================================================

#[tokio::test]
async fn test_no_logger_no_metrics() {
    // 不传 BusLogger,EventBus 应正常工作(向后兼容)
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = make_quest_event(1);
    bus.publish(event.clone()).await.unwrap();

    let received = rx.recv().await.unwrap();
    assert_eq!(received, event, "无 logger 时事件仍应正常收发");

    // 验证 logger() 返回 None(无 logger 可访问)
    assert!(bus.logger().is_none(), "无 logger 时 logger() 应返回 None");
}

// ============================================================
// 补充测试: EventTopic::all() 验证 10 类 topic 完整性
// ============================================================

#[test]
fn test_event_topic_count_matches_labels() {
    // 确保 EventTopic 有 10 个变体,与 Prometheus 标签值一一对应
    let all = EventTopic::all();
    assert_eq!(all.len(), 10, "EventTopic 应有 10 个变体");
}
