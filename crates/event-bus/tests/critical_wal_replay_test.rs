//! WI-G5 — Critical WAL 总线级「写入 → 重启 → 重放零丢失」覆盖（ADR-191 闭合件）
//!
//! # 与既有测试的分工
//! `critical_sink.rs` 的 `file_wal_tests` 直接调 `on_critical`（sink 单元面）;
//! 本文件覆盖**真实总线链**：`publish` → 空 mpsc 订阅者分支 →
//! `with_critical_fallback` 注入的 `FileWalCriticalSink` → JSONL 落盘 →
//! drop 总线（模拟持有者进程终止）→ **全新实例** reopen → `replay()` 零丢失。
//!
//! # 既定决策不动（测试不改默认）
//! - `FileWalCriticalSink` 非默认（默认 `LogCriticalSink`），本测试以显式注入验证；
//! - 无显式 fsync 维持既定决策：持久化前提 = `LineWriter` 逐行 flush
//!   （`critical_sink.rs:165-166` 注），本测试即对该前提的总线级实测。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, FileWalCriticalSink, NexusEvent};

fn budget_exceeded(source: &str, budget_type: &str) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new(source),
        budget_type: budget_type.into(),
        current: 10_000,
        limit: 8_000,
    }
}

fn wal_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "chimera_wal_bus_replay_{}.logl",
        uuid::Uuid::now_v7()
    ))
}

/// 主链：无订阅者 → 保底 sink → WAL → 进程终止（drop）→ 重启 → 重放零丢失。
#[tokio::test]
async fn bus_level_wal_write_restart_replay_zero_loss() {
    let path = wal_path();
    let event_id;
    {
        let sink = FileWalCriticalSink::open(&path).expect("open wal");
        let bus = EventBus::new().with_critical_fallback(Arc::new(sink));
        let ev = budget_exceeded("decb-governor", "token");
        event_id = ev.metadata().event_id.to_string();
        // 无 mpsc 订阅者 → send_critical_mpsc 空分支 → 保底 sink（不丢弃）
        bus.publish(ev).await.unwrap();
        assert_eq!(
            bus.critical_no_subscriber_total(),
            1,
            "空订阅者保底计数应为 1"
        );
    } // bus drop ⇒ sink drop ⇒ 文件句柄关闭（模拟持有者进程终止）

    // 「重启」：全新实例打开同一磁盘 WAL
    let reopened = FileWalCriticalSink::open(&path).expect("reopen wal");
    let replayed = reopened.replay().expect("replay after restart");
    assert_eq!(replayed.len(), 1, "写入→重启→重放零丢失");
    match &replayed[0] {
        NexusEvent::BudgetExceeded {
            metadata,
            budget_type,
            current,
            limit,
        } => {
            assert_eq!(metadata.event_id.to_string(), event_id, "event_id 逐位保真");
            assert_eq!(metadata.source, "decb-governor");
            assert_eq!(budget_type, "token");
            assert_eq!((*current, *limit), (10_000, 8_000), "载荷完整重放");
        }
        other => panic!("重放事件变体不符：{other:?}"),
    }
    drop(reopened);
    let _ = std::fs::remove_file(&path);
}

/// 负对照：真实 mpsc 消费者在位 ⇒ 事件走 mpsc，保底 WAL 零写入。
#[tokio::test]
async fn subscriber_present_keeps_wal_empty() {
    let path = wal_path();
    {
        let sink = FileWalCriticalSink::open(&path).expect("open wal");
        let bus = EventBus::new().with_critical_fallback(Arc::new(sink));
        let mut rx = bus.subscribe_critical_events(); // 真实消费者在位（红线推荐态）
        bus.publish(budget_exceeded("src-a", "token"))
            .await
            .unwrap();
        let got = rx.try_recv().expect("mpsc 旁路应收到 Critical 事件");
        assert!(matches!(got, NexusEvent::BudgetExceeded { .. }));
        assert_eq!(
            bus.critical_no_subscriber_total(),
            0,
            "有 mpsc 订阅者不得走保底分支"
        );
    }
    let reopened = FileWalCriticalSink::open(&path).expect("reopen wal");
    assert!(
        reopened.replay().expect("replay").is_empty(),
        "有真实消费者时保底 WAL 必须零写入（不制造双写噪声）"
    );
    let _ = std::fs::remove_file(&path);
}
