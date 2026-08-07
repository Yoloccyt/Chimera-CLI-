//! S9 会话集成 — 将 StreamSessionCompleted 事件桥接到 S9RouteLearner
//!
//! 对应架构层: L6 Router(omega-learner)
//! 对应 ADR: ADR-065(MCA M3), ADR-068
//!
//! # 设计动机
//!
//! mca-gateway 完成一次流式会话后发布 `StreamSessionCompleted` 事件,
//! 包含实际成本(cost_actual_micro)、首 token 延迟(ttft_ms)、token 用量等计量。
//! `S9SessionIntegration` 订阅此事件流,将计量转换为 `S9Reward`,
//! 驱动 `S9RouteLearner` 的 LinUCB 臂权重更新。
//!
//! # 依赖方向(§2.2 铁律)
//! omega-learner(L6) 不依赖 mca-gateway(L10)。本模块仅消费事件总线
//! 上已发布的 `NexusEvent`,事件中的 `cost_actual_micro`/`ttft_ms` 等字段
//! 由 event-bus(L1) 的 `StreamSessionCompleted` 变体承载。
//!
//! # 线程安全
//! S9RouteLearner 内部使用 `Arc<Mutex<>>` 保护,`observe` 是同步操作。
//! 调用方在 async 事件订阅任务中调用,锁内不跨 await(C7 红线:锁内取快照→
//! 释放锁→await 快照,本模块锁内仅调用同步 observe,无 await)。

use std::sync::{Arc, Mutex};

use event_bus::{EventBus, NexusEvent};
use tracing::warn;

use crate::s9_route::{S9Context, S9Reward, S9RouteLearner};

/// S9 会话集成 — 将 StreamSessionCompleted 事件桥接到 S9RouteLearner
///
/// 启动后台任务订阅 `StreamSessionCompleted` 事件,解析 `route_key` 为 arm_id
/// 并调用 `learner.observe()` 更新 LinUCB 模型。
///
/// # 默认上下文
/// `StreamSessionCompleted` 事件不携带任务复杂度/预算水位/延迟敏感度等
/// S9Context 字段,集成使用默认值 0.5(中等)。实际生产环境中,调用方可
/// 在构造时提供上下文提取器或通过 `S9Context` 默认值推导。
///
/// 臂 ID 构造
/// 事件中的 `route_key` 格式为 `provider/model`(如 `zhipu/glm-5.2`),
/// 而 arm_id 格式为 `provider/model/mode`(如 `zhipu/glm-5.2/standard`)。
/// 由于事件不携带思考模式(thinking_mode),集成默认追加 `standard` 档。
pub struct S9SessionIntegration {
    /// S9 路由学习器(Arc<Mutex<>> 保护,锁内不跨 await)
    learner: Arc<Mutex<S9RouteLearner>>,
    /// 事件总线(订阅 StreamSessionCompleted)
    event_bus: EventBus,
    /// 预期最大成本(微元),用于归一化 normalized_cost ∈ [0,1]
    max_expected_cost: u64,
    /// 预期最大 TTFT(毫秒),用于归一化 normalized_latency ∈ [0,1]
    max_expected_ttft: u64,
}

impl S9SessionIntegration {
    /// 创建 S9 会话集成
    ///
    /// # 参数
    /// - `learner`: S9 路由学习器(Arc<Mutex<>> 包装,跨线程共享)
    /// - `event_bus`: 事件总线(用于订阅 StreamSessionCompleted)
    /// - `max_expected_cost`: 预期最大成本(微元),用于归一化 cost
    /// - `max_expected_ttft`: 预期最大 TTFT(毫秒),用于归一化延迟
    pub fn new(
        learner: Arc<Mutex<S9RouteLearner>>,
        event_bus: EventBus,
        max_expected_cost: u64,
        max_expected_ttft: u64,
    ) -> Self {
        Self {
            learner,
            event_bus,
            max_expected_cost,
            max_expected_ttft,
        }
    }

    /// 启动后台事件订阅任务
    ///
    /// 订阅 `StreamSessionCompleted` 事件,解析 arm_id 并调用 `learner.observe()`
    /// 更新 LinUCB 模型。后台任务在 `tokio::spawn` 中运行,调用方需确保
    /// tokio runtime 已初始化。
    ///
    /// # 线程安全(§4.4 反模式 3 + C7 红线)
    /// - `subscribe()` 在 `tokio::spawn()` **之前同步调用**(反模式 3),
    ///   确保不会错过后续发布的 StreamSessionCompleted 事件。
    /// - 锁内不跨 await(C7 红线):`lock()` → `observe()` → 隐式 drop,
    ///   整个临界区纯同步,无 `.await` 点。
    pub fn start(self) {
        // §4.4 反模式 3:subscribe 必须在 spawn 之前同步调用
        let mut rx = self.event_bus.subscribe();
        let learner = self.learner.clone();
        let max_cost = self.max_expected_cost.max(1);
        let max_ttft = self.max_expected_ttft.max(1);

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let NexusEvent::StreamSessionCompleted {
                            route_key,
                            cost_actual_micro,
                            ttft_ms,
                            ..
                        } = event
                        {
                            // 构造 arm_id: route_key 为 provider/model,补齐 standard 思考模式
                            // WHY standard 默认:事件不携带 thinking_mode,统一用 standard 作为
                            // 保守默认。实际生产中,调用方应提供 thinking_mode 提取逻辑。
                            let arm_id = format!("{route_key}/standard");

                            // 默认上下文(事件不携带 S9Context 字段,使用中等值)
                            let ctx = S9Context {
                                task_complexity: 0.5,
                                budget_water_level: 0.5,
                                latency_sensitivity: 0.5,
                                cache_hit_history: 0.5,
                                risk_level: 0.2,
                            };

                            // 构造奖励信号
                            let reward = S9Reward {
                                success: true,
                                quality: 0.8,
                                normalized_cost: (cost_actual_micro as f32) / (max_cost as f32),
                                normalized_latency: (ttft_ms as f32) / (max_ttft as f32),
                            };

                            // C7 红线合规:锁内不跨 await
                            // lock → observe → 隐式 drop,纯同步路径
                            match learner.lock() {
                                Ok(mut guard) => {
                                    if let Err(e) = guard.observe(&arm_id, ctx, reward) {
                                        warn!(%arm_id, error = %e,
                                            "S9SessionIntegration: observe failed");
                                    }
                                }
                                Err(poisoned) => {
                                    // 中毒锁降级访问(§4.4 反模式 8 一致)
                                    let mut guard = poisoned.into_inner();
                                    if let Err(e) = guard.observe(&arm_id, ctx, reward) {
                                        warn!(%arm_id, error = %e,
                                            "S9SessionIntegration: observe failed (poisoned lock)");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // broadcast::RecvError::Lagged 或 Closed
                        // Lagged:慢消费者丢弃了事件,仅记日志不 panic
                        // Closed:总线关闭,任务正常退出
                        warn!(error = %e, "S9SessionIntegration: recv error, stopping");
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;
    use std::sync::Mutex;

    /// 构造样本臂集(8 provider-model × 3 mode = 24 臂)
    fn sample_arms() -> Vec<String> {
        let providers_models = [
            ("zhipu", "glm-5.2"),
            ("deep_seek", "deepseek-v4-flash"),
            ("moonshot", "kimi-k3"),
            ("mini_max", "MiniMax-M3"),
            ("volcano_ark", "doubao-seed-2.1-pro"),
            ("alibaba_cloud", "qwen-max"),
            ("step_fun", "step-3.5-flash-2603"),
        ];
        let modes = ["fast", "standard", "deep"];
        let mut arms = Vec::new();
        for (p, m) in providers_models {
            for mode in modes {
                arms.push(format!("{p}/{m}/{mode}"));
            }
        }
        arms
    }

    #[tokio::test]
    async fn test_integration_publishes_event_and_observes() {
        let arms = sample_arms();
        let learner = Arc::new(Mutex::new(S9RouteLearner::new(&arms, 1.0).unwrap()));
        let bus = EventBus::new();

        let integration = S9SessionIntegration::new(learner.clone(), bus.clone(), 100_000, 5000);
        // 记录初始步数
        let initial_steps = {
            let guard = learner.lock().unwrap();
            guard.total_steps()
        };
        assert_eq!(initial_steps, 0);

        // 启动后台订阅(在 publish 之前 subscribe,符合反模式 3)
        integration.start();

        // 发布 StreamSessionCompleted 事件
        let event = NexusEvent::StreamSessionCompleted {
            metadata: EventMetadata::new("test"),
            intent_id: "test-intent".into(),
            route_key: "zhipu/glm-5.2".into(),
            input_tokens: 100,
            output_tokens: 50,
            cache_hit_tokens: 20,
            cost_actual_micro: 500,
            ttft_ms: 200,
            semantic_cache_hit: false,
            trimmed_before_tokens: None,
            trimmed_after_tokens: None,
            compressed_ratio: None,
            early_stop_reason: None,
            coalesced: false,
        };
        bus.publish(event).await.unwrap();

        // 等待事件被消费(异步,给后台任务一点时间)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 验证 learner 步数递增
        let steps = {
            let guard = learner.lock().unwrap();
            guard.total_steps()
        };
        assert_eq!(steps, 1, "integration must observe one event");
    }

    #[tokio::test]
    async fn test_integration_ignores_non_stream_session_events() {
        let arms = sample_arms();
        let learner = Arc::new(Mutex::new(S9RouteLearner::new(&arms, 1.0).unwrap()));
        let bus = EventBus::new();

        let integration = S9SessionIntegration::new(learner.clone(), bus.clone(), 100_000, 5000);
        integration.start();

        // 发布一个非 StreamSessionCompleted 事件
        bus.publish(NexusEvent::ModelRouteSelected {
            metadata: EventMetadata::new("test"),
            quest_id: "test-quest".into(),
            model_id: "glm-5.2".into(),
            route_reason: "default".into(),
        })
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let steps = {
            let guard = learner.lock().unwrap();
            guard.total_steps()
        };
        assert_eq!(
            steps, 0,
            "non-StreamSessionCompleted events must be ignored"
        );
    }

    #[tokio::test]
    async fn test_integration_handles_multiple_events() {
        let arms = sample_arms();
        let learner = Arc::new(Mutex::new(S9RouteLearner::new(&arms, 1.0).unwrap()));
        let bus = EventBus::new();

        let integration = S9SessionIntegration::new(learner.clone(), bus.clone(), 100_000, 5000);
        integration.start();

        // 发布 3 个事件
        for i in 0..3 {
            let event = NexusEvent::StreamSessionCompleted {
                metadata: EventMetadata::new("test"),
                intent_id: format!("intent-{i}"),
                route_key: "zhipu/glm-5.2".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cache_hit_tokens: 20,
                cost_actual_micro: 500 + i as u64 * 100,
                ttft_ms: 200 + i as u64 * 50,
                semantic_cache_hit: false,
                trimmed_before_tokens: None,
                trimmed_after_tokens: None,
                compressed_ratio: None,
                early_stop_reason: None,
                coalesced: false,
            };
            bus.publish(event).await.unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        let steps = {
            let guard = learner.lock().unwrap();
            guard.total_steps()
        };
        assert_eq!(steps, 3, "must observe all 3 events");
    }

    #[tokio::test]
    async fn test_integration_zero_cost_does_not_panic() {
        // 边界情况:cost_actual_micro = 0, ttft_ms = 0
        // 不应导致除零 panic(max_expected_cost 已 max(1) 保护)
        let arms = sample_arms();
        let learner = Arc::new(Mutex::new(S9RouteLearner::new(&arms, 1.0).unwrap()));
        let bus = EventBus::new();

        let integration = S9SessionIntegration::new(learner.clone(), bus.clone(), 0, 0);
        integration.start();

        let event = NexusEvent::StreamSessionCompleted {
            metadata: EventMetadata::new("test"),
            intent_id: "test".into(),
            route_key: "zhipu/glm-5.2".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_hit_tokens: 0,
            cost_actual_micro: 0,
            ttft_ms: 0,
            semantic_cache_hit: false,
            trimmed_before_tokens: None,
            trimmed_after_tokens: None,
            compressed_ratio: None,
            early_stop_reason: None,
            coalesced: false,
        };
        bus.publish(event).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let steps = {
            let guard = learner.lock().unwrap();
            guard.total_steps()
        };
        assert_eq!(steps, 1, "zero cost must not panic");
    }
}
