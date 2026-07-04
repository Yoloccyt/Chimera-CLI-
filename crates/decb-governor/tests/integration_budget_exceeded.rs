//! BudgetExceeded 事件发布集成测试 — 验证 §6.2 红线双通道投递
//!
//! 测试目标:
//! 1. 预算超限时,decb-governor 通过 `publish_blocking` 发布 `BudgetExceeded` 事件
//! 2. 事件 `severity() == EventSeverity::Critical`(F-001 修复守护)
//! 3. 事件同时通过主 broadcast 通道与 Critical mpsc 旁路通道投递(§6.2 红线)
//!
//! 设计依据:
//! - §6.2 红线:BudgetExceeded 必须用 mpsc channel 确保送达
//! - F-001 修复:BudgetExceeded severity 必须为 Critical
//! - Week 5 Task 37:decb-governor 集成 event-bus 发布 BudgetExceeded

use std::time::Duration;

use decb_governor::{BudgetConsumption, DecbConfig, DecbGovernor};
use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};

// ============================================================
// 测试 1:预算超限触发 BudgetExceeded 事件,severity == Critical
//
// WHY 此测试是 F-001 修复的端到端守护:从 decb-governor 业务逻辑
// 到 event-bus 事件发布的完整链路,确保 BudgetExceeded 事件
// 在发布时 severity 为 Critical(不依赖运行时值)。
// ============================================================

#[tokio::test]
async fn test_budget_exceeded_event_published_with_critical_severity() {
    // 共享 EventBus:governor 发布事件,测试订阅接收
    let bus = EventBus::new();
    let mut main_rx = bus.subscribe();

    // 创建 governor 并绑定共享 EventBus
    // WHY tier_switch_lag_ms=0:允许溢出后立即切换档位,避免滞后机制干扰测试
    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus.clone()).unwrap();

    // 触发预算超限:消耗 110% 预算(> 100% critical threshold)
    // total_budget_limit 默认 1_000_000,消耗 1_100_000 触发 critical overflow
    let consumption = BudgetConsumption {
        total_cost: 1_100_000.0,
        ..BudgetConsumption::zero()
    };

    // record_consumption 内部会 publish_blocking BudgetExceeded 事件
    // 注意:record_consumption 在溢出时会触发降级到 Degraded,但 110% 仍在
    // Degraded 阈值内,不会返回 DegradedModeRejected(需 Degraded 模式下再超限)
    governor.record_consumption(&consumption).unwrap();

    // 从主 broadcast 通道接收事件
    // 预期事件顺序:BudgetExceeded(Critical) → BudgetAdjusted(降级通知)
    let mut received_budget_exceeded = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), main_rx.recv()).await {
            Ok(Ok(event)) => {
                if let NexusEvent::BudgetExceeded { current, limit, .. } = &event {
                    // F-001 守护:severity 必须为 Critical
                    assert_eq!(
                        event.severity(),
                        EventSeverity::Critical,
                        "BudgetExceeded severity 必须为 Critical (F-001 / §6.2 红线)"
                    );
                    // 验证字段正确性
                    assert_eq!(*current, 1_100_000u64, "current 应为消耗值");
                    assert_eq!(*limit, 1_000_000u64, "limit 应为预算上限");
                    received_budget_exceeded = true;
                    break;
                }
                // 其他事件(BudgetAdjusted 等)继续接收
            }
            _ => break,
        }
    }

    assert!(
        received_budget_exceeded,
        "应通过主 broadcast 通道收到 BudgetExceeded 事件"
    );
}

// ============================================================
// 测试 2:BudgetExceeded 通过 Critical mpsc 旁路通道投递
//
// WHY 此测试是 §6.2 红线的核心验证:BudgetExceeded 事件必须通过
// mpsc 旁路通道确保送达,即使主 broadcast 通道 Lagged 也不丢失。
// 这是双通道化的根本目的。
// ============================================================

#[tokio::test]
async fn test_budget_exceeded_received_via_critical_mpsc_bypass() {
    let bus = EventBus::new();

    // 同步订阅 Critical mpsc 旁路(§4.4 反模式 3:必须在 publish 之前订阅)
    let mut critical_rx = bus.subscribe_critical_events();

    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus.clone()).unwrap();

    // 触发预算超限(85% 消耗,触发 LowTier 降级 + BudgetExceeded 事件)
    let consumption = BudgetConsumption {
        total_cost: 850_000.0,
        ..BudgetConsumption::zero()
    };
    governor.record_consumption(&consumption).unwrap();

    // 通过 mpsc 旁路通道接收 BudgetExceeded 事件
    let mut received = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), critical_rx.recv()).await {
            Ok(Some(event)) => {
                if let NexusEvent::BudgetExceeded { budget_type, .. } = &event {
                    // §6.2 红线守护:severity 必须为 Critical
                    assert_eq!(
                        event.severity(),
                        EventSeverity::Critical,
                        "BudgetExceeded 通过 mpsc 旁路接收时 severity 必须为 Critical"
                    );
                    // 验证 budget_type 字段(decb-governor 固定使用 "total_cost")
                    assert_eq!(budget_type, "total_cost", "budget_type 应为 'total_cost'");
                    received = true;
                    break;
                }
                // 其他 Critical 事件继续接收(若有)
            }
            _ => break,
        }
    }

    assert!(
        received,
        "应通过 mpsc 旁路通道收到 BudgetExceeded 事件(§6.2 红线)"
    );
}

// ============================================================
// 测试 3:BudgetExceeded 在主通道 Lagged 场景下仍通过 mpsc 旁路到达
//
// WHY 此测试模拟最恶劣场景:主 broadcast 通道因慢消费者 Lagged 丢弃事件,
// 断言 BudgetExceeded 仍能通过 mpsc 旁路通道到达订阅者。这是双通道化
// 的核心价值 — §6.2 红线"Critical 安全事件必须确保送达"。
// ============================================================

#[tokio::test]
async fn test_budget_exceeded_survives_main_channel_lagged() {
    // 主通道容量 4,慢消费者不消费 → 触发 Lagged
    let bus = EventBus::with_capacity(4);
    let mut slow_main_rx = bus.subscribe(); // 主通道慢消费者
    let mut critical_rx = bus.subscribe_critical_events();

    let config = DecbConfig {
        tier_switch_lag_ms: 0,
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::with_event_bus(config, bus.clone()).unwrap();

    // 步骤 1:先填充大量非 Critical 事件让主通道 Lagged
    // WHY 先填充:确保主通道在 BudgetExceeded 发布前已处于 Lagged 状态
    for i in 0..20 {
        bus.publish(NexusEvent::CacheHit {
            metadata: EventMetadata::new("test-fill"),
            cache_key: format!("fill-{i}"),
        })
        .await
        .unwrap();
    }

    // 步骤 2:触发 BudgetExceeded 事件
    let consumption = BudgetConsumption {
        total_cost: 1_100_000.0,
        ..BudgetConsumption::zero()
    };
    governor.record_consumption(&consumption).unwrap();

    // 步骤 3:主通道慢消费者应收到 Lagged 错误
    let main_result = slow_main_rx.recv().await;
    assert!(
        main_result.is_err(),
        "主通道慢消费者应收到 Lagged 错误,实际: {main_result:?}"
    );

    // 步骤 4(关键断言):mpsc 旁路应能收到 BudgetExceeded 事件
    let mut received = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), critical_rx.recv()).await {
            Ok(Some(event)) => {
                if matches!(event, NexusEvent::BudgetExceeded { .. }) {
                    assert_eq!(
                        event.severity(),
                        EventSeverity::Critical,
                        "BudgetExceeded severity 必须为 Critical"
                    );
                    received = true;
                    break;
                }
            }
            _ => break,
        }
    }

    assert!(
        received,
        "主通道 Lagged 时,BudgetExceeded 必须通过 mpsc 旁路到达(§6.2 红线)"
    );
}

// ============================================================
// 测试 4:未超限时不发布 BudgetExceeded 事件
//
// WHY 此测试守护"不误报":正常消耗(未超限)不应触发 BudgetExceeded,
// 避免 Critical 通道被噪声事件污染。
// ============================================================

#[tokio::test]
async fn test_no_budget_exceeded_when_under_limit() {
    let bus = EventBus::new();
    let mut critical_rx = bus.subscribe_critical_events();

    let config = DecbConfig::default();
    let governor = DecbGovernor::with_event_bus(config, bus.clone()).unwrap();

    // 少量消耗(50% 预算,未触发 80% warn 阈值)
    let consumption = BudgetConsumption {
        total_cost: 500_000.0,
        ..BudgetConsumption::zero()
    };
    governor.record_consumption(&consumption).unwrap();

    // mpsc 旁路通道不应收到任何事件(200ms 内无事件)
    let result = tokio::time::timeout(Duration::from_millis(200), critical_rx.recv()).await;
    assert!(
        result.is_err(),
        "未超限时不应发布 BudgetExceeded 事件,但收到了: {result:?}"
    );
}
