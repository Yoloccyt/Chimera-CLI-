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
use std::sync::Arc;
use std::time::Instant;

use crate::cacr::{CacrConfig, CacrDecision, CacrGuard};
use crate::error::RouterError;
use crate::history::HistoryStore;
use crate::moe::MoeGate;
use crate::registry::ModelRegistry;
use crate::strategies;
use crate::trajectory::{RouteHook, TrajectoryEvent, TrajectoryOutcome};
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
///
/// P4-W16.1.1 扩展:`hooks` 字段允许上层注入 `RouteHook` trait 实现,
/// 在 route() 边界捕获轨迹(请求/响应/延迟/token 成本/路由决策)。
/// 默认为空 Vec,行为与既有完全一致(向后兼容)。
pub struct ModelRouter {
    registry: ModelRegistry,
    event_bus: EventBus,
    /// CACR 守卫 — `None` 表示禁用成本保护(向后兼容)
    cacr_guard: Option<CacrGuard>,
    /// P4-W16.1.1: 轨迹捕获 hook 列表 — 默认空,route() 末尾依次调用
    hooks: Vec<Arc<dyn RouteHook>>,
    /// 历史路由存储(可选)— 用于 MoE 五维门控评分
    ///
    /// WHY Option<Arc<dyn HistoryStore>>:Send + Sync + Clone 廉价,
    /// spawn_blocking 需要 'static 生命周期,Arc 满足此约束。
    /// 设置后,`route()` 在 Auto 策略下会用 `spawn_blocking` 包装
    /// `SqliteHistoryStore` 的同步 rusqlite 调用,避免阻塞 tokio runtime
    /// (§4.4 #2 反模式:rusqlite 必须 spawn_blocking)。
    history_store: Option<Arc<dyn HistoryStore>>,
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
            hooks: Vec::new(),
            history_store: None,
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
            hooks: Vec::new(),
            history_store: None,
        }
    }

    /// P4-W16.1.1: 创建带单个轨迹捕获 hook 的 ModelRouter
    ///
    /// # 设计意图
    /// 提供便捷的单 hook 注入入口。多 hook 场景请使用 `with_hooks()`。
    ///
    /// # 向后兼容
    /// hook 注入仅观察路由轨迹,不修改路由决策,与既有行为兼容。
    pub fn with_hook(
        registry: ModelRegistry,
        event_bus: EventBus,
        hook: Arc<dyn RouteHook>,
    ) -> Self {
        Self {
            registry,
            event_bus,
            cacr_guard: None,
            hooks: vec![hook],
            history_store: None,
        }
    }

    /// P4-W16.1.1: 创建带多个轨迹捕获 hook 的 ModelRouter
    ///
    /// # 调用顺序
    /// hooks 按 Vec 顺序依次调用 `on_route_completed`。
    /// 单个 hook panic 不应影响其他 hook,但当前实现未隔离 panic(留待后续增强)。
    pub fn with_hooks(
        registry: ModelRegistry,
        event_bus: EventBus,
        hooks: Vec<Arc<dyn RouteHook>>,
    ) -> Self {
        Self {
            registry,
            event_bus,
            cacr_guard: None,
            hooks,
            history_store: None,
        }
    }

    /// P4-W16.1.1: 创建同时带 CACR 守卫与单个 hook 的 ModelRouter
    ///
    /// # 使用场景
    /// 当既需要成本保护又需要轨迹捕获时使用。
    pub fn with_cacr_and_hook(
        registry: ModelRegistry,
        event_bus: EventBus,
        cacr_config: CacrConfig,
        hook: Arc<dyn RouteHook>,
    ) -> Self {
        Self {
            registry,
            event_bus,
            cacr_guard: Some(CacrGuard::new(cacr_config)),
            hooks: vec![hook],
            history_store: None,
        }
    }

    /// P4-W16.1.1: 追加单个 hook 到现有 router
    ///
    /// # 使用场景
    /// 路由器构造后追加 hook(如运行时动态注册观测组件)。
    pub fn add_hook(&mut self, hook: Arc<dyn RouteHook>) {
        self.hooks.push(hook);
    }

    /// P1-3: 设置历史路由存储(用于 MoE 五维门控评分)
    ///
    /// 设置后,`route()` 在 Auto 策略下会用 `spawn_blocking` 包装
    /// `SqliteHistoryStore` 的同步 rusqlite 调用,避免阻塞 tokio runtime
    /// (§4.4 #2 反模式:rusqlite 必须 spawn_blocking)。
    /// `InMemoryHistoryStore` 无需 spawn_blocking(DashMap 纯内存,纳秒级),
    /// 但统一使用 spawn_blocking 可简化分支逻辑,且开销可忽略
    /// (一次 spawn_blocking 调度 ~1-5μs)。
    ///
    /// # 使用示例
    /// ```no_run
    /// use std::sync::Arc;
    /// use model_router::{ModelRouter, ModelRegistry, SqliteHistoryStore};
    /// use event_bus::EventBus;
    ///
    /// let bus = EventBus::new();
    /// let registry = ModelRegistry::new();
    /// let store = Arc::new(SqliteHistoryStore::new(std::path::Path::new("history.db")).unwrap());
    /// let router = ModelRouter::new(registry, bus).with_history_store(store);
    /// ```
    pub fn with_history_store(mut self, store: Arc<dyn HistoryStore>) -> Self {
        self.history_store = Some(store);
        self
    }

    /// P4-W16.1.1: 获取已注册 hook 数量(便于测试与诊断)
    pub fn hook_count(&self) -> usize {
        self.hooks.len()
    }

    /// 路由请求:按策略分发 → CACR 拦截 → 发布事件 → 触发 hook
    ///
    /// # 处理流程
    /// 1. 校验注册表非空(提前返回,避免策略函数重复检查)
    /// 2. 按 `request.strategy` 分发到对应策略,获得初始决策
    /// 3. 若启用 CACR,对初始决策进行成本拦截:
    ///    - `Allow`:放行原决策
    ///    - `Downgrade`:切换到次优模型(`candidates[0]`),重算成本
    ///    - `Block`:发布 `BudgetExceeded` 事件,返回 `BudgetExceeded` 错误
    /// 4. 发布 `ModelRouteSelected` 事件
    /// 5. **P4-W16.1.1**:构造 `TrajectoryEvent` 并依次调用 hooks(成功与错误路径均触发)
    ///
    /// # 错误处理
    /// - 注册表为空 → `RouterError::NoModelsRegistered`
    /// - CACR Block → `RouterError::BudgetExceeded`(同时发布事件)
    /// - 事件发布失败 → `RouterError::EventBusError`
    ///
    /// # P4-W16.1.1 hook 行为
    /// - hooks 在事件发布之后调用(确保事件已广播)
    /// - 错误路径也触发 hooks(便于追溯失败路由)
    /// - hooks 仅观察,不可修改决策(不可变借用契约)
    /// - 单个 hook panic 会传播(后续可考虑 catch_unwind 隔离)
    pub async fn route(&self, request: RoutingRequest) -> Result<RoutingDecision, RouterError> {
        // P4-W16.1.1: 计时起点 — 端到端延迟计量
        let start = Instant::now();

        // 1. 前置校验:注册表非空
        if self.registry.count() == 0 {
            let result = Err(RouterError::NoModelsRegistered);
            // P4-W16.1.1: 错误路径也触发 hook(便于追溯失败路由)
            self.emit_trajectory(&request, start, &result);
            return result;
        }

        // 2. 按策略分发,获得初始决策
        let mut decision = match request.strategy {
            RoutingStrategy::Lite => match strategies::route_lite(&self.registry, &request) {
                Ok(d) => d,
                Err(e) => {
                    let result = Err(e);
                    self.emit_trajectory(&request, start, &result);
                    return result;
                }
            },
            RoutingStrategy::Efficient => {
                match strategies::route_efficient(&self.registry, &request) {
                    Ok(d) => d,
                    Err(e) => {
                        let result = Err(e);
                        self.emit_trajectory(&request, start, &result);
                        return result;
                    }
                }
            }
            RoutingStrategy::Auto => {
                if let Some(ref store) = self.history_store {
                    // WHY spawn_blocking:SqliteHistoryStore 的 rusqlite 调用是同步阻塞的,
                    // 在 async 上下文中直接调用会阻塞 tokio runtime 工作线程(§4.4 #2 反模式)。
                    // spawn_blocking 将同步操作移到专用阻塞线程池,不阻塞 async 工作线程。
                    // 即使使用 InMemoryHistoryStore(DashMap 纯内存),统一走 spawn_blocking
                    // 可简化分支逻辑,开销可忽略(一次调度 ~1-5μs)。
                    let store = Arc::clone(store);
                    let registry = self.registry.clone();
                    let req = request.clone();
                    let gate = MoeGate::default();
                    match tokio::task::spawn_blocking(move || {
                        strategies::route_auto_with_gate(
                            &registry,
                            &req,
                            &gate,
                            Some(store.as_ref()),
                            req.thinking_pref,
                        )
                    })
                    .await
                    {
                        Ok(Ok(d)) => d,
                        Ok(Err(e)) => {
                            let result = Err(e);
                            self.emit_trajectory(&request, start, &result);
                            return result;
                        }
                        Err(join_err) => {
                            let result = Err(RouterError::SpawnBlockingError(format!(
                                "route_auto_with_gate join error: {}",
                                join_err
                            )));
                            self.emit_trajectory(&request, start, &result);
                            return result;
                        }
                    }
                } else {
                    match strategies::route_auto(&self.registry, &request) {
                        Ok(d) => d,
                        Err(e) => {
                            let result = Err(e);
                            self.emit_trajectory(&request, start, &result);
                            return result;
                        }
                    }
                }
            }
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
                    if let Err(e) = self.event_bus.publish(event).await {
                        let result: Result<RoutingDecision, RouterError> =
                            Err(RouterError::from(e));
                        self.emit_trajectory(&request, start, &result);
                        return result;
                    }

                    // WHY:reason 的详细信息(成本/预算/阈值)已通过 BudgetExceeded
                    // 事件的 current/limit 字段传递,此处返回错误时携带 cost/limit 供调用方决策
                    let result = Err(RouterError::BudgetExceeded {
                        cost: decision.estimated_cost,
                        limit: guard.budget_limit(),
                    });
                    // P4-W16.1.1: 错误路径触发 hook
                    self.emit_trajectory(&request, start, &result);
                    return result;
                }
            }
        }

        // 4. 发布 ModelRouteSelected 事件,供 Quest Engine 等订阅者消费
        let event = NexusEvent::ModelRouteSelected {
            metadata: EventMetadata::new(ROUTER_SOURCE),
            quest_id: request.quest_id.clone(),
            model_id: decision.model_id.clone(),
            route_reason: decision.route_reason.clone(),
        };
        let publish_result = self
            .event_bus
            .publish(event)
            .await
            .map_err(RouterError::from);

        // 5. P4-W16.1.1: 构造 TrajectoryEvent 并触发 hooks
        let result = match publish_result {
            Ok(()) => Ok(decision),
            Err(e) => Err(e),
        };
        self.emit_trajectory(&request, start, &result);

        result
    }

    /// P4-W16.1.1: 内部辅助 — 构造 TrajectoryEvent 并依次调用 hooks
    ///
    /// # 设计决策
    /// - 抽取为独立方法避免 route() 主体过长(符合 §6.1 单函数 ≤200 行)
    /// - 所有错误路径均调用此方法,确保 hook 全覆盖
    /// - hook 在已发布的 event 之后调用(事件优先,hook 观察次要)
    /// - 即使 hooks 为空也调用(零成本抽象,Vec::iter 空迭代开销 ~0ns)
    fn emit_trajectory(
        &self,
        request: &RoutingRequest,
        start: Instant,
        result: &Result<RoutingDecision, RouterError>,
    ) {
        if self.hooks.is_empty() {
            return;
        }

        let event = TrajectoryEvent {
            quest_id: request.quest_id.clone(),
            strategy: request.strategy,
            estimated_tokens: request.estimated_tokens,
            latency: start.elapsed(),
            outcome: TrajectoryOutcome::from_result(result),
        };

        for hook in &self.hooks {
            hook.on_route_completed(event.clone());
        }
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
    use nexus_contracts::affinity::ThinkingPreference;
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
            thinking_pref: ThinkingPreference::Standard,
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

    // ============================================================
    // P4-W16.1.1: RouteHook trait 集成测试(单元层)
    // ============================================================

    /// 测试用 hook — 计数 on_route_completed 调用次数
    #[derive(Debug, Default)]
    struct CountingHook {
        count: std::sync::Arc<std::sync::Mutex<usize>>,
    }

    impl CountingHook {
        fn new() -> Self {
            Self::default()
        }

        fn get(&self) -> usize {
            *self.count.lock().expect("count mutex poisoned")
        }
    }

    impl RouteHook for CountingHook {
        fn on_route_completed(&self, _event: TrajectoryEvent) {
            *self.count.lock().expect("count mutex poisoned") += 1;
        }
    }

    #[test]
    fn test_new_router_has_zero_hooks() {
        let (router, _bus) = make_router();
        assert_eq!(router.hook_count(), 0);
    }

    #[tokio::test]
    async fn test_hook_triggered_on_success() {
        let bus = EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        let hook = std::sync::Arc::new(CountingHook::new());
        let router = ModelRouter::with_hook(registry, bus, hook.clone());

        let _ = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .unwrap();

        assert_eq!(hook.get(), 1, "成功路径必须触发 1 次 hook");
    }

    #[tokio::test]
    async fn test_hook_triggered_on_error() {
        let bus = EventBus::new();
        let registry = ModelRegistry::new(); // 空注册表
        let hook = std::sync::Arc::new(CountingHook::new());
        let router = ModelRouter::with_hook(registry, bus, hook.clone());

        let _ = router.route(make_request(RoutingStrategy::Lite)).await;
        // 空注册表错误路径也触发 hook
        assert_eq!(hook.get(), 1, "错误路径必须触发 1 次 hook");
    }

    #[tokio::test]
    async fn test_hook_triggered_on_cacr_block() {
        let bus = EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        let cacr_config = CacrConfig {
            budget_limit: 0, // 预算为 0,任何 cost > 0 都会 Block
            ..Default::default()
        };
        let hook = std::sync::Arc::new(CountingHook::new());
        let router = ModelRouter::with_cacr_and_hook(registry, bus, cacr_config, hook.clone());

        let _ = router.route(make_request(RoutingStrategy::Lite)).await;
        assert_eq!(hook.get(), 1, "CACR Block 错误路径必须触发 hook");
    }

    #[test]
    fn test_add_hook_appends_to_existing() {
        let bus = EventBus::new();
        let registry = ModelRegistry::from_config(&RouterConfig::default());
        let hook1 = std::sync::Arc::new(CountingHook::new());
        let mut router = ModelRouter::with_hook(registry, bus, hook1.clone());
        assert_eq!(router.hook_count(), 1);

        let hook2 = std::sync::Arc::new(CountingHook::new());
        router.add_hook(hook2.clone() as std::sync::Arc<dyn RouteHook>);
        assert_eq!(router.hook_count(), 2);
    }
}
