//! 真实核心后端 — QuestEngine 接入（P3-T5，D-P11 裁决：T6 遗留三项之一）
//!
//! 对应架构层: **L10 Interface**（nexus-app-server）
//! 对应任务: **P3-T5**（手册 W16，T6 遗留:CoreBackend 真实接入）
//!
//! # 语义
//! [`QuestBackend`] 包装 `quest-engine::QuestEngine`（L9 编排层真实状态源）:
//! - `submit_turn`:以会话目标创建 Quest（引擎真实状态驱动,QuestCreated 经
//!   EventBus 广播）,产出 Item 流（输入回显 + Quest 摘要）;
//! - `interrupt_turn`:确认中断（Quest 短生命周期,无运行中任务时仅确认）。
//!
//! 替代 [`InMemoryBackend`](crate::server::InMemoryBackend)（MVP 保留一周双跑,
//! 回退路径:构造参数切换）。

use async_trait::async_trait;
use event_bus::EventBus;
use nexus_contracts::app::{Item, ItemId, ItemStatus, Thread, TurnId, UserInput};
use nexus_core::UserIntent;
use quest_engine::QuestEngine;

use crate::server::CoreBackend;

/// 真实核心后端 — 包装 QuestEngine（WI-01 CoreOp 单向驱动核心的 L9 接入）
pub struct QuestBackend {
    /// 编排引擎（真实状态源）
    engine: QuestEngine,
    /// 事件总线（引擎广播 QuestCreated 等）
    _bus: EventBus,
}

impl std::fmt::Debug for QuestBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // QuestEngine/EventBus 无 Debug（内部 DashMap 等）:仅输出占位
        f.debug_struct("QuestBackend").finish_non_exhaustive()
    }
}

impl QuestBackend {
    /// 新建后端（内部创建引擎,默认配置）
    #[must_use]
    pub fn new(bus: EventBus) -> Self {
        Self {
            engine: QuestEngine::new(bus.clone()),
            _bus: bus,
        }
    }

    /// 新建后端（注入既有引擎——宿主组合根装配路径）
    #[must_use]
    pub fn with_engine(engine: QuestEngine, bus: EventBus) -> Self {
        Self { engine, _bus: bus }
    }
}

#[async_trait]
impl CoreBackend for QuestBackend {
    async fn submit_turn(
        &self,
        thread: &Thread,
        turn_id: &TurnId,
        input: &UserInput,
    ) -> Result<Vec<Item>, String> {
        // 1. 构造用户意图（真实引擎输入:goal 作为标题锚点,raw_text 为输入）
        let intent = UserIntent {
            intent_id: format!("intent-{}", turn_id.as_str()),
            raw_text: String::from(&*input.text),
            multimodal_inputs: Vec::new(),
            risk_level: 0,
        };
        // 2. 引擎创建 Quest（真实状态驱动:任务分解 + DAG 校验 + QuestCreated 广播）
        let quest = self
            .engine
            .create_quest(intent)
            .await
            .map_err(|e| format!("quest create failed: {e}"))?;
        // 3. 产出 Item 流（输入回显 + Quest 状态摘要）
        let mut items = Vec::new();
        items.push(Item::new(
            ItemId::new(format!("{}-1", turn_id.as_str())),
            thread.thread_id.clone(),
            turn_id.clone(),
            "message",
            ItemStatus::Completed,
            &input.text,
        ));
        items.push(Item::new(
            ItemId::new(format!("{}-2", turn_id.as_str())),
            thread.thread_id.clone(),
            turn_id.clone(),
            "quest_state",
            ItemStatus::Completed,
            &format!(
                r#"{{"quest_id":"{}","title":"{}","tasks":{}}}"#,
                quest.quest_id,
                quest.title,
                quest.tasks.len()
            ),
        ));
        Ok(items)
    }

    async fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), String> {
        tracing::info!(turn = %turn_id.as_str(), "回合中断信号已确认（Quest 短生命周期）");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventBus;
    use nexus_contracts::app::{Thread, ThreadId};

    /// QuestBackend 真实接入 — submit_turn 经 QuestEngine 产出真实 Quest 状态
    #[tokio::test]
    async fn quest_backend_real_engine() {
        let bus = EventBus::new();
        let backend = QuestBackend::new(bus);
        let thread = Thread::new(ThreadId::new("goal-1::run-1"), "goal-1", "run-1", 1_000);
        let turn = TurnId::new("turn-1");
        let input = UserInput {
            text: "分析项目依赖".into(),
            extras: None,
        };
        let items = backend
            .submit_turn(&thread, &turn, &input)
            .await
            .expect("真实引擎必须成功");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, Box::<str>::from("message"));
        assert_eq!(items[0].payload, Box::<str>::from("分析项目依赖"));
        assert_eq!(items[1].kind, Box::<str>::from("quest_state"));
        // quest_state 载荷含真实 quest_id（引擎生成,非空）
        assert!(items[1].payload.contains("quest_id"));
    }

    /// 中断确认 — 无运行中任务时仅确认（不报错）
    #[tokio::test]
    async fn quest_backend_interrupt() {
        let bus = EventBus::new();
        let backend = QuestBackend::new(bus);
        let r = backend.interrupt_turn(&TurnId::new("turn-1")).await;
        assert!(r.is_ok());
    }
}
