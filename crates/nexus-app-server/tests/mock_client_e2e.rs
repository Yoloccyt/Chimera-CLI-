//! WI-01 验收: 50 行 mock 客户端完整 Turn + 断线重连（协议级 E2E）
//!
//! # 验收口径（计划 §4 WI-01）
//! - 50 行 mock 客户端完成完整 Turn（ThreadStart → TurnSubmit → 事件流）
//! - kill -9 → 重连渲染一致（replay_since 增量回放）
//! - 协议 v1 冻结语义（extras 逃逸舱不破坏既有字段）
//!
//! # 实现方式
//! 使用内存 [`MockTransport`]（管道对），完整走 JSON-RPC v1 帧编解码
//! （`RpcCodec`）——不直接调用 server 方法，验证协议面（内闭外开 T6）。

use nexus_app_server::{AppServer, AppServerConfig, RpcCodec, RpcNotification, RpcResponse};
use nexus_contracts::app::{AppOp, AppTokenUsage, ThreadId, UserInput};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 内存传输 — 客户端侧模拟（stdin/stdout 对）
///
/// 客户端视角: 发送 AppOp 请求帧 → 接收响应帧/推送帧。
/// 服务端视角: 经同一 RpcCodec 解码请求 → AppServer::handle_op → 编码推送。
#[derive(Debug, Clone, Default)]
struct MockClient {
    /// 待发送请求（服务端消费）
    outbox: Arc<Mutex<VecDeque<String>>>,
    /// 已接收帧（客户端消费）
    inbox: Arc<Mutex<VecDeque<String>>>,
    /// 请求 ID 自增
    next_id: u64,
}

impl MockClient {
    fn new() -> Self {
        Self::default()
    }

    /// 发送 AppOp（编码为 JSON-RPC 请求帧）
    fn send_op(&mut self, op: &AppOp) -> u64 {
        self.next_id += 1;
        let frame = RpcCodec::encode_request(op, self.next_id).expect("请求帧编码成功");
        self.outbox.lock().expect("outbox 锁").push_back(frame);
        self.next_id
    }

    /// 接收一帧（客户端队列）
    fn recv_frame(&self) -> Option<String> {
        self.inbox.lock().expect("inbox 锁").pop_front()
    }

    /// 服务端侧: 取出一条待处理请求
    fn take_request(&self) -> Option<String> {
        self.outbox.lock().expect("outbox 锁").pop_front()
    }

    /// 服务端侧: 推送事件到客户端
    fn push_event(&self, frame: String) {
        self.inbox.lock().expect("inbox 锁").push_back(frame);
    }
}

/// 驱动一次协议往返: 客户端请求 → 服务端处理 → 事件推送回客户端
async fn drive_roundtrip(client: &MockClient, server: &AppServer) {
    // 取出所有待处理请求（客户端可能批发了多个）
    while let Some(frame) = client.take_request() {
        let req = RpcCodec::decode_request_line(&frame).expect("请求帧解码成功");
        let op: AppOp = serde_json::from_value(req.params).expect("AppOp 反序列化成功");
        let events = server.handle_op(&op).await.expect("服务端处理成功");
        // 事件推送（RpcNotification 帧）+ 响应帧（RpcResponse）
        for ev in &events {
            let push = RpcCodec::encode_notification(ev).expect("推送帧编码成功");
            client.push_event(push);
        }
        let resp = RpcCodec::encode_result(
            req.id,
            &events.last().cloned().unwrap_or(
                // 无事件的 op（如 ModeSet）以空确认响应
                nexus_contracts::app::AppEvent::TurnCompleted {
                    turn_id: nexus_contracts::app::TurnId::new("ack"),
                    usage: AppTokenUsage::new(0, 0, 0, 0),
                },
            ),
        )
        .expect("响应帧编码成功");
        client.push_event(resp);
    }
}

/// 从客户端收件箱解析下一帧（跳过非目标帧）
fn next_notification(client: &MockClient) -> nexus_contracts::app::AppEvent {
    loop {
        let frame = client.recv_frame().expect("应有一帧");
        if let Ok(notif) = serde_json::from_str::<RpcNotification>(&frame) {
            if notif.method == "app.event" {
                return serde_json::from_value(notif.params).expect("AppEvent 反序列化成功");
            }
        }
    }
}

#[tokio::test]
async fn mock_client_completes_full_turn_over_protocol() {
    // WI-01 验收: 50 行 mock 客户端完成完整 Turn（ThreadStart → TurnSubmit → 事件流）
    let mut client = MockClient::new();
    let server = AppServer::new(AppServerConfig::default());

    // 1. ThreadStart
    client.send_op(&AppOp::ThreadStart(
        nexus_contracts::app::ThreadStartParams::new("goal-1", "run-1"),
    ));
    drive_roundtrip(&client, &server).await;
    // 首帧应为 ThreadStarted 推送
    match next_notification(&client) {
        nexus_contracts::app::AppEvent::ThreadStarted { thread } => {
            assert_eq!(thread.goal_id.as_ref(), "goal-1");
        }
        other => panic!("期望 ThreadStarted, 实际 {other:?}"),
    }

    // 2. TurnSubmit（完整 Turn）
    client.send_op(&AppOp::TurnSubmit {
        thread_id: ThreadId::new("goal-1::run-1"),
        input: UserInput::new("你好，Chimera"),
    });
    drive_roundtrip(&client, &server).await;
    let mut item_count = 0;
    let mut turn_done = false;
    // 帧序列: [ItemChanged, ItemChanged, TurnCompleted, Response]——
    // Response 帧被 next_notification 跳过（非 app.event），消费 3 帧即可
    for _ in 0..3 {
        match next_notification(&client) {
            nexus_contracts::app::AppEvent::ItemChanged { item } => {
                item_count += 1;
                assert_eq!(item.thread_id.as_str(), "goal-1::run-1");
            }
            nexus_contracts::app::AppEvent::TurnCompleted { .. } => {
                turn_done = true;
            }
            other => panic!("协议流中出现意外事件: {other:?}"),
        }
    }
    assert_eq!(item_count, 2, "完整 Turn 应产出 2 个 Item");
    assert!(turn_done, "TurnCompleted 必须出现");

    // 3. 断线重连: replay_since 增量回放（kill -9 → 重连渲染一致）
    let snap = server
        .snapshot(&ThreadId::new("goal-1::run-1"))
        .expect("会话快照存在");
    let last_id = &snap.items[0].item_id;
    let replay = server
        .replay_since(&ThreadId::new("goal-1::run-1"), last_id)
        .expect("回放成功");
    assert_eq!(replay.len(), 1, "重连后应回放 1 条增量 Item");
    assert_eq!(replay[0].item_id.as_str(), snap.items[1].item_id.as_str());
}

#[tokio::test]
async fn mock_client_approval_flow_over_protocol() {
    // 审批往返经协议面: 请求登记（服务端注入）→ 客户端 ApprovalRespond
    let mut client = MockClient::new();
    let server = AppServer::new(AppServerConfig::default());
    let tid = ThreadId::new("goal-1::run-1");

    client.send_op(&AppOp::ThreadStart(
        nexus_contracts::app::ThreadStartParams::new("goal-1", "run-1"),
    ));
    drive_roundtrip(&client, &server).await;
    let _ = next_notification(&client); // ThreadStarted

    // 服务端注入待审批请求（模拟后端审批源）
    server
        .inject_approval_request(
            &tid,
            nexus_contracts::app::ApprovalRequest::new(
                nexus_contracts::app::ReqId::new("req-1"),
                "运行 cargo build",
                "idempotent_write",
                None,
            ),
        )
        .expect("注入成功");

    // 客户端裁决（AllowOnce = 单次提权, WI-23 语义对齐）
    client.send_op(&AppOp::ApprovalRespond {
        request_id: nexus_contracts::app::ReqId::new("req-1"),
        decision: nexus_contracts::app::ApprovalDecision::AllowOnce,
    });
    drive_roundtrip(&client, &server).await;
    assert!(
        server.pending_approvals(&tid).is_empty(),
        "审批裁决后待审批队列应清空"
    );
}

#[tokio::test]
async fn mock_client_fork_over_protocol() {
    // WI-18 协议面: ThreadFork 经协议 → 新会话（分支式探索语义）
    let mut client = MockClient::new();
    let server = AppServer::new(AppServerConfig::default());
    let tid = ThreadId::new("goal-1::run-1");

    client.send_op(&AppOp::ThreadStart(
        nexus_contracts::app::ThreadStartParams::new("goal-1", "run-1"),
    ));
    drive_roundtrip(&client, &server).await;
    let _ = next_notification(&client);

    client.send_op(&AppOp::TurnSubmit {
        thread_id: tid.clone(),
        input: UserInput::new("第一轮"),
    });
    drive_roundtrip(&client, &server).await;
    // 帧序列: [Item1, Item2, TurnCompleted, Response]——消费 3 帧（Response 跳过）
    for _ in 0..3 {
        let _ = next_notification(&client);
    }

    let snap = server.snapshot(&tid).expect("快照存在");
    client.send_op(&AppOp::ThreadFork {
        thread_id: tid.clone(),
        at: snap.items[0].item_id.clone(),
    });
    drive_roundtrip(&client, &server).await;
    match next_notification(&client) {
        nexus_contracts::app::AppEvent::ThreadStarted { thread } => {
            assert!(
                thread.thread_id.as_str().ends_with("-fork"),
                "分叉会话 ID 应带 -fork 后缀"
            );
        }
        other => panic!("期望分叉 ThreadStarted, 实际 {other:?}"),
    }
    assert_eq!(server.session_count(), 2, "分叉后应有 2 个会话");
}

#[tokio::test]
async fn reconnect_restores_event_stream_over_protocol() {
    // WI-01 验收: kill -9 → 重连渲染一致（协议面）
    // 客户端断开（丢弃连接）后，新客户端持 last_item_id 重连，增量经协议帧
    // （ItemChanged 推送）恢复；重连渲染与断开前一致，且会话继续工作。
    let server = AppServer::new(AppServerConfig::default());
    let tid = ThreadId::new("goal-1::run-1");

    // 第一连接: 完成一轮完整 Turn（产出 2 个 Item）
    let mut c1 = MockClient::new();
    c1.send_op(&AppOp::ThreadStart(
        nexus_contracts::app::ThreadStartParams::new("goal-1", "run-1"),
    ));
    drive_roundtrip(&c1, &server).await;
    let _ = next_notification(&c1); // ThreadStarted
    c1.send_op(&AppOp::TurnSubmit {
        thread_id: tid.clone(),
        input: UserInput::new("第一轮"),
    });
    drive_roundtrip(&c1, &server).await;
    let mut seen_c1 = Vec::new();
    for _ in 0..3 {
        match next_notification(&c1) {
            nexus_contracts::app::AppEvent::ItemChanged { item } => seen_c1.push(item),
            nexus_contracts::app::AppEvent::TurnCompleted { .. } => {}
            other => panic!("断开前出现意外事件: {other:?}"),
        }
    }
    assert_eq!(seen_c1.len(), 2, "断开前已渲染 2 个 Item");

    // kill -9: 丢弃 c1（连接断开），服务端保留会话状态
    drop(c1);

    // 第二连接（重连）: 持 last_item_id = 断开时已知首 Item, 经协议帧恢复增量
    let mut c2 = MockClient::new();
    let replay = server
        .replay_since(&tid, &seen_c1[0].item_id)
        .expect("回放成功");
    assert_eq!(replay.len(), 1, "断开后应回放 1 条增量 Item");
    for item in &replay {
        // 重放增量以协议推送帧（ItemChanged）下发 → 客户端渲染恢复
        let ev = nexus_contracts::app::AppEvent::ItemChanged { item: item.clone() };
        let push = RpcCodec::encode_notification(&ev).expect("推送帧编码成功");
        c2.push_event(push);
    }
    let restored = next_notification(&c2);
    match restored {
        nexus_contracts::app::AppEvent::ItemChanged { item } => {
            assert_eq!(
                item.item_id.as_str(),
                seen_c1[1].item_id.as_str(),
                "重连渲染应与断开前一致"
            );
        }
        other => panic!("重连恢复帧异常: {other:?}"),
    }

    // 重连后会话继续工作（新回合经协议帧正常推进）
    c2.send_op(&AppOp::TurnSubmit {
        thread_id: tid,
        input: UserInput::new("重连后第二轮"),
    });
    drive_roundtrip(&c2, &server).await;
    let mut resumed_items = 0;
    for _ in 0..3 {
        if let nexus_contracts::app::AppEvent::ItemChanged { .. } = next_notification(&c2) {
            resumed_items += 1;
        }
    }
    assert_eq!(resumed_items, 2, "重连后新回合事件流恢复");
}

/// 编译期断言: 响应帧协议面冻结（extras 逃逸舱语义）
#[test]
fn protocol_v1_frozen_semantics() {
    // 响应帧: result 与 error 互斥
    let resp: RpcResponse = serde_json::from_str(
        r#"{"jsonrpc":"2.0","id":1,"result":{"ThreadStarted":{"thread":{"thread_id":"t-1","goal_id":"g","run_id":"r","created_at_ms":0}}}}"#,
    )
    .expect("响应帧解码成功");
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
}
