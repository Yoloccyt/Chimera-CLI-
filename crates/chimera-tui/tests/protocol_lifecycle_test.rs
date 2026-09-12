//! 协议生命周期事件测试(FC-B / FC-C 修复验收,2026-09-06 复评)
//!
//! - FC-B:`TuiApp` 启动期发布 `TuiHello` 握手帧(ADR-082 SEC-4),
//!   此前 cli 侧 HandshakeResponder 永远等不到(握手链路空转);
//! - FC-C:`RefreshStateRequested` 经真实 DataPipeline 消费后跳过一次
//!   休眠立即重建快照(revision 前进,下游 update 重新对齐)。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use chimera_tui::{DataPipeline, DataSourceConfig, EventSubscriber, TuiApp, TuiConfig};
use event_bus::{EventBus, NexusEvent};

// ============================================================
// FC-B:启动期 TuiHello 发布
// ============================================================

#[test]
fn publish_tui_hello_sends_protocol_handshake() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let app = TuiApp::with_event_bus(
        TuiApp::new(TuiConfig {
            persist_state: false,
            ..Default::default()
        })
        .unwrap(),
        bus,
    );

    app.publish_tui_hello();

    let event = rx
        .try_recv()
        .expect("应收到握手帧")
        .expect("接收缓冲不应为空");
    match event {
        NexusEvent::TuiHello { proto, caps, .. } => {
            assert_eq!(proto, "1.0.0", "协议版本应与 cli 侧 handshake 同步(1.0.0)");
            assert!(!caps.is_empty(), "caps 应为诚实能力清单(非空)");
        }
        other => panic!("expected TuiHello, got {other:?}"),
    }
}

#[test]
fn publish_tui_hello_without_bus_is_silent_noop() {
    // 无 EventBus(测试/离线模式)时不 panic、无副作用
    let app = TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.publish_tui_hello();
}

// ============================================================
// FC-C:RefreshStateRequested 触发立即重建快照
// ============================================================

#[tokio::test]
async fn refresh_request_triggers_extra_immediate_tick() {
    let bus = EventBus::new();
    // 慢 tick(1s):使"跳过一次休眠"的额外 tick 与常规间隔(1s)可分辨
    let pipeline = std::sync::Arc::new(DataPipeline::new(
        EventSubscriber::new(bus.clone()),
        DataSourceConfig {
            tick_interval_ms: 1000,
            ..Default::default()
        },
    ));

    // 等待基线 revision(首个 tick 后 ≥1)
    let deadline = Instant::now() + Duration::from_secs(3);
    let base;
    loop {
        let rev = pipeline.snapshot().revision;
        if rev >= 1 {
            base = rev;
            break;
        }
        assert!(Instant::now() < deadline, "等待首个快照超时");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // 发布刷新请求
    bus.publish(NexusEvent::RefreshStateRequested {
        metadata: event_bus::EventMetadata::new("test"),
        requested_by: "test".into(),
    })
    .await
    .unwrap();

    // 计数式断言(避免 Instant 饱和导致的空洞比较):2.5s 窗口内
    // revision 累计前进 ≥3 —— 消费 tick(≤1s)+ 跳过休眠的额外 tick(+ε)
    // + 常规 tick(≤2.5s);无跳过语义时仅 +2。
    let deadline = Instant::now() + Duration::from_millis(2500);
    loop {
        if pipeline.snapshot().revision >= base + 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "2.5s 内 revision 应累计 ≥ base+3(含跳过休眠的额外 tick);base={base}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    pipeline.shutdown().await;
}
