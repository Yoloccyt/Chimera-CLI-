//! 快照载荷共享（Phase 6B / T6B.1）
//!
//! 两条断言必须成对成立，缺一条就测不出正确性：
//! 1. **无新消息 ⇒ 共享同一分配**（`Arc::ptr_eq` 为真）——这是"消除每 tick 深拷
//!    ChatMessage 载荷"的验收点；
//! 2. **有新消息 ⇒ 必须换新分配且旧快照内容不变**——否则 `Rc` + 内部可变之类的
//!    "假共享"也能过第 1 条，而实际会把已交付给面板的历史就地改掉（数据竞争语义破口）。
//!
//! 快照只在"本 tick 有新事件"时重建（pipeline 既有纪律），因此两条事件之间用轮询
//! 等待隔开，保证它们落在不同 rebuild 里；否则两次 `snapshot()` 拿到同一个 Arc，
//! 第 1 条断言就成了空转。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chimera_tui::{DataPipeline, DataSnapshot, DataSourceConfig, EventSubscriber};
use event_bus::{ChatStatus, EventBus, EventMetadata, NexusEvent};

/// 测试配置：tick 50ms 便于快速重建快照
fn test_config() -> DataSourceConfig {
    DataSourceConfig {
        max_event_history: 256,
        max_quest_list_size: 64,
        budget_metrics_ttl_ms: 5000,
        tick_interval_ms: 50,
        max_history_len: 64,
        max_security_summaries: 10,
        max_frozen_capabilities: 20,
        snapshot_interval_s: 30,
        max_snapshots: 100,
        eco_tick_interval_ms: 1000,
        event_backlog_threshold: 100,
        max_chat_messages: 500,
    }
}

/// 构造一条会写入 chat 历史的事件
fn chat_submitted(query: &str) -> NexusEvent {
    NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("chimera-tui-test"),
        session_id: "s-1".into(),
        query: query.into(),
        slash_command: None,
    }
}

/// 构造一条**不触碰 chat 历史**的事件（只改会话状态），用于强制快照重建
fn status_changed() -> NexusEvent {
    NexusEvent::TuiChatStatusChanged {
        metadata: EventMetadata::new("chimera-tui-test"),
        session_id: "s-1".into(),
        status: ChatStatus::Thinking,
    }
}

/// 轮询等待快照满足条件（最多 2s），返回那一刻的快照 Arc
///
/// WHY 轮询:pipeline 的 tick 与测试线程并发,固定 sleep 会 flaky。
async fn wait_for(
    pipeline: &DataPipeline,
    expect: impl Fn(&DataSnapshot) -> bool,
    what: &str,
) -> Arc<DataSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snap = pipeline.snapshot();
        if expect(&snap) {
            return snap;
        }
        assert!(
            Instant::now() < deadline,
            "等待「{what}」超时,当前 chat={} events={}",
            snap.chat_messages.len(),
            snap.latest_events.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 无新消息时，重建后的快照必须共享同一份 chat 载荷
#[tokio::test]
async fn chat_snapshot_shares_arc_when_unchanged() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    bus.publish(chat_submitted("第一个问题")).await.unwrap();
    let first = wait_for(&pipeline, |s| !s.chat_messages.is_empty(), "1 条 chat 历史").await;
    assert_eq!(first.chat_messages.len(), 1);
    assert_eq!(first.latest_events.len(), 1, "first 必须来自第一次 rebuild");

    // 再来一条不改 chat 历史的事件 ⇒ 强制一次新的 rebuild
    bus.publish(status_changed()).await.unwrap();
    let second = wait_for(&pipeline, |s| s.latest_events.len() >= 2, "2 条事件").await;

    assert!(
        !Arc::ptr_eq(&first, &second),
        "两次必须是真的不同 rebuild,否则共享断言为空转"
    );
    assert_eq!(second.chat_messages.len(), 1, "第二条事件不写 chat 历史");
    assert!(
        Arc::ptr_eq(&first.chat_messages, &second.chat_messages),
        "无新消息时 chat 载荷必须共享同一分配,不得每 tick 深拷"
    );
}

/// 有新消息时必须换新分配，且已交付的旧快照不被就地修改
#[tokio::test]
async fn chat_snapshot_reallocates_on_write_and_keeps_old_snapshot() {
    let bus = EventBus::with_capacity(1024);
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = DataPipeline::new(subscriber, test_config());

    bus.publish(chat_submitted("问题 A")).await.unwrap();
    let first = wait_for(&pipeline, |s| !s.chat_messages.is_empty(), "1 条 chat 历史").await;

    bus.publish(chat_submitted("问题 B")).await.unwrap();
    let second = wait_for(&pipeline, |s| s.chat_messages.len() >= 2, "2 条 chat 历史").await;

    assert_eq!(first.chat_messages.len(), 1, "旧快照的历史不得被就地追加");
    assert_eq!(second.chat_messages.len(), 2);
    assert!(
        !Arc::ptr_eq(&first.chat_messages, &second.chat_messages),
        "写入必须经 Arc::make_mut 换分配,保证已交付快照不可变"
    );
    assert_eq!(first.chat_messages[0].content, "问题 A");
    assert_eq!(second.chat_messages[1].content, "问题 B");
}
