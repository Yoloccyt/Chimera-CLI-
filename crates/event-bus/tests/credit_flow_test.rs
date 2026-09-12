//! CBF 信用流接入集成测试 — P1-T11(Phase 1 地基波次,手册 §8.5 / T-06)
//!
//! 覆盖(任务交付物 2/3 的集成侧;方案 A 语义:信用流仅作分片背压载体,
//! 评审 Issue 2 修复后无「扣而无归还」路径):
//! - 未启用分片:publish 不扣信用、不计 shed(非分片路径无扣减 —— broadcast
//!   本有 Lagged 保护,不需要信用;若扣减则无人归还,池将耗尽)
//! - 启用分片 + Unordered:publish 扣信用,worker 汇入自动归还(信用守恒闭环);
//!   信用耗尽回退 broadcast 兜底(事件仍可达),累计 shed(分片路径的 shed)
//! - OrderSensitive 直投单流不经信用流(无归还方的路径不扣,评审 Issue 2)
//! - Critical 路径不经信用流(publish / publish_critical_blocking 后
//!   credit_available 不变 —— 豁免红线:Critical 背压 = 死锁源,推演 9)
//! - release_credit 手动归还(含封顶防膨胀)
//! - 高优等待经 EventBus::credit_flow 接入(Notify 唤醒)
//! - 混沌:慢消费者(订阅者阻塞不消费)下 publish_critical_blocking 的
//!   13 变体全部送达(mpsc 有界 4096 内),broadcast 普通事件 Lagged 丢弃
//!   但出 SlowConsumerDropped 告警
//! - 公共常量断言:CRITICAL_MPSC_VARIANTS(13)/ CRITICAL_TOTAL(17)/
//!   LANE_FORBIDDEN_SHARD(17 名字,分片禁区声明)

use std::collections::HashSet;
use std::time::{Duration, Instant};

use event_bus::{
    EventBus, EventMetadata, EventSeverity, NexusEvent, Priority, CRITICAL_MPSC_VARIANTS,
    CRITICAL_TOTAL, LANE_FORBIDDEN_SHARD,
};

// ============================================================
// 辅助构造
// ============================================================

/// 构造普通事件(QuestCreated 最常见高频事件)
fn make_event(i: u64) -> NexusEvent {
    NexusEvent::QuestCreated {
        metadata: EventMetadata::new("test"),
        quest_id: format!("q-{i}"),
        title: "credit flow test".into(),
        task_count: 1,
    }
}

/// 构造 Critical 但非 mpsc 的历史事件(CheckpointSaved,只走 broadcast)
fn make_checkpoint_saved() -> NexusEvent {
    NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-ckpt".into(),
        checkpoint_id: "c-1".into(),
        memory_snapshot_hash: "sha256:deadbeef".into(),
    }
}

/// 全量 13 个 mpsc 旁路变体构造(与 bus.rs 双清单守护同源,D-8 口径)
fn all_mpsc_critical_variants() -> Vec<NexusEvent> {
    vec![
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("t"),
            quest_id: "q".into(),
            veto_reason: "r".into(),
            frozen_capabilities: vec![],
        },
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("t"),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 1,
            total_probes: 2,
            detection_rate: 0.5,
            remediation_suggestion: "s".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("t"),
            budget_type: "token".into(),
            current: 2,
            limit: 1,
        },
        NexusEvent::AgentTaskFailed {
            metadata: EventMetadata::new("t"),
            from: "agent-a".into(),
            to: "root".into(),
            task_id: "t-1".into(),
            error: "timeout".into(),
            retry_count: 0,
        },
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("t"),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.1,
            block_reason: Some("unsafe".into()),
            alternative_suggestion: None,
        },
        NexusEvent::AffinityQuotaExhausted {
            metadata: EventMetadata::new("t"),
            route_key: "zhipu/glm-5.2".into(),
            reason: "quota".into(),
        },
        NexusEvent::R2FreezeViolation {
            metadata: EventMetadata::new("t"),
            violation_type: "CiDetection".into(),
            evidence: "ev".into(),
        },
        NexusEvent::R2FreezeRollbackFailed {
            metadata: EventMetadata::new("t"),
            reason: "git revert conflict".into(),
        },
        NexusEvent::FormalViolation {
            metadata: EventMetadata::new("t"),
            contract_id: "bc-1".into(),
            target_type: "event_bus::EventBus".into(),
            violations: vec!["v1".into()],
            context: nexus_contracts::behavior_contract::ContractContext::Runtime,
        },
        NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("t"),
            quest_id: "q".into(),
            proposal_id: "p".into(),
            veto_reason: "r".into(),
            override_reason: "o".into(),
            override_by: "admin".into(),
        },
        NexusEvent::R1ShadowRollbackFailed {
            metadata: EventMetadata::new("t"),
            reason: "rollback conflict".into(),
            trigger_type: event_bus::types::RollbackTriggerType::Unknown,
            triggered_at: None,
            details: String::new(),
            diagnostic: event_bus::types::RollbackDiagnosticContext::default(),
        },
        NexusEvent::StopRulingIssued {
            metadata: EventMetadata::new("t"),
            quest_id: "q".into(),
            reason: "stagnation".into(),
            preserve_best: true,
        },
        NexusEvent::ErrorSignatureMatched {
            metadata: EventMetadata::new("t"),
            error_hash: "h".into(),
            matched_card_ids: vec![],
        },
    ]
}

// ============================================================
// 公共常量断言(分片禁区声明 + D-8 口径)
// ============================================================

#[test]
fn test_public_d8_constants_consistent() {
    // 常量口径:13(mpsc 旁路)+ 4(历史 broadcast-only)= 17(Critical 总数)
    assert_eq!(CRITICAL_MPSC_VARIANTS, 13);
    assert_eq!(CRITICAL_TOTAL, 17);
    assert_eq!(LANE_FORBIDDEN_SHARD.len(), 17, "分片禁区必须恰好 17 个名字");
    // LANE_FORBIDDEN_SHARD 与全量 13 变体名一一覆盖(13 ⊆ 17)
    let forbidden: HashSet<&str> = LANE_FORBIDDEN_SHARD.iter().copied().collect();
    assert_eq!(forbidden.len(), 17, "LANE_FORBIDDEN_SHARD 名字必须唯一");
    for ev in all_mpsc_critical_variants() {
        assert!(
            forbidden.contains(ev.type_name()),
            "mpsc 变体 {} 未声明在分片禁区(13 ⊆ 17 违反)",
            ev.type_name()
        );
        assert_eq!(ev.severity(), EventSeverity::Critical);
    }
}

// ============================================================
// publish 信用语义(方案 A:仅分片路径扣减)
// ============================================================

#[tokio::test]
async fn test_publish_no_sharding_does_not_deduct_credit() {
    // 方案 A(评审 Issue 2 修复):未启用分片时 publish 不扣信用 ——
    // 信用流仅作分片背压载体,broadcast 路径本有 Lagged 保护,无需信用;
    // 若扣减则无人归还(池耗尽,信用维度失效)。
    let bus = EventBus::new();
    assert!(!bus.sharding_enabled(), "灰度默认不启用分片");
    for i in 0..300 {
        // > 256 信用池:若扣减早已耗尽;300 < 1024 broadcast 容量(无 Lagged)
        bus.publish(make_event(i)).await.unwrap();
    }
    let stats = bus.credit_stats();
    assert_eq!(stats.available, 256, "未启用分片:credit_available 不变");
    assert_eq!(stats.shed_total, 0, "未启用分片:无 shed 计数");
    assert_eq!(stats.high_wait_total, 0);
}

#[tokio::test]
async fn test_publish_with_sharding_deducts_credit_and_restores() {
    // 方案 A 分片路径:Unordered 事件先扣信用(信用流生效),worker 汇入后
    // release_many 归还 —— 信用守恒闭环(扣减 = 归还,池不耗尽)
    let bus = EventBus::new();
    bus.enable_sharding(64).unwrap();
    // current_thread runtime:publish 内部同步无 await 点,worker 尚未被调度,
    // 扣减即时可见(确定性断言 256 - 5 = 251)
    for i in 0..5 {
        bus.publish(make_event(i)).await.unwrap();
    }
    assert_eq!(
        bus.credit_stats().available,
        256 - 5,
        "分片路径 Unordered 事件必须先扣信用"
    );
    // 轮询等待 worker 汇入归还:水位恢复 256(信用守恒闭环)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while bus.credit_stats().available != 256 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker 汇入后必须归还信用:available = {}",
            bus.credit_stats().available
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(bus.credit_stats().shed_total, 0, "容量内无 shed");
}

#[tokio::test]
async fn test_publish_with_sharding_credit_exhausted_fallback() {
    // 分片路径信用耗尽:片满 + 无信用 → shed 计数(分片路径的 shed)并回退
    // broadcast 直接投递 —— 事件不丢弃,漏发率恒 0(原 T11 兜底语义,仅限分片路径)
    let bus = EventBus::new();
    bus.enable_sharding(64).unwrap();
    let mut rx = bus.subscribe();
    let total = 300u64; // > 256 信用池,< 1024 broadcast 容量(无 Lagged)
    for i in 0..total {
        bus.publish(make_event(i)).await.unwrap();
    }
    // current_thread:worker 未调度,确定性账目 —— 256 入片 + 44 回退 shed
    let stats = bus.credit_stats();
    assert_eq!(stats.available, 0, "256 信用全部耗尽");
    assert_eq!(
        stats.shed_total,
        total - 256,
        "超出信用池的部分计入 shed(分片路径)"
    );
    // 事件仍全部可达(44 直接 broadcast + 256 由 worker 汇入)
    let mut received = 0u64;
    while received < total {
        let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("广播兜底不应超时")
            .expect("通道不应关闭");
        assert_eq!(ev.severity(), EventSeverity::Normal);
        received += 1;
    }
    assert_eq!(
        received, total,
        "信用耗尽时事件仍应全部可达(broadcast 兜底)"
    );
}

#[tokio::test]
async fn test_credit_stats_observation() {
    let bus = EventBus::new();
    // 初始观测
    let initial = bus.credit_stats();
    assert_eq!(initial.available, 256);
    assert_eq!(initial.shed_total, 0);
    assert_eq!(initial.high_wait_total, 0);
    // 未启用分片:发布 10 个后观测(方案 A:不扣信用)
    for i in 0..10 {
        bus.publish(make_event(i)).await.unwrap();
    }
    let after = bus.credit_stats();
    assert_eq!(after.available, 256, "未启用分片:信用水位不变");
    assert_eq!(after.shed_total, 0);
}

#[tokio::test]
async fn test_release_credit_restores_and_caps() {
    let bus = EventBus::new();
    // 方案 A:未启用分片 publish 不扣信用,水位恒 256
    for i in 0..10 {
        bus.publish(make_event(i)).await.unwrap();
    }
    assert_eq!(bus.credit_stats().available, 256);
    // 扣减后归还恢复水位(直接经信用流原语,验证 release_credit 通路)
    assert!(bus.credit_flow().try_acquire(10).is_ok());
    assert_eq!(bus.credit_stats().available, 246);
    bus.release_credit(10);
    assert_eq!(bus.credit_stats().available, 256, "归还后信用恢复");
    // 过度归还不膨胀(封顶到初始池)
    bus.release_credit(10_000);
    assert_eq!(bus.credit_stats().available, 256, "归还封顶,信用不可膨胀");
}

// ============================================================
// Critical 豁免:任何 Critical 路径都不经信用流
// ============================================================

#[tokio::test]
async fn test_publish_critical_exempt_from_credit() {
    let bus = EventBus::new();
    let _critical_rx = bus.subscribe_critical_events(); // mpsc 订阅者
    let event = all_mpsc_critical_variants()[0].clone(); // SkepticVeto
                                                         // publish_critical(异步显式 API)不扣信用
    bus.publish_critical(event.clone()).await.unwrap();
    // publish(自动判定 is_critical_mpsc_event)也不扣信用
    bus.publish(event).await.unwrap();
    let stats = bus.credit_stats();
    assert_eq!(
        stats.available, 256,
        "Critical 事件经 publish_critical/publish 均不经过信用流(豁免红线)"
    );
    assert_eq!(stats.shed_total, 0);
}

#[tokio::test]
async fn test_publish_critical_blocking_exempt_from_credit() {
    let bus = EventBus::new();
    let _critical_rx = bus.subscribe_critical_events();
    // 13 个 mpsc 变体各发布一次(同步 API),信用纹丝不动
    for ev in all_mpsc_critical_variants() {
        bus.publish_critical_blocking(ev).unwrap();
    }
    let stats = bus.credit_stats();
    assert_eq!(stats.available, 256, "publish_critical_blocking 不经信用流");
    assert_eq!(stats.shed_total, 0);
    assert_eq!(stats.high_wait_total, 0);
}

#[tokio::test]
async fn test_publish_severity_critical_exempt_from_credit() {
    // 历史 Critical(CheckpointSaved,只走 broadcast)经 publish 也不扣信用
    let bus = EventBus::new();
    let _rx = bus.subscribe();
    for _ in 0..5 {
        bus.publish(make_checkpoint_saved()).await.unwrap();
    }
    let stats = bus.credit_stats();
    assert_eq!(
        stats.available, 256,
        "severity()=Critical 的事件(含 broadcast-only 历史事件)豁免信用流"
    );
    assert_eq!(stats.shed_total, 0);
}

// ============================================================
// 分片启用下的 OrderSensitive 豁免(方案 A:无归还方的路径不扣)
// ============================================================

#[tokio::test]
async fn test_publish_with_sharding_order_sensitive_no_credit() {
    // 方案 A(评审 Issue 2):分片启用时 OrderSensitive 直投单流也不扣信用 ——
    // 它不走分片、无 worker 归还,扣减即泄漏(与 Critical 同属「无归还方」)
    let bus = EventBus::new();
    bus.enable_sharding(64).unwrap();
    for i in 0..5 {
        // correlation_id 为广义会话键 → OrderSensitive 车道(单流保序)
        bus.publish(NexusEvent::QuestCreated {
            metadata: EventMetadata::with_correlation("test", format!("corr-{i}")),
            quest_id: format!("q-{i}"),
            title: "order sensitive".into(),
            task_count: 1,
        })
        .await
        .unwrap();
    }
    let stats = bus.credit_stats();
    assert_eq!(stats.available, 256, "OrderSensitive 直投单流不扣信用");
    assert_eq!(stats.shed_total, 0);
}

// ============================================================
// 高优等待经 EventBus::credit_flow 接入
// ============================================================

#[tokio::test]
async fn test_eventbus_high_priority_wait_via_credit_flow() {
    let bus = EventBus::new();
    // 方案 A:publish 不再扣信用,改为直接经信用流原语耗尽 —— 本测试目标是
    // 「归还唤醒高优等待」,与发布路径无关(信用流原语语义不变)
    assert!(bus.credit_flow().try_acquire(256).is_ok(), "耗尽信用池");
    assert_eq!(bus.credit_stats().available, 0);
    // 后台归还唤醒高优等待(30ms 后批归还 10)
    let bus2 = bus.clone();
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        bus2.release_credit(10);
    });
    let start = Instant::now();
    let result = bus.credit_flow().acquire_priority(Priority::High, 1).await;
    assert!(result.is_ok(), "高优等待应被归还唤醒: {result:?}");
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "应由归还唤醒而非超时,实际 {:?}",
        start.elapsed()
    );
    releaser.await.expect("归还任务 panic");
    let stats = bus.credit_stats();
    assert_eq!(stats.high_wait_total, 1, "高优等待统计应累计");
    assert_eq!(stats.available, 9, "10 归还 - 1 被高优等待者扣走 = 9");
}

// ============================================================
// 混沌:慢消费者 Critical 零丢失 + broadcast 普通事件 Lagged 告警
// ============================================================

#[tokio::test]
async fn test_chaos_slow_consumer_critical_zero_loss() {
    // 场景:广播订阅者阻塞不消费(慢消费者),Critical 事件必须零丢失
    let bus = EventBus::new(); // capacity 1024,credit 256
    let mut critical_rx = bus.subscribe_critical_events(); // mpsc 旁路(4096)
    let mut slow_rx = bus.subscribe(); // 广播订阅者 —— 故意不消费(阻塞)

    // 1) 13 个 mpsc Critical 变体各发布一次(同步 API)
    let variants = all_mpsc_critical_variants();
    assert_eq!(variants.len(), 13);
    for ev in &variants {
        bus.publish_critical_blocking(ev.clone()).unwrap();
    }

    // 2) 洪泛 2000 个普通事件(broadcast 容量 1024 → 慢消费者必然 Lagged)
    for i in 0..2000u64 {
        bus.publish_blocking(make_event(i)).unwrap();
    }

    // 3) Critical 零丢失:mpsc 旁路全部收到 13 个(有界 4096 内,不受慢消费者影响)
    let mut received = Vec::with_capacity(13);
    for _ in 0..13 {
        let ev = tokio::time::timeout(Duration::from_secs(2), critical_rx.recv())
            .await
            .expect("Critical mpsc 投递不应超时(豁免背压)")
            .expect("mpsc 不应关闭");
        assert_eq!(
            ev.severity(),
            EventSeverity::Critical,
            "mpsc 旁路收到的必须是 Critical 事件"
        );
        received.push(ev.type_name());
    }
    assert_eq!(received.len(), 13, "慢消费者场景下 Critical 事件必须零丢失");
    // 13 个名字全部到位(去重后仍 13 —— 无重复无缺失)
    let unique: HashSet<&str> = received.iter().copied().collect();
    assert_eq!(unique.len(), 13, "13 个 Critical 变体应各出现一次");
    // 与分片禁区交集:收到的正是 13 个 mpsc 变体
    for name in &unique {
        assert!(
            LANE_FORBIDDEN_SHARD.contains(name),
            "收到的 {name} 应在分片禁区清单中"
        );
    }

    // 4) 慢消费者收到 SlowConsumerDropped 告警(broadcast Lagged 语义)
    let slow_result = tokio::time::timeout(Duration::from_secs(2), slow_rx.recv())
        .await
        .expect("慢消费者 recv 不应超时");
    assert!(
        matches!(
            slow_result,
            Err(event_bus::EventBusError::SlowConsumerDropped { .. })
        ),
        "慢消费者应触发 SlowConsumerDropped 告警,实际: {slow_result:?}"
    );

    // 5) 背压与信用观测:lagged_count 递增;未启用分片(方案 A)普通事件
    //    不扣信用不 shed —— 信用流仅作分片背压载体,慢消费者由 broadcast
    //    Lagged 保护兜底
    let (lagged, _warnings) = bus.backpressure_stats();
    assert!(
        lagged > 0,
        "lagged_count 应递增(慢消费者丢弃),实际 {lagged}"
    );
    let stats = bus.credit_stats();
    assert_eq!(stats.available, 256, "未启用分片:2000 普通事件不扣信用");
    assert_eq!(stats.shed_total, 0, "未启用分片:无 shed 计数");
    assert_eq!(stats.high_wait_total, 0);
}
