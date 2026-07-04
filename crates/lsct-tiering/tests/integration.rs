//! LSCT-tiering 集成测试 — EventBus 集成与 QuestCreated 订阅链路验证
//!
//! 对应架构层:L3 Storage
//!
//! # 测试覆盖
//! - with_event_bus 构造器注入共享 EventBus
//! - apply_decision 发布 LsctTierSwitched 事件
//! - handle_quest_created 完整链路:标题 → 画像 → tick → apply → 事件
//! - Keep 决策不发布事件
//! - 多能力批量处理与事件投递
//!
//! # 关键时序(WHY)
//! broadcast 不缓存历史消息,subscribe() 必须在 publish() 之前调用。
//! 测试中先创建 receiver,再触发操作,确保事件不丢失。

use std::time::Duration;

use cmt_tiering::Tier;
use event_bus::EventBus;
use lsct_tiering::{
    compute_target_tier, LsctConfig, LsctCoordinator, TaskLoadProfile, TaskType, TierSwitchDecision,
};

/// 辅助:创建带 EventBus 的 coordinator 与 receiver
fn setup_with_bus() -> (LsctCoordinator, event_bus::EventReceiver) {
    let bus = EventBus::new();
    // WHY 先 subscribe:broadcast 不缓存历史,subscribe 必须在 publish 前
    let rx = bus.subscribe();
    let coordinator = LsctCoordinator::with_event_bus(LsctConfig::default(), bus);
    (coordinator, rx)
}

#[tokio::test]
async fn test_apply_decision_promote_publishes_event() {
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Warm);

    let decision = TierSwitchDecision::Promote {
        capability_id: "cap-1".into(),
        from: Tier::Warm,
        to: Tier::Hot,
        reason: "compile high intensity".into(),
    };

    coordinator.apply_decision(&decision).await.unwrap();

    // 验证收到 LsctTierSwitched 事件
    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应收到 LsctTierSwitched 事件");

    match event {
        event_bus::NexusEvent::LsctTierSwitched {
            capability_id,
            from_tier,
            to_tier,
            ..
        } => {
            assert_eq!(capability_id, "cap-1");
            assert_eq!(from_tier, "Warm");
            assert_eq!(to_tier, "Hot");
        }
        other => panic!("期望 LsctTierSwitched,得到 {:?}", other),
    }
}

#[tokio::test]
async fn test_apply_decision_demote_publishes_event() {
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Hot);

    let decision = TierSwitchDecision::Demote {
        capability_id: "cap-1".into(),
        from: Tier::Hot,
        to: Tier::Warm,
        reason: "debug low intensity".into(),
    };

    coordinator.apply_decision(&decision).await.unwrap();

    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应收到 LsctTierSwitched 事件");

    match event {
        event_bus::NexusEvent::LsctTierSwitched {
            from_tier, to_tier, ..
        } => {
            assert_eq!(from_tier, "Hot");
            assert_eq!(to_tier, "Warm");
        }
        other => panic!("期望 LsctTierSwitched,得到 {:?}", other),
    }
}

#[tokio::test]
async fn test_keep_decision_does_not_publish() {
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Hot);

    let decision = TierSwitchDecision::Keep {
        capability_id: "cap-1".into(),
        tier: Tier::Hot,
        reason: "at target".into(),
    };

    coordinator.apply_decision(&decision).await.unwrap();

    // 验证无事件(1 秒超时应返回 Err)
    let result = rx.recv_timeout(Duration::from_secs(1)).await;
    assert!(
        result.is_err(),
        "Keep 决策不应发布事件,但收到了 {:?}",
        result
    );
}

#[tokio::test]
async fn test_handle_quest_created_publishes_events() {
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Cold);
    coordinator.register_capability("cap-2", Tier::Warm);

    // "compile release" → Compile(0.9) → target Hot
    // cap-1(Cold) → Promote Cold→Warm
    // cap-2(Warm) → Promote Warm→Hot
    let decisions = coordinator
        .handle_quest_created("compile production release")
        .await
        .unwrap();

    assert_eq!(decisions.len(), 2);

    // 应收到 2 个 LsctTierSwitched 事件(cap-1 和 cap-2 各一个)
    let event1 = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应收到第一个 LsctTierSwitched 事件");
    assert!(matches!(
        event1,
        event_bus::NexusEvent::LsctTierSwitched { .. }
    ));

    let event2 = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应收到第二个 LsctTierSwitched 事件");
    assert!(matches!(
        event2,
        event_bus::NexusEvent::LsctTierSwitched { .. }
    ));
}

#[tokio::test]
async fn test_handle_quest_created_debug_demotes() {
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Hot);

    // "debug memory leak" → Debug(0.2) → target Ice
    // cap-1(Hot) → Demote Hot→Warm(逐级)
    let decisions = coordinator
        .handle_quest_created("debug memory leak")
        .await
        .unwrap();

    assert!(!decisions.is_empty());

    // 验证收到降温事件
    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("应收到 LsctTierSwitched 事件");

    if let event_bus::NexusEvent::LsctTierSwitched {
        from_tier, to_tier, ..
    } = event
    {
        assert_eq!(from_tier, "Hot");
        assert_eq!(to_tier, "Warm");
    } else {
        panic!("期望 LsctTierSwitched 事件");
    }
}

#[tokio::test]
async fn test_no_bus_does_not_crash() {
    // 无 EventBus 时,apply_decision 应正常执行,不崩溃
    let coordinator = LsctCoordinator::new(LsctConfig::default());
    coordinator.register_capability("cap-1", Tier::Warm);

    let decision = TierSwitchDecision::Promote {
        capability_id: "cap-1".into(),
        from: Tier::Warm,
        to: Tier::Hot,
        reason: "test".into(),
    };

    // 不应 panic
    coordinator.apply_decision(&decision).await.unwrap();
    assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Hot));
}

#[tokio::test]
async fn test_multi_tick_progressive_promotion_with_events() {
    // 多 tick 逐步升温:Ice → Cold → Warm → Hot,每步发布事件
    let (coordinator, mut rx) = setup_with_bus();
    coordinator.register_capability("cap-1", Tier::Ice);

    let profile = TaskLoadProfile::new(TaskType::Run, 0.9, 1);

    // Tick 1: Ice → Cold
    let decisions = coordinator.tick(&profile);
    coordinator.apply_decision(&decisions[0]).await.unwrap();
    assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Cold));

    let e1 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let event_bus::NexusEvent::LsctTierSwitched {
        from_tier, to_tier, ..
    } = e1
    {
        assert_eq!(from_tier, "Ice");
        assert_eq!(to_tier, "Cold");
    } else {
        panic!("期望 LsctTierSwitched");
    }

    // Tick 2: Cold → Warm
    let decisions = coordinator.tick(&profile);
    coordinator.apply_decision(&decisions[0]).await.unwrap();
    assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Warm));

    let e2 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let event_bus::NexusEvent::LsctTierSwitched {
        from_tier, to_tier, ..
    } = e2
    {
        assert_eq!(from_tier, "Cold");
        assert_eq!(to_tier, "Warm");
    } else {
        panic!("期望 LsctTierSwitched");
    }

    // Tick 3: Warm → Hot
    let decisions = coordinator.tick(&profile);
    coordinator.apply_decision(&decisions[0]).await.unwrap();
    assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Hot));

    let e3 = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let event_bus::NexusEvent::LsctTierSwitched {
        from_tier, to_tier, ..
    } = e3
    {
        assert_eq!(from_tier, "Warm");
        assert_eq!(to_tier, "Hot");
    } else {
        panic!("期望 LsctTierSwitched");
    }
}

#[tokio::test]
async fn test_compute_target_tier_integration() {
    // 验证 compute_target_tier 与 coordinator 的集成
    let profile = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);
    let target = compute_target_tier(&profile);
    assert_eq!(target, Tier::Hot);

    let profile_low = TaskLoadProfile::new(TaskType::Debug, 0.1, 1);
    let target_low = compute_target_tier(&profile_low);
    assert_eq!(target_low, Tier::Ice);
}
