#![allow(clippy::unwrap_used, clippy::expect_used)] // test-unit idiom
//! ADR-223 D2 遥测事件族的生产者→消费者集成测试。
//!
//! WHY 放在 `nexus-telemetry` 而不是 `event-bus/tests`：生产者就在本 crate
//! （`reporter_tick` / `spawn_saturation_reporter`）。只有用真实发布路径驱动，
//! 才能证明“事件会被发布”，而不是“事件能被构造”——后者在 event-bus 里造个假源
//! 就能过，对 R8-4 的“独立事件族”要求毫无约束力。

use std::sync::Arc;
use std::time::Duration;

use event_bus::{EventBus, EventReceiver, EventSeverity, EventTopic, NexusEvent};
use nexus_telemetry::{
    reporter_tick, spawn_saturation_reporter, CalibrationEvidence, SaturationSource,
};

/// 只控制 `apply_calibration` 返回值的源；采样与证据保持 no-op。
struct FixedSource {
    applied: usize,
}

impl SaturationSource for FixedSource {
    fn sample_and_report(&self, _bus: &EventBus) {}

    fn apply_calibration(&self) -> usize {
        self.applied
    }

    fn calibration_evidence(&self) -> Vec<CalibrationEvidence> {
        Vec::new()
    }
}

fn drain(rx: &mut EventReceiver) -> Vec<NexusEvent> {
    let mut out = Vec::new();
    while let Ok(Some(event)) = rx.try_recv() {
        out.push(event);
    }
    out
}

#[test]
fn calibration_applied_is_published_only_when_rows_written() {
    let bus = EventBus::with_capacity(64);
    let mut rx = bus.subscribe();

    // applied == 0：非 apply 模式恒为该值，不得为零变化刷屏
    reporter_tick(&FixedSource { applied: 0 }, &bus);
    let quiet = drain(&mut rx);
    assert!(
        !quiet
            .iter()
            .any(|e| matches!(e, NexusEvent::TelemetryCalibrationApplied { .. })),
        "applied==0 时不得发布 TelemetryCalibrationApplied"
    );

    // applied > 0：必须发布且透传行数
    reporter_tick(&FixedSource { applied: 3 }, &bus);
    let events = drain(&mut rx);
    let published = events
        .iter()
        .find_map(|e| match e {
            NexusEvent::TelemetryCalibrationApplied { applied_rows, .. } => Some(*applied_rows),
            _ => None,
        })
        .expect("applied>0 时必须发布 TelemetryCalibrationApplied");
    assert_eq!(published, 3, "applied_rows 必须透传实写行数");

    // 定级锁：Normal/System ⇒ 不进 mpsc 旁路、不进分片禁区
    let event = events
        .iter()
        .find(|e| matches!(e, NexusEvent::TelemetryCalibrationApplied { .. }))
        .unwrap();
    assert_eq!(event.severity(), EventSeverity::Normal);
    assert_eq!(event.topic(), EventTopic::System);
}

#[tokio::test(flavor = "current_thread")]
async fn reporter_degraded_is_published_once_before_first_sleep() {
    let bus = EventBus::with_capacity(64);
    let mut rx = bus.subscribe();
    let source: Arc<dyn SaturationSource> = Arc::new(FixedSource { applied: 0 });

    // period=0 会被 clamped_period 钳到 1s；降级事件在首个 sleep 之前发布。
    let handle = spawn_saturation_reporter(bus, 0, source);
    // 给 spawn 的任务一次调度机会；不需要等满 1s。
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.abort();

    let hits: Vec<(u64, u64)> = drain(&mut rx)
        .iter()
        .filter_map(|e| match e {
            NexusEvent::TelemetryReporterDegraded {
                requested_period_secs,
                effective_period_secs,
                ..
            } => Some((*requested_period_secs, *effective_period_secs)),
            _ => None,
        })
        .collect();
    assert_eq!(
        hits,
        vec![(0, 1)],
        "period=0 必须且只发一次 TelemetryReporterDegraded(0 -> 1)"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_degraded_event_when_period_is_valid() {
    let bus = EventBus::with_capacity(64);
    let mut rx = bus.subscribe();
    let source: Arc<dyn SaturationSource> = Arc::new(FixedSource { applied: 0 });

    let handle = spawn_saturation_reporter(bus, 5, source);
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.abort();

    assert!(
        !drain(&mut rx)
            .iter()
            .any(|e| matches!(e, NexusEvent::TelemetryReporterDegraded { .. })),
        "合法周期不得发布降级事件"
    );
}
