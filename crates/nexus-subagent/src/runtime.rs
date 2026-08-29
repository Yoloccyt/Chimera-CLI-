//! SubAgent 运行时 — Arena 竞争 + 禁嵌套 + 规模上限（P3-T9，v4.0 WI-25）
//!
//! 对应架构层: L7 Execution（nexus-subagent，ADR-148）
//!
//! # 设计
//! - [`SubAgentRuntime`]:注册档案 + `spawn_arena`（JoinSet 有界并发,E8-2:
//!   LLM 类任务禁 rayon——用 JoinSet 而非计算池）;
//! - **禁嵌套**:spawn 时检查任务来源（运行中任务再 spawn = 嵌套）,
//!   触发 [`NestedSubAgentForbidden`]（L0 契约,运行期断言;编译期由
//!   API 面保证:spawn 只能从运行时入口调用）;
//! - **规模上限**:`SWARM_LIMIT = 8`（ADR-148 门禁,超额拒绝）;
//! - 取消:每任务 [`CancellationToken`] 四因传播。

use std::sync::Arc;

use nexus_contracts::NexusError;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::auction::{AuctionOutcome, TaskAuction, TaskOffer};
use crate::cancel::CancellationToken;
use crate::types::{SubAgentProfile, SubAgentSpec, SWARM_LIMIT};

/// 子代理任务 — 执行体（接入方提供:同一执行引擎换参数）
///
/// 返回 `Result<String, String>`（结果 / 错误文案）。
pub type SubAgentTask = Box<dyn FnOnce(SubAgentSpec, Arc<CancellationToken>) -> Result<String, String> + Send>;

/// 任务句柄 — 结果 + 取消令牌（调用方 await 结果 / 主动取消）
pub struct SubAgentHandle {
    /// 任务 ID
    pub task_id: String,
    /// 取消令牌（四因取消）
    pub cancel: Arc<CancellationToken>,
}

/// SubAgent 运行时 — 注册 + Arena 派发 + 禁嵌套 + 规模上限
pub struct SubAgentRuntime {
    /// 拍卖市场（档案注册 + 择胜）
    auction: TaskAuction,
    /// 活跃任务集合（JoinSet 有界并发）
    active: JoinSet<(String, Result<String, String>)>,
    /// 嵌套检测:任务 ID → 是否允许再 spawn（运行中 = 禁止）
    nesting_guard: std::sync::RwLock<std::collections::HashMap<String, bool>>,
    /// 规模计数（诊断）
    spawned_total: u64,
}

impl Default for SubAgentRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SubAgentRuntime {
    /// 新建运行时（独立 JoinSet）
    #[must_use]
    pub fn new() -> Self {
        Self {
            auction: TaskAuction::new(),
            active: JoinSet::new(),
            nesting_guard: std::sync::RwLock::new(std::collections::HashMap::new()),
            spawned_total: 0,
        }
    }

    /// 注册档案（幂等覆盖）
    pub fn register(&mut self, profile: SubAgentProfile) {
        self.auction.register(profile);
    }

    /// 注销档案
    pub fn unregister(&mut self, profile_id: &str) {
        self.auction.unregister(profile_id);
    }

    /// 档案数（诊断）
    #[must_use]
    pub fn profile_count(&self) -> usize {
        self.auction.len()
    }

    /// 活跃任务数（诊断）
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// 累计派发数（诊断）
    #[must_use]
    pub fn spawned_total(&self) -> u64 {
        self.spawned_total
    }

    /// 竞价派发 — Auction 择胜后 spawn（短任务派发;与 mas-sched Claim 分工）
    ///
    /// # 禁嵌套（运行期断言）
    /// `from_task`:发起方任务 ID（None = 顶层编排）。若发起方仍活跃
    /// （嵌套 spawn）→ [`NexusError::NestedSubAgentForbidden`]。
    ///
    /// # 规模上限
    /// 活跃任务 ≥ [`SWARM_LIMIT`] → 拒绝（Err）。
    pub fn spawn(
        &mut self,
        spec: SubAgentSpec,
        task: SubAgentTask,
        from_task: Option<&str>,
    ) -> Result<SubAgentHandle, NexusError> {
        // 1. 规模上限（ADR-148:Swarm ≤ 8）
        if self.active.len() >= SWARM_LIMIT {
            return Err(NexusError::NestedSubAgentForbidden); // 复用:超限即拒（保守）
        }
        // 2. 禁嵌套断言（运行期;编译期由 API 面保证——spawn 仅运行时入口）
        if let Some(parent) = from_task {
            let guard = self
                .nesting_guard
                .read()
                .unwrap_or_else(|p| p.into_inner());
            if guard.get(parent).copied().unwrap_or(false) {
                return Err(NexusError::NestedSubAgentForbidden);
            }
        }
        // 3. 竞价择胜（能力匹配;无档案 → 默认按类型兜底直接派发）
        let offer = TaskOffer {
            task_id: format!("task-{}", Uuid::now_v7()),
            required_capabilities: spec.kind.capability_tag().into(),
        };
        let winner = match self.auction.auction(&offer) {
            AuctionOutcome::Won(b) => Some(b.profile_id.clone()),
            AuctionOutcome::NoBid => None, // 兜底:无档案也派发（默认执行引擎）
        };
        let _ = winner; // 档案 ID 供审计;执行体由 task 提供（同引擎换参）
        // 4. spawn（JoinSet 有界并发;LLM 类禁 rayon,E8-2）
        let task_id = offer.task_id;
        let task_id_for_closure = task_id.clone();
        let cancel = Arc::new(CancellationToken::new());
        let cancel_for_task = Arc::clone(&cancel);
        self.nesting_guard
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.clone(), true);
        self.spawned_total += 1;
        self.active.spawn(async move {
            // 执行前检查取消（四因:父级撤销可预取消）
            if let Some(reason) = cancel_for_task.poll() {
                return (task_id_for_closure.clone(), Err(format!("cancelled: {}", reason.as_str())));
            }
            let result = tokio::task::spawn_blocking(move || task(spec, cancel_for_task))
                .await
                .unwrap_or_else(|e| Err(format!("subagent task panicked: {e}")));
            (task_id_for_closure, result)
        });
        Ok(SubAgentHandle { task_id, cancel })
    }

    /// 等待下一任务完成 — JoinSet 语义（调用方循环驱动）
    pub async fn join_next(&mut self) -> Option<(String, Result<String, String>)> {
        // JoinError（任务 panic）:跳过（隔离语义,不传播）
        let done = match self.active.join_next().await {
            Some(Ok(d)) => d,
            Some(Err(_)) => return None,
            None => return None,
        };
        // 清理嵌套守卫（任务完成 = 可再 spawn）
        self.nesting_guard
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&done.0);
        Some(done)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::SubAgentKind;
    use super::*;

    /// 3 类型并行 E2E — 注册 → 派发 → 完成（门禁）
    #[tokio::test]
    async fn three_types_parallel_e2e() {
        let mut rt = SubAgentRuntime::new();
        rt.register(SubAgentProfile::new("coder-a", SubAgentKind::Coder, 2.0));
        rt.register(SubAgentProfile::new("explore-b", SubAgentKind::Explore, 1.0));
        rt.register(SubAgentProfile::new("plan-c", SubAgentKind::Plan, 1.0));
        for kind in SubAgentKind::ALL {
            let spec = SubAgentSpec::new(kind);
            let handle = rt
                .spawn(
                    spec.clone(),
                    Box::new(move |spec, _cancel| Ok(format!("done:{}", spec.kind.capability_tag()))),
                    None,
                )
                .expect("派发必须成功");
            assert!(!handle.cancel.is_cancelled());
        }
        assert_eq!(rt.active_count(), 3);
        let mut done = 0;
        while let Some((_id, result)) = rt.join_next().await {
            assert!(result.is_ok());
            done += 1;
        }
        assert_eq!(done, 3, "3 类型必须全部完成");
    }

    /// 禁嵌套断言 — 运行中任务再 spawn 拒绝（NestedSubAgentForbidden）
    #[tokio::test]
    async fn nesting_forbidden() {
        let mut rt = SubAgentRuntime::new();
        // 模拟:任务 A 运行中,B 由 A 发起 → 拒绝
        let handle_a = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Explore),
                Box::new(|_s, _c| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    Ok("a".into())
                }),
                None,
            )
            .expect("A 派发成功");
        // A 仍活跃 → B 嵌套 → 拒绝
        let err = rt.spawn(
            SubAgentSpec::new(SubAgentKind::Explore),
            Box::new(|_s, _c| Ok("b".into())),
            Some(&handle_a.task_id),
        );
        assert!(matches!(err, Err(NexusError::NestedSubAgentForbidden)), "嵌套必须拒绝");
        // 顶层（无来源）允许
        let _ = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Explore),
                Box::new(|_s, _c| Ok("c".into())),
                None,
            )
            .expect("顶层允许");
        // 等 A 完成后清理
        while rt.join_next().await.is_some() {}
    }

    /// 规模上限 — 活跃 ≥ 8 拒绝
    #[tokio::test]
    async fn swarm_limit_enforced() {
        let mut rt = SubAgentRuntime::new();
        for i in 0..SWARM_LIMIT {
            let _ = rt
                .spawn(
                    SubAgentSpec::new(SubAgentKind::Explore),
                    Box::new(move |_s, _c| Ok(format!("t{i}"))),
                    None,
                )
                .expect("前 8 个必须成功");
        }
        let err = rt.spawn(
            SubAgentSpec::new(SubAgentKind::Explore),
            Box::new(|_s, _c| Ok("overflow".into())),
            None,
        );
        assert!(err.is_err(), "超限必须拒绝");
        // 清理
        while rt.join_next().await.is_some() {}
    }

    /// 取消传播 — 预取消任务直接失败（四因）
    #[tokio::test]
    async fn pre_cancelled_fails_fast() {
        let mut rt = SubAgentRuntime::new();
        let handle = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Explore),
                Box::new(|_s, _c| Ok("x".into())),
                None,
            )
            .expect("派发成功");
        handle.cancel.cancel(crate::cancel::CancelReason::UserCancelled);
        let (_id, result) = rt.join_next().await.expect("任务必须完成");
        assert!(result.is_err(), "预取消必须失败");
        assert!(result.unwrap_err().contains("cancelled"));
    }

    /// 最低价兜底 — 无档案时仍派发成功（防饿死,ADR-148/RK-P14）
    ///
    /// 无档案注册（空市场）→ auction NoBid → 默认执行引擎兜底派发（不饿死）
    #[tokio::test]
    async fn no_profile_fallback_dispatches() {
        let mut rt = SubAgentRuntime::new();
        assert_eq!(rt.profile_count(), 0, "空市场");
        let handle = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Coder),
                Box::new(|_s, _c| Ok("fallback-ok".into())),
                None,
            )
            .expect("无档案必须兜底派发（不饿死）");
        assert!(!handle.cancel.is_cancelled());
        let (id, result) = rt.join_next().await.expect("兜底任务必须完成");
        assert_eq!(result.expect("兜底执行成功"), "fallback-ok");
        assert!(id.contains("task-"));
    }
}
