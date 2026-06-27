//! 模型路由器主入口 — 协调注册表、策略、CACR 守卫与事件总线
//!
//! 对应架构:L1 Core,被 L9 Quest Engine 调用
//!
//! # 职责
//! - 持有 `ModelRegistry`、`EventBus` 与可选的 `CacrGuard`
//! - 按 `RoutingStrategy` 分发到对应策略函数,获得初始决策
//! - 若启用 CACR,对初始决策进行成本拦截(Allow/Downgrade/Block)
//! - 路由成功后发布 `ModelRouteSelected` 事件
//!
//! # 事件流
//! ```text
//! Quest Engine ──RoutingRequest──> ModelRouter
//! ModelRouter ──(CACR 拦截)──> 策略函数
//! ModelRouter ──ModelRouteSelected──> EventBus ──> Quest/Parliament
//! ModelRouter ──BudgetExceeded──> EventBus ──> Parliament (Block 时)
//! ```

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::cacr::{CacrConfig, CacrDecision, CacrGuard};
use crate::error::RouterError;
use crate::registry::ModelRegistry;
use crate::strategies;
use crate::types::{RoutingDecision, RoutingRequest, RoutingStrategy};

/// 事件源标识 — 用于 `EventMetadata.source`,标识事件发布者
const ROUTER_SOURCE: &str = "model-router";

/// 模型路由器 — 协调注册表、策略、CACR 守卫与事件总线
///
/// 持有 `ModelRegistry`(可 Clone 共享)、`EventBus`(可 Clone 共享)
/// 与可选的 `CacrGuard`(成本感知守卫)。
///
/// # 向后兼容
/// `ModelRouter::new` 不启用 CACR,行为与 Task 6 完全一致。
/// 需要成本保护时使用 `ModelRouter::with_cacr`。
pub struct ModelRouter {
    registry: ModelRegistry,
    event_bus: EventBus,
    /// CACR 守卫 — `None` 表示禁用成本保护(向后兼容)
    cacr_guard: Option<CacrGuard>,
}

impl ModelRouter {
    /// 创建路由器,绑定注册表与事件总线(不启用 CACR)
    ///
    /// 行为与 Task 6 完全一致,保证向后兼容。
    pub fn new(registry: ModelRegistry, event_bus: EventBus) -> Self {
        Self {
            registry,
            event_bus,
            cacr_guard: None,
        }
    }

    /// 创建带 CACR 保护的 ModelRouter
    ///
    /// WHY:单独的构造函数明确表达"启用成本保护"的意图,
    /// 避免在 `new` 中加入配置参数破坏向后兼容。
    pub fn with_cacr(
        registry: ModelRegistry,
        event_bus: EventBus,
        cacr_config: CacrConfig,
    ) -> Self {
        Self {
            registry,
            event_bus,
            cacr_guard: Some(CacrGuard::new(cacr_config)),
        }
    }

    /// 路由请求:按策略分发 → CACR 拦截 → 发布事件
    ///
    /// # 处理流程
    /// 1. 校验注册表非空(提前返回,避免策略函数重复检查)
    /// 2. 按 `request.strategy` 分发到对应策略,获得初始决策
    /// 3. 若启用 CACR,对初始决策进行成本拦截:
    ///    - `Allow`:放行原决策
    ///    - `Downgrade`:切换到次优模型(`candidates[0]`),重算成本
    ///    - `Block`:发布 `BudgetExceeded` 事件,返回 `BudgetExceeded` 错误
    /// 4. 发布 `ModelRouteSelected` 事件
    ///
    /// # 错误处理
    /// - 注册表为空 → `RouterError::NoModelsRegistered`
    /// - CACR Block → `RouterError::BudgetExceeded`(同时发布事件)
    /// - 事件发布失败 → `RouterError::EventBusError`
    pub async fn route(&self, request: RoutingRequest) -> Result<RoutingDecision, RouterError> {
        // 1. 前置校验:注册表非空
        if self.registry.count() == 0 {
            return Err(RouterError::NoModelsRegistered);
        }

        // 2. 按策略分发,获得初始决策
        let mut decision = match request.strategy {
            RoutingStrategy::Lite => strategies::route_lite(&self.registry, &request)?,
            RoutingStrategy::Efficient => strategies::route_efficient(&self.registry, &request)?,
            RoutingStrategy::Auto => strategies::route_auto(&self.registry, &request)?,
        };

        // 3. CACR 拦截检查(若启用)
        if let Some(guard) = &self.cacr_guard {
            // Week 2 阶段:剩余预算 = 预算上限(静态值)
            // Week 5 接入 DECB 后,改为查询动态剩余预算
            let remaining_budget = guard.budget_limit();
            let cacr_decision = guard.check(decision.estimated_cost, remaining_budget);

            match cacr_decision {
                CacrDecision::Allow => {
                    // 正常路由,继续发布事件
                }
                CacrDecision::Downgrade(reason) => {
                    // 降级到次优模型:candidates[0] 是除首选外最优的候选
                    // WHY:candidates 列表已按策略优先级降序排序,
                    // index 0 即为次优。若无候选,则降级失败但仍允许路由(避免死锁)。
                    if !decision.candidates.is_empty() {
                        let original_model = decision.model_id.clone();
                        let downgrade_target = decision.candidates[0].clone();
                        // 从注册表查询次优模型信息,重算预估成本
                        if let Some(model) = self.registry.get(&downgrade_target) {
                            decision.estimated_cost = strategies::estimate_cost(
                                request.estimated_tokens,
                                model.cost_per_1k_tokens,
                            );
                        }
                        decision.model_id = downgrade_target;
                        decision.route_reason =
                            format!("CACR Downgrade: {} (original: {})", reason, original_model);
                    }
                    // 若无次优候选,继续使用原决策(降级失败但仍允许)
                }
                CacrDecision::Block(_reason) => {
                    // 发布 BudgetExceeded 事件,供 L8 Parliament 感知预算状态
                    let event = NexusEvent::BudgetExceeded {
                        metadata: EventMetadata::new(ROUTER_SOURCE),
                        budget_type: "cacr".into(),
                        current: decision.estimated_cost,
                        limit: guard.budget_limit(),
                    };
                    self.event_bus.publish(event).await?;

                    // WHY:reason 的详细信息(成本/预算/阈值)已通过 BudgetExceeded
                    // 事件的 current/limit 字段传递,此处返回错误时携带 cost/limit 供调用方决策
                    return Err(RouterError::BudgetExceeded {
                        cost: decision.estimated_cost,
                        limit: guard.budget_limit(),
                    });
                }
            }
        }

        // 4. 发布 ModelRouteSelected 事件,供 Quest Engine 等订阅者消费
        let event = NexusEvent::ModelRouteSelected {
            metadata: EventMetadata::new(ROUTER_SOURCE),
            quest_id: request.quest_id,
            model_id: decision.model_id.clone(),
            route_reason: decision.route_reason.clone(),
        };
        self.event_bus.publish(event).await?;

        Ok(decision)
    }

    /// 获取注册表引用(用于动态注册/注销模型)
    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    /// 获取事件总线引用(用于额外订阅)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 获取 CACR 守卫引用(若启用)
    pub fn cacr_guard(&self) -> Option<&CacrGuard> {
        self.cacr_guard.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RouterConfig;
    use crate::types::ModelInfo;
    use nexus_core::{MultimodalInput, UserIntent};

    fn make_intent() -> UserIntent {
        UserIntent {
            intent_id: "i-1".into(),
            raw_text: "test".into(),
            multimodal_inputs: vec![MultimodalInput::Text("test".into())],
            risk_level: 10,
        }
    }

    fn make_request(strategy: RoutingStrategy) -> RoutingRequest {
        RoutingRequest {
            quest_id: "q-1".into(),
            intent: make_intent(),
            estimated_tokens: 1000,
            strategy,
        }
    }

    fn make_router() -> (ModelRouter, EventBus) {
        let bus = EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        let router = ModelRouter::new(registry, bus.clone());
        (router, bus)
    }

    #[tokio::test]
    async fn test_route_lite_publishes_event() {
        let (router, bus) = make_router();
        let mut rx = bus.subscribe();

        let decision = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .unwrap();
        assert_eq!(decision.model_id, "lite-model");

        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::ModelRouteSelected {
                quest_id,
                model_id,
                route_reason,
                ..
            } => {
                assert_eq!(quest_id, "q-1");
                assert_eq!(model_id, "lite-model");
                assert!(route_reason.contains("Lite"));
            }
            other => panic!("expected ModelRouteSelected, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_route_efficient_publishes_event() {
        let (router, bus) = make_router();
        let mut rx = bus.subscribe();

        let decision = router
            .route(make_request(RoutingStrategy::Efficient))
            .await
            .unwrap();
        assert_eq!(decision.model_id, "lite-model");

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, NexusEvent::ModelRouteSelected { .. }));
    }

    #[tokio::test]
    async fn test_route_auto_publishes_event() {
        let (router, bus) = make_router();
        let mut rx = bus.subscribe();

        let decision = router
            .route(make_request(RoutingStrategy::Auto))
            .await
            .unwrap();
        assert!(decision.model_id == "lite-model" || decision.model_id == "efficient-model");

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, NexusEvent::ModelRouteSelected { .. }));
    }

    #[tokio::test]
    async fn test_route_empty_registry_returns_error() {
        let bus = EventBus::new();
        let registry = ModelRegistry::new();
        let router = ModelRouter::new(registry, bus);

        let result = router.route(make_request(RoutingStrategy::Lite)).await;
        assert!(matches!(result, Err(RouterError::NoModelsRegistered)));
    }

    #[tokio::test]
    async fn test_dynamic_registration() {
        let bus = EventBus::new();
        let registry = ModelRegistry::new();
        let router = ModelRouter::new(registry, bus);

        // 初始为空,路由失败
        let result = router.route(make_request(RoutingStrategy::Lite)).await;
        assert!(matches!(result, Err(RouterError::NoModelsRegistered)));

        // 动态注册模型
        router
            .registry()
            .register(ModelInfo {
                model_id: "new-model".into(),
                provider: "test".into(),
                cost_per_1k_tokens: 0.001,
                avg_latency_ms: 100,
                max_context: 8192,
                quality_score: 0.8,
            })
            .unwrap();

        // 现在路由成功
        let decision = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .unwrap();
        assert_eq!(decision.model_id, "new-model");
    }

    // ============================================================
    // CACR 集成测试(单元层)
    // ============================================================

    #[test]
    fn test_new_router_has_no_cacr_guard() {
        let (router, _bus) = make_router();
        assert!(router.cacr_guard().is_none());
    }

    #[test]
    fn test_with_cacr_has_guard() {
        let bus = EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        let router = ModelRouter::with_cacr(registry, bus, CacrConfig::default());
        assert!(router.cacr_guard().is_some());
        assert_eq!(router.cacr_guard().unwrap().budget_limit(), 1_000_000);
    }

    #[tokio::test]
    async fn test_route_without_cacr_backward_compatible() {
        // 不启用 CACR 时,路由行为与 Task 6 一致
        let (router, _bus) = make_router();
        let decision = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .unwrap();
        assert_eq!(decision.model_id, "lite-model");
        // route_reason 不应包含 CACR 标识
        assert!(!decision.route_reason.contains("CACR"));
    }
}
