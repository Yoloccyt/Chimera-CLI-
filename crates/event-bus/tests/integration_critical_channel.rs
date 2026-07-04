//! Critical 事件双通道集成测试 — 验证 §6.2 红线双通道化
//!
//! 测试目标:
//! 1. 4 类 Critical 安全告警事件通过 `subscribe_critical_events()` 正常接收
//! 2. 主通道 broadcast Lagged 时,Critical 事件仍能通过 mpsc 旁路接收
//! 3. 非 Critical 事件不通过 mpsc 旁路接收
//!
//! 设计依据:§6.2 红线"Critical 安全事件(SkepticVeto/RedTeamAudit/
//! AsaIntervention/BudgetExceeded)必须用 mpsc channel 确保送达"。
//! 双通道实现见 `bus.rs::EventBus::subscribe_critical_events`。

use std::time::Duration;

use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};

// ============================================================
// 测试辅助函数 — 构造 4 类 Critical 安全告警事件 + 普通 Normal 事件
// ============================================================

fn make_skeptic_veto(quest_id: &str) -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.into(),
        veto_reason: "unsafe shell injection detected".into(),
        frozen_capabilities: vec!["shell_exec".into()],
    }
}

fn make_red_team_audit() -> NexusEvent {
    NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 8,
        total_probes: 20,
        detection_rate: 0.4,
        remediation_suggestion: "add input sanitization".into(),
    }
}

fn make_asa_intervention_block() -> NexusEvent {
    // WHY Block 级:AsaIntervention 的 severity() 返回 Normal(同步函数不依赖
    // 运行时值),但 Block 级在语义上等价于 Critical,通过 is_critical_mpsc_event
    // 强制走 mpsc 旁路(见 bus.rs::is_critical_mpsc_event 注释)
    NexusEvent::AsaIntervention {
        metadata: EventMetadata::new("seccore"),
        operation_id: "op-block-001".into(),
        action: "Block".into(),
        safety_score: 0.15,
        block_reason: Some("unsafe operation".into()),
        alternative_suggestion: Some("use sandboxed tool".into()),
    }
}

fn make_budget_exceeded(current: u64, limit: u64) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "total_cost".into(),
        current,
        limit,
    }
}

fn make_normal_event(id: u32) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("key-{id}"),
    }
}

// ============================================================
// 测试 1:4 类 Critical 事件通过 mpsc 旁路通道正常接收
// ============================================================

#[tokio::test]
async fn test_critical_events_received_via_mpsc_bypass() {
    let bus = EventBus::new();
    let mut crit_rx = bus.subscribe_critical_events();

    // 发布 4 类 Critical 安全告警事件
    let skeptic = make_skeptic_veto("q-skeptic");
    let red_team = make_red_team_audit();
    let asa_block = make_asa_intervention_block();
    let budget = make_budget_exceeded(10_000, 8_000);

    bus.publish(skeptic.clone()).await.unwrap();
    bus.publish(red_team.clone()).await.unwrap();
    bus.publish(asa_block.clone()).await.unwrap();
    bus.publish(budget.clone()).await.unwrap();

    // 通过 mpsc 旁路通道接收,验证 4 个事件全部到达
    let received_skeptic = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("SkepticVeto 应通过 mpsc 旁路接收")
        .expect("mpsc channel 不应关闭");
    assert_eq!(received_skeptic, skeptic);
    assert_eq!(received_skeptic.severity(), EventSeverity::Critical);

    let received_red_team = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("RedTeamAudit 应通过 mpsc 旁路接收")
        .expect("mpsc channel 不应关闭");
    assert_eq!(received_red_team, red_team);

    let received_asa = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("AsaIntervention 应通过 mpsc 旁路接收")
        .expect("mpsc channel 不应关闭");
    assert_eq!(received_asa, asa_block);

    let received_budget = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("BudgetExceeded 应通过 mpsc 旁路接收")
        .expect("mpsc channel 不应关闭");
    assert_eq!(received_budget, budget);
    assert_eq!(received_budget.severity(), EventSeverity::Critical);
}

// ============================================================
// 测试 2:主通道 broadcast Lagged 时,Critical 事件仍能通过 mpsc 旁路接收
//
// WHY 此测试是 §6.2 红线的核心验证:模拟慢消费者场景下主通道丢弃事件,
// 断言 Critical 事件不丢失。这是双通道化的根本目的。
// ============================================================

#[tokio::test]
async fn test_critical_event_survives_main_channel_lagged() {
    // 主通道容量 4,发布大量非 Critical 事件触发 Lagged
    let bus = EventBus::with_capacity(4);
    let mut slow_main_rx = bus.subscribe(); // 主通道慢消费者,不消费
    let mut crit_rx = bus.subscribe_critical_events();

    // 填充 20 个非 Critical 事件,远超主通道容量 4,触发 Lagged
    // 这些事件不走 mpsc 旁路(is_critical_mpsc_event=false),不影响旁路通道
    for i in 0..20 {
        bus.publish(make_normal_event(i)).await.unwrap();
    }

    // 发布一个 Critical 事件(预算超限)
    let critical_event = make_budget_exceeded(99_999, 50_000);
    bus.publish(critical_event.clone()).await.unwrap();

    // 主通道慢消费者应收到 Lagged 错误(broadcast 通道行为)
    // WHY 主通道会 Lagged:容量 4,积压 20+1 个事件,receiver 落后 17+ 个
    let main_result = slow_main_rx.recv().await;
    assert!(
        main_result.is_err(),
        "主通道慢消费者应收到错误(Lagged),实际: {main_result:?}"
    );

    // 关键断言:Critical 事件必须通过 mpsc 旁路通道到达订阅者
    // 即使主通道 Lagged,mpsc 旁路 Unbounded 不会丢失
    let received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("Critical 事件应在 1s 内通过 mpsc 旁路到达")
        .expect("mpsc channel 不应关闭");
    assert_eq!(
        received, critical_event,
        "mpsc 旁路接收到的事件应与发布的一致"
    );
    assert_eq!(received.severity(), EventSeverity::Critical);
}

// ============================================================
// 测试 3:非 Critical 事件不通过 mpsc 旁路接收
//
// WHY 此测试守护通道隔离:Normal 事件仅走主 broadcast,
// 不应泄漏到 mpsc 旁路(否则旁路订阅者会被噪声事件淹没)。
// ============================================================

#[tokio::test]
async fn test_normal_event_not_received_via_mpsc_bypass() {
    let bus = EventBus::new();
    let mut crit_rx = bus.subscribe_critical_events();
    let mut main_rx = bus.subscribe(); // 主通道订阅者,验证 Normal 事件确实发布了

    // 发布非 Critical 事件(CacheHit,Normal 级)
    let normal = make_normal_event(42);
    bus.publish(normal.clone()).await.unwrap();

    // 主通道应正常接收
    let main_received = tokio::time::timeout(Duration::from_secs(1), main_rx.recv())
        .await
        .expect("主通道应收到 Normal 事件")
        .expect("主通道不应关闭");
    assert_eq!(main_received, normal);

    // mpsc 旁路通道不应收到任何事件(100ms 内无事件)
    let bypass_result = tokio::time::timeout(Duration::from_millis(100), crit_rx.recv()).await;
    assert!(
        bypass_result.is_err(),
        "mpsc 旁路不应收到 Normal 事件,但收到了: {bypass_result:?}"
    );
}

// ============================================================
// 测试 4:多订阅者 fan-out — 多个 Critical 订阅者各自收到事件
//
// WHY 此测试验证 fan-out 模式:多个组件(efficiency-monitor/parliament/
// seccore)同时订阅 Critical 流,每个订阅者独立收到事件拷贝。
// ============================================================

#[tokio::test]
async fn test_multiple_critical_subscribers_fanout() {
    let bus = EventBus::new();
    let mut crit_rx1 = bus.subscribe_critical_events();
    let mut crit_rx2 = bus.subscribe_critical_events();
    let mut crit_rx3 = bus.subscribe_critical_events();

    let event = make_skeptic_veto("q-fanout");
    bus.publish(event.clone()).await.unwrap();

    // 三个订阅者应各自收到一份事件拷贝
    for (i, rx) in [(&mut crit_rx1), (&mut crit_rx2), (&mut crit_rx3)]
        .into_iter()
        .enumerate()
    {
        let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("订阅者 {i} 应收到 Critical 事件"))
            .expect("mpsc channel 不应关闭");
        assert_eq!(received, event, "订阅者 {i} 收到的事件应与发布的一致");
    }
}

// ============================================================
// 测试 5:publish_critical 显式 API — 不依赖 is_critical_mpsc_event 判定
//
// WHY 此测试验证 explicit API:调用方明确知道事件为 Critical 时,
// 可使用 publish_critical 直接走双通道,绕过 is_critical_mpsc_event 判定。
// 适用于未来扩展的 Critical 事件(如 AsaIntervention Block 级)。
// ============================================================

#[tokio::test]
async fn test_publish_critical_explicit_api() {
    let bus = EventBus::new();
    let mut crit_rx = bus.subscribe_critical_events();
    let mut main_rx = bus.subscribe();

    // 使用显式 publish_critical 发布(即使事件本身 severity=Normal 也会走双通道)
    // 这里用 BudgetExceeded(severity=Critical)验证语义一致性
    let event = make_budget_exceeded(500, 400);
    bus.publish_critical(event.clone()).await.unwrap();

    // mpsc 旁路应收到
    let crit_received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("publish_critical 应通过 mpsc 旁路投递")
        .expect("mpsc channel 不应关闭");
    assert_eq!(crit_received, event);

    // 主 broadcast 也应收到(双推)
    let main_received = tokio::time::timeout(Duration::from_secs(1), main_rx.recv())
        .await
        .expect("publish_critical 应同时通过 broadcast 投递")
        .expect("broadcast 不应关闭");
    assert_eq!(main_received, event);
}

// ============================================================
// 测试 6:publish_blocking 同步双推 — Critical 事件经同步路径到达旁路
//
// WHY 此测试验证 sync 路径:decb-governor 等组件在同步方法中调用
// publish_blocking 发布 BudgetExceeded,必须确保 mpsc 旁路也收到。
// ============================================================

#[tokio::test]
async fn test_publish_blocking_dual_channel() {
    let bus = EventBus::new();
    let mut crit_rx = bus.subscribe_critical_events();
    let mut main_rx = bus.subscribe();

    let event = make_budget_exceeded(1_000_000, 500_000);
    bus.publish_blocking(event.clone()).unwrap();

    // mpsc 旁路应收到
    let crit_received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("publish_blocking 应通过 mpsc 旁路投递 Critical 事件")
        .expect("mpsc channel 不应关闭");
    assert_eq!(crit_received, event);

    // 主 broadcast 也应收到
    let main_received = tokio::time::timeout(Duration::from_secs(1), main_rx.recv())
        .await
        .expect("publish_blocking 应同时通过 broadcast 投递")
        .expect("broadcast 不应关闭");
    assert_eq!(main_received, event);
}
