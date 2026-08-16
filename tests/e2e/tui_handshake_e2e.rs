//! TUI ↔ 编排器协议握手 E2E 测试 — Concord W10 T10.2/T10.3(ADR-082)
//!
//! 对应架构层:L10 Interface(chimera-cli handshake + chimera-tui 管道回环)
//!
//! # 覆盖(方案 §7.1 验收:握手三态 + 偏移场景零静默)
//! - Full 回环:TuiHello → responder 协商 → TuiHelloAck → DataPipeline
//!   HandshakeSync → 快照 handshake 字段;
//! - Degraded:客户端次版本高于服务端 → 携降级项;
//! - Refused:主版本断裂 → 快照显式 Refused(零静默)+ app 门控状态构造;
//! - SEC-4:重复 TuiHello 不产生第二个 Ack(responder 侧单测已覆盖,
//!   此处经管道视角复验状态不被篡改)。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use chimera_tui::{DataPipeline, DataSourceConfig, EventSubscriber};
use event_bus::{CompatLevel, EventBus, EventMetadata, NexusEvent};

/// 启动管道并等待快照谓词成立(超时 fail)
async fn wait_snapshot<F: Fn(&chimera_tui::DataSnapshot) -> bool>(
    pipeline: &Arc<DataPipeline>,
    what: &str,
    f: F,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if let Ok(snap) = chimera_tui::TuiDataSource::snapshot(pipeline.as_ref()) {
            if f(&snap) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("等待超时: {what}");
}

/// 组装总线 + 应答器 + 快 tick 管道,发布 TuiHello 后等待握手状态
async fn handshake_roundtrip(client_proto: &str) -> chimera_tui::HandshakeState {
    let bus = EventBus::new();
    let responder = chimera_cli::handshake::spawn_handshake_responder(bus.clone());
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = Arc::new(DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 50,
            ..Default::default()
        },
    ));

    bus.publish(NexusEvent::TuiHello {
        metadata: EventMetadata::new("chimera-tui"),
        proto: client_proto.into(),
        tui_version: "2.26.0".into(),
        caps: vec!["orchestrated-commands".into()],
    })
    .await
    .expect("publish TuiHello");

    let p = Arc::clone(&pipeline);
    wait_snapshot(&p, "握手状态入快照", |snap| snap.handshake.is_some()).await;
    let state = {
        let snap = chimera_tui::TuiDataSource::snapshot(p.as_ref()).expect("snapshot");
        snap.handshake.clone().expect("handshake state")
    };

    responder.abort();
    pipeline.shutdown().await;
    state
}

#[tokio::test]
async fn handshake_full_roundtrip() {
    let state = handshake_roundtrip("1.0.0").await;
    assert_eq!(state.compat, CompatLevel::Full, "同协议版本应 Full");
    assert!(!state.server_version.is_empty(), "应携带服务端版本(零静默)");
}

#[tokio::test]
async fn handshake_client_newer_minor_degrades_with_items() {
    let state = handshake_roundtrip("1.3.0").await;
    match state.compat {
        CompatLevel::Degraded(items) => {
            assert!(
                !items.is_empty(),
                "降级应显式携带降级项(banner 展示,零静默)"
            );
        }
        other => panic!("客户端次版本更高应 Degraded,实际 {other:?}"),
    }
}

#[tokio::test]
async fn handshake_major_skew_refused_explicitly() {
    let state = handshake_roundtrip("2.0.0").await;
    assert_eq!(
        state.compat,
        CompatLevel::Refused,
        "主版本断裂应显式 Refused(Codex #37536 式静默故障防护)"
    );
}

#[tokio::test]
async fn handshake_repeat_does_not_tamper_state() {
    // SEC-4 E2E:首个握手建立 Full 后,重复(伪造)握手不得篡改快照状态
    let bus = EventBus::new();
    let responder = chimera_cli::handshake::spawn_handshake_responder(bus.clone());
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = Arc::new(DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 50,
            ..Default::default()
        },
    ));

    bus.publish(NexusEvent::TuiHello {
        metadata: EventMetadata::new("chimera-tui"),
        proto: "1.0.0".into(),
        tui_version: "2.26.0".into(),
        caps: vec![],
    })
    .await
    .expect("publish first hello");
    let p = Arc::clone(&pipeline);
    wait_snapshot(&p, "首个握手状态", |s| s.handshake.is_some()).await;

    // 伪造主版本断裂的重复握手
    bus.publish(NexusEvent::TuiHello {
        metadata: EventMetadata::new("attacker"),
        proto: "9.9.9".into(),
        tui_version: "0.0.1".into(),
        caps: vec![],
    })
    .await
    .expect("publish forged hello");
    // 给足时间让任何(错误的)状态变更入快照
    tokio::time::sleep(Duration::from_millis(300)).await;

    let snap = chimera_tui::TuiDataSource::snapshot(p.as_ref()).expect("snapshot");
    let state = snap.handshake.clone().expect("握手状态应保持");
    assert_eq!(
        state.compat,
        CompatLevel::Full,
        "SEC-4:伪造重复握手不得篡改已建立的兼容状态"
    );

    responder.abort();
    pipeline.shutdown().await;
}

#[test]
fn refused_handshake_gates_legacy_commands() {
    // T10.3 门控:state.handshake = Refused 时 LegacyFallback 命令只读化,
    // 状态栏诚实反馈(不伪造执行);以 /quest pause 为例(orchestrated 桥接)
    use chimera_tui::{InputMode, TuiApp, TuiConfig};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .expect("app init");
    app.state_mut().handshake = Some(chimera_tui::HandshakeState {
        compat: CompatLevel::Refused,
        proto: "1.0.0".into(),
        server_version: "2.26.0".into(),
    });

    // 经斜杠路径提交 /quest pause(→ DispatchPlan::LegacyFallback)
    app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for c in "quest pause q-1".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let (msg, _) = app
        .state()
        .status_message
        .as_ref()
        .expect("Refused 门控应有状态栏反馈");
    assert!(
        msg.contains("不可用") || msg.contains("unavailable"),
        "门控反馈应说明编排命令不可用,got: {msg}"
    );
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "门控后应回到 Normal 模式(命令未执行)"
    );
}
