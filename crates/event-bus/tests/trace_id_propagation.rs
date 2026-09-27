//! O-4 trace-id 传播基础件验证（`metadata_mut` + 显式盖戳不破坏事件相等性）。
//!
//! 锁定两条契约：
//! 1. `metadata_mut()` 可原地设 trace_id（传播点用它继承上游 trace）。
//! 2. trace_id 由**发起方显式** `ensure_trace_id` 而非 publish 自动注入——后者会给
//!    发出事件补 id 使 `recv == sent` 相等断言回归；本测证明"显式盖戳"两端一致。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, NexusEvent};

fn quest_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test-harness"),
        quest_id: "q-trace-1".to_string(),
        title: "trace".to_string(),
        task_count: 1,
    }
}

/// metadata_mut 能把 trace_id 从 None 置为 Some（传播点写上游 trace 的能力）。
#[test]
fn metadata_mut_sets_trace_id() {
    let mut ev = quest_event();
    assert!(
        ev.metadata().trace_id.is_none(),
        "new() 默认 trace_id 应为 None"
    );
    ev.metadata_mut().trace_id = Some("trace-abc".to_string());
    assert_eq!(
        ev.metadata().trace_id.as_deref(),
        Some("trace-abc"),
        "metadata_mut 应可原地写 trace_id"
    );
}

/// 发起方显式 ensure_trace_id 后 publish → recv：两端事件相等（不破坏相等性），
/// 且收到的事件确实带上了 trace_id。对照"publish 自动注入"会令 sent(无)≠recv(有)。
#[tokio::test]
async fn explicit_trace_survives_publish_roundtrip() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let mut ev = quest_event();
    ev.metadata_mut().ensure_trace_id(); // 显式盖戳（返回 &str 就地丢弃）
    let sent = ev.clone();
    assert!(
        sent.metadata().trace_id.is_some(),
        "显式盖戳后 sent 应带 trace_id"
    );

    bus.publish(ev).await.expect("publish 应成功");
    let got = rx.recv().await.expect("recv 应收到事件");

    assert_eq!(
        got, sent,
        "显式盖 trace 后收发事件应相等（相等性未被 publish 破坏）"
    );
    assert!(got.metadata().trace_id.is_some(), "trace_id 应随事件送达");
}

/// O-4 传播链端到端：根事件 ensure_trace_id 定锚 → 子事件 `child_of` 继承 →
/// 经总线送达后子事件携带与根相同的 trace_id（跨事件可关联为一条追踪链）。
#[tokio::test]
async fn child_of_inherits_trace_across_bus() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let mut root = quest_event();
    let root_trace = root.metadata_mut().ensure_trace_id().to_string();
    bus.publish(root).await.expect("publish root");
    let got_root = rx.recv().await.expect("recv root");
    assert_eq!(
        got_root.metadata().trace_id.as_deref(),
        Some(root_trace.as_str()),
        "根事件应携带 ensure_trace_id 定的 trace"
    );

    // 派生子事件：child_of 继承父 trace_id/correlation（source 标派生点）
    let child_meta = EventMetadata::child_of(got_root.metadata(), "child-crate");
    let child = NexusEvent::QuestCreated {
        metadata: child_meta,
        quest_id: "q-child".to_string(),
        title: "child".to_string(),
        task_count: 0,
    };
    bus.publish(child.clone()).await.expect("publish child");
    let got_child = rx.recv().await.expect("recv child");
    assert_eq!(
        got_child.metadata().trace_id.as_deref(),
        Some(root_trace.as_str()),
        "子事件应继承父 trace_id，形成跨事件追踪链"
    );
}
