//! E-2 压力下降采样端到端验证(ADR-191 D2,2026-09-26)
//!
//! 覆盖三条关键契约:
//! 1. 非降采样策略(with_capacity/default)= 行为与历史完全一致(零回归)
//! 2. 压力区内 Normal 事件按「每 every 条广播 1 条」放行,间距恒定;
//!    `downsampled_total` 观测降级量
//! 3. Critical 事件在压力区仍全量入广播(红线:降级不得伤害关键告警)

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use event_bus::{BackpressurePolicy, EventBus, EventMetadata, NexusEvent};
use std::time::Duration;
use tokio::time::timeout;

/// 构造带唯一 quest_id 的进度事件(Normal 车道;非分片路径仍走广播)
fn progress_event(i: usize) -> NexusEvent {
    NexusEvent::QuestProgressUpdated {
        metadata: EventMetadata::new("downsample-e2e"),
        quest_id: format!("q-{i}"),
        completed: i as u32,
        total: 100,
    }
}

/// 构造 Critical 事件(SkepticVeto,走 mpsc 旁路+广播)
fn critical_event() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("downsample-e2e"),
        quest_id: "q-veto".into(),
        veto_reason: "e2e".into(),
        frozen_capabilities: vec!["cap-1".into()],
    }
}

fn ds_policy(capacity: usize, every: u64) -> BackpressurePolicy {
    BackpressurePolicy::DownsampleUnderPressure {
        broadcast_capacity: capacity,
        every,
        trigger_permille: 500, // 50% 即触发(便于测试:无消费者时快速入压力区)
        resume_permille: 100,  // 10% 解除
    }
}

/// 先 subscribe 再 publish(E-6 红线):消费端故意不 recv,占用持续走高
#[tokio::test]
async fn default_policy_never_downsamples() {
    let bus = EventBus::with_capacity(64);
    let _rx = bus.subscribe();
    for i in 0..200 {
        bus.publish(progress_event(i)).await.unwrap();
    }
    assert_eq!(bus.downsampled_total(), 0, "默认策略必须零降级");
    assert!(!bus.pressure_engaged(), "默认策略 latch 恒不锁存");
}

#[tokio::test]
async fn pressure_engages_and_downsamples_normal_with_fixed_stride() {
    // capacity=64,trigger=50% → 第 33 次 publish 前置采样 queued=32 即锁存;
    // 消费端故意不 recv(仅持有 rx 使 sender.len() 累积),latch 稳定保持。
    // 锁存后 Normal 序号 seq=0..127 递增,放行 seq%8==0 共 16 条 → 降级 112 条(确定性)
    let bus = EventBus::with_policy(ds_policy(64, 8));
    let _rx = bus.subscribe();
    let total = 160u64;
    for i in 0..total {
        bus.publish(progress_event(i as usize)).await.unwrap();
    }
    assert!(bus.pressure_engaged(), "无消费持续发布必须进入压力区");
    assert_eq!(
        bus.downsampled_total(),
        112,
        "锁存后 128 条 Normal 按模 8 放行 16 条,降级量必为 112(间距恒定 = every)"
    );
    // 默认策略侧的发布计数不变(降级不影响 published_total 入口计数)
    assert_eq!(bus.published_total(), total);
}

#[tokio::test]
async fn critical_events_never_downsampled() {
    // 与上一测试同策略:Critical 必须全量入广播(同时 mpsc 旁路也全量)
    let bus = EventBus::with_policy(ds_policy(64, 8));
    let _rx = bus.subscribe();
    let mut crx = bus.subscribe_critical_events();
    // 先把队列灌满入压力区(Normal 会被降采样)
    for i in 0..80 {
        bus.publish(progress_event(i)).await.unwrap();
    }
    assert!(bus.pressure_engaged());
    let skipped_before = bus.downsampled_total();
    assert!(skipped_before > 0, "前置:压力区已生效");
    // 再发 5 个 Critical:必须 5/5 全量到广播与 mpsc
    for _ in 0..5 {
        bus.publish(critical_event()).await.unwrap();
    }
    assert_eq!(
        bus.downsampled_total(),
        skipped_before,
        "Critical 不得计入降采样(红线)"
    );
    // 广播侧因 rx 容量滞后无法精确计数(不依赖该面);mpsc 旁路才是 Critical 的
    // 红线送达通道——必须 5/5 全量(既有语义 + 本策略双重保证)
    let mut criticals_in_mpsc = 0;
    while timeout(Duration::from_millis(50), crx.recv()).await.is_ok() {
        criticals_in_mpsc += 1;
    }
    assert_eq!(criticals_in_mpsc, 5, "Critical mpsc 旁路必须全量(红线)");
}
