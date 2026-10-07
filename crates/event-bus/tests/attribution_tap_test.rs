//! ADR-200 D1 验收 V3 — AttributionTap 集成测试:触发集事件自动归因产出因果链
//!
//! 覆盖:
//! 1. SlowConsumerDropped 触发 → 同源前驱（Before）+ 跨源观察者（Concurrent）入链;
//! 2. BudgetExceeded 触发 → 归因同样生效（触发集双变体端到端）;
//! 3. 慢消费者 Lagged 只计数不中断（tap 继续消费后续事件）。
//!
//! 断言面 = `AttributionTapHandle` 只读查询口（与 tap 内部自动归因同一台账）;
//! 时钟语义 = 单进程 per-source 时钟（ADR-200 §2 D1 诚实登记）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::time::Duration;

use event_bus::attribution::{
    is_attribution_trigger, spawn_attribution_tap, ATTRIBUTION_TRIGGER_VARIANTS,
};
use event_bus::{event_fingerprint, CausalRelation, EventBus, EventMetadata, NexusEvent};

/// 等待条件成立（5ms × 400 = 最长 2s;tap 为异步任务,recorded 计数为同步观测点）
async fn wait_until(mut pred: impl FnMut() -> bool, what: &str) {
    for _ in 0..400 {
        if pred() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(pred(), "等待超时:{what}");
}

fn quest(source: &str, quest_id: &str) -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new(source),
        quest_id: quest_id.into(),
        title: format!("t-{quest_id}"),
        task_count: 1,
    }
}

fn slow_dropped(source: &str, subscriber_id: &str) -> NexusEvent {
    NexusEvent::SlowConsumerDropped {
        metadata: EventMetadata::new(source),
        subscriber_id: subscriber_id.into(),
        lag: 10,
        dropped_count: 5,
    }
}

fn budget_exceeded(source: &str) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new(source),
        budget_type: "token".into(),
        current: 10_000,
        limit: 8_000,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 触发集常量锁（集成面负对照）:常量表 ↔ 判定函数 ↔ 非触发集排除
#[test]
fn trigger_set_constant_lock() {
    assert_eq!(
        ATTRIBUTION_TRIGGER_VARIANTS,
        ["SlowConsumerDropped", "BudgetExceeded"],
        "触发集定格;扩展须评审（ADR-200 D1）"
    );
    assert!(is_attribution_trigger(&slow_dropped("s", "sub-1")));
    assert!(is_attribution_trigger(&budget_exceeded("s")));
    assert!(
        !is_attribution_trigger(&quest("s", "q-1")),
        "普通业务事件不得进入触发集（不制造高频归因）"
    );
}

/// V3 主链:SlowConsumerDropped 触发 → 同源前驱 Before ×2 + 跨源观察者 Concurrent ×1
#[tokio::test]
async fn slow_consumer_dropped_triggers_attribution_chain() {
    let bus = EventBus::new();
    let tap = spawn_attribution_tap(&bus);

    // 前置窗口事件:同源 src-a 两条（触发同源 → Before）
    bus.publish(quest("src-a", "q-1")).await.unwrap();
    bus.publish(quest("src-a", "q-2")).await.unwrap();
    // 跨源并发观察者:src-b 一条（触发异源 → Concurrent）
    bus.publish(quest("src-b", "q-3")).await.unwrap();
    // 触发集事件:src-a 的 SlowConsumerDropped
    let trigger = slow_dropped("src-a", "sub-1");
    let trigger_fp = event_fingerprint(&trigger);
    bus.publish(trigger).await.unwrap();

    wait_until(
        || tap.recorded() >= 4 && tap.trigger_firings() >= 1,
        "tap 消费全部 4 条且触发一次归因",
    )
    .await;
    assert_eq!(tap.trigger_firings(), 1, "仅触发集事件触发归因");
    assert_eq!(tap.ledger_len(), 4, "台账=窗口内全部 4 条");

    let result = tap.attribute(trigger_fp, now_ms());
    assert!(result.diff_found, "触发事件本体在台账中:{result:?}");
    assert_eq!(result.window_ms, 5_000);
    let before = result
        .chain
        .iter()
        .filter(|n| n.relation == CausalRelation::Before)
        .count();
    let concurrent = result
        .chain
        .iter()
        .filter(|n| n.relation == CausalRelation::Concurrent)
        .count();
    assert_eq!(
        (before, concurrent),
        (2, 1),
        "链 = 同源前驱 2 + 跨源并发观察者 1:{:?}",
        result.chain
    );
    let deltas: Vec<i64> = result.chain.iter().map(|n| n.delta_ms).collect();
    assert!(
        deltas.windows(2).all(|w| w[0] <= w[1]),
        "delta 升序（时序直观）:{deltas:?}"
    );
}

/// V3 补链:BudgetExceeded（触发集第二变体）端到端同样自动归因
#[tokio::test]
async fn budget_exceeded_triggers_attribution_too() {
    let bus = EventBus::new();
    let tap = spawn_attribution_tap(&bus);

    bus.publish(quest("src-a", "q-1")).await.unwrap();
    let trigger = budget_exceeded("src-b");
    let trigger_fp = event_fingerprint(&trigger);
    bus.publish(trigger).await.unwrap();

    wait_until(
        || tap.recorded() >= 2 && tap.trigger_firings() >= 1,
        "tap 消费 2 条且 BudgetExceeded 触发归因",
    )
    .await;
    let result = tap.attribute(trigger_fp, now_ms());
    assert!(result.diff_found);
    assert_eq!(result.chain.len(), 1);
    assert_eq!(result.chain[0].relation, CausalRelation::Concurrent);
}

/// 韧性:Lagged 只计数不中断,tap 继续消费后续事件（broadcast 不传导背压）
#[tokio::test]
async fn tap_survives_lagged_and_keeps_consuming() {
    // 容量 2:当前线程运行时下测试任务连续 publish（ready 不 yield）→ 覆写触发 Lagged
    let bus = EventBus::with_capacity(2);
    let tap = spawn_attribution_tap(&bus);

    bus.publish(quest("src-a", "q-1")).await.unwrap();
    bus.publish(quest("src-a", "q-2")).await.unwrap();
    bus.publish(quest("src-a", "q-3")).await.unwrap();

    wait_until(
        || tap.lagged() >= 1 && tap.recorded() >= 1,
        "Lagged 计数且 tap 仍消费到最新事件",
    )
    .await;

    // 后续事件继续记账（不中断）
    bus.publish(quest("src-a", "q-4")).await.unwrap();
    wait_until(|| tap.recorded() >= 2, "Lagged 后继续消费").await;
}
