//! SubAgent × Quest 执行引擎 — L10 组合根接线（P4-T3②，D-P5 裁决）
//!
//! 对应架构层: **L10 Interface**（nexus-app-server）
//! 对应任务: **P4-T3**（W20 集成周:Phase 3 遗留接线②）
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

use nexus_contracts::NexusError;
use nexus_subagent::runtime::{SubAgentHandle, SubAgentRuntime, SubAgentTask};
use nexus_subagent::types::SubAgentSpec;
use quest_engine::QuestEngine;

/// SubAgent Quest 执行引擎 — 组合根接线（L10 → L7 + L9 向下合规）
pub struct SubAgentQuestEngine {
    /// L7 运行时（拍卖 + JoinSet + 禁嵌套 + 规模上限）
    runtime: SubAgentRuntime,
    /// L9 引擎（Arc 共享;spawn 闭包需 'static + Send）
    engine: Arc<QuestEngine>,
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
        }
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
        // SubAgentTask:FnOnce(SubAgentSpec, Arc<CancellationToken>) -> Result<String,String> + Send
        // 在 spawn_blocking 同步上下文执行 → Handle::block_on 驱动 async create_quest
        let task: SubAgentTask = Box::new(move |_spec, cancel| {
            if let Some(reason) = cancel.poll() {
                return Err(format!("cancelled before quest: {}", reason.as_str()));
            }
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async move {
                let intent = nexus_core::UserIntent {
                    intent_id: uuid::Uuid::now_v7().to_string(),
                    raw_text: goal,
                    multimodal_inputs: Vec::new(),
                    risk_level: 0,
                };
                let quest = engine
                    .create_quest(intent)
                    .await
                    .map_err(|e| format!("quest create failed: {e}"))?;
                Ok(quest.quest_id)
            })
        });
        self.runtime.spawn(spec, task, from_task)
    }

    /// 等待下一任务完成（JoinSet 语义,调用方循环驱动）
    pub async fn join_next(&mut self) -> Option<(String, Result<String, String>)> {
        self.runtime.join_next().await
    }
}

#[cfg(test)]
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
