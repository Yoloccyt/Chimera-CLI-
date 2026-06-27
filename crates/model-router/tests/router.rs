//! 集成测试 — ModelRouter 多策略路由端到端验证
//!
//! 覆盖场景:
//! 1. 三策略路由(Lite/Efficient/Auto)选择正确模型
//! 2. 10 条标注用例路由准确率 > 90%
//! 3. ModelRouteSelected 事件发布
//! 4. 模型动态注册/注销
//! 5. 空注册表错误
//! 6. 候选列表正确性

use event_bus::{EventBus, NexusEvent};
use model_router::{
    ModelInfo, ModelRegistry, ModelRouter, RouterConfig, RouterError, RoutingRequest,
    RoutingStrategy,
};
use nexus_core::{MultimodalInput, UserIntent};

/// 构造测试用 UserIntent
fn make_test_intent(risk: u8) -> UserIntent {
    UserIntent {
        intent_id: "i-1".into(),
        raw_text: "test".into(),
        multimodal_inputs: vec![MultimodalInput::Text("test".into())],
        risk_level: risk,
    }
}

/// 构造测试用 RoutingRequest
fn make_request(quest_id: &str, strategy: RoutingStrategy, tokens: u32) -> RoutingRequest {
    RoutingRequest {
        quest_id: quest_id.into(),
        intent: make_test_intent(10),
        estimated_tokens: tokens,
        strategy,
    }
}

/// 构造测试用 router 与 event bus
fn make_router() -> (ModelRouter, EventBus) {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = ModelRouter::new(registry, bus.clone());
    (router, bus)
}

// ============================================================
// 测试 1:三策略路由
// ============================================================

#[tokio::test]
async fn test_three_strategies() {
    let (router, _bus) = make_router();

    // Lite 策略:选择成本最低的 lite-model ($0.0001/1k)
    let req = make_request("q-lite", RoutingStrategy::Lite, 1000);
    let decision = router.route(req).await.unwrap();
    assert_eq!(
        decision.model_id, "lite-model",
        "Lite 策略应选择成本最低的 lite-model"
    );

    // Efficient 策略:选择延迟最低的 lite-model (100ms)
    let req = make_request("q-eff", RoutingStrategy::Efficient, 1000);
    let decision = router.route(req).await.unwrap();
    assert_eq!(
        decision.model_id, "lite-model",
        "Efficient 策略应选择延迟最低的 lite-model"
    );

    // Auto 策略:综合评分最高(lite-model 评分 0.867 > efficient 0.757 > premium 0.19)
    let req = make_request("q-auto", RoutingStrategy::Auto, 1000);
    let decision = router.route(req).await.unwrap();
    assert!(
        decision.model_id == "lite-model" || decision.model_id == "efficient-model",
        "Auto 策略应选择 lite-model 或 efficient-model,实际选择: {}",
        decision.model_id
    );
}

// ============================================================
// 测试 2:10 条标注用例路由准确率 > 90%
// ============================================================

/// 标注用例:每个用例包含 (策略, token 数, 预期模型 ID)
///
/// 默认配置下:
/// - lite-model: cost=0.0001, latency=100, quality=0.6
/// - efficient-model: cost=0.002, latency=300, quality=0.8
/// - premium-model: cost=0.015, latency=800, quality=0.95
///
/// Auto 评分:
/// - lite: 0.4*(1-0.0067) + 0.4*(1-0.125) + 0.2*0.6 = 0.867
/// - efficient: 0.4*(1-0.133) + 0.4*(1-0.375) + 0.2*0.8 = 0.757
/// - premium: 0.4*0 + 0.4*0 + 0.2*0.95 = 0.19
///
/// 因此默认配置下所有策略都选 lite-model。
fn labeled_cases() -> Vec<(RoutingStrategy, u32, &'static str)> {
    vec![
        (RoutingStrategy::Lite, 100, "lite-model"),
        (RoutingStrategy::Lite, 1000, "lite-model"),
        (RoutingStrategy::Lite, 10000, "lite-model"),
        (RoutingStrategy::Efficient, 100, "lite-model"),
        (RoutingStrategy::Efficient, 1000, "lite-model"),
        (RoutingStrategy::Efficient, 10000, "lite-model"),
        (RoutingStrategy::Auto, 100, "lite-model"),
        (RoutingStrategy::Auto, 1000, "lite-model"),
        (RoutingStrategy::Auto, 10000, "lite-model"),
        (RoutingStrategy::Auto, 100000, "lite-model"),
    ]
}

#[tokio::test]
async fn test_labeled_cases_accuracy() {
    let (router, _bus) = make_router();
    let cases = labeled_cases();
    let total = cases.len() as f32;
    let mut correct = 0;

    for (idx, (strategy, tokens, expected)) in cases.iter().enumerate() {
        let req = make_request(&format!("q-{idx}"), *strategy, *tokens);
        let decision = router.route(req).await.unwrap();
        if decision.model_id == *expected {
            correct += 1;
        } else {
            eprintln!(
                "用例 {idx}: 策略={:?} tokens={} 预期={} 实际={}",
                strategy, tokens, expected, decision.model_id
            );
        }
    }

    let accuracy = correct as f32 / total;
    assert!(
        accuracy > 0.9,
        "路由准确率 {:.1}% 低于 90% 阈值 ({}/{})",
        accuracy * 100.0,
        correct,
        cases.len()
    );
}

// ============================================================
// 测试 3:ModelRouteSelected 事件发布
// ============================================================

#[tokio::test]
async fn test_event_published() {
    let (router, bus) = make_router();
    let mut rx = bus.subscribe();

    let req = make_request("q-event", RoutingStrategy::Lite, 1000);
    let decision = router.route(req).await.unwrap();

    let event = rx.recv().await.unwrap();
    match event {
        NexusEvent::ModelRouteSelected {
            quest_id,
            model_id,
            route_reason,
            ..
        } => {
            assert_eq!(quest_id, "q-event");
            assert_eq!(model_id, decision.model_id);
            assert!(!route_reason.is_empty());
        }
        other => panic!("期望 ModelRouteSelected 事件,实际: {:?}", other),
    }
}

#[tokio::test]
async fn test_event_metadata_source() {
    let (router, bus) = make_router();
    let mut rx = bus.subscribe();

    let req = make_request("q-src", RoutingStrategy::Auto, 1000);
    router.route(req).await.unwrap();

    let event = rx.recv().await.unwrap();
    assert_eq!(event.metadata().source, "model-router");
}

// ============================================================
// 测试 4:模型动态注册/注销
// ============================================================

#[tokio::test]
async fn test_dynamic_register_then_route() {
    let bus = EventBus::new();
    let registry = ModelRegistry::new();
    let router = ModelRouter::new(registry, bus);

    // 初始为空,路由失败
    let result = router
        .route(make_request("q-1", RoutingStrategy::Lite, 1000))
        .await;
    assert!(matches!(result, Err(RouterError::NoModelsRegistered)));

    // 动态注册新模型
    router
        .registry()
        .register(ModelInfo {
            model_id: "new-model".into(),
            provider: "test".into(),
            cost_per_1k_tokens: 0.0005,
            avg_latency_ms: 150,
            max_context: 8192,
            quality_score: 0.7,
        })
        .unwrap();

    // 现在路由成功,且选中刚注册的模型(唯一候选)
    let decision = router
        .route(make_request("q-2", RoutingStrategy::Lite, 1000))
        .await
        .unwrap();
    assert_eq!(decision.model_id, "new-model");
}

#[tokio::test]
async fn test_unregister_then_not_selectable() {
    let (router, _bus) = make_router();

    // 注销 lite-model
    router.registry().unregister("lite-model").unwrap();

    // Lite 策略现在应选 efficient-model(成本次低)
    let req = make_request("q-1", RoutingStrategy::Lite, 1000);
    let decision = router.route(req).await.unwrap();
    assert_eq!(decision.model_id, "efficient-model");
    // 候选列表不应包含已注销的 lite-model
    assert!(!decision.candidates.contains(&"lite-model".to_string()));
}

// ============================================================
// 测试 5:空注册表错误
// ============================================================

#[tokio::test]
async fn test_empty_registry_returns_error() {
    let bus = EventBus::new();
    let registry = ModelRegistry::new();
    let router = ModelRouter::new(registry, bus);

    let strategies = [
        RoutingStrategy::Lite,
        RoutingStrategy::Efficient,
        RoutingStrategy::Auto,
    ];
    for strategy in strategies {
        let req = make_request("q-empty", strategy, 1000);
        let result = router.route(req).await;
        assert!(
            matches!(result, Err(RouterError::NoModelsRegistered)),
            "策略 {:?} 应返回 NoModelsRegistered 错误",
            strategy
        );
    }
}

// ============================================================
// 测试 6:候选列表正确性
// ============================================================

#[tokio::test]
async fn test_candidates_lite_sorted_by_cost() {
    let (router, _bus) = make_router();

    let req = make_request("q-cand", RoutingStrategy::Lite, 1000);
    let decision = router.route(req).await.unwrap();

    // 候选列表应包含所有已注册模型(除选中的),按成本升序
    // 默认配置:lite(0.0001) < efficient(0.002) < premium(0.015)
    // 选中 lite-model,候选应为 [efficient-model, premium-model]
    assert_eq!(decision.candidates.len(), 2);
    assert_eq!(decision.candidates[0], "efficient-model");
    assert_eq!(decision.candidates[1], "premium-model");
}

#[tokio::test]
async fn test_candidates_efficient_sorted_by_latency() {
    let (router, _bus) = make_router();

    let req = make_request("q-cand", RoutingStrategy::Efficient, 1000);
    let decision = router.route(req).await.unwrap();

    // 候选列表按延迟升序:lite(100) < efficient(300) < premium(800)
    // 选中 lite-model,候选应为 [efficient-model, premium-model]
    assert_eq!(decision.candidates.len(), 2);
    assert_eq!(decision.candidates[0], "efficient-model");
    assert_eq!(decision.candidates[1], "premium-model");
}

#[tokio::test]
async fn test_candidates_auto_contains_all_others() {
    let (router, _bus) = make_router();

    let req = make_request("q-cand", RoutingStrategy::Auto, 1000);
    let decision = router.route(req).await.unwrap();

    // 候选列表应包含所有未选中的模型
    assert_eq!(decision.candidates.len(), 2);
    // 选中的不应在候选列表中
    assert!(!decision.candidates.contains(&decision.model_id));
    // 候选列表应包含其他两个模型
    let all_models: std::collections::HashSet<&str> =
        decision.candidates.iter().map(|s| s.as_str()).collect();
    let selected = decision.model_id.as_str();
    let remaining: Vec<&str> = ["lite-model", "efficient-model", "premium-model"]
        .iter()
        .filter(|m| **m != selected)
        .copied()
        .collect();
    for m in &remaining {
        assert!(all_models.contains(m), "候选列表应包含 {}", m);
    }
}

// ============================================================
// 测试 7:预估成本正确性
// ============================================================

#[tokio::test]
async fn test_estimated_cost_calculation() {
    let (router, _bus) = make_router();

    // lite-model: 10000 tokens * $0.0001/1k * 100 = 0.1 美分 -> round to 0
    let req = make_request("q-cost", RoutingStrategy::Lite, 10000);
    let decision = router.route(req).await.unwrap();
    assert_eq!(decision.model_id, "lite-model");
    // 10000 * 0.0001 / 1000 * 100 = 0.1 美分,round 为 0
    assert_eq!(decision.estimated_cost, 0);

    // premium-model: 100000 tokens * $0.015/1k * 100 = 150 美分
    // 但 Lite 策略不会选 premium,需要手动构造场景
    let bus = EventBus::new();
    let registry = ModelRegistry::new();
    registry
        .register(ModelInfo {
            model_id: "premium-only".into(),
            provider: "anthropic".into(),
            cost_per_1k_tokens: 0.015,
            avg_latency_ms: 800,
            max_context: 200000,
            quality_score: 0.95,
        })
        .unwrap();
    let router = ModelRouter::new(registry, bus);
    let req = make_request("q-premium", RoutingStrategy::Lite, 100000);
    let decision = router.route(req).await.unwrap();
    // 100000 * 0.015 / 1000 * 100 = 150 美分
    assert_eq!(decision.estimated_cost, 150);
}

// ============================================================
// 测试 8:RouterConfig 序列化
// ============================================================

#[test]
fn test_config_serde_roundtrip() {
    let config = RouterConfig::default();
    let json = serde_json::to_string(&config).unwrap();
    let de: RouterConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(de.models.len(), config.models.len());
    assert_eq!(de.default_strategy, config.default_strategy);
}

// ============================================================
// 测试 9:ModelRegistry Clone 共享状态
// ============================================================

#[test]
fn test_registry_clone_shares_state() {
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let cloned = registry.clone();

    // 在 clone 上注册新模型
    cloned
        .register(ModelInfo {
            model_id: "extra".into(),
            provider: "test".into(),
            cost_per_1k_tokens: 0.001,
            avg_latency_ms: 100,
            max_context: 8192,
            quality_score: 0.7,
        })
        .unwrap();

    // 原 registry 也能看到新模型
    assert!(registry.get("extra").is_some());
    assert_eq!(registry.count(), 4);
}
