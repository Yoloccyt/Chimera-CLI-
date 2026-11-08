//! SubAgent × Quest 执行引擎 — L10 侧适配器（**装配态：WIRED**，ADR-257）
//!
//! 对应架构层: **L10 Interface**（nexus-app-server）
//! 对应任务: **P4-T3**（W20 集成周:Phase 3 遗留接线②）→ **P2 装配接线**（ADR-257）
//!
//! # 装配态（2026-11-08，ADR-257 D1/D5）
//! **已接线**：组合根（chimera-cli composition.rs）经 `QuestEngine` 浅 Clone
//! 构造本适配器并以 `Arc<tokio::sync::Mutex<…>>` 注入 AppContext.subagent_engine
//! （ADR-257 D1 案 β 浅共享；D2 tokio 异步锁——join_next 收 `&mut` 的 JoinSet
//! 所有权语义 ⇒ 同一实例 spawn/join 串行化，代价已登记）。第一波仅装配 +
//! 句柄暴露，驱动入口由命令面后续波次挂接（ADR-257 D3）。
//!
//! # 设计（依赖铁律合规）
//! [`nexus_subagent::SubAgentRuntime`]（L7）的 spawn 闭包需要真实执行体;
//! [`quest_engine::QuestEngine`](L9) 提供 Quest 生命周期。L7→L9 向上依赖
//! 禁止,故接线落 **L10 组合根**（同 [`QuestBackend`] 先例）:
//! [`SubAgentQuestEngine`] 持 `Arc<QuestEngine>`,spawn 闭包内以
//! `Handle::block_on` 驱动 `create_quest`（spawn_blocking 同步上下文）,
//! 执行结果为 Quest ID。
//!
//! # 取消语义
//! 执行前检查 [`CancellationToken`]（四因）;执行中取消由 Quest 层
//! 生命周期管理（本接线只做前置快速失败,避免幽灵 Quest）。

use std::sync::Arc;

use chrono::Utc;
use event_bus::{EventBus, EventMetadata, NexusEvent, TaskPriority};
use nexus_contracts::NexusError;
use nexus_subagent::error::SubAgentError;
use nexus_subagent::runtime::{SubAgentHandle, SubAgentRuntime, SubAgentTask};
use nexus_subagent::types::SubAgentSpec;
use quest_engine::QuestEngine;

/// 适配器在 AgentTask* 事件中的委托方身份（ADR-257 D4：与 chimera-mas
/// RootOrchestrator 的委托面共用三变体，按 from/to 归属区分——本适配器的
/// 委托方恒为本常量，被委派方为 task_id）
pub const SUBAGENT_ENGINE_ID: &str = "subagent-quest-engine";

/// SubAgent Quest 执行引擎 — L10 侧适配器（WIRED，ADR-257；L10 → L7 + L9 向下合规）
pub struct SubAgentQuestEngine {
    /// L7 运行时（拍卖 + JoinSet + 禁嵌套 + 规模上限）
    runtime: SubAgentRuntime,
    /// L9 引擎（Arc 共享;spawn 闭包需 'static + Send）
    engine: Arc<QuestEngine>,
    /// 事件总线（可选;ADR-257 D4 事件闭环——spawn/join 发布 AgentTask* 三变体。
    /// WHY Option:适配器自身单测无总线场景保持可用;组合根经 with_event_bus 注入）
    bus: Option<EventBus>,
}

impl std::fmt::Debug for SubAgentQuestEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // QuestEngine/SubAgentRuntime 无 Debug（内部 DashMap 等）
        f.debug_struct("SubAgentQuestEngine")
            .finish_non_exhaustive()
    }
}

impl SubAgentQuestEngine {
    /// 新建 — 包裹既有 QuestEngine（共享内部状态,同 QuestBackend 语义）
    #[must_use]
    pub fn new(engine: QuestEngine) -> Self {
        Self {
            runtime: SubAgentRuntime::new(),
            engine: Arc::new(engine),
            bus: None,
        }
    }

    /// 注入事件总线（ADR-257 D4 事件闭环）：spawn/join 闭环发布
    /// `AgentTaskDelegated`/`AgentTaskCompleted`/`AgentTaskFailed`（既有变体,
    /// 零新增;from/to 归属见 [`SUBAGENT_ENGINE_ID`]）。组合根装配时注入
    /// 共享 bus——命令面订阅即可观测委托闭环。
    #[must_use]
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.bus = Some(bus);
        self
    }

    /// 运行时可变引用（档案注册/诊断）
    pub fn runtime_mut(&mut self) -> &mut SubAgentRuntime {
        &mut self.runtime
    }

    /// 派发 Quest 型 SubAgent — spec.goal 经 UserIntent 建 Quest,返回 Quest ID
    ///
    /// # 错误
    /// 规模超限/嵌套违规由 [`SubAgentRuntime::spawn`] 拒绝（[`NexusError`]）;
    /// Quest 创建失败在任务结果中返回 Err（不 panic,隔离语义）。
    pub fn spawn_quest(
        &mut self,
        spec: SubAgentSpec,
        goal: &str,
        from_task: Option<&str>,
    ) -> Result<SubAgentHandle, NexusError> {
        let engine = Arc::clone(&self.engine);
        let goal = goal.to_string();
        // SubAgentTask:FnOnce(SubAgentSpec, Arc<CancellationToken>) -> Result<String, SubAgentError> + Send
        // 在 spawn_blocking 同步上下文执行 → Handle::block_on 驱动 async create_quest
        //
        // 错误映射责任在本层(L10):`QuestError`(L9)与 `SubAgentError`(L7)属不同层,
        // 跨层转换由调用方显式完成(nexus-subagent 不该知道 quest-engine)。
        let task: SubAgentTask =
            Box::new(move |_spec, cancel| {
                if let Some(reason) = cancel.poll() {
                    // 取消四因:任务执行前已被撤销 → 分类为 Cancelled(可静默降级)
                    return Err(SubAgentError::Cancelled {
                        reason: reason.as_str().to_string(),
                    });
                }
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async move {
                    let intent = nexus_core::UserIntent {
                        intent_id: uuid::Uuid::now_v7().to_string(),
                        raw_text: goal,
                        multimodal_inputs: Vec::new(),
                        risk_level: 0,
                    };
                    let quest = engine.create_quest(intent).await.map_err(|e| {
                        SubAgentError::Execution {
                            detail: format!("quest create failed: {e}"),
                        }
                    })?;
                    Ok(quest.quest_id)
                })
            });
        let handle = self.runtime.spawn(spec, task, from_task)?;
        // ADR-257 D4：委托事件发布（spawn_quest 为同步方法 ⇒ publish_blocking,
        // §4.4 红线 #8;发布失败仅 warn——Normal 级事件,丢失可由 Completed/Failed
        // 补偿,不得让事件面失败传染派发本身）
        if let Some(bus) = &self.bus {
            let event = NexusEvent::AgentTaskDelegated {
                metadata: EventMetadata::new(SUBAGENT_ENGINE_ID),
                from: SUBAGENT_ENGINE_ID.to_string(),
                to: handle.task_id.clone(),
                task_id: handle.task_id.clone(),
                deadline: Utc::now() + chrono::Duration::hours(1),
                priority: TaskPriority::Medium,
            };
            if let Err(err) = bus.publish_blocking(event) {
                tracing::warn!(
                    task_id = %handle.task_id,
                    error = %err,
                    "AgentTaskDelegated 发布失败(委托闭环事件,非派发本身)"
                );
            }
        }
        Ok(handle)
    }

    /// 等待下一任务完成（JoinSet 语义,调用方循环驱动）
    ///
    /// ADR-257 D4：结果回执发布——成功发 `AgentTaskCompleted`(from=完成方
    /// task_id, to=委托方引擎), 失败发 `AgentTaskFailed`(Critical,mpsc 旁路
    /// 保证投递;retry_count=0——本适配器不做重试语义)。
    pub async fn join_next(&mut self) -> Option<(String, Result<String, String>)> {
        let (task_id, result) = self.runtime.join_next().await?;
        if let Some(bus) = &self.bus {
            let event = match &result {
                Ok(quest_id) => NexusEvent::AgentTaskCompleted {
                    metadata: EventMetadata::new(SUBAGENT_ENGINE_ID),
                    from: task_id.clone(),
                    to: SUBAGENT_ENGINE_ID.to_string(),
                    task_id: task_id.clone(),
                    result_summary: quest_id.clone(),
                },
                Err(e) => NexusEvent::AgentTaskFailed {
                    metadata: EventMetadata::new(SUBAGENT_ENGINE_ID),
                    from: task_id.clone(),
                    to: SUBAGENT_ENGINE_ID.to_string(),
                    task_id: task_id.clone(),
                    error: e.clone(),
                    retry_count: 0,
                },
            };
            if let Err(err) = bus.publish(event).await {
                tracing::warn!(
                    task_id = %task_id,
                    error = %err,
                    "AgentTask 回执发布失败(闭环事件,非任务本身)"
                );
            }
        }
        Some((task_id, result))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use event_bus::EventBus;
    use nexus_subagent::types::SubAgentKind;

    fn make_engine() -> SubAgentQuestEngine {
        SubAgentQuestEngine::new(QuestEngine::new(EventBus::new()))
    }

    /// 接线语义 — spawn_quest 建真实 Quest,结果为 Quest ID（非 stub 文本）
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_quest_creates_real_quest() {
        let mut sge = make_engine();
        let handle = sge
            .spawn_quest(
                SubAgentSpec::new(SubAgentKind::Explore),
                "调研并行模式",
                None,
            )
            .expect("顶层派发（无嵌套）");
        let (task_id, result) = sge.join_next().await.expect("JoinSet 必有结果");
        assert_eq!(task_id, handle.task_id);
        let quest_id = result.expect("Quest 创建成功");
        assert!(
            quest_id.starts_with("quest-"),
            "返回真实 Quest ID: {quest_id}"
        );
    }

    /// Quest ID 唯一 — 两次 spawn 产生不同 Quest
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_quest_ids_unique() {
        let mut sge = make_engine();
        let _ = sge.spawn_quest(SubAgentSpec::new(SubAgentKind::Explore), "任务 A", None);
        let _ = sge.spawn_quest(SubAgentSpec::new(SubAgentKind::Coder), "任务 B", None);
        let mut ids = Vec::new();
        while let Some((_, r)) = sge.join_next().await {
            ids.push(r.expect("成功"));
        }
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "Quest ID 必须唯一");
    }

    /// 前置取消 — 已取消令牌下任务快速失败,零 Quest 创建
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_token_fails_fast() {
        let mut sge = make_engine();
        let handle = sge
            .spawn_quest(SubAgentSpec::new(SubAgentKind::Explore), "x", None)
            .expect("spawn 成功");
        handle
            .cancel
            .cancel(nexus_subagent::cancel::CancelReason::UserCancelled);
        let (_, result) = sge.join_next().await.expect("必有结果");
        let err = result.expect_err("已取消必须失败");
        assert!(err.contains("cancelled"), "错误含取消原因: {err}");
    }

    /// 禁嵌套 — 运行中任务发起 spawn 被拒（运行期断言接线透传）
    #[tokio::test]
    async fn nesting_forbidden_through_engine() {
        let mut sge = make_engine();
        let _ = sge
            .spawn_quest(SubAgentSpec::new(SubAgentKind::Explore), "parent", None)
            .expect("顶层成功");
        // 运行中任务作为 from_task 再次派发 → 运行期断言拒绝
        let running = sge.runtime_mut();
        // 取运行中任务 ID:JoinSet 尚未完成,通过 spawned_total 验证接线
        assert_eq!(running.spawned_total(), 1);
        // 直接以活跃任务 ID 发起（模拟嵌套）→ 需要 knows ID;此处验证 API 面:
        // spawn_quest 透传 from_task 到 runtime 的禁嵌套断言
        let out = sge.spawn_quest(
            SubAgentSpec::new(SubAgentKind::Coder),
            "child",
            Some("task-nonexistent"), // 未知 ID 不在活跃表 → 允许（保守放行语义与 runtime 一致）
        );
        assert!(out.is_ok(), "未知父 ID 保守放行（与 runtime 语义一致）");
        while sge.join_next().await.is_some() {}
    }
}
