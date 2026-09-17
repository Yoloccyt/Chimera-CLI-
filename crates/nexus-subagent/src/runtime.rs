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
//! - 取消:每任务 [`CancellationToken`] 四因传播;
//! - **错误分类**(架构方向 F-c,2026-09-12):任务结果与 [`join_next_typed`]
//!   ([`SubAgentRuntime::join_next_typed`]) 携带 [`SubAgentError`]
//!   ——取消 / panic / 业务失败可判定;旧 [`join_next`]
//!   ([`SubAgentRuntime::join_next`]) 保留为兼容壳(文案取自同一 `Display`)。

use std::sync::Arc;

use nexus_contracts::NexusError;
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::auction::{AuctionOutcome, TaskAuction, TaskOffer};
use crate::cancel::CancellationToken;
use crate::error::SubAgentError;
use crate::types::{SubAgentProfile, SubAgentSpec, SWARM_LIMIT};

/// 子代理任务 — 执行体（接入方提供:同一执行引擎换参数）
///
/// 返回 `Result<String, [`SubAgentError`]>`:成功=结果文案,
/// 失败=**分类化**错误(业务失败用 [`SubAgentError::Execution`];
/// 取消与 panic 由运行时在派发层判定,见 [`SubAgentRuntime::spawn`])。
///
/// WHY 具体错误类型而非 `Result<String, String>`(架构方向 F-c):
/// 字符串文案无法让调用方区分"预期内取消 / 实现缺陷 panic / 业务失败",
/// 三者降级策略不同;错误被拍平会迫使调用方做子串匹配(脆弱)。
pub type SubAgentTask =
    Box<dyn FnOnce(SubAgentSpec, Arc<CancellationToken>) -> Result<String, SubAgentError> + Send>;

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
    /// 活跃任务集合（JoinSet 有界并发）— 结果携带**分类化**错误
    active: JoinSet<(String, Result<String, SubAgentError>)>,
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
            let guard = self.nesting_guard.read().unwrap_or_else(|p| p.into_inner());
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
                return (
                    task_id_for_closure.clone(),
                    Err(SubAgentError::Cancelled {
                        reason: reason.as_str().to_string(),
                    }),
                );
            }
            // spawn_blocking 的 JoinError = 任务体 panic → 分类为 Panicked
            // (不重试语义:panic 是实现缺陷,重试通常再次 panic)
            let result = tokio::task::spawn_blocking(move || task(spec, cancel_for_task))
                .await
                .unwrap_or_else(|e| {
                    Err(SubAgentError::Panicked {
                        detail: e.to_string(),
                    })
                });
            (task_id_for_closure, result)
        });
        Ok(SubAgentHandle { task_id, cancel })
    }

    /// 等待下一任务完成(**分类化错误**,推荐路径)
    ///
    /// # 返回
    /// `Some((task_id, result))`:`result` 的 Err 为 [`SubAgentError`],
    /// 调用方可 `match` 出取消 / panic / 业务失败并分别降级;`None` = 无更多任务
    /// (或任务 panic 被隔离,同 `JoinSet` 语义)。
    pub async fn join_next_typed(&mut self) -> Option<(String, Result<String, SubAgentError>)> {
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

    /// 等待下一任务完成（**兼容签名**:错误以 `String` 文案返回）
    ///
    /// WHY 保留:既有接入方（如 `nexus-app-server::subagent_engine`）按
    /// `Result<String, String>` 消费;为不破坏其编译,本方法作为兼容壳保留,
    /// 内部委托 [`join_next_typed`](Self::join_next_typed) 并把分类错误
    /// 经 [`SubAgentError::as_message`] 转回文案 —— 文案唯一来源仍是
    /// `SubAgentError` 的 `Display`,不会出现两套分叉。
    ///
    /// 新代码请用 [`join_next_typed`](Self::join_next_typed) 以获得可判定语义。
    pub async fn join_next(&mut self) -> Option<(String, Result<String, String>)> {
        self.join_next_typed()
            .await
            .map(|(id, result)| (id, result.map_err(|e| e.as_message())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SubAgentKind;

    /// 3 类型并行 E2E — 注册 → 派发 → 完成（门禁）
    #[tokio::test]
    async fn three_types_parallel_e2e() {
        let mut rt = SubAgentRuntime::new();
        rt.register(SubAgentProfile::new("coder-a", SubAgentKind::Coder, 2.0));
        rt.register(SubAgentProfile::new(
            "explore-b",
            SubAgentKind::Explore,
            1.0,
        ));
        rt.register(SubAgentProfile::new("plan-c", SubAgentKind::Plan, 1.0));
        for kind in SubAgentKind::ALL {
            let spec = SubAgentSpec::new(kind);
            let handle = rt
                .spawn(
                    spec.clone(),
                    Box::new(move |spec, _cancel| {
                        Ok(format!("done:{}", spec.kind.capability_tag()))
                    }),
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
        assert!(
            matches!(err, Err(NexusError::NestedSubAgentForbidden)),
            "嵌套必须拒绝"
        );
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
        handle
            .cancel
            .cancel(crate::cancel::CancelReason::UserCancelled);
        // 用 typed 出口:直接断言**分类**(而非子串匹配)
        let (_id, result) = rt.join_next_typed().await.expect("任务必须完成");
        let err = result.expect_err("预取消必须失败");
        assert!(err.is_cancelled(), "应分类为取消, got: {err}");
        assert!(
            err.to_string().contains("cancelled"),
            "文案应保留可读原因: {err}"
        );
    }

    /// 业务失败分类 — 任务体返回 [`SubAgentError::Execution`] 时调用方可判定
    /// (F-c 核心价值:不再需要子串匹配"failed"来猜测失败类别)
    #[tokio::test]
    async fn execution_error_is_classified() {
        let mut rt = SubAgentRuntime::new();
        let _ = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Explore),
                Box::new(|_s, _c| {
                    Err(SubAgentError::Execution {
                        detail: "tool missing".into(),
                    })
                }),
                None,
            )
            .expect("派发成功");
        let (_id, result) = rt.join_next_typed().await.expect("任务必须完成");
        let err = result.expect_err("任务体失败");
        assert!(err.is_execution(), "应分类为业务失败, got: {err}");
        assert!(!err.is_cancelled() && !err.is_panic());
    }

    /// 兼容壳一致性 — 旧签名 `join_next()` 的错误文案必须等于新类型的 `Display`
    ///
    /// WHY 此断言:兼容路径若自行拼文案,会与 `SubAgentError` 分叉(两套真相);
    /// 本测试锁定 `as_message()` 为唯一转换点。
    #[tokio::test]
    async fn compat_join_next_message_matches_typed() {
        let mut rt = SubAgentRuntime::new();
        let _ = rt
            .spawn(
                SubAgentSpec::new(SubAgentKind::Explore),
                Box::new(|_s, _c| {
                    Err(SubAgentError::Execution {
                        detail: "same message".into(),
                    })
                }),
                None,
            )
            .expect("派发成功");
        let (_id, result) = rt.join_next().await.expect("任务必须完成");
        let msg = result.expect_err("任务体失败");
        let expected = SubAgentError::Execution {
            detail: "same message".into(),
        };
        assert_eq!(
            msg,
            expected.as_message(),
            "兼容文案必须等于分类错误 Display"
        );
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
