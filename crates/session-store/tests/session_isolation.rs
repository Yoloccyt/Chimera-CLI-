//! 跨会话隔离与 read-your-writes（R9 / W3-T4）
//!
//! 锁定两个契约：
//! 1. 不同 `session_id` 的事件在读取面互不污染（按会话过滤）；
//! 2. 同一会话 flush 后立即可读（read-your-writes）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use session_store::{CbmrWriter, SessionEvent, SessionId, StoreConfig};

#[tokio::test]
async fn two_sessions_are_isolated_and_read_your_writes_holds() {
    let dir = tempfile::tempdir().expect("创建临时目录");
    let writer = CbmrWriter::open(StoreConfig::test_config(dir.path()))
        .await
        .expect("打开写入器");

    let a = SessionId::new("sess-a");
    let b = SessionId::new("sess-b");

    for i in 0..3u8 {
        writer
            .append(&a, SessionEvent::with_payload("a.event", vec![i]))
            .await
            .expect("会话 A 追加");
    }
    for i in 0..2u8 {
        writer
            .append(&b, SessionEvent::with_payload("b.event", vec![i]))
            .await
            .expect("会话 B 追加");
    }
    writer.flush().await.expect("冲刷");

    let a_events = writer.read_events(&a, None).await.expect("读 A");
    let b_events = writer.read_events(&b, None).await.expect("读 B");

    assert_eq!(a_events.len(), 3, "会话 A 只应看到自己的 3 条");
    assert_eq!(b_events.len(), 2, "会话 B 只应看到自己的 2 条");
    assert!(
        a_events.iter().all(|e| e.event.event_type == "a.event"),
        "会话 A 不得出现会话 B 的事件"
    );
    assert!(
        b_events.iter().all(|e| e.event.event_type == "b.event"),
        "会话 B 不得出现会话 A 的事件"
    );

    // read-your-writes：flush 之后同一会话立即可读（含刚追加的最后一条）
    let a_payloads: Vec<Vec<u8>> = a_events
        .iter()
        .filter_map(|e| e.event.payload.clone())
        .collect();
    assert_eq!(a_payloads, vec![vec![0], vec![1], vec![2]]);
}
