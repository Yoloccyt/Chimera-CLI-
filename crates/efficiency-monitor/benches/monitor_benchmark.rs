//! 性能基准 — 指标采集开销测量
//!
//! 对应 SubTask 4.7:性能基准测试
//!
//! # 验收标准
//! - 指标采集开销 ≤ 1ms/样本
//! - 告警延迟 ≤ 100ms
//!
//! # 运行
//! ```bash
//! cargo bench -p efficiency-monitor -- --ignored
//! ```
//!
//! 注意:基准测试标记为 `#[ignore]`,因为 criterion 基准测试
//! 在常规 `cargo test` 中不应运行。使用 `--ignored` 标志显式运行。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use efficiency_monitor::{
    AlertRule, AlertSeverity, Comparison, EfficiencyMonitor, EventMetricCollector, MetricCollector,
    MonitorConfig,
};
use event_bus::{EventMetadata, NexusEvent};
use std::time::Duration;
// WHY: Instant 仅在下方 #[test] #[ignore] 性能测试函数中使用。
// bench 目标以 `harness = false` + `criterion_main!` 为入口,clippy 在 bench 模式下
// 不会编译 `#[test]` 函数,导致 Instant 被误报为 unused。`cargo test --benches` 运行时
// 这些 #[test] 函数会被编译并使用 Instant,因此 import 必须保留。
#[allow(unused_imports)]
use std::time::Instant;

/// 构造指定数量的测试事件(混合 Critical 与 Normal)
fn make_events(n: usize) -> Vec<NexusEvent> {
    (0..n)
        .map(|i| {
            let meta = EventMetadata::new("bench-source");
            match i % 4 {
                0 => NexusEvent::SkepticVeto {
                    metadata: meta,
                    quest_id: format!("q-{i}"),
                    veto_reason: "bench".into(),
                    frozen_capabilities: vec![],
                },
                1 => NexusEvent::CacheHit {
                    metadata: meta,
                    cache_key: format!("k-{i}"),
                },
                2 => NexusEvent::BudgetExceeded {
                    metadata: meta,
                    budget_type: "token".into(),
                    current: 100,
                    limit: 50,
                },
                _ => NexusEvent::AsaIntervention {
                    metadata: meta,
                    operation_id: format!("op-{i}"),
                    action: "Block".into(),
                    safety_score: 0.2,
                    block_reason: Some("unsafe".into()),
                    alternative_suggestion: None,
                },
            }
        })
        .collect()
}

/// 基准:测量 record_event 单次调用开销
///
/// 验收标准:≤ 1ms/样本
fn bench_record_event(c: &mut Criterion) {
    let events = make_events(100);
    let collector = EventMetricCollector::new();

    let mut group = c.benchmark_group("record_event");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("single_event", |b| {
        b.iter(|| {
            let event = &events[0];
            collector.record_event(black_box(event));
        });
    });

    group.bench_function("100_events", |b| {
        b.iter(|| {
            for event in &events {
                collector.record_event(black_box(event));
            }
        });
    });

    group.finish();
}

/// 基准:测量 collect() 采集开销
///
/// 验收标准:≤ 1ms/样本
fn bench_collect(c: &mut Criterion) {
    let events = make_events(100);
    let collector = EventMetricCollector::new();

    // 预填充计数器
    for event in &events {
        collector.record_event(event);
    }
    collector.record_alert("critical");
    collector.record_alert("warning");

    let mut group = c.benchmark_group("collect_samples");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("collect_after_100_events", |b| {
        b.iter(|| {
            let samples = collector.collect();
            black_box(samples);
        });
    });

    group.finish();
}

/// 基准:测量 render_metrics() 渲染开销
fn bench_render_metrics(c: &mut Criterion) {
    let events = make_events(100);
    let collector = EventMetricCollector::new();

    for event in &events {
        collector.record_event(event);
    }
    collector.record_alert("critical");

    let mut group = c.benchmark_group("render_metrics");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("render_after_100_events", |b| {
        b.iter(|| {
            let output = efficiency_monitor::dashboard::render_metrics(black_box(&collector));
            black_box(output);
        });
    });

    group.finish();
}

/// 基准:测量 check_alerts() 告警检查开销
///
/// 验收标准:≤ 100ms
fn bench_check_alerts(c: &mut Criterion) {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // 添加多条规则
    for i in 0..10 {
        monitor.add_alert_rule(AlertRule::new(
            format!("rule-{i}"),
            "nexus_event_total",
            5.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
    }

    // 预填充事件
    let events = make_events(100);
    for event in &events {
        monitor.record_event(event);
    }

    let mut group = c.benchmark_group("check_alerts");
    group.sample_size(100);
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("check_10_rules_100_events", |b| {
        b.iter(|| {
            // 清除 cooldown 以确保每次都触发
            monitor.alert_engine().clear_all_cooldowns();
            let alerts = monitor.check_alerts();
            black_box(alerts);
        });
    });

    group.finish();
}

/// 基准:测量完整链路开销(record_event + check_alerts + render_metrics)
fn bench_full_pipeline(c: &mut Criterion) {
    let events = make_events(100);

    let mut group = c.benchmark_group("full_pipeline");
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("record_check_render", |b| {
        b.iter(|| {
            let monitor = EfficiencyMonitor::new(MonitorConfig::default());
            monitor.add_alert_rule(AlertRule::new(
                "r-1",
                "nexus_event_total",
                5.0,
                Comparison::GreaterThan,
                AlertSeverity::Warning,
            ));

            for event in &events {
                monitor.record_event(black_box(event));
            }

            let alerts = monitor.check_alerts();
            let output = monitor.render_metrics();
            black_box((alerts, output));
        });
    });

    group.finish();
}

/// 忽略测试:手动测量单次操作延迟(不依赖 criterion)
///
/// 运行:`cargo test -p efficiency-monitor --bench monitor_benchmark -- --ignored`
#[test]
#[ignore = "perf: run with --ignored"]
fn test_record_event_latency_under_1ms() {
    let events = make_events(1000);
    let collector = EventMetricCollector::new();

    // 预热
    for event in &events[..100] {
        collector.record_event(event);
    }

    // 测量单次 record_event 延迟
    let start = Instant::now();
    for event in &events[100..] {
        collector.record_event(event);
    }
    let elapsed = start.elapsed();

    let per_event = elapsed / (events.len() as u32 - 100);
    println!("record_event 平均延迟: {per_event:?}");

    // 验收标准:≤ 1ms/样本
    assert!(
        per_event < Duration::from_millis(1),
        "record_event 延迟 {per_event:?} 超过 1ms 阈值"
    );
}

/// 忽略测试:测量 collect() 采集延迟
#[test]
#[ignore = "perf: run with --ignored"]
fn test_collect_latency_under_1ms() {
    let events = make_events(100);
    let collector = EventMetricCollector::new();

    for event in &events {
        collector.record_event(event);
    }

    // 预热
    let _ = collector.collect();

    // 测量 collect 延迟
    let start = Instant::now();
    let samples = collector.collect();
    let elapsed = start.elapsed();

    println!("collect() 延迟: {elapsed:?}({} 个样本)", samples.len());

    // 验收标准:≤ 1ms
    assert!(
        elapsed < Duration::from_millis(1),
        "collect 延迟 {elapsed:?} 超过 1ms 阈值"
    );
}

/// 忽略测试:测量 check_alerts 告警延迟
#[test]
#[ignore = "perf: run with --ignored"]
fn test_check_alerts_latency_under_100ms() {
    let monitor = EfficiencyMonitor::new(MonitorConfig::default());

    // 添加 20 条规则
    for i in 0..20 {
        monitor.add_alert_rule(AlertRule::new(
            format!("rule-{i}"),
            "nexus_event_total",
            5.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
    }

    // 预填充 100 个事件
    let events = make_events(100);
    for event in &events {
        monitor.record_event(event);
    }

    // 预热
    monitor.alert_engine().clear_all_cooldowns();
    let _ = monitor.check_alerts();

    // 测量 check_alerts 延迟
    monitor.alert_engine().clear_all_cooldowns();
    let start = Instant::now();
    let alerts = monitor.check_alerts();
    let elapsed = start.elapsed();

    println!(
        "check_alerts 延迟: {elapsed:?}(触发 {} 条告警)",
        alerts.len()
    );

    // 验收标准:≤ 100ms
    assert!(
        elapsed < Duration::from_millis(100),
        "check_alerts 延迟 {elapsed:?} 超过 100ms 阈值"
    );
}

criterion_group!(
    benches,
    bench_record_event,
    bench_collect,
    bench_render_metrics,
    bench_check_alerts,
    bench_full_pipeline,
);
criterion_main!(benches);
