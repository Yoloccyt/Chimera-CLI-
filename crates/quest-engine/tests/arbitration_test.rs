//! ArbitrationLayer 集成测试 — ACB/DECB 保守取严仲裁策略验证
//!
//! 对应架构层:L9 Quest (TTG 仲裁层)
//! 对应创新点:N7 TTG ACB/DECB 仲裁层
//!
//! # 测试覆盖
//! - 保守取严策略:ACB L0→Degraded, L1→LowTier, L2/L3→跟随 DECB
//! - 事件订阅:FilteredSubscriber 仅接收 Parliament topic 事件
//! - 无事件时返回 None(让调用方使用 fallback)
//! - TtgGovernor 集成:effective_tier() 方法

use decb_governor::BudgetTier;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::{ArbitrationLayer, TtgConfig, TtgGovernor};

/// 构造 ACB BudgetAdjusted 事件
fn make_acb_budget_adjusted(new_tier: &str) -> NexusEvent {
    NexusEvent::BudgetAdjusted {
        metadata: EventMetadata::new("acb-governor"),
        quest_id: String::new(),
        old_tier: "L3_abundant".into(),
        new_tier: new_tier.into(),
        coefficient: 0.25,
        reason: "acb tier switch".into(),
    }
}

/// 构造 DECB BudgetAdjusted 事件
fn make_decb_budget_adjusted(new_tier: &str) -> NexusEvent {
    NexusEvent::BudgetAdjusted {
        metadata: EventMetadata::new("decb-governor"),
        quest_id: String::new(),
        old_tier: "high_tier".into(),
        new_tier: new_tier.into(),
        coefficient: 1.0,
        reason: "decb tier switch".into(),
    }
}

/// 构造非 Parliament topic 事件(用于验证过滤)
fn make_routing_event() -> NexusEvent {
    NexusEvent::ExpertRegistered {
        metadata: EventMetadata::new("faae-router"),
        tool_id: "tool-1".into(),
    }
}

// ============================================================
// 测试 1:ACB L0 → 仲裁结果为 Degraded(保守取严)
// ============================================================
#[tokio::test]
async fn test_arbitration_acb_l0_maps_to_degraded() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    // 发布 ACB L0 降级事件
    bus.publish(make_acb_budget_adjusted("L0_degraded"))
        .await
        .unwrap();

    // 排空事件并获取仲裁结果
    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::Degraded),
        "ACB L0 应映射为 DECB Degraded(保守取严)"
    );
}

// ============================================================
// 测试 2:ACB L1 → 仲裁结果为 LowTier
// ============================================================
#[tokio::test]
async fn test_arbitration_acb_l1_maps_to_low_tier() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    bus.publish(make_acb_budget_adjusted("L1_basic"))
        .await
        .unwrap();

    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::LowTier),
        "ACB L1 应映射为 DECB LowTier"
    );
}

// ============================================================
// 测试 3:ACB L2 → 跟随 DECB(使用 DECB 最新档位)
// ============================================================
#[tokio::test]
async fn test_arbitration_acb_l2_follows_decb() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    // 先发 ACB L2(标准级别)
    bus.publish(make_acb_budget_adjusted("L2_standard"))
        .await
        .unwrap();
    // 再发 DECB LowTier
    bus.publish(make_decb_budget_adjusted("low_tier"))
        .await
        .unwrap();

    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::LowTier),
        "ACB L2 应跟随 DECB 最新档位"
    );
}

// ============================================================
// 测试 4:ACB L3 → 跟随 DECB(使用 DECB 最新档位)
// ============================================================
#[tokio::test]
async fn test_arbitration_acb_l3_follows_decb() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    bus.publish(make_acb_budget_adjusted("L3_abundant"))
        .await
        .unwrap();
    bus.publish(make_decb_budget_adjusted("high_tier"))
        .await
        .unwrap();

    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::HighTier),
        "ACB L3 应跟随 DECB 最新档位"
    );
}

// ============================================================
// 测试 5:ACB L0 优先于 DECB HighTier(保守取严)
// ============================================================
#[tokio::test]
async fn test_arbitration_acb_l0_overrides_decb_high_tier() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    // DECB 认为资源充足
    bus.publish(make_decb_budget_adjusted("high_tier"))
        .await
        .unwrap();
    // ACB 发出 L0 降级信号
    bus.publish(make_acb_budget_adjusted("L0_degraded"))
        .await
        .unwrap();

    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::Degraded),
        "ACB L0 应优先于 DECB HighTier(保守取严)"
    );
}

// ============================================================
// 测试 6:无事件时返回 None
// ============================================================
#[tokio::test]
async fn test_arbitration_no_events_returns_none() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    let tier = layer.arbitrated_tier();
    assert_eq!(tier, None, "无事件时应返回 None");
}

// ============================================================
// 测试 7:disabled() 返回 None(向后兼容)
// ============================================================
#[test]
fn test_arbitration_disabled_returns_none() {
    let layer = ArbitrationLayer::disabled();
    assert_eq!(layer.arbitrated_tier(), None, "disabled 应返回 None");
    assert!(!layer.is_enabled(), "disabled 应报告未启用");
}

// ============================================================
// 测试 8:非 Parliament 事件被过滤(不干扰仲裁)
// ============================================================
#[tokio::test]
async fn test_arbitration_ignores_non_parliament_events() {
    let bus = EventBus::new();
    let layer = ArbitrationLayer::new(&bus);

    // 发布一个 Routing 事件(不应被仲裁层接收)
    bus.publish(make_routing_event()).await.unwrap();
    // 发布 ACB L0
    bus.publish(make_acb_budget_adjusted("L0_degraded"))
        .await
        .unwrap();

    let tier = layer.arbitrated_tier();
    assert_eq!(
        tier,
        Some(BudgetTier::Degraded),
        "Routing 事件不应干扰仲裁,ACB L0 应正确映射"
    );
}

// ============================================================
// 测试 9:TtgGovernor effective_tier 集成
// ============================================================
#[tokio::test]
async fn test_ttg_governor_effective_tier_with_arbitration() {
    let bus = EventBus::new();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus.clone());

    // 无事件时使用 fallback
    let effective = governor.effective_tier(BudgetTier::HighTier);
    assert_eq!(
        effective,
        BudgetTier::HighTier,
        "无仲裁事件时应使用 fallback tier"
    );

    // 发布 ACB L0
    bus.publish(make_acb_budget_adjusted("L0_degraded"))
        .await
        .unwrap();

    let effective = governor.effective_tier(BudgetTier::HighTier);
    assert_eq!(
        effective,
        BudgetTier::Degraded,
        "ACB L0 仲裁应覆盖 fallback HighTier"
    );
}

// ============================================================
// 测试 10:TtgGovernor 无 EventBus 时 effective_tier 返回 fallback
// ============================================================
#[test]
fn test_ttg_governor_effective_tier_without_event_bus() {
    let governor = TtgGovernor::new(TtgConfig::default());
    let effective = governor.effective_tier(BudgetTier::HighTier);
    assert_eq!(
        effective,
        BudgetTier::HighTier,
        "无 EventBus 时应直接返回 fallback"
    );
}

// ============================================================
// 测试 11:select_mode_with_arbitration 使用仲裁档位
// ============================================================
#[tokio::test]
async fn test_select_mode_with_arbitration_uses_arbitrated_tier() {
    let bus = EventBus::new();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus.clone());

    // 复杂 Quest(20 任务),fallback 为 HighTier → 正常应选 Deep
    let quest = Quest {
        quest_id: "q-arb-1".into(),
        title: "complex quest".into(),
        tasks: (0..20)
            .map(|i| Task {
                task_id: format!("task-{i}"),
                description: format!("do task {i}"),
                status: TaskStatus::Pending,
                dependencies: vec![],
            })
            .collect(),
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    };

    // 发布 ACB L0 → 仲裁为 Degraded → 应选 Fast(即使 fallback 为 HighTier)
    bus.publish(make_acb_budget_adjusted("L0_degraded"))
        .await
        .unwrap();

    let (mode, _) = governor.select_mode_with_arbitration(&quest, BudgetTier::HighTier);
    assert_eq!(
        mode,
        ThinkingMode::Fast,
        "ACB L0 仲裁为 Degraded,即使 fallback 为 HighTier 也应选 Fast"
    );
}
