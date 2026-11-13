//! ADR-292 验收线 — threads 404 回放桥（404 注入测试）
//!
//! # 场景（ADR-292 §背景,蓝图 §二/§5.5）
//! Anthropic Message threads 的 `continue` 返回 404（服务端线程 best-effort,
//! 空闲过期）→ harness 重新 `create` 前按 Thread 段**完整回放重建对话**。
//!
//! # 验收断言（ADR-292 §验收）
//! 1. 404 注入:构造 `thread.started` + N 条 `item.append` 事件流落盘 → 重开
//!    （模拟进程重启/线程丢失,内存态消失）→ 回放 + 重构 → 断言 `Vec<Item>`
//!    与原始**逐条一致零丢失**;
//! 2. 线程丢失后仍可从 store 重建（`replay_thread_from_store` 桥 +
//!    `replay_since` store 兜底分支——生产段真实路径）;
//! 3. 旧数据兼容:仅含 `turn.submit` 的历史会话回放**不 panic**（重构器对
//!    新事件类型缺省容忍 → 空结果 → `replay_since` 维持 `None` 语义）。
//!
//! # 运行
//! `cargo test -p nexus-app-server --test threads_404_replay_bridge`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use nexus_app_server::server::InMemoryBackend;
use nexus_app_server::{AppServer, AppServerConfig};
use nexus_contracts::app::{
    AppOp, Item, ItemId, ItemStatus, ThreadId, ThreadStartParams, TurnId, UserInput,
};
use session_store::{
    item_append_event, rebuild_items, thread_started_event, CbmrWriter, Offset, ReplayItem,
    SessionEvent, SessionId, StoreConfig, EVENT_TYPE_ITEM_APPEND,
};

/// 构造可区分的测试 Item（kind/status/payload/extras 逐条变化,覆盖字段往返面）
fn sample_item(n: usize) -> Item {
    let status = match n % 4 {
        0 => ItemStatus::Started,
        1 => ItemStatus::InProgress,
        2 => ItemStatus::Completed,
        _ => ItemStatus::Failed,
    };
    let item = Item::new(
        ItemId::new(format!("turn-{}-{}", n / 2 + 1, n % 2 + 1)),
        ThreadId::new("goal-1::run-1"),
        TurnId::new(format!("turn-{}", n / 2 + 1)),
        if n.is_multiple_of(2) { "message" } else { "tool_call" },
        status,
        &format!(r#"{{"content":"404重建-{n}","seq":{n}}}"#),
    );
    // extras 逃逸舱往返（n=2 时附加,验证零丢失含 extras 字段）
    if n == 2 {
        item.with_extras("extras-2")
    } else {
        item
    }
}

/// 验收 1:纯存储面——thread.started + N 条 item.append 落盘 → 重开 →
/// 回放 + 重构 → Vec<Item> 逐条一致零丢失 + 游标对账表与 store offset 对齐
#[tokio::test]
async fn item_append_stream_survives_reopen_and_rebuilds_zero_loss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false; // 确定性:仅显式 flush 触发
    let sid = SessionId::new("goal-1::run-1");
    let originals: Vec<Item> = (0..4).map(sample_item).collect();

    // 第一生命周期:thread.started（流首契约）+ N 条 item.append 落盘
    {
        let store = CbmrWriter::new(cfg.clone()).expect("打开写入器");
        store
            .append(&sid, thread_started_event("goal-1::run-1", 1_700_000_000_000))
            .await
            .expect("thread.started 追加");
        for item in &originals {
            store
                .append(&sid, item_append_event(item))
                .await
                .expect("item.append 追加");
        }
        store.flush().await.expect("冲刷落盘");
    } // drop = 模拟进程重启（内存态消失,段文件 + 树索引留盘）

    // 重开 + 回放（纯段文件权威源,CbmrWriter::replay_session）+ 重构
    let store2 = CbmrWriter::new(cfg.clone()).expect("重开写入器");
    let replayed = store2
        .replay_session(&sid, Offset::new(0, 0))
        .await
        .expect("k-way 归并回放");
    assert_eq!(
        replayed.len(),
        originals.len() + 1,
        "thread.started + N 条 item.append 全部落盘(零丢失前提)"
    );

    let events: Vec<SessionEvent> = replayed.iter().map(|ri| ri.event.clone()).collect();
    let rebuilt = rebuild_items(&events);
    assert_eq!(rebuilt.items.len(), originals.len(), "重建条数 == 写入条数");
    for (a, b) in rebuilt.items.iter().zip(&originals) {
        assert_eq!(a, b, "Item 逐字段一致(零丢失验收线,含 status/extras 往返)");
    }

    // 游标对账（ADR-292 D3）:全量流下 rebuild 游标 offset == store Offset.seq
    // （对齐 item.append 事件的回放位次——「下标 == seq」不变量经真实 store 验证）
    let append_replay: Vec<&ReplayItem> = replayed
        .iter()
        .filter(|ri| ri.event.event_type == EVENT_TYPE_ITEM_APPEND)
        .collect();
    for (cursor, ri) in rebuilt.cursors.iter().zip(&append_replay) {
        assert_eq!(
            cursor.offset, ri.offset.seq,
            "全量流下 rebuild 游标 offset == store Offset.seq"
        );
    }
    for (cursor, original) in rebuilt.cursors.iter().zip(&originals) {
        assert_eq!(cursor.item_id, original.item_id.as_str(), "游标 item_id 对账");
    }
}

/// 验收 2:AppServer 桥——404 注入（新 AppServer 内存态为空 = 线程丢失）,
/// 经生产写面构造的流从 store 重建:replay_thread_from_store 全量一致 +
/// replay_since store 兜底分支按游标续读
#[tokio::test]
async fn threads_404_rebuilds_from_store_after_thread_loss() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false;

    // 第一生命周期:生产写面构造流
    //（ThreadStart → thread.started;TurnSubmit → turn.submit + item.append×2）
    let store = CbmrWriter::new(cfg.clone()).expect("打开写入器");
    let server = AppServer::with_session_store(
        AppServerConfig::default(),
        Box::new(InMemoryBackend),
        store,
    );
    server
        .handle_op(&AppOp::ThreadStart(ThreadStartParams::new("goal-1", "run-1")))
        .await
        .expect("启动成功");
    let tid = ThreadId::new("goal-1::run-1");
    server
        .handle_op(&AppOp::TurnSubmit {
            thread_id: tid.clone(),
            input: UserInput::new("第一轮"),
        })
        .await
        .expect("提交成功");
    let expected: Vec<Item> = server.snapshot(&tid).expect("快照存在").items;
    assert_eq!(expected.len(), 2, "InMemoryBackend 单回合产出 2 个 Item");
    drop(server); // 404 注入:进程重启,内存态（DashMap）消失

    // 第二生命周期:重开同一 store（磁盘权威源仍在）+ 全新 AppServer（无内存态）
    let store2 = CbmrWriter::new(cfg.clone()).expect("重开写入器");
    let server2 = AppServer::with_session_store(
        AppServerConfig::default(),
        Box::new(InMemoryBackend),
        store2,
    );
    assert_eq!(server2.session_count(), 0, "404 注入:内存态线程丢失");

    // 回放桥:store 事件流 → rebuild → Vec<Item> 与丢失前逐条一致（零丢失）
    let rebuilt = server2
        .replay_thread_from_store(&tid)
        .await
        .expect("回放桥重建");
    assert_eq!(rebuilt.len(), expected.len(), "重建条数 == 丢失前 Item 数");
    for (a, b) in rebuilt.iter().zip(&expected) {
        assert_eq!(a, b, "回放桥重建与丢失前 Item 逐条一致(零丢失)");
    }

    // replay_since store 兜底分支（ADR-292 D4 生产调用点）:线程不在内存 →
    // store 重建后按 last_item_id 过滤增量
    let replay = server2
        .replay_since(&tid, &expected[0].item_id)
        .await
        .expect("store 兜底回放");
    assert_eq!(replay.len(), 1, "兜底分支回放游标后增量");
    assert_eq!(replay[0].item_id.as_str(), expected[1].item_id.as_str());
}

/// 验收 3:旧数据兼容——仅含 `turn.submit` 的历史会话流（ADR-292 D1 之前）
/// 回放不 panic,重构为空,replay_since 维持 `None` 语义（线程未知）
#[tokio::test]
async fn legacy_turn_submit_only_stream_tolerated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = StoreConfig::with_dir(dir.path());
    cfg.spawn_flush_loop = false;

    let store = CbmrWriter::new(cfg).expect("打开写入器");
    let sid = SessionId::new("legacy-thread");
    store
        .append(
            &sid,
            SessionEvent::with_payload("turn.submit", br#"{"turn_id":"turn-1"}"#.to_vec()),
        )
        .await
        .expect("历史 turn.submit 追加");
    store.flush().await.expect("冲刷落盘");

    let server =
        AppServer::with_session_store(AppServerConfig::default(), Box::new(InMemoryBackend), store);
    // 桥不 panic（ADR-292 验收线:旧数据容忍）
    let rebuilt = server
        .replay_thread_from_store(&ThreadId::new("legacy-thread"))
        .await
        .expect("旧数据流回放不 panic");
    assert!(rebuilt.is_empty(), "仅 turn.submit → 空重建(无 Item 内容可重构)");
    // 空重建 → 线程未知 → replay_since 维持 None 原语义
    let replay = server
        .replay_since(&ThreadId::new("legacy-thread"), &ItemId::new("turn-1-1"))
        .await;
    assert!(replay.is_none(), "空重建维持 None 语义(旧数据容忍)");
}
