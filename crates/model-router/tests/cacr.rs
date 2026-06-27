//! 集成测试 — CACR(Cost-Aware Cognitive Routing)成本感知路由守卫
//!
//! 覆盖场景:
//! 1. 预算充足时 Allow:正常路由,不触发 CACR 干预
//! 2. 预算 80% 时 Downgrade:降级到次优模型
//! 3. 预算 100% 时 Block:返回 BudgetExceeded 错误
//! 4. BudgetExceeded 事件发布:Block 时订阅 EventBus 验证事件
//! 5. 阈值可通过配置调整:不同 budget_limit 下路由器行为不同
//! 6. 无 CACR 时正常路由:ModelRouter::new 行为与 Task 6 一致
//! 7. 降级后 ModelRouteSelected 事件包含降级原因
//! 8. CacrGuard::check 边界条件(单元测试在 cacr.rs 中,此处验证集成行为)

use event_bus::{EventBus, NexusEvent};
use model_router::{
    CacrConfig, CacrDecision, CacrGuard, ModelRegistry, ModelRouter, RouterConfig, RouterError,
    RoutingRequest, RoutingStrategy,
};
use nexus_core::{MultimodalInput, UserIntent};

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造测试用 UserIntent
fn make_intent() -> UserIntent {
    UserIntent {
        intent_id: "i-1".into(),
        raw_text: "test".into(),
        multimodal_inputs: vec![MultimodalInput::Text("test".into())],
        risk_level: 10,
    }
}

/// 构造测试用 RoutingRequest
fn make_request(strategy: RoutingStrategy, tokens: u32) -> RoutingRequest {
    RoutingRequest {
        quest_id: "q-1".into(),
        intent: make_intent(),
        estimated_tokens: tokens,
        strategy,
    }
}

/// 构造默认注册表(含 lite/efficient/premium 三模型)
fn make_registry() -> ModelRegistry {
    ModelRegistry::from_config(&RouterConfig::default())
}

// ============================================================
// 测试 1:预算充足时 Allow
// ============================================================

#[tokio::test]
async fn test_cacr_allow_when_budget_sufficient() {
    let bus = EventBus::new();
    let registry = make_registry();

    // 默认预算 1_000_000 美分(10000 美元),warn 0.8,block 1.0
    let router = ModelRouter::with_cacr(registry, bus, CacrConfig::default());

    // Lite 策略,1000 tokens,lite-model 成本 = 0 美分
    // 0 < 800000(warn_limit)→ Allow
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 1000))
        .await
        .expect("预算充足时应允许路由");

    assert_eq!(decision.model_id, "lite-model");
    // Allow 时 route_reason 不应包含 CACR 标识
    assert!(
        !decision.route_reason.contains("CACR"),
        "Allow 时 route_reason 不应包含 CACR,实际: {}",
        decision.route_reason
    );
}

#[tokio::test]
async fn test_cacr_allow_with_high_cost_but_within_budget() {
    let bus = EventBus::new();
    let registry = make_registry();

    // 预算 1000000 美分,warn 0.8 → warn_limit = 800000
    let router = ModelRouter::with_cacr(registry, bus, CacrConfig::default());

    // Lite 策略,1000000 tokens,lite-model 成本 = 100 美分
    // 100 < 800000 → Allow
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 1_000_000))
        .await
        .expect("成本在预算内时应允许路由");

    assert_eq!(decision.model_id, "lite-model");
    assert_eq!(decision.estimated_cost, 10); // 1000000 * 0.0001 / 1000 * 100 = 10 美分
}

// ============================================================
// 测试 2:预算 80% 时 Downgrade
// ============================================================

#[tokio::test]
async fn test_cacr_downgrade_to_next_candidate() {
    let bus = EventBus::new();
    let registry = make_registry();

    // 预算 10 美分,warn 0.8 → warn_limit = 8,block 1.0 → block_limit = 10
    let cacr_config = CacrConfig {
        budget_limit: 10,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    // Lite 策略,800000 tokens,lite-model 成本 = 8 美分
    // 8 >= 8(warn_limit)且 8 < 10(block_limit)→ Downgrade
    // candidates[0] = efficient-model(Lite 按成本升序,选中 lite 后候选 [efficient, premium])
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 800_000))
        .await
        .expect("Downgrade 应返回降级后的决策,而非错误");

    assert_eq!(
        decision.model_id, "efficient-model",
        "Downgrade 应切换到次优模型 efficient-model"
    );
    assert!(
        decision.route_reason.contains("CACR Downgrade"),
        "route_reason 应包含 CACR Downgrade 标识,实际: {}",
        decision.route_reason
    );
    assert!(
        decision.route_reason.contains("lite-model"),
        "route_reason 应包含原始模型 lite-model,实际: {}",
        decision.route_reason
    );
}

#[tokio::test]
async fn test_cacr_downgrade_recalculates_cost() {
    let bus = EventBus::new();
    let registry = make_registry();

    let cacr_config = CacrConfig {
        budget_limit: 10,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    // 800000 tokens,lite-model 成本 = 8 美分 → Downgrade
    // 降级到 efficient-model,成本 = 800000 * 0.002 / 1000 * 100 = 160 美分
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 800_000))
        .await
        .unwrap();

    assert_eq!(decision.model_id, "efficient-model");
    // 预估成本应基于 efficient-model 重新计算
    assert_eq!(
        decision.estimated_cost, 160,
        "降级后预估成本应基于 efficient-model 重新计算"
    );
}

// ============================================================
// 测试 3:预算 100% 时 Block
// ============================================================

#[tokio::test]
async fn test_cacr_block_returns_budget_exceeded_error() {
    let bus = EventBus::new();
    let registry = make_registry();

    // 预算 1 美分,warn 0.8 → warn_limit = 0,block 1.0 → block_limit = 1
    let cacr_config = CacrConfig {
        budget_limit: 1,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    // Lite 策略,100000 tokens,lite-model 成本 = 1 美分
    // 1 >= 1(block_limit)→ Block
    let result = router
        .route(make_request(RoutingStrategy::Lite, 100_000))
        .await;

    match result {
        Err(RouterError::BudgetExceeded { cost, limit }) => {
            assert_eq!(cost, 1, "错误中应携带实际成本");
            assert_eq!(limit, 1, "错误中应携带预算上限");
        }
        other => panic!("期望 BudgetExceeded 错误,实际: {:?}", other),
    }
}

// ============================================================
// 测试 4:BudgetExceeded 事件发布
// ============================================================

#[tokio::test]
async fn test_cacr_block_publishes_budget_exceeded_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let registry = make_registry();

    let cacr_config = CacrConfig {
        budget_limit: 1,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    // 触发 Block
    let result = router
        .route(make_request(RoutingStrategy::Lite, 100_000))
        .await;
    assert!(matches!(result, Err(RouterError::BudgetExceeded { .. })));

    // 验证 BudgetExceeded 事件已发布
    let event = rx.recv().await.expect("应收到 BudgetExceeded 事件");
    match event {
        NexusEvent::BudgetExceeded {
            budget_type,
            current,
            limit,
            metadata,
        } => {
            assert_eq!(budget_type, "cacr", "budget_type 应为 'cacr'");
            assert_eq!(current, 1, "current 应为实际成本");
            assert_eq!(limit, 1, "limit 应为预算上限");
            assert_eq!(metadata.source, "model-router", "source 应为 model-router");
        }
        other => panic!("期望 BudgetExceeded 事件,实际: {:?}", other),
    }
}

// ============================================================
// 测试 5:阈值可通过配置调整
// ============================================================

#[tokio::test]
async fn test_cacr_threshold_configurable_block_at_different_budget() {
    // 场景:相同请求,不同预算,行为不同
    let registry = make_registry();

    // 预算 100 美分:100000 tokens lite-model 成本 = 1 美分 → Allow(1 < 80)
    let bus1 = EventBus::new();
    let router1 = ModelRouter::with_cacr(
        registry.clone(),
        bus1,
        CacrConfig {
            budget_limit: 100,
            warn_threshold: 0.8,
            block_threshold: 1.0,
        },
    );
    let decision = router1
        .route(make_request(RoutingStrategy::Lite, 100_000))
        .await
        .expect("预算 100 时应 Allow");
    assert_eq!(decision.model_id, "lite-model");

    // 预算 1 美分:同样请求 → Block(1 >= 1)
    let bus2 = EventBus::new();
    let router2 = ModelRouter::with_cacr(
        registry,
        bus2,
        CacrConfig {
            budget_limit: 1,
            warn_threshold: 0.8,
            block_threshold: 1.0,
        },
    );
    let result = router2
        .route(make_request(RoutingStrategy::Lite, 100_000))
        .await;
    assert!(
        matches!(result, Err(RouterError::BudgetExceeded { .. })),
        "预算 1 时应 Block"
    );
}

#[tokio::test]
async fn test_cacr_warn_threshold_configurable() {
    // 降低 warn 阈值,使原本 Allow 的成本触发 Downgrade
    let bus = EventBus::new();
    let registry = make_registry();

    // 预算 100,warn 0.01 → warn_limit = 1,block 1.0 → block_limit = 100
    // lite-model 100000 tokens 成本 = 1 美分
    // 1 >= 1(warn_limit)且 1 < 100(block_limit)→ Downgrade
    let cacr_config = CacrConfig {
        budget_limit: 100,
        warn_threshold: 0.01,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    let decision = router
        .route(make_request(RoutingStrategy::Lite, 100_000))
        .await
        .expect("应 Downgrade 而非 Block");
    assert_eq!(decision.model_id, "efficient-model");
    assert!(decision.route_reason.contains("CACR Downgrade"));
}

#[tokio::test]
async fn test_cacr_block_threshold_configurable() {
    // 降低 block 阈值,使原本 Downgrade 的成本触发 Block
    let bus = EventBus::new();
    let registry = make_registry();

    // 预算 10,warn 0.8 → warn_limit = 8,block 0.85 → block_limit = 8
    // lite-model 800000 tokens 成本 = 8 美分
    // 8 >= 8(block_limit)→ Block(若 block_threshold = 1.0 则为 Downgrade)
    let cacr_config = CacrConfig {
        budget_limit: 10,
        warn_threshold: 0.8,
        block_threshold: 0.85,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    let result = router
        .route(make_request(RoutingStrategy::Lite, 800_000))
        .await;
    assert!(
        matches!(result, Err(RouterError::BudgetExceeded { .. })),
        "block_threshold=0.85 时 8 美分应 Block(block_limit=8)"
    );
}

// ============================================================
// 测试 6:无 CACR 时正常路由
// ============================================================

#[tokio::test]
async fn test_router_without_cacr_behaves_like_task6() {
    let bus = EventBus::new();
    let registry = make_registry();
    let router = ModelRouter::new(registry, bus);

    // 验证 cacr_guard 为 None
    assert!(router.cacr_guard().is_none());

    // 即使成本很高,不启用 CACR 时也不会被拦截
    // 1000000 tokens,lite-model 成本 = 10 美分,但无 CACR 保护 → 正常路由
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 1_000_000))
        .await
        .expect("无 CACR 时应正常路由");
    assert_eq!(decision.model_id, "lite-model");
    assert!(!decision.route_reason.contains("CACR"));
}

#[tokio::test]
async fn test_router_without_cacr_allows_high_cost() {
    let bus = EventBus::new();
    let registry = make_registry();
    let router = ModelRouter::new(registry, bus);

    // 极高成本,无 CACR → 正常路由(无保护)
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 100_000_000))
        .await
        .expect("无 CACR 时即使成本极高也应路由");
    assert_eq!(decision.model_id, "lite-model");
    // 100000000 * 0.0001 / 1000 * 100 = 1000 美分
    assert_eq!(decision.estimated_cost, 1000);
}

// ============================================================
// 测试 7:降级后 ModelRouteSelected 事件包含降级原因
// ============================================================

#[tokio::test]
async fn test_downgrade_event_contains_cacr_reason() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let registry = make_registry();

    let cacr_config = CacrConfig {
        budget_limit: 10,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let router = ModelRouter::with_cacr(registry, bus, cacr_config);

    // 触发 Downgrade
    let decision = router
        .route(make_request(RoutingStrategy::Lite, 800_000))
        .await
        .unwrap();
    assert_eq!(decision.model_id, "efficient-model");

    // 验证 ModelRouteSelected 事件
    let event = rx.recv().await.expect("应收到 ModelRouteSelected 事件");
    match event {
        NexusEvent::ModelRouteSelected {
            model_id,
            route_reason,
            quest_id,
            ..
        } => {
            assert_eq!(
                model_id, "efficient-model",
                "事件中 model_id 应为降级后的模型"
            );
            assert!(
                route_reason.contains("CACR Downgrade"),
                "事件 route_reason 应包含 CACR Downgrade,实际: {}",
                route_reason
            );
            assert_eq!(quest_id, "q-1");
        }
        other => panic!("期望 ModelRouteSelected 事件,实际: {:?}", other),
    }
}

#[tokio::test]
async fn test_allow_event_does_not_contain_cacr() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let registry = make_registry();

    let router = ModelRouter::with_cacr(registry, bus, CacrConfig::default());

    // Allow 场景
    router
        .route(make_request(RoutingStrategy::Lite, 1000))
        .await
        .unwrap();

    let event = rx.recv().await.unwrap();
    if let NexusEvent::ModelRouteSelected { route_reason, .. } = event {
        assert!(
            !route_reason.contains("CACR"),
            "Allow 时事件 route_reason 不应包含 CACR,实际: {}",
            route_reason
        );
    }
}

// ============================================================
// 测试 8:CacrGuard::check 集成验证
// ============================================================

#[test]
fn test_cacr_guard_check_allow() {
    let config = CacrConfig {
        budget_limit: 1_000_000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let guard = CacrGuard::new(config);

    // 预估成本 100 美分,剩余预算 1000000 美分 → Allow
    let decision = guard.check(100, 1_000_000);
    assert_eq!(decision, CacrDecision::Allow);
}

#[test]
fn test_cacr_guard_check_downgrade() {
    let config = CacrConfig {
        budget_limit: 1_000_000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let guard = CacrGuard::new(config);

    // 预估成本 800 美分,剩余预算 1000 美分 → Downgrade(800 >= 800 且 < 1000)
    let decision = guard.check(800, 1000);
    assert!(matches!(decision, CacrDecision::Downgrade(_)));
}

#[test]
fn test_cacr_guard_check_block() {
    let config = CacrConfig {
        budget_limit: 1000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let guard = CacrGuard::new(config);

    // 预估成本 1500 美分,剩余预算 1000 美分 → Block(1500 >= 1000)
    let decision = guard.check(1500, 1000);
    assert!(matches!(decision, CacrDecision::Block(_)));
}

#[test]
fn test_cacr_guard_check_boundary_at_warn_threshold() {
    let guard = CacrGuard::new(CacrConfig {
        budget_limit: 1_000_000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    });

    // 边界:成本恰好等于 warn_limit → Downgrade
    // 1000 * 0.8 = 800
    let decision = guard.check(800, 1000);
    assert!(matches!(decision, CacrDecision::Downgrade(_)));
}

#[test]
fn test_cacr_guard_check_boundary_just_below_warn_threshold() {
    let guard = CacrGuard::new(CacrConfig {
        budget_limit: 1_000_000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    });

    // 边界:成本恰好低于 warn_limit → Allow
    // 1000 * 0.8 = 800,成本 799 < 800
    let decision = guard.check(799, 1000);
    assert_eq!(decision, CacrDecision::Allow);
}

#[test]
fn test_cacr_guard_check_boundary_at_block_threshold() {
    let guard = CacrGuard::new(CacrConfig {
        budget_limit: 1_000_000,
        warn_threshold: 0.8,
        block_threshold: 1.0,
    });

    // 边界:成本恰好等于 block_limit → Block
    // 1000 * 1.0 = 1000
    let decision = guard.check(1000, 1000);
    assert!(matches!(decision, CacrDecision::Block(_)));
}

// ============================================================
// 测试 9:CacrConfig 序列化(集成验证)
// ============================================================

#[test]
fn test_cacr_config_serde_roundtrip() {
    let config = CacrConfig {
        budget_limit: 5000,
        warn_threshold: 0.75,
        block_threshold: 0.95,
    };
    let json = serde_json::to_string(&config).expect("序列化失败");
    let de: CacrConfig = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(de.budget_limit, 5000);
    assert!((de.warn_threshold - 0.75).abs() < f32::EPSILON);
    assert!((de.block_threshold - 0.95).abs() < f32::EPSILON);
}

// ============================================================
// 测试 10:RouterConfig 集成 CACR 配置
// ============================================================

#[test]
fn test_router_config_contains_cacr() {
    let config = RouterConfig::default();
    assert_eq!(config.cacr.budget_limit, 1_000_000);
}

#[test]
fn test_router_config_serde_with_cacr() {
    let config = RouterConfig {
        cacr: CacrConfig {
            budget_limit: 999,
            warn_threshold: 0.5,
            block_threshold: 0.9,
        },
        ..Default::default()
    };
    let json = serde_json::to_string(&config).expect("序列化失败");
    let de: RouterConfig = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(de.cacr.budget_limit, 999);
}
