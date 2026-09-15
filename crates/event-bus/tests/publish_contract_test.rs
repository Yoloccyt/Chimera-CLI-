//! EventBus 发布路径契约守护测试 — H-另案(M0,2026-09-12)
//!
//! # 契约(被本文件固化为可执行断言)
//!
//! `EventBus` 的发布 API(`publish` / `publish_blocking` / `publish_batch` /
//! `publish_batch_blocking` / `publish_critical` / `publish_critical_blocking`)
//! **恒返回 `Ok(())`**——发布路径不存在失败模式:
//! - 无订阅者 → 事件丢弃 + 观测计数/告警(bus 内部处理),不视为错误
//!   (`bus.rs` `publish` 文档原文"若无订阅者,事件被丢弃但不视为错误");
//!   Critical 事件自 B-a 起另有保底 sink 落点,仍不影响 Ok 语义;
//! - 订阅者缓冲区满(lag)→ 发送端照常 Ok,慢消费侧在 `recv()` 以
//!   `SlowConsumerDropped` 暴露(错误归接收端,不归发布端);
//! - 分片启用 → 片满无信用时回退 broadcast(事件不丢),发布端仍 Ok。
//!
//! # WHY 需要守护
//!
//! `Result` 返回类型是 API 稳定性预留(未来跨进程投递/异步序列化),当前恒 Ok。
//! 若未来有人为发布路径新增真实失败模式,**37 个调用方的错误处理语义将同时
//! 被激活**——其中 model-router 曾出现"publish 失败反转路由决策"(H-另案 已修),
//! 其他调用方未必审过同类问题。本测试把"恒 Ok"从文档变成 CI 断言:任何新增
//! 失败模式必须先过本门(修改本测试 = 显式的全调用方语义审查),不得静默上线。
//!
//! 参考:`docs/reports/arch-refactor-directions-v2-2026-09-12.md` 方向 H-另案。

use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use proptest::prelude::*;

/// 构造 Critical 清单内事件(BudgetExceeded)
fn critical_event(source: &str) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new(source),
        budget_type: "total_cost".into(),
        current: 9_999,
        limit: 5_000,
    }
}

/// 构造非 Critical 事件(QuestCreated)
fn normal_event() -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-1".into(),
        title: "示例".into(),
        task_count: 1,
    }
}

// ============================================================
// 契约 1:无订阅者 — 全部发布 API 恒 Ok(事件丢弃 + bus 内部观测,不失败)
// ============================================================

#[tokio::test]
async fn contract_publish_is_ok_without_subscribers() {
    let bus = EventBus::new();
    bus.publish(critical_event("decb-governor"))
        .await
        .expect("契约违反:无订阅者 publish 不得返回 Err");
    bus.publish(normal_event())
        .await
        .expect("契约违反:无订阅者 publish 不得返回 Err");
    bus.publish_blocking(critical_event("decb-governor"))
        .expect("契约违反:publish_blocking 不得返回 Err");
    bus.publish_batch(vec![
        critical_event("a"),
        normal_event(),
        critical_event("b"),
    ])
    .await
    .expect("契约违反:publish_batch 不得返回 Err");
    bus.publish_batch_blocking(vec![])
        .expect("契约违反:publish_batch_blocking(空批)不得返回 Err");
    bus.publish_critical(critical_event("decb-governor"))
        .await
        .expect("契约违反:publish_critical 不得返回 Err");
    bus.publish_critical_blocking(critical_event("decb-governor"))
        .expect("契约违反:publish_critical_blocking 不得返回 Err");
}

// ============================================================
// 契约 2:订阅者缓冲区满(lag) — 发布端恒 Ok,错误归接收端 recv()
// ============================================================

#[tokio::test]
async fn contract_publish_is_ok_under_lag_pressure() {
    let bus = EventBus::new();
    let rx = bus.subscribe(); // 唯一订阅者,故意不消费制造 lag

    // 超出 broadcast 容量(DEFAULT_CAPACITY=1024)2 倍,制造必然 lag
    let total = event_bus::DEFAULT_CAPACITY * 2;
    for i in 0..total {
        let ev = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("lag-maker"),
            budget_type: format!("lag-{i}"),
            current: 1,
            limit: 0,
        };
        // Critical 清单事件 → 同时走 mpsc 旁路;此处混入 Normal 避免旁路干扰
        let ev = if i % 3 == 0 {
            NexusEvent::QuestCreated {
                metadata: EventMetadata::new("lag-maker"),
                quest_id: format!("q-{i}"),
                title: "x".into(),
                task_count: 1,
            }
        } else {
            ev
        };
        bus.publish(ev)
            .await
            .expect("契约违反:lag 压力下 publish 不得返回 Err");
    }
    drop(rx); // 接收端错误(SlowConsumerDropped)归 recv() 侧,与发布端无关
}

// ============================================================
// 契约 3:分片启用 — 发布端恒 Ok(片满无信用回退 broadcast,事件不丢)
// ============================================================

#[tokio::test]
async fn contract_publish_is_ok_with_sharding_enabled() {
    let bus = EventBus::new();
    bus.enable_sharding(64).expect("分片应启用成功");
    let mut rx = bus.subscribe();
    for _ in 0..64 {
        bus.publish(normal_event())
            .await
            .expect("契约违反:分片启用下 publish 不得返回 Err");
    }
    // 事件经 worker 汇入 broadcast,最终全部到达(漏发率恒 0 的旁证)
    // EventReceiver::recv 返回 Result<NexusEvent, EventBusError>:
    // Ok(事件)=收到;Err=通道关闭/慢消费错误(归接收端,与发布端无关);超时=排干完毕
    let mut received = 0;
    while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
        received += 1;
    }
    assert_eq!(received, 64, "分片路径事件必须全部送达");
}

// ============================================================
// proptest:任意 n 条混合事件(Critical/Normal 交错)发布恒 Ok,
// 且 published_total 与发布数守恒(publish 的副作用面收敛于计数)
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn prop_publish_always_ok_and_counted(
        n in 0usize..24,
        critical_flags in proptest::collection::vec(any::<bool>(), 24),
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let bus = EventBus::new();
            for i in 0..n {
                let ev = if *critical_flags.get(i).unwrap_or(&false) {
                    critical_event("proptest")
                } else {
                    normal_event()
                };
                // 契约:任何事件、任何状态,publish 恒 Ok
                bus.publish(ev).await.expect("契约违反:publish 返回 Err");
            }
            prop_assert_eq!(bus.published_total() as usize, n, "发布计数守恒");
            Ok(())
        })?;
    }
}
