//! Quest Engine 控制事件订阅器 — 消费 TUI 发布的控制请求
//!
//! 对应架构层:L9 Quest
//!
//! # 核心职责
//! - 订阅 EventBus 上的 `QuestPauseRequested` 与 `QuestResumeRequested`
//! - 调用 `QuestEngine::pause_quest` / `resume_quest` 更新暂停状态
//! - 发布 `QuestPaused` / `QuestResumed` 状态变更事件,供 TUI 感知结果
//!
//! # 设计决策(WHY)
//! - 独立模块:将控制事件消费与引擎核心解耦,便于单独测试与替换。
//! - 单事件处理函数 `handle_control_event`:测试可直接投喂事件,无需启动后台任务。
//! - `spawn_control_subscriber` 内部 `tokio::spawn`:调用方负责在运行时启动,
//!   测试可选择同步 `handle_control_event` 路径,避免任务生命周期管理。

use event_bus::{EventBus, EventReceiver, NexusEvent};
use std::sync::Arc;
use tracing::{error, info};

use crate::engine::QuestEngine;
use crate::error::QuestError;

/// 处理单个控制事件
///
/// 仅处理与 Quest 生命周期相关的请求事件,其他事件被忽略(返回 Ok)。
/// 测试可调用此函数直接验证控制逻辑,无需启动订阅任务。
pub async fn handle_control_event(
    engine: &QuestEngine,
    event: NexusEvent,
) -> Result<(), QuestError> {
    match event {
        NexusEvent::QuestPauseRequested {
            quest_id,
            requested_by,
            ..
        } => {
            info!(quest_id = %quest_id, "收到 Quest 暂停请求");
            engine.pause_quest(&quest_id, &requested_by).await
        }
        NexusEvent::QuestResumeRequested {
            quest_id,
            requested_by,
            ..
        } => {
            info!(quest_id = %quest_id, "收到 Quest 恢复请求");
            engine.resume_quest(&quest_id, &requested_by).await
        }
        _ => Ok(()),
    }
}

/// 启动后台控制事件订阅任务
///
/// 在运行中的 tokio 运行时上调用。订阅者在后台循环接收事件并委托给
/// `handle_control_event`;遇到错误时记录日志并继续,不会终止任务。
pub fn spawn_control_subscriber(
    engine: Arc<QuestEngine>,
    bus: EventBus,
) -> tokio::task::JoinHandle<()> {
    // WHY 先 subscribe 再 spawn:遵循 event-bus "subscribe-before-spawn" 规则,
    // 避免启动瞬间的事件丢失(§4.4 反模式 #3)。
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = handle_control_event(&engine, event).await {
                        error!(error = %e, "处理控制事件失败");
                    }
                }
                Err(e) => {
                    error!(error = %e, "控制订阅者接收错误,退出");
                    break;
                }
            }
        }
    })
}

/// 从已有接收者启动后台控制事件订阅任务
///
/// 适用于调用方已提前订阅、希望直接传入接收者的场景(如测试)。
pub fn spawn_control_subscriber_with_receiver(
    engine: Arc<QuestEngine>,
    mut rx: EventReceiver,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(e) = handle_control_event(&engine, event).await {
                        error!(error = %e, "处理控制事件失败");
                    }
                }
                Err(e) => {
                    error!(error = %e, "控制订阅者接收错误,退出");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;
    use nexus_core::{MultimodalInput, UserIntent};
    use std::sync::Arc;

    use crate::QuestEngine;

    fn make_intent(text: &str) -> UserIntent {
        UserIntent {
            intent_id: "i-1".into(),
            raw_text: text.into(),
            multimodal_inputs: vec![MultimodalInput::Text(text.into())],
            risk_level: 10,
        }
    }

    #[tokio::test]
    async fn test_handle_control_event_ignores_unrelated_events() {
        let bus = EventBus::new();
        let engine = QuestEngine::new(bus.clone());
        let event = NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k-1".into(),
        };
        // 不 panic 即为通过
        handle_control_event(&engine, event).await.unwrap();
    }

    #[tokio::test]
    async fn test_handle_control_event_pause_publishes_paused() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = QuestEngine::new(bus.clone());
        let quest = engine.create_quest(make_intent("分析。")).await.unwrap();
        let _ = rx.recv().await.unwrap(); // QuestCreated

        let request = NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest.quest_id.clone(),
            requested_by: "operator".into(),
        };
        handle_control_event(&engine, request).await.unwrap();

        assert!(engine.is_paused(&quest.quest_id));
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, NexusEvent::QuestPaused { .. }));
    }

    #[tokio::test]
    async fn test_spawn_control_subscriber_processes_event() {
        let bus = EventBus::new();
        let rx = bus.subscribe();
        let engine = Arc::new(QuestEngine::new(bus.clone()));
        let quest = engine.create_quest(make_intent("分析。")).await.unwrap();

        let handle = spawn_control_subscriber_with_receiver(engine.clone(), rx);

        bus.publish(NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest.quest_id.clone(),
            requested_by: "operator".into(),
        })
        .await
        .unwrap();

        // 等待后台任务处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(engine.is_paused(&quest.quest_id));

        // 终止后台任务:engine 与 bus 仍持有 EventBus 克隆,receiver 不会自然关闭,
        // 因此通过 abort 结束测试任务,避免挂起。
        handle.abort();
    }
}
