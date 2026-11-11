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
//! - **规模上限**:`SWARM_LIMIT = 8`（ADR-148 门禁,满载拒绝并返回
//!   [`NexusError::SwarmCapacityExceeded`]——与嵌套违规变体可区分,C-23）;
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
    /// 活跃任务 ≥ [`SWARM_LIMIT`] → 拒绝（[`NexusError::SwarmCapacityExceeded`]，
    /// 携带 active/limit 快照；与嵌套违规 [`NexusError::NestedSubAgentForbidden`]
    /// 可区分，C-23）。
    pub fn spawn(
        &mut self,
        spec: SubAgentSpec,
        task: SubAgentTask,
        from_task: Option<&str>,
    ) -> Result<SubAgentHandle, NexusError> {
        // 1. 规模上限（ADR-148:Swarm ≤ 8）——满载专用变体（C-23）:
        //    此前复用 NestedSubAgentForbidden 保守拒否,满载与嵌套不可诊断区分
        if self.active.len() >= SWARM_LIMIT {
            return Err(NexusError::SwarmCapacityExceeded {
                active: self.active.len(),
                limit: SWARM_LIMIT,
            });
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
                        // 4. 守卫登记 + 计数后委托共用提交体（闭包构造见 spawn_inner;
                        //    登记不进共用体——批量路径要求写锁整批仅 1 次,见 spawn_batch）
        let task_id = offer.task_id;
        self.nesting_guard
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.clone(), true);
        self.spawned_total += 1;
        Ok(self.spawn_inner(task_id, spec, task))
    }

    /// 批量竞价派发 — 原子容量/嵌套检查 + 顺序竞拍 + 批量入 JoinSet（WP-D）
    ///
    /// # 原子性
    /// 任一前置失败（容量不足 / 嵌套违规）→ 整批拒绝，零副作用
    /// （一个任务都不派发，与逐次 spawn 的"部分成功"语义不同——后者第
    /// k 个撞限时前 k-1 个已派发，调用方无从回滚）。
    ///
    /// # 性能
    /// - 守卫锁：读 1 次 + 写 1 次（逐次 spawn 为 2N 次）
    /// - 竞拍：auction 为纯内存 `&self` 计算（无 IO 无 await），顺序循环调用
    ///   N 次即最优——"并发竞拍"在纯同步函数上是伪需求（线程池开销 > 竞拍本身）
    /// - 批量派发后 JoinSet 并行执行语义与逐次 spawn 完全一致（提交体共用
    ///   [`spawn_inner`](Self::spawn_inner)）
    ///
    /// # 空批
    /// `batch` 为空 → 直接 `Ok(vec![])`：空批无物可派，不触发锁与检查
    /// （即使 `from_task` 指向运行中任务也不报嵌套违规——没有嵌套发生）。
    pub fn spawn_batch(
        &mut self,
        batch: Vec<(SubAgentSpec, SubAgentTask)>,
        from_task: Option<&str>,
    ) -> Result<Vec<SubAgentHandle>, NexusError> {
        // 空批快速路径 — 零副作用（锁与检查都不碰）
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        // 1. 原子容量检查（ADR-148:Swarm ≤ 8,满载专用变体 C-23）。
        //    WHY 边界取法与单发对等:单发是"加入前已满即拒"（`active >= LIMIT`,
        //    等价于加入后 active+1 > LIMIT）;批量是"加入后不超限即拒"
        //    （`active + batch.len() > LIMIT`）——batch.len()==1 时两者数学等价。
        //    批量把 N 次逐个判定合并为派发前的一次总量判定（竞拍纯内存不释放
        //    容量,无"部分派发"中间态）,故边界语义等价成立。
        let active = self.active.len();
        if active + batch.len() > SWARM_LIMIT {
            return Err(NexusError::SwarmCapacityExceeded {
                active,
                limit: SWARM_LIMIT,
            });
        }
        // 2. 原子嵌套断言（运行期;同批共享同一 parent,读锁 1 次足够）
        if let Some(parent) = from_task {
            let guard = self.nesting_guard.read().unwrap_or_else(|p| p.into_inner());
            if guard.get(parent).copied().unwrap_or(false) {
                return Err(NexusError::NestedSubAgentForbidden);
            }
        }
        // 3. 顺序竞拍 — auction 纯内存 `&self` 计算,顺序 N 次即最优;
        //    NoBid → 兜底派发（与单发一致,防饿死 ADR-148/RK-P14）。
        //    竞拍无失败路径（Won/NoBid 均可派发）,"全部竞拍成功"恒成立,
        //    原子性由 1/2 的前置检查独立保证——失败即发生在任何副作用之前。
        let mut prepared: Vec<(String, SubAgentSpec, SubAgentTask)> =
            Vec::with_capacity(batch.len());
        for (spec, task) in batch {
            let offer = TaskOffer {
                task_id: format!("task-{}", Uuid::now_v7()),
                required_capabilities: spec.kind.capability_tag().into(),
            };
            let winner = match self.auction.auction(&offer) {
                AuctionOutcome::Won(b) => Some(b.profile_id.clone()),
                AuctionOutcome::NoBid => None, // 兜底:无档案也派发（默认执行引擎）
            };
            let _ = winner; // 档案 ID 供审计;执行体由 task 提供（同引擎换参）
            prepared.push((offer.task_id, spec, task));
        }
        // 4. 批量提交 — 全部竞拍完成后守卫写锁整批 1 次（登记全部 task_id）,
        //    再循环计数 + 派发:守卫锁合计读 1 + 写 1（逐次 spawn 为 2N）。
        {
            let mut guard = self
                .nesting_guard
                .write()
                .unwrap_or_else(|p| p.into_inner());
            for (task_id, _, _) in &prepared {
                guard.insert(task_id.clone(), true);
            }
        }
        let mut handles = Vec::with_capacity(prepared.len());
        for (task_id, spec, task) in prepared {
            self.spawned_total += 1;
            handles.push(self.spawn_inner(task_id, spec, task));
        }
        Ok(handles)
    }

    /// 单任务提交体 — 构造闭包并派发进 JoinSet（[`spawn`](Self::spawn) 与
    /// [`spawn_batch`](Self::spawn_batch) 共用,防止闭包逻辑双份漂移）
    ///
    /// WHY 不含守卫登记/计数:批量路径要求守卫写锁整批仅 1 次（逐个 insert
    /// 会退化为 N 次锁操作）,故登记与计数由调用方按各自语义负责——单发
    /// 原位登记,批量一次性批量登记。
    ///
    /// 执行语义（WP-D 重构自原单发实现原样移动,行为不变）:
    /// - 预取消检查（四因:父级撤销可预取消）→ 分类 [`SubAgentError::Cancelled`];
    /// - `spawn_blocking` 隔离任务体,JoinError(panic) → 分类
    ///   [`SubAgentError::Panicked`]（不重试:panic 是实现缺陷,重试通常再次 panic）;
    /// - JoinSet 有界并发（LLM 类任务禁 rayon,E8-2）。
    fn spawn_inner(
        &mut self,
        task_id: String,
        spec: SubAgentSpec,
        task: SubAgentTask,
    ) -> SubAgentHandle {
        let task_id_for_closure = task_id.clone();
        let cancel = Arc::new(CancellationToken::new());
        let cancel_for_task = Arc::clone(&cancel);
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
        SubAgentHandle { task_id, cancel }
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
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为 Rust 惯用法；E-5 lint 意在治理生产码
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

    /// 规模上限 — 活跃 ≥ 8 拒绝,且满载（SwarmCapacityExceeded）与嵌套
    /// （NestedSubAgentForbidden）两路径变体可区分（C-23 可诊断性要求）
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
        // 满载必须以专用变体拒绝,携带 active/limit 快照（≠ 嵌套变体,可诊断区分）
        assert!(
            matches!(
                err,
                Err(NexusError::SwarmCapacityExceeded {
                    active: SWARM_LIMIT,
                    limit: SWARM_LIMIT,
                })
            ),
            "满载必须以 SwarmCapacityExceeded{{active: {SWARM_LIMIT}, limit: {SWARM_LIMIT}}} 拒绝"
        );
        assert!(
            !matches!(err, Err(NexusError::NestedSubAgentForbidden)),
            "满载不得再复用嵌套变体（C-23 前的保守行为,已退役）"
        );
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

    // ============ spawn_batch（WP-D:批量竞拍 + 批量派发,原子前置检查）============

    /// 批量原子容量 — 预派 6 个占容量后批量 3 个(6+3 > 8)必须**整批拒绝**
    /// 且零副作用(active_count 不变);容量释放后同规模批量成功。
    ///
    /// WHY 此测试:逐次 spawn 在第 5 个撞限时前 4 个已派发("部分成功");
    /// 批量在派发前一次性判定总量,前置失败 → 一个任务都不派发。
    #[tokio::test]
    async fn spawn_batch_atomic_capacity() {
        let mut rt = SubAgentRuntime::new();
        // 预派 6 个占容量(SWARM_LIMIT=8,余 2)
        for i in 0..6 {
            let _ = rt
                .spawn(
                    SubAgentSpec::new(SubAgentKind::Explore),
                    Box::new(move |_s, _c| Ok(format!("pre{i}"))),
                    None,
                )
                .expect("前 6 个必须成功");
        }
        assert_eq!(rt.active_count(), 6);
        // 批量 3 个:6+3 > 8 → 整批拒绝,携带 active/limit 快照(与单发同变体)
        let batch: Vec<(SubAgentSpec, SubAgentTask)> = (0..3)
            .map(|i| {
                (
                    SubAgentSpec::new(SubAgentKind::Explore),
                    Box::new(move |_s: SubAgentSpec, _c: Arc<CancellationToken>| {
                        Ok(format!("b{i}"))
                    }) as SubAgentTask,
                )
            })
            .collect();
        let err = rt.spawn_batch(batch, None);
        assert!(
            matches!(
                err,
                Err(NexusError::SwarmCapacityExceeded {
                    active: 6,
                    limit: SWARM_LIMIT
                })
            ),
            "整批必须以 SwarmCapacityExceeded{{active: 6, limit: {SWARM_LIMIT}}} 拒绝"
        );
        // 零副作用:一个都没派发
        assert_eq!(rt.active_count(), 6, "整批拒绝后活跃数必须仍是 6(零副作用)");
        // join 全部释放容量后,同规模批量派发成功
        while rt.join_next().await.is_some() {}
        let batch2: Vec<(SubAgentSpec, SubAgentTask)> = (0..3)
            .map(|i| {
                (
                    SubAgentSpec::new(SubAgentKind::Explore),
                    Box::new(move |_s: SubAgentSpec, _c: Arc<CancellationToken>| {
                        Ok(format!("r{i}"))
                    }) as SubAgentTask,
                )
            })
            .collect();
        let handles = rt
            .spawn_batch(batch2, None)
            .expect("容量释放后批量必须成功");
        assert_eq!(handles.len(), 3);
        assert_eq!(rt.active_count(), 3);
        while rt.join_next().await.is_some() {}
    }

    /// 批量与逐次 spawn 等价 — 同配置(3 档案注册 + sleep 10ms 任务体)下,
    /// 批量派发的完成数/结果/任务 ID 形态与逐次 spawn 同批次完全一致。
    #[tokio::test]
    async fn spawn_batch_matches_sequential_spawn() {
        let mut batch_rt = SubAgentRuntime::new();
        let mut seq_rt = SubAgentRuntime::new();
        for rt in [&mut batch_rt, &mut seq_rt] {
            rt.register(SubAgentProfile::new("coder-a", SubAgentKind::Coder, 2.0));
            rt.register(SubAgentProfile::new(
                "explore-b",
                SubAgentKind::Explore,
                1.0,
            ));
            rt.register(SubAgentProfile::new("plan-c", SubAgentKind::Plan, 1.0));
        }
        // 批量派 3 任务(各 sleep 10ms 后返回标识文案)
        let batch: Vec<(SubAgentSpec, SubAgentTask)> = SubAgentKind::ALL
            .into_iter()
            .map(|kind| {
                (
                    SubAgentSpec::new(kind),
                    Box::new(move |spec: SubAgentSpec, _c: Arc<CancellationToken>| {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        Ok(format!("done:{}", spec.kind.capability_tag()))
                    }) as SubAgentTask,
                )
            })
            .collect();
        let handles = batch_rt.spawn_batch(batch, None).expect("批量派发成功");
        assert_eq!(handles.len(), 3, "句柄数与输入同长且同序");
        assert_eq!(batch_rt.active_count(), 3);
        // 对照组:逐次 spawn 同批任务
        for kind in SubAgentKind::ALL {
            seq_rt
                .spawn(
                    SubAgentSpec::new(kind),
                    Box::new(move |spec: SubAgentSpec, _c: Arc<CancellationToken>| {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        Ok(format!("done:{}", spec.kind.capability_tag()))
                    }),
                    None,
                )
                .expect("逐次派发成功");
        }
        // 两路径结果形态一致:完成数 3、结果 Ok、id 含 "task-"
        for (rt, label) in [(&mut batch_rt, "batch"), (&mut seq_rt, "sequential")] {
            let mut done = 0;
            while let Some((id, result)) = rt.join_next().await {
                assert!(result.is_ok(), "{label} 结果必须 Ok");
                assert!(id.contains("task-"), "{label} id 必须含 task- 前缀: {id}");
                done += 1;
            }
            assert_eq!(done, 3, "{label} 完成数必须为 3");
        }
        assert_eq!(batch_rt.spawned_total(), 3);
        assert_eq!(seq_rt.spawned_total(), 3);
    }

    /// 空批快速路径 — `Ok(vec![])` 且零副作用(不触发锁与检查)
    #[tokio::test]
    async fn spawn_batch_empty_is_ok() {
        let mut rt = SubAgentRuntime::new();
        let handles = rt.spawn_batch(Vec::new(), None).expect("空批必须 Ok");
        assert!(handles.is_empty());
        assert_eq!(rt.active_count(), 0);
        assert_eq!(rt.spawned_total(), 0, "空批必须零副作用");
    }

    /// 批量嵌套禁止 — from_task 指向运行中任务 → 整批
    /// [`NexusError::NestedSubAgentForbidden`] 零副作用(与单发共享同一嵌套
    /// 断言语义,读锁整批 1 次)
    #[tokio::test]
    async fn spawn_batch_nested_forbidden() {
        let mut rt = SubAgentRuntime::new();
        // 模拟:任务 A 运行中,Batch 由 A 发起 → 整批拒绝
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
        let batch: Vec<(SubAgentSpec, SubAgentTask)> = (0..2)
            .map(|_| {
                (
                    SubAgentSpec::new(SubAgentKind::Explore),
                    Box::new(|_s: SubAgentSpec, _c: Arc<CancellationToken>| Ok("nested".into()))
                        as SubAgentTask,
                )
            })
            .collect();
        let err = rt.spawn_batch(batch, Some(&handle_a.task_id));
        assert!(
            matches!(err, Err(NexusError::NestedSubAgentForbidden)),
            "嵌套批量必须整批拒绝"
        );
        assert_eq!(rt.active_count(), 1, "拒绝后只有 A 活跃(零副作用)");
        while rt.join_next().await.is_some() {}
    }
}
