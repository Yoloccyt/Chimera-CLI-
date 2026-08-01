//! hcw_integration — MCA P5 窗口亲和映射的 HCW 集成模块
//!
//! 对应架构层:L10 mca-gateway → L2 hcw-window(经 event-bus 跨层通信,C6)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.2
//!
//! # 职责
//! 消费 `ModelAffinitySelected` 事件,提取 `route_key` 并查询 spec 的
//! `context_window` 字段,调用 `WindowAffinity::fold()` 确定折减后的窗口档位,
//! 发布 `WindowAffinityApplied` 事件供 HCW 消费。
//!
//! # 跨层通信(C6)
//! L10 → L2 跨层不直接依赖运行时状态,全部经过 event-bus:
//! - 订阅: `ModelAffinitySelected`(L10 mca-gateway 发布)
//! - 发布: `WindowAffinityApplied`(L2 hcw-window 消费)
//!
//! WHY 直接依赖 `hcw-window` crate:WindowAffinity::fold() 是纯同步 O(1) 查表,
//! 不涉及 HCW 运行时状态。纯函数可直接调用,无需经 event-bus 转发。
//!
//! # 性能
//! `WindowAffinity::fold()` 是 O(1) 查表,不进任何热路径分配。
//! 本模块仅在事件到达时执行,不在请求热路径上。

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::McaGateway;

/// 事件源标识
const EVENT_SOURCE: &str = "mca-gateway";

/// 启动 HCW 窗口亲和映射监听任务
///
/// 订阅 `ModelAffinitySelected` 事件,提取 spec 的 `context_window` 字段,
/// 调用 `WindowAffinity::fold()` 确定折减后的窗口档位,发布
/// `WindowAffinityApplied` 事件。
///
/// # 参数
/// - `gateway`: MCA 网关(用于 lookup_spec)
/// - `bus`: 事件总线
///
/// # 返回
/// `JoinHandle`,调用方可用于等待或取消监听
///
/// # 锁纪律(C7)
/// - 本模块不持有任何锁跨 `.await`
/// - `gateway.lookup_spec` 是 ArcSwap 读操作(<5ns,同步,不 await)
/// - `bus.publish` 是异步操作,调用前已释放所有临时变量
/// - `WindowAffinity::fold` 是纯同步函数,零锁
pub fn spawn_hcw_integration(gateway: McaGateway, bus: EventBus) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let NexusEvent::ModelAffinitySelected { ref route_key, .. } = event {
                        // 1. 查 spec 获取 context_window(ArcSwap 读,<5ns,同步)
                        let context_window = match gateway.lookup_spec(route_key) {
                            Some(spec) => spec.capabilities.context_window,
                            None => {
                                tracing::warn!(
                                    route_key = %route_key,
                                    "hcw_integration: spec not found for route_key"
                                );
                                continue;
                            }
                        };

                        // 2. 调用 WindowAffinity::fold() 确定折减档位(纯同步,O(1)查表)
                        // WHY 使用 L3 作为请求档位:路由选择该通道说明意图需要最高上下文
                        // 实际折减由模型上限决定,请求档位取 L3 保守触发折减。
                        let result = hcw_window::WindowAffinity::fold(
                            hcw_window::WindowTier::L3,
                            context_window,
                        );

                        tracing::debug!(
                            route_key = %route_key,
                            context_window,
                            tier = %result.tier.as_str(),
                            folded = result.folded,
                            needs_chunking = result.needs_chunking,
                            "hcw_integration: window affinity mapped"
                        );

                        // 3. 发布 WindowAffinityApplied 事件(已在 event-bus 注册)
                        let event = NexusEvent::WindowAffinityApplied {
                            metadata: EventMetadata::new(EVENT_SOURCE),
                            route_key: route_key.clone(),
                            folded: result.folded,
                            needs_chunking: result.needs_chunking,
                            tier: result.tier.as_str().to_string(),
                        };
                        // 发布失败不中断监听(观测面事件)
                        let _ = bus.publish(event).await;
                    }
                }
                Err(event_bus::EventBusError::ChannelClosed) => {
                    tracing::info!("hcw_integration: EventBus channel closed, stopping");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "hcw_integration: EventBus recv error, continuing");
                    continue;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{McaGateway, McaGatewayConfig};
    use nexus_contracts::affinity::{ModelAffinitySpec, ProtocolDialect, ProviderId};
    use std::time::Duration;

    /// 创建测试用 spec(指定 context_window)
    fn spec_with_window(
        provider: ProviderId,
        model: &str,
        context_window: u32,
    ) -> ModelAffinitySpec {
        let mut s = ModelAffinitySpec::minimal(provider, model, ProtocolDialect::OpenAiChat);
        s.capabilities.context_window = context_window;
        s
    }

    #[tokio::test]
    async fn test_hcw_integration_receives_event_and_publishes_result() {
        let bus = EventBus::new();
        let gw = McaGateway::new(McaGatewayConfig::default());

        // 注册 spec(context_window=256K → Step 类模型)
        gw.register_spec(spec_with_window(
            ProviderId::StepFun,
            "step-3.5-flash-2603",
            262_144,
        ));

        // 启动 HCW 集成监听
        let _handle = spawn_hcw_integration(gw.clone(), bus.clone());

        // 订阅 WindowAffinityApplied 事件
        let mut rx = bus.subscribe();

        // 发布 ModelAffinitySelected 事件(spec 已注册)
        bus.publish(NexusEvent::ModelAffinitySelected {
            metadata: EventMetadata::new("test"),
            intent_id: "test-intent".into(),
            route_key: "step_fun/step-3.5-flash-2603".into(),
            dialect: "open_ai_chat".into(),
            cost_estimate_micro: 100,
            peak_factor_percent: 100,
        })
        .await
        .unwrap();

        // 验证 WindowAffinityApplied 事件被正确发布
        // WHY 循环 recv: broadcast 模式下所有订阅者都收到同一份事件流,
        // 测试的订阅者可能先收到 ModelAffinitySelected(原始事件),需跳过
        // 直到收到 WindowAffinityApplied。
        let event = loop {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timeout waiting for WindowAffinityApplied")
                .expect("recv error");
            if matches!(event, NexusEvent::WindowAffinityApplied { .. }) {
                break event;
            }
        };

        match event {
            NexusEvent::WindowAffinityApplied {
                route_key,
                folded,
                needs_chunking,
                tier,
                ..
            } => {
                assert_eq!(route_key, "step_fun/step-3.5-flash-2603");
                assert!(folded, "256K 模型请求 L3 应被折减");
                assert!(needs_chunking, "256K 折减到 L2 应触发分块标记");
                assert_eq!(tier, "L2", "256K 模型最高档应为 L2");
            }
            other => panic!("Expected WindowAffinityApplied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_hcw_integration_ignores_unregistered_spec() {
        let bus = EventBus::new();
        let gw = McaGateway::new(McaGatewayConfig::default());
        let _handle = spawn_hcw_integration(gw, bus.clone());

        // 发布未注册 route_key 的事件 → 应静默跳过(不 panic)
        bus.publish(NexusEvent::ModelAffinitySelected {
            metadata: EventMetadata::new("test"),
            intent_id: "test-intent".into(),
            route_key: "unknown/unknown".into(),
            dialect: "open_ai_chat".into(),
            cost_estimate_micro: 100,
            peak_factor_percent: 100,
        })
        .await
        .unwrap();

        // 等待短暂时间验证不 panic
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
