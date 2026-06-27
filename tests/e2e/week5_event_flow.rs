//! Week 5 事件流 E2E 测试 — W7-2 + W7-4
//!
//! 对应任务:
//! - W7-2:E2E 事件流测试(验证 Week 5 关键事件的发布/订阅链路)
//! - W7-4:DegradedModeRejected 错误路径覆盖
//!
//! 架构层:L1 Core(EventBus)+ L8 Parliament(DECB / RoleRegistry)
//!
//! # 测试用例
//! 1. `test_degraded_mode_rejected_e2e` — DECB Degraded 模式拒绝新 Quest(错误路径,W7-4 核心)
//! 2. `test_budget_adjusted_event_flow` — DECB 档位切换发布 BudgetAdjusted 事件
//! 3. `test_budget_exceeded_event_flow` — DECB 溢出降级发布 BudgetExceeded 事件
//! 4. `test_role_registered_event_flow` — RoleRegistry 注册发布 RoleRegistered 事件
//!    (依赖 W7-1 的 `RoleRegistry::with_event_bus` 构造器)
//!
//! # 设计要点
//! - **共享 EventBus**:DECB governor 通过 `with_event_bus(config, bus)` 注入共享总线,
//!   事件发布用 `publish_blocking`(同步),测试用 `recv_timeout`(异步)接收
//! - **50ms 单事件超时**:与 week6_setup.rs 保持一致,平衡测试速度与可靠性
//! - **DegradedModeRejected 触发路径**:先消耗 110% 预算触发降级到 Degraded,
//!   再继续消耗 → 检测到溢出 + 当前 tier==Degraded → 返回错误

use std::time::Duration;

use decb_governor::{BudgetConsumption, BudgetTier, DecbConfig, DecbError, DecbGovernor};
use event_bus::{EventBus, EventReceiver, NexusEvent};

// ============================================================
// 辅助函数 — 排空事件接收器(与 week6_setup.rs 保持一致)
// ============================================================

/// 排空事件接收器,收集最多 `max_count` 个事件(每个事件 50ms 超时)
///
/// WHY 50ms 单事件超时:与 week6_setup.rs 的 `drain_events` 保持一致。
/// DECB 的事件发布用 `publish_blocking`(同步 broadcast::send),
/// 事件在 `record_consumption`/`switch_tier` 返回前已投递到所有接收者,
/// 50ms 足以让 tokio runtime 调度 `recv_timeout` 完成读取。
async fn drain_events(rx: &mut EventReceiver, max_count: usize) -> Vec<NexusEvent> {
    let mut events = Vec::with_capacity(max_count);
    for _ in 0..max_count {
        match rx.recv_timeout(Duration::from_millis(50)).await {
            Ok(event) => events.push(event),
            Err(_) => break,
        }
    }
    events
}

/// 断言事件列表中包含至少一个指定类型的事件(按 `type_name()` 匹配)
fn assert_has_event(events: &[NexusEvent], type_name: &str) {
    let count = events.iter().filter(|e| e.type_name() == type_name).count();
    assert!(
        count > 0,
        "未找到期望的事件类型: {type_name}(实际收集到 {} 个事件:{:?})",
        events.len(),
        events.iter().map(|e| e.type_name()).collect::<Vec<_>>()
    );
}

// ============================================================
// 测试 1:DegradedModeRejected 错误路径 E2E(W7-4 核心)
// ============================================================

/// 验证 DECB Degraded 模式下仍超预算时返回 `DegradedModeRejected` 错误
///
/// # 流程
/// 1. 构造 `DecbGovernor` with shared EventBus(`tier_switch_lag_ms=0` 允许立即降级)
/// 2. 订阅 EventBus 接收事件
/// 3. 第一次消耗 110% 预算 → 触发降级到 Degraded + 发布 `BudgetExceeded` 事件
/// 4. 第二次消耗 → 当前 tier==Degraded 且仍超预算 → 返回 `DegradedModeRejected`
/// 5. 验证错误类型 + `BudgetExceeded` 事件已发布
///
/// WHY 此测试为 W7-4 核心:覆盖 DECB 最低档位无法继续降级时的拒绝路径,
/// 确保预算耗尽时系统能正确拒绝新 Quest 而非 panic 或静默继续(§6 架构红线)。
#[tokio::test]
async fn test_degraded_mode_rejected_e2e() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    // 配置:无滞后(tier_switch_lag_ms=0),允许立即降级
    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus).expect("DECB 构造应成功");
    assert_eq!(
        governor.current_tier(),
        BudgetTier::HighTier,
        "初始档位应为 HighTier"
    );

    // 步骤 1:消耗 110% 预算,触发降级到 Degraded
    // WHY 110%:超过 100% 阈值,OverflowDetector 建议降级到 Degraded
    let exhaust = BudgetConsumption {
        total_cost: governor.config().total_budget_limit * 1.1,
        ..BudgetConsumption::zero()
    };
    governor
        .record_consumption(&exhaust)
        .expect("首次消耗不应报错(触发降级,非拒绝)");
    assert_eq!(
        governor.current_tier(),
        BudgetTier::Degraded,
        "110% 消耗应降级到 Degraded"
    );

    // 步骤 2:Degraded 模式下继续消耗,应返回 DegradedModeRejected
    let more = BudgetConsumption {
        total_cost: 100.0,
        ..BudgetConsumption::zero()
    };
    let result = governor.record_consumption(&more);
    assert!(
        matches!(result, Err(DecbError::DegradedModeRejected { .. })),
        "Degraded 模式下继续消耗应返回 DegradedModeRejected,实际: {:?}",
        result
    );

    // 步骤 3:验证 BudgetExceeded 事件已发布(Critical 级事件)
    // WHY 两次 record_consumption 都会发布 BudgetExceeded(每次检测到溢出都发布)
    let events = drain_events(&mut rx, 10).await;
    assert_has_event(&events, "BudgetExceeded");
}

// ============================================================
// 测试 2:BudgetAdjusted 事件流(档位切换)
// ============================================================

/// 验证 DECB `switch_tier` 档位切换时发布 `BudgetAdjusted` 事件
///
/// # 流程
/// 1. 构造 `DecbGovernor` with shared EventBus
/// 2. 订阅 EventBus
/// 3. 手动调用 `switch_tier(LowTier)` 切换档位
/// 4. 验证 `BudgetAdjusted` 事件已发布,且字段包含 old_tier/new_tier
///
/// WHY 此测试验证 Ω-Event 定律:所有状态变更必须经 EventBus 广播,
/// 档位切换是 DECB 的核心状态变更,必须发布事件供 Parliament/Quest 订阅。
#[tokio::test]
async fn test_budget_adjusted_event_flow() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus).expect("DECB 构造应成功");

    // 手动切换档位:HighTier → LowTier
    governor
        .switch_tier(BudgetTier::LowTier)
        .expect("档位切换应成功");
    assert_eq!(
        governor.current_tier(),
        BudgetTier::LowTier,
        "切换后档位应为 LowTier"
    );

    // 验证 BudgetAdjusted 事件已发布
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "BudgetAdjusted");

    // 进一步验证事件字段:old_tier=high_tier, new_tier=low_tier
    let adjusted = events
        .iter()
        .find(|e| e.type_name() == "BudgetAdjusted")
        .expect("应找到 BudgetAdjusted 事件");
    if let NexusEvent::BudgetAdjusted {
        old_tier, new_tier, ..
    } = adjusted
    {
        assert_eq!(
            old_tier, "high_tier",
            "old_tier 应为 high_tier,实际: {old_tier}"
        );
        assert_eq!(
            new_tier, "low_tier",
            "new_tier 应为 low_tier,实际: {new_tier}"
        );
    } else {
        panic!("事件类型匹配失败,期望 BudgetAdjusted 变体");
    }
}

// ============================================================
// 测试 3:BudgetExceeded 事件流(溢出降级)
// ============================================================

/// 验证 DECB `record_consumption` 检测到溢出时发布 `BudgetExceeded` 事件
///
/// # 流程
/// 1. 构造 `DecbGovernor` with shared EventBus
/// 2. 订阅 EventBus
/// 3. 消耗 85% 预算 → 触发降级到 LowTier + 发布 `BudgetExceeded` 事件
/// 4. 验证 `BudgetExceeded` 事件已发布,且字段包含 current/limit
///
/// WHY 85% 触发 LowTier:OverflowDetector 在 80% 阈值时建议 LowTier,
/// 85% 确保超过阈值,触发降级链路。
#[tokio::test]
async fn test_budget_exceeded_event_flow() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus).expect("DECB 构造应成功");

    // 消耗 85% 预算,触发降级到 LowTier
    let consumption = BudgetConsumption {
        total_cost: governor.config().total_budget_limit * 0.85,
        ..BudgetConsumption::zero()
    };
    governor
        .record_consumption(&consumption)
        .expect("消耗记录应成功");

    assert_eq!(
        governor.current_tier(),
        BudgetTier::LowTier,
        "85% 消耗应降级到 LowTier"
    );

    // 验证 BudgetExceeded 事件已发布
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "BudgetExceeded");

    // 进一步验证事件字段:budget_type=total_cost
    let exceeded = events
        .iter()
        .find(|e| e.type_name() == "BudgetExceeded")
        .expect("应找到 BudgetExceeded 事件");
    if let NexusEvent::BudgetExceeded {
        budget_type,
        current,
        limit,
        ..
    } = exceeded
    {
        assert_eq!(
            budget_type, "total_cost",
            "budget_type 应为 total_cost,实际: {budget_type}"
        );
        assert!(*current > 0, "current 应大于 0(消耗值),实际: {current}");
        assert!(*limit > 0, "limit 应大于 0(预算上限),实际: {limit}");
    } else {
        panic!("事件类型匹配失败,期望 BudgetExceeded 变体");
    }
}

// ============================================================
// 测试 4:RoleRegistered 事件流(依赖 W7-1 的 with_event_bus 构造器)
// ============================================================

/// 验证 `RoleRegistry::with_event_bus` 注册新角色时发布 `RoleRegistered` 事件
///
/// # 依赖
/// W7-1 已完成:`RoleRegistry::with_event_bus` 构造器已实现,
/// `register()` 成功后会通过 `publish_blocking` 发布 `RoleRegistered` 事件。
///
/// # 流程
/// 1. 构造共享 EventBus
/// 2. 用 `RoleRegistry::with_event_bus(&config, bus)` 注入总线
/// 3. 订阅 EventBus
/// 4. 注册新角色(动态注册,非默认 5 角色)
/// 5. 验证 `RoleRegistered` 事件已发布,字段包含 role_id/voting_weight
///
/// WHY 此测试验证 Ω-Event 定律:角色注册是 Parliament 的状态变更,
/// 必须发布事件通知内部组件建立投票权重表。
#[tokio::test]
async fn test_role_registered_event_flow() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let config = parliament::ParliamentConfig::default();
    let registry = parliament::RoleRegistry::with_event_bus(&config, bus);

    // 注册新角色(动态注册,触发 RoleRegistered 事件)
    let profile = parliament::RoleProfile::new(
        "role-e2e-test",
        parliament::Role::Architect,
        "E2E 测试角色",
        "test-model",
        0.05,
        false,
    );
    registry.register(profile).expect("动态注册角色应成功");

    // 验证 RoleRegistered 事件已发布
    let events = drain_events(&mut rx, 5).await;
    assert_has_event(&events, "RoleRegistered");

    // 进一步验证事件字段
    let registered = events
        .iter()
        .find(|e| e.type_name() == "RoleRegistered")
        .expect("应找到 RoleRegistered 事件");
    if let NexusEvent::RoleRegistered {
        role_id,
        voting_weight,
        ..
    } = registered
    {
        assert_eq!(
            role_id, "role-e2e-test",
            "role_id 应为 role-e2e-test,实际: {role_id}"
        );
        assert!(
            (voting_weight - 0.05).abs() < 1e-6,
            "voting_weight 应为 0.05,实际: {voting_weight}"
        );
    } else {
        panic!("事件类型匹配失败,期望 RoleRegistered 变体");
    }
}
