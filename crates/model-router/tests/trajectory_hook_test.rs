//! P4-W16.1.1 + P4-W16.1.2: model-router RouteHook trait 契约 + 生产级 RecordingHook 集成测试
//!
//! 对应架构:L1 Core(model-router)
//! 对应 spec.md §Scenario "model-router 轨迹捕获"
//!
//! # 测试目标
//! 验证 `ModelRouter::route()` 边界已扩展 `RouteHook` trait,允许上层注入
//! 轨迹捕获逻辑(请求/响应/延迟/token 成本/路由决策),并满足:
//! 1. **向后兼容**:未配置 hook 时行为与既有完全一致
//! 2. **依赖倒置**:hook trait 在 L1 model-router 定义,上层(L9/L10)实现
//! 3. **Send+Sync**:hook 可在 async 任务间共享
//! 4. **不可变借用**:hook 仅观察请求/响应,不可修改路由决策
//! 5. **延迟计量**:hook 接收 `Duration` 形式的真实延迟
//! 6. **错误路径覆盖**:成功与失败路径均触发 after_route
//! 7. **P4-W16.1.2**: 生产级 `RecordingHook` 集成验证(捕获点 1 端到端)
//!
//! # 设计来源
//! spec.md:434 "**model-router 轨迹捕获**:在所有模型调用必经边界
//! (实际入口 `Router::route()`)扩展 hook trait,捕获请求/响应/延迟/token 成本/路由决策"
//!
//! # 实施次序
//! 1. P4-W16.1.1 本文件(RED):定义 RouteHook/TrajectoryEvent API 契约
//! 2. P4-W16.1.1 GREEN:实现 trajectory 模块 + ModelRouter 集成
//! 3. P4-W16.1.2 捕获点 1:具体实现 RecordingHook(请求/响应/延迟/token 成本/路由决策)
//! 4. P4-W16.1.3 捕获点 2:quest-engine Checkpoint → 轨迹导出器四元组

#![forbid(unsafe_code)]

use model_router::{
    ModelRegistry, ModelRouter, RecordingHook, RouteHook, RouterConfig, RouterError,
    RoutingRequest, RoutingStrategy, TrajectoryEvent, TrajectoryOutcome, TrajectoryStats,
};
use nexus_core::{MultimodalInput, UserIntent};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ============================================================
// 测试夹具
// ============================================================

fn make_intent() -> UserIntent {
    UserIntent {
        intent_id: "i-traj".into(),
        raw_text: "trajectory-test".into(),
        multimodal_inputs: vec![MultimodalInput::Text("trajectory-test".into())],
        risk_level: 10,
    }
}

fn make_request(strategy: RoutingStrategy) -> RoutingRequest {
    RoutingRequest {
        quest_id: "q-traj".into(),
        intent: make_intent(),
        estimated_tokens: 1000,
        strategy,
    }
}

fn make_router() -> (ModelRouter, event_bus::EventBus) {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = ModelRouter::new(registry, bus.clone());
    (router, bus)
}

// ============================================================
// 测试用的轻量记录型 hook — 收集 TrajectoryEvent 供断言
// ============================================================

/// 测试用 hook — 记录所有触发的 TrajectoryEvent
/// P4-W16.1.2 起重命名为 TestRecordingHook,避免与生产级 RecordingHook 冲突
#[derive(Debug, Default)]
struct TestRecordingHook {
    events: Arc<Mutex<Vec<TrajectoryEvent>>>,
}

impl TestRecordingHook {
    fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self) -> Vec<TrajectoryEvent> {
        self.events.lock().expect("events mutex poisoned").clone()
    }

    fn event_count(&self) -> usize {
        self.events.lock().expect("events mutex poisoned").len()
    }
}

impl RouteHook for TestRecordingHook {
    fn on_route_completed(&self, event: TrajectoryEvent) {
        self.events
            .lock()
            .expect("events mutex poisoned")
            .push(event);
    }
}

// ============================================================
// 测试 1: 未配置 hook 时向后兼容(行为与 Task 6 一致)
// ============================================================

#[tokio::test]
async fn test_no_hook_backward_compatible() {
    // 未通过 with_hook 注入 hook 时,行为必须与既有完全一致
    let (router, _bus) = make_router();

    let decision = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("无 hook 时路由必须成功");

    assert_eq!(decision.model_id, "lite-model");
    // route_reason 不应包含任何 hook 相关标识
    assert!(!decision.route_reason.contains("hook"));
}

// ============================================================
// 测试 2: with_hook builder 注入 hook
// ============================================================

#[tokio::test]
async fn test_with_hook_injects_hook() {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus.clone(), hook.clone());

    // 路由后 hook 必须收到一个 TrajectoryEvent
    let _ = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    assert_eq!(hook.event_count(), 1, "hook 必须收到 1 个事件");
}

// ============================================================
// 测试 3: TrajectoryEvent 含完整字段(quest_id/latency/decision)
// ============================================================

#[tokio::test]
async fn test_trajectory_event_contains_full_fields() {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    let _ = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    let events = hook.snapshot();
    assert_eq!(events.len(), 1, "应恰好收到 1 个事件");

    let event = &events[0];
    assert_eq!(event.quest_id, "q-traj", "quest_id 必须匹配请求");
    assert_eq!(event.strategy, RoutingStrategy::Lite, "策略必须匹配");
    assert_eq!(event.estimated_tokens, 1000, "estimated_tokens 必须匹配");
    assert!(event.latency > Duration::ZERO, "延迟必须为正值");
    assert!(
        matches!(event.outcome, TrajectoryOutcome::Success { .. }),
        "成功路径 outcome 必须为 Success"
    );

    // 验证 decision snapshot 字段
    if let TrajectoryOutcome::Success {
        ref model_id,
        ref route_reason,
        estimated_cost,
        ref candidates,
    } = event.outcome
    {
        assert_eq!(model_id, "lite-model", "model_id 必须匹配决策");
        assert!(!route_reason.is_empty(), "route_reason 不能为空");
        // lite-model 1000 token × 0.001/千 = 0.1 美分,round 后为 0;此处不限制具体值,
        // 仅校验字段可读(estimated_cost 为 u64,可序列化到回放池)
        let _ = estimated_cost;
        assert!(!candidates.is_empty(), "candidates 不能为空");
    }
}

// ============================================================
// 测试 4: 错误路径也触发 hook(CACR Block 场景)
// ============================================================

#[tokio::test]
async fn test_error_path_triggers_hook() {
    // 设置预算为 0 的 CACR,触发 Block 错误路径
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let cacr_config = model_router::CacrConfig {
        budget_limit: 0, // 预算为 0,任何 cost > 0 都会 Block
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let hook = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_cacr_and_hook(registry, bus, cacr_config, hook.clone());

    let result = router.route(make_request(RoutingStrategy::Lite)).await;
    assert!(result.is_err(), "应触发 Block 错误");

    assert_eq!(hook.event_count(), 1, "错误路径也必须触发 hook");

    let events = hook.snapshot();
    let event = &events[0];
    assert!(
        matches!(event.outcome, TrajectoryOutcome::Error { .. }),
        "错误路径 outcome 必须为 Error"
    );

    if let TrajectoryOutcome::Error { ref error_kind } = event.outcome {
        assert!(
            error_kind.contains("BudgetExceeded") || error_kind.contains("budget"),
            "error_kind 应包含 BudgetExceeded 标识: {}",
            error_kind
        );
    }
}

// ============================================================
// 测试 5: 多 hook 全部触发(链式调用)
// ============================================================

#[tokio::test]
async fn test_multiple_hooks_all_triggered() {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook1 = Arc::new(TestRecordingHook::new());
    let hook2 = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_hooks(
        registry,
        bus,
        vec![
            hook1.clone() as Arc<dyn RouteHook>,
            hook2.clone() as Arc<dyn RouteHook>,
        ],
    );

    let _ = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    assert_eq!(hook1.event_count(), 1, "hook1 必须收到事件");
    assert_eq!(hook2.event_count(), 1, "hook2 必须收到事件");
}

// ============================================================
// 测试 6: hook 不可修改路由决策(不可变借用契约)
// ============================================================

#[tokio::test]
async fn test_hook_cannot_modify_decision() {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook);

    let decision = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    // 决策必须与既有行为一致 — hook 不能改变 model_id
    assert_eq!(decision.model_id, "lite-model");
}

// ============================================================
// 测试 7: Send + Sync 静态断言(hook 可在 async 任务间共享)
// ============================================================

#[test]
fn test_route_hook_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn RouteHook>>();
    // 验证生产级 RecordingHook 与测试用 TestRecordingHook 均 Send+Sync
    assert_send_sync::<RecordingHook>();
    assert_send_sync::<TestRecordingHook>();
    assert_send_sync::<TrajectoryEvent>();
    assert_send_sync::<TrajectoryStats>();
}

// ============================================================
// 测试 8: 空注册表错误路径也触发 hook
// ============================================================

#[tokio::test]
async fn test_empty_registry_triggers_hook() {
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::new(); // 空注册表
    let hook = Arc::new(TestRecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    let result = router.route(make_request(RoutingStrategy::Lite)).await;
    assert!(
        matches!(result, Err(RouterError::NoModelsRegistered)),
        "空注册表必须返回 NoModelsRegistered"
    );

    // 空注册表错误也应触发 hook(便于追溯失败路由)
    assert_eq!(hook.event_count(), 1, "空注册表错误也必须触发 hook");

    let events = hook.snapshot();
    if let TrajectoryOutcome::Error { ref error_kind } = events[0].outcome {
        assert!(
            error_kind.contains("NoModelsRegistered") || error_kind.contains("no models"),
            "error_kind 应包含 NoModelsRegistered 标识: {}",
            error_kind
        );
    } else {
        panic!("空注册表 outcome 必须为 Error");
    }
}

// ============================================================
// 测试 9: TrajectoryEvent 序列化往返(为 P4-W16.2 回放池做准备)
// ============================================================

#[test]
fn test_trajectory_event_serde_roundtrip() {
    use serde_json;

    let event = TrajectoryEvent {
        quest_id: "q-serde".into(),
        strategy: RoutingStrategy::Auto,
        estimated_tokens: 500,
        latency: Duration::from_millis(42),
        outcome: TrajectoryOutcome::Success {
            model_id: "auto-model".into(),
            route_reason: "test".into(),
            estimated_cost: 100,
            candidates: vec!["alt-model".into()],
        },
    };

    let json = serde_json::to_string(&event).expect("序列化必须成功");
    let de: TrajectoryEvent = serde_json::from_str(&json).expect("反序列化必须成功");

    assert_eq!(de.quest_id, event.quest_id);
    assert_eq!(de.strategy, event.strategy);
    assert_eq!(de.estimated_tokens, event.estimated_tokens);
    assert_eq!(de.latency, event.latency);
    assert!(matches!(de.outcome, TrajectoryOutcome::Success { .. }));
}

// ============================================================
// P4-W16.1.2 测试 10-15: 生产级 RecordingHook 集成测试
// ============================================================

#[tokio::test]
async fn test_production_recording_hook_end_to_end() {
    // 端到端验证:ModelRouter + 生产级 RecordingHook
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(RecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    let _ = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    // 验证 hook 接收了事件
    assert_eq!(hook.total_events(), 1, "total_events 应为 1");
    assert_eq!(hook.success_count(), 1, "成功路径 success_count 应为 1");
    assert_eq!(hook.error_count(), 0);
    assert_eq!(hook.evicted_count(), 0);
    assert_eq!(hook.len(), 1, "缓冲区应有 1 个事件");

    // 验证 drain() 取出事件
    let events = hook.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].quest_id, "q-traj");

    // drain 后缓冲区清空,但计数器保留
    assert!(hook.is_empty());
    assert_eq!(hook.total_events(), 1, "计数器应保留");
}

#[tokio::test]
async fn test_production_recording_hook_error_path_stats() {
    // 错误路径统计:空注册表触发 NoModelsRegistered
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::new(); // 空注册表
    let hook = Arc::new(RecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    let _ = router.route(make_request(RoutingStrategy::Lite)).await;

    assert_eq!(hook.total_events(), 1);
    assert_eq!(hook.success_count(), 0);
    assert_eq!(hook.error_count(), 1, "错误路径 error_count 应为 1");

    let stats = hook.stats();
    assert_eq!(stats.total_events, 1);
    assert_eq!(stats.error_count, 1);
    assert_eq!(stats.buffer_len, 1);
    // 错误率 = 1/1 = 1.0
    assert!(
        (stats.error_rate() - 1.0).abs() < f64::EPSILON,
        "全错误路径错误率应为 1.0"
    );
}

#[tokio::test]
async fn test_production_recording_hook_mixed_routes() {
    // 混合路由场景:多次成功 + 多次错误
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(RecordingHook::new());
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    // 3 次成功路由
    for _ in 0..3 {
        let _ = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .expect("路由成功");
    }

    // 1 次错误路由(空注册表的 router,需要新建)
    let empty_bus = event_bus::EventBus::new();
    let empty_registry = ModelRegistry::new();
    let err_hook = Arc::new(RecordingHook::new());
    let err_router = ModelRouter::with_hook(empty_registry, empty_bus, err_hook.clone());
    let _ = err_router.route(make_request(RoutingStrategy::Lite)).await;

    // 验证成功 hook 统计
    assert_eq!(hook.total_events(), 3);
    assert_eq!(hook.success_count(), 3);
    assert_eq!(hook.error_count(), 0);

    // 验证错误 hook 统计
    assert_eq!(err_hook.total_events(), 1);
    assert_eq!(err_hook.success_count(), 0);
    assert_eq!(err_hook.error_count(), 1);
}

#[tokio::test]
async fn test_production_recording_hook_drain_for_replay_pool() {
    // 验证 drain() 接口适合 P4-W16.2 经验回放池消费
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(RecordingHook::with_capacity(100));
    let router = ModelRouter::with_hook(registry, bus, hook.clone());

    // 写入 50 个事件
    for _ in 0..50 {
        let _ = router
            .route(make_request(RoutingStrategy::Auto))
            .await
            .expect("路由成功");
    }

    assert_eq!(hook.len(), 50);

    // 第一次 drain — 取出全部 50 个事件
    let batch1 = hook.drain();
    assert_eq!(batch1.len(), 50, "第一次 drain 应取 50 个");
    assert!(hook.is_empty(), "drain 后应清空");

    // 继续写入 30 个事件
    for _ in 0..30 {
        let _ = router
            .route(make_request(RoutingStrategy::Lite))
            .await
            .expect("路由成功");
    }

    // 第二次 drain — 只取新写入的 30 个
    let batch2 = hook.drain();
    assert_eq!(batch2.len(), 30, "第二次 drain 应取 30 个(增量)");

    // 累计统计(不因 drain 重置)
    assert_eq!(hook.total_events(), 80, "total_events 应累计 80");
    assert_eq!(hook.success_count(), 80);
}

#[tokio::test]
async fn test_production_recording_hook_concurrent_routes() {
    // 并发路由场景:多任务同时调用 route,验证 RecordingHook 线程安全
    use std::sync::Arc;
    use tokio::task::JoinSet;

    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let hook = Arc::new(RecordingHook::with_capacity(500));
    // WHY Arc::clone(&hook) 而非 hook.clone():async 任务需共享 mutate 状态
    // §4.4 反模式 5 — Arc::clone 增加引用计数,clone 会创建独立副本
    let router = Arc::new(ModelRouter::with_hook(registry, bus, hook.clone()));

    let mut tasks = JoinSet::new();
    for _ in 0..10 {
        let router_clone = Arc::clone(&router);
        tasks.spawn(async move {
            for _ in 0..10 {
                let _ = router_clone
                    .route(make_request(RoutingStrategy::Auto))
                    .await
                    .expect("路由成功");
            }
        });
    }

    while tasks.join_next().await.is_some() {}

    // 10 任务 × 10 路由 = 100 事件
    assert_eq!(hook.total_events(), 100, "应累计 100 个事件");
    assert_eq!(hook.success_count(), 100);
    assert_eq!(hook.len(), 100, "未超容量,缓冲区应有 100 个");
    assert_eq!(hook.evicted_count(), 0, "未超容量,不应淘汰");
}

#[tokio::test]
async fn test_production_recording_hook_with_cacr_and_drain() {
    // 验证 with_cacr_and_hook 构造器 + drain 接口
    let bus = event_bus::EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let cacr_config = model_router::CacrConfig {
        budget_limit: 1_000_000, // 充足预算,允许路由
        warn_threshold: 0.8,
        block_threshold: 1.0,
    };
    let hook = Arc::new(RecordingHook::new());
    let router = ModelRouter::with_cacr_and_hook(registry, bus, cacr_config, hook.clone());

    let _ = router
        .route(make_request(RoutingStrategy::Lite))
        .await
        .expect("路由成功");

    let events = hook.drain();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].outcome,
        TrajectoryOutcome::Success { .. }
    ));
}
