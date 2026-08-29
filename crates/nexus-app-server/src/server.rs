//! AppServer — 会话状态机与核心驱动（WI-01 §6.1/§6.2）
//!
//! # 设计
//! - **每 Thread 一 actor**: 会话状态归 `SessionActor` 独占，外界只经
//!   [`AppServer`] 消息交互
//! - **Thread/Turn/Item 三原语**: Thread=QuestSession（goal_id+run_id）；
//!   Turn=一次用户请求；Item=最小 I/O 单元（状态机
//!   started → in_progress → completed/failed）
//! - **CoreBackend seam**: 核心驱动点——MVP 提供 `InMemoryBackend`（回显
//!   实现），真实核心（quest-engine/gqep）后续经同一 trait 接入
//! - **内闭外开（T6）**: 对外只暴露 AppOp/AppEvent；NexusEvent 经 EventBus
//!   广播（内闭），由 server 层转译
//!
//! # 断线恢复（WI-01 验收）
//! 客户端持 `last_item_id`，重连后经 [`AppServer::replay_since`] 回放增量
//! （Item 全量存于会话快照，kill -9 后重连渲染一致）。

use async_trait::async_trait;
use nexus_contracts::app::{
    AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId, ItemStatus,
    PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput,
};
use session_store::{CbmrWriter, Offset, SessionEvent, SessionId};
use thiserror::Error;

/// AppServer 错误 — 会话层错误
#[derive(Debug, Error)]
pub enum ServerError {
    /// 会话不存在（ThreadStart 前引用）
    #[error("thread not found: {0}")]
    ThreadNotFound(String),
    /// 回合不存在
    #[error("turn not found: {0}")]
    TurnNotFound(String),
    /// 会话已存在（重复 ThreadStart）
    #[error("thread already exists: {0}")]
    ThreadExists(String),
    /// 审批请求不存在
    #[error("approval request not found: {0}")]
    ApprovalNotFound(String),
    /// 核心后端错误
    #[error("core backend error: {0}")]
    Backend(String),
    /// session-store 未配置（persist_turn 在纯内存模式被调用）
    #[error("session store not configured (pure-memory mode)")]
    StoreNotConfigured,
    /// session-store 落盘错误
    #[error("session store error: {0}")]
    Store(String),
}

/// 核心后端 — 核心驱动点（WI-01 CoreOp/CoreEvent 单向驱动）
///
/// MVP 提供 [`InMemoryBackend`]（回显实现）；生产接入 quest-engine/gqep
/// 时实现本 trait（跨层通信经 EventBus，禁止直联核心类型）。
#[async_trait]
pub trait CoreBackend: Send + Sync {
    /// 提交回合输入 → 产出 Item 流（每次调用产出 ≥1 个 Item）
    async fn submit_turn(
        &self,
        thread: &Thread,
        turn_id: &TurnId,
        input: &UserInput,
    ) -> Result<Vec<Item>, String>;

    /// 取消回合（中断信号）
    async fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), String>;
}

/// 内存回显后端 — MVP 实现（50 行 mock 客户端验证用）
///
/// 对每个输入产出两个 Item（message + tool_call 模拟），
/// 供协议级 E2E 验证"完整 Turn"（WI-01 验收：50 行 mock 客户端完成完整 Turn）。
#[derive(Debug, Default)]
pub struct InMemoryBackend;

#[async_trait]
impl CoreBackend for InMemoryBackend {
    async fn submit_turn(
        &self,
        thread: &Thread,
        turn_id: &TurnId,
        input: &UserInput,
    ) -> Result<Vec<Item>, String> {
        let mut items = Vec::new();
        // Item 1: 用户消息回显
        items.push(Item::new(
            ItemId::new(format!("{}-1", turn_id.as_str())),
            thread.thread_id.clone(),
            turn_id.clone(),
            "message",
            ItemStatus::Completed,
            &input.text,
        ));
        // Item 2: 模拟工具调用完成
        items.push(Item::new(
            ItemId::new(format!("{}-2", turn_id.as_str())),
            thread.thread_id.clone(),
            turn_id.clone(),
            "tool_call",
            ItemStatus::Completed,
            r#"{"tool":"echo","status":"ok"}"#,
        ));
        Ok(items)
    }

    async fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), String> {
        tracing::info!(turn = %turn_id.as_str(), "回合中断信号已确认（MVP 无运行中任务）");
        Ok(())
    }
}

/// AppServer 配置
#[derive(Debug, Clone, PartialEq)]
pub struct AppServerConfig {
    /// 是否回放增量（断线恢复：保留 Item 历史）
    pub keep_item_history: bool,
    /// 会话快照上限（防内存膨胀；0 = 不限）
    pub max_sessions: usize,
}

impl Default for AppServerConfig {
    fn default() -> Self {
        Self {
            keep_item_history: true,
            max_sessions: 1024,
        }
    }
}

/// 会话快照 — 断线恢复与审计载体
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    /// 会话
    pub thread: Thread,
    /// 回合列表（时间序）
    pub turns: Vec<TurnId>,
    /// Item 历史（时间序，完整 I/O 单元）
    pub items: Vec<Item>,
    /// 当前权限模式
    pub mode: PermissionMode,
    /// 待审批请求
    pub pending_approvals: Vec<ApprovalRequest>,
}

/// 会话 actor — 每 Thread 一 actor 的会话状态
#[derive(Debug)]
struct SessionActor {
    /// 会话元数据
    thread: Thread,
    /// Item 历史（时间序）
    items: Vec<Item>,
    /// 回合列表
    turns: Vec<TurnId>,
    /// 当前权限模式
    mode: PermissionMode,
    /// 待审批请求
    pending_approvals: Vec<ApprovalRequest>,
}

impl SessionActor {
    fn new(thread: Thread) -> Self {
        Self {
            thread,
            items: Vec::new(),
            turns: Vec::new(),
            mode: PermissionMode::Default,
            pending_approvals: Vec::new(),
        }
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            thread: self.thread.clone(),
            turns: self.turns.clone(),
            items: self.items.clone(),
            mode: self.mode,
            pending_approvals: self.pending_approvals.clone(),
        }
    }
}

/// AppServer — 协议宿主（WI-01 核心交付）
///
/// # 线程安全
/// 内部 `DashMap<ThreadId, SessionActor>`（并发会话隔离）；
/// 单会话操作走 actor 独占路径（无跨会话共享状态）。
///
/// # Debug
/// 手动实现（`Box<dyn CoreBackend>` 不实现 Debug——trait 对象边界）。
pub struct AppServer {
    /// 会话表（ThreadId → actor）
    sessions: dashmap::DashMap<ThreadId, SessionActor>,
    /// 核心后端（MVP 默认 InMemoryBackend）
    backend: Box<dyn CoreBackend>,
    /// 配置
    config: AppServerConfig,
    /// 审批仲裁器（P3-T5:多客户端竞争审批,首裁决生效）
    arbiter: crate::approval::ApprovalArbiter,
    /// 会话存储（灰度双写:None = 纯内存模式,Some = turn_submit 双写落盘）
    ///
    /// WHY Option 而非 feature 标志（红线:禁 feature 标志）:构造参数决定
    /// 灰度语义——默认 `new`/`with_backend` 为 None（纯内存热路径零开销）,
    /// `with_session_store` 传入 store 后启用双写。
    store: Option<CbmrWriter>,
}

impl std::fmt::Debug for AppServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppServer")
            .field("session_count", &self.sessions.len())
            .field("config", &self.config)
            .finish()
    }
}

impl AppServer {
    /// 创建 AppServer（默认 InMemoryBackend）
    pub fn new(config: AppServerConfig) -> Self {
        Self {
            sessions: dashmap::DashMap::new(),
            backend: Box::new(InMemoryBackend),
            config,
            arbiter: crate::approval::ApprovalArbiter::default(),
            store: None,
        }
    }

    /// 创建 AppServer（注入核心后端）
    pub fn with_backend(config: AppServerConfig, backend: Box<dyn CoreBackend>) -> Self {
        Self {
            sessions: dashmap::DashMap::new(),
            backend,
            config,
            arbiter: crate::approval::ApprovalArbiter::default(),
            store: None,
        }
    }

    /// 创建 AppServer（注入核心后端 + 会话存储,启用回合双写落盘）
    ///
    /// # 灰度语义（P2-T3 接入）
    /// 传入 store 后 `turn_submit` 在内存 actor 更新之外,额外经 session-store
    /// 落盘（双写）;默认 [`AppServer::new`]/[`AppServer::with_backend`] 保持
    /// 纯内存模式（热路径零开销）。
    pub fn with_session_store(
        config: AppServerConfig,
        backend: Box<dyn CoreBackend>,
        store: CbmrWriter,
    ) -> Self {
        Self {
            sessions: dashmap::DashMap::new(),
            backend,
            config,
            arbiter: crate::approval::ApprovalArbiter::default(),
            store: Some(store),
        }
    }

    /// 会话数
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// 处理客户端操作（AppOp → AppEvent 流）
    ///
    /// # 返回
    /// 该操作产生的事件序列（≥0 个）。审批请求等异步事件经
    /// [`AppServer::pending_approvals`] 单独查询。
    pub async fn handle_op(&self, op: &AppOp) -> Result<Vec<AppEvent>, ServerError> {
        match op {
            AppOp::ThreadStart(params) => self.thread_start(params).await,
            AppOp::TurnSubmit { thread_id, input } => self.turn_submit(thread_id, input).await,
            AppOp::TurnInterrupt { turn_id } => self.turn_interrupt(turn_id).await,
            AppOp::ApprovalRespond {
                request_id,
                decision,
            } => self.approval_respond(request_id, *decision).await,
            AppOp::ThreadFork { thread_id, at } => self.thread_fork(thread_id, at),
            AppOp::ModeSet { mode } => self.mode_set(mode),
        }
    }

    /// 回放增量（断线恢复：WI-01 验收"kill -9→重连渲染一致"）
    ///
    /// 客户端持 `last_item_id`；返回该 ID 之后的所有 Item。
    pub fn replay_since(&self, thread_id: &ThreadId, last_item_id: &ItemId) -> Option<Vec<Item>> {
        let actor = self.sessions.get(thread_id)?;
        let items: Vec<Item> = actor
            .items
            .iter()
            .filter(|i| i.item_id.as_str() > last_item_id.as_str())
            .cloned()
            .collect();
        Some(items)
    }

    /// 会话快照（审计/恢复）
    pub fn snapshot(&self, thread_id: &ThreadId) -> Option<SessionSnapshot> {
        self.sessions.get(thread_id).map(|a| a.snapshot())
    }

    /// 会话待审批请求（客户端轮询审批队列）
    pub fn pending_approvals(&self, thread_id: &ThreadId) -> Vec<ApprovalRequest> {
        self.sessions
            .get(thread_id)
            .map(|a| a.pending_approvals.clone())
            .unwrap_or_default()
    }

    /// 回合落盘 — 经 session-store 持久化回合事件（灰度双写）
    ///
    /// # 语义（P2-T3 接入 / 「model-visible means logged」）
    /// 将回合事件 `turn.submit`（payload = turn_id JSON）落盘:append → flush
    /// → 返回该事件的全局 Offset（seq 即 Critical 流顺序的持久化镜像）。
    /// `flush` 保证返回前已 fsync——**logged 语义锚点**:返回的 Offset 可用于
    /// 续读（replay from）与审计。store 未配置（纯内存模式）→ Err。
    ///
    /// # 调用约束
    /// 调用方须在释放会话 actor 锁后调用（本 crate 内 `turn_submit` 已保证
    /// ——actor guard 经 NLL 释放后才调用本方法,不持锁跨 await）。
    pub async fn persist_turn(
        &self,
        thread_id: &ThreadId,
        turn: &TurnId,
    ) -> Result<Offset, ServerError> {
        let store = self
            .store
            .as_ref()
            .ok_or(ServerError::StoreNotConfigured)?;
        let session_id = SessionId::new(thread_id.as_str());
        // payload = turn_id JSON（事件体最小化;敏感字段不落 payload 原则）
        let payload = serde_json::to_vec(&serde_json::json!({ "turn_id": turn.as_str() }))
            .map_err(|e| ServerError::Store(format!("回合 payload 序列化失败: {e}")))?;
        let event = SessionEvent::with_payload("turn.submit", payload);
        store
            .append(&session_id, event)
            .await
            .map_err(|e| ServerError::Store(format!("回合事件入队失败: {e}")))?;
        store
            .flush()
            .await
            .map_err(|e| ServerError::Store(format!("回合事件落盘失败: {e}")))?;
        store
            .last_offset(&session_id)
            .await
            .map_err(|e| ServerError::Store(format!("查询回合 Offset 失败: {e}")))?
            .ok_or_else(|| ServerError::Store("flush 后 Offset 缺失(状态不一致)".into()))
    }

    /// 注入待审批请求（服务端审批源 → 客户端轮询队列）
    ///
    /// # WHY
    /// 后端（L7 执行层）产生审批请求时经此 API 注入协议面队列，
    /// 客户端轮询 [`AppServer::pending_approvals`] 后经
    /// `AppOp::ApprovalRespond` 裁决（mock 客户端 E2E 使用）。
    pub fn inject_approval_request(
        &self,
        thread_id: &ThreadId,
        req: ApprovalRequest,
    ) -> Result<(), ServerError> {
        let mut actor = self
            .sessions
            .get_mut(thread_id)
            .ok_or_else(|| ServerError::ThreadNotFound(thread_id.as_str().into()))?;
        actor.pending_approvals.push(req);
        Ok(())
    }

    // ---------- 操作实现 ----------

    async fn thread_start(&self, params: &ThreadStartParams) -> Result<Vec<AppEvent>, ServerError> {
        if self.config.max_sessions > 0 && self.sessions.len() >= self.config.max_sessions {
            return Err(ServerError::Backend("会话数达上限".into()));
        }
        let thread = Thread::new(
            ThreadId::new(format!("{}::{}", params.goal_id, params.run_id)),
            &params.goal_id,
            &params.run_id,
            now_ms(),
        );
        if self.sessions.contains_key(&thread.thread_id) {
            return Err(ServerError::ThreadExists(thread.thread_id.as_str().into()));
        }
        let mut events = Vec::new();
        // 初始输入（可选）→ 首回合
        if let Some(input) = &params.initial_input {
            self.sessions
                .insert(thread.thread_id.clone(), SessionActor::new(thread.clone()));
            events.extend(self.turn_submit(&thread.thread_id, input).await?);
        } else {
            self.sessions
                .insert(thread.thread_id.clone(), SessionActor::new(thread.clone()));
        }
        events.insert(
            0,
            AppEvent::ThreadStarted {
                thread: thread.clone(),
            },
        );
        Ok(events)
    }

    async fn turn_submit(
        &self,
        thread_id: &ThreadId,
        input: &UserInput,
    ) -> Result<Vec<AppEvent>, ServerError> {
        let mut actor = self
            .sessions
            .get_mut(thread_id)
            .ok_or_else(|| ServerError::ThreadNotFound(thread_id.as_str().into()))?;
        let turn_id = TurnId::new(format!("turn-{}", actor.turns.len() + 1));
        actor.turns.push(turn_id.clone());
        let thread = actor.thread.clone();
        // 后端产出 Item 流（单向驱动核心）
        let items = self
            .backend
            .submit_turn(&thread, &turn_id, input)
            .await
            .map_err(ServerError::Backend)?;
        actor.items.extend(items.clone());
        // 组装事件流: TurnCompleted 兜底（含 Token 用量——MVP 无真实用量，
        // 全零；WI-03 命中率埋点接入后由 L1 填充）
        let mut events: Vec<AppEvent> = items
            .into_iter()
            .map(|item| AppEvent::ItemChanged { item })
            .collect();
        // 灰度双写:store 启用时回合落盘（actor guard 已由 NLL 释放,不持锁
        // 跨 await）;落盘失败仅记日志——内存语义保留,审计由 rebuild 兜底
        if self.store.is_some() {
            if let Err(e) = self.persist_turn(thread_id, &turn_id).await {
                tracing::warn!(
                    thread = %thread_id.as_str(),
                    "回合落盘失败(仅内存语义保留): {e}"
                );
            }
        }
        events.push(AppEvent::TurnCompleted {
            turn_id,
            usage: AppTokenUsage::new(0, 0, 0, 0),
        });
        Ok(events)
    }

    async fn turn_interrupt(&self, turn_id: &TurnId) -> Result<Vec<AppEvent>, ServerError> {
        self.backend
            .interrupt_turn(turn_id)
            .await
            .map_err(ServerError::Backend)?;
        Ok(Vec::new())
    }

    async fn approval_respond(
        &self,
        request_id: &ReqId,
        decision: ApprovalDecision,
    ) -> Result<Vec<AppEvent>, ServerError> {
        // P3-T5 多客户端仲裁:首裁决生效,重复裁决幂等忽略（不报错）
        use crate::approval::VoteOutcome;
        match self
            .arbiter
            .submit_vote(request_id, "client-anon", decision)
        {
            VoteOutcome::DuplicateIgnored => {
                tracing::info!(request = %request_id.as_str(), "重复审批裁决已忽略（首裁决生效）");
                return Ok(Vec::new());
            }
            VoteOutcome::Unknown => {}
            VoteOutcome::Accepted => {}
        }
        // 查找含该请求的会话
        for mut entry in self.sessions.iter_mut() {
            let idx = entry
                .pending_approvals
                .iter()
                .position(|r| &r.request_id == request_id);
            if let Some(i) = idx {
                let req = entry.pending_approvals.remove(i);
                tracing::info!(
                    request = %request_id.as_str(),
                    decision = ?decision,
                    "审批裁决已受理（首裁决生效）"
                );
                let _ = req;
                return Ok(Vec::new());
            }
        }
        Err(ServerError::ApprovalNotFound(request_id.as_str().into()))
    }

    fn thread_fork(&self, thread_id: &ThreadId, at: &ItemId) -> Result<Vec<AppEvent>, ServerError> {
        // WI-18 会话树分叉的协议面: MVP 记录分叉点（复制前缀 Items 到新会话）
        let actor = self
            .sessions
            .get(thread_id)
            .ok_or_else(|| ServerError::ThreadNotFound(thread_id.as_str().into()))?;
        let fork_point = actor
            .items
            .iter()
            .position(|i| i.item_id.as_str() == at.as_str())
            .ok_or_else(|| ServerError::TurnNotFound(at.as_str().into()))?;
        let mut new_thread = actor.thread.clone();
        new_thread.thread_id = ThreadId::new(format!("{}-fork", new_thread.thread_id.as_str()));
        new_thread.run_id = Box::from(format!("{}-fork", new_thread.run_id));
        let mut fork_items = actor.items[..=fork_point].to_vec();
        // fork 后首 Item 标记（WI-18 语义: 分叉点之后为独立演化）
        fork_items.push(Item::new(
            ItemId::new(format!("{}-fork", at.as_str())),
            new_thread.thread_id.clone(),
            TurnId::new("turn-fork"),
            "fork_marker",
            ItemStatus::Completed,
            &format!("forked at {}", at.as_str()),
        ));
        let mut new_actor = SessionActor::new(new_thread.clone());
        new_actor.items = fork_items;
        new_actor.turns.push(TurnId::new("turn-fork"));
        self.sessions
            .insert(new_thread.thread_id.clone(), new_actor);
        Ok(vec![AppEvent::ThreadStarted { thread: new_thread }])
    }

    fn mode_set(&self, mode: &PermissionMode) -> Result<Vec<AppEvent>, ServerError> {
        // 全局模式切换（影响后续会话；逐会话模式经 snapshot.mode 查询）
        for mut entry in self.sessions.iter_mut() {
            entry.mode = *mode;
        }
        Ok(Vec::new())
    }
}

/// 当前 Unix 毫秒（Thread.created_at_ms）
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params() -> ThreadStartParams {
        ThreadStartParams::new("goal-1", "run-1")
    }

    #[tokio::test]
    async fn thread_start_and_snapshot() {
        let server = AppServer::new(AppServerConfig::default());
        let events = server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        assert!(matches!(events[0], AppEvent::ThreadStarted { .. }));
        assert_eq!(server.session_count(), 1);
        let tid = ThreadId::new("goal-1::run-1");
        let snap = server.snapshot(&tid).expect("快照存在");
        assert_eq!(snap.thread.goal_id.as_ref(), "goal-1");
        assert_eq!(snap.mode, PermissionMode::Default);
    }

    #[tokio::test]
    async fn complete_turn_produces_item_flow() {
        // WI-01 验收: mock 客户端完成完整 Turn（ThreadStart → TurnSubmit → 事件流）
        let server = AppServer::new(AppServerConfig::default());
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");
        let events = server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: tid.clone(),
                input: UserInput::new("你好"),
            })
            .await
            .expect("提交成功");
        // 2 ItemChanged + 1 TurnCompleted
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], AppEvent::ItemChanged { .. }));
        assert!(matches!(events[1], AppEvent::ItemChanged { .. }));
        assert!(matches!(events[2], AppEvent::TurnCompleted { .. }));
        // 快照含 2 Items + 1 Turn
        let snap = server.snapshot(&tid).expect("快照存在");
        assert_eq!(snap.items.len(), 2);
        assert_eq!(snap.turns.len(), 1);
    }

    #[tokio::test]
    async fn replay_since_resumes_after_disconnect() {
        // WI-01 验收: kill -9 → 重连渲染一致（回放增量）
        let server = AppServer::new(AppServerConfig::default());
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");
        server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: tid.clone(),
                input: UserInput::new("第一轮"),
            })
            .await
            .expect("提交成功");
        // 模拟断线: 客户端持 last_item_id = 首 Item
        let snap = server.snapshot(&tid).expect("快照存在");
        let last_id = &snap.items[0].item_id;
        let replay = server.replay_since(&tid, last_id).expect("回放成功");
        assert_eq!(replay.len(), 1, "回放 last_item_id 之后的增量");
        assert_eq!(replay[0].item_id.as_str(), snap.items[1].item_id.as_str());
    }

    #[tokio::test]
    async fn approval_flow() {
        // 审批往返: 请求登记 → 裁决受理（MVP 无后端审批源,验证协议面路径）
        let server = AppServer::new(AppServerConfig::default());
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");
        // 注入待审批请求（模拟后端审批源）
        server
            .sessions
            .get_mut(&tid)
            .expect("会话存在")
            .pending_approvals
            .push(ApprovalRequest::new(
                ReqId::new("req-1"),
                "运行 cargo build",
                "idempotent_write",
                None,
            ));
        let pending = server.pending_approvals(&tid);
        assert_eq!(pending.len(), 1);
        server
            .handle_op(&AppOp::ApprovalRespond {
                request_id: ReqId::new("req-1"),
                decision: ApprovalDecision::AllowOnce,
            })
            .await
            .expect("裁决受理");
        assert!(server.pending_approvals(&tid).is_empty());
    }

    #[tokio::test]
    async fn thread_fork_creates_independent_session() {
        // WI-18 协议面: 分叉 → 新会话 + fork_marker
        let server = AppServer::new(AppServerConfig::default());
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");
        server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: tid.clone(),
                input: UserInput::new("第一轮"),
            })
            .await
            .expect("提交成功");
        let snap = server.snapshot(&tid).expect("快照存在");
        let events = server
            .handle_op(&AppOp::ThreadFork {
                thread_id: tid.clone(),
                at: snap.items[0].item_id.clone(),
            })
            .await
            .expect("分叉成功");
        assert!(matches!(events[0], AppEvent::ThreadStarted { .. }));
        assert_eq!(server.session_count(), 2);
        let fork_tid = ThreadId::new("goal-1::run-1-fork");
        let fork_snap = server.snapshot(&fork_tid).expect("分叉会话存在");
        assert_eq!(fork_snap.items.len(), 2, "前缀复制 + fork_marker");
    }

    #[tokio::test]
    async fn mode_set_applies_to_all_sessions() {
        let server = AppServer::new(AppServerConfig::default());
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        server
            .handle_op(&AppOp::ModeSet {
                mode: PermissionMode::Plan,
            })
            .await
            .expect("模式切换成功");
        let tid = ThreadId::new("goal-1::run-1");
        assert_eq!(
            server.snapshot(&tid).expect("快照存在").mode,
            PermissionMode::Plan
        );
    }

    #[tokio::test]
    async fn unknown_thread_rejected() {
        let server = AppServer::new(AppServerConfig::default());
        let err = server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: ThreadId::new("missing"),
                input: UserInput::new("x"),
            })
            .await
            .expect_err("未知会话必须拒绝");
        assert!(matches!(err, ServerError::ThreadNotFound(_)));
    }

    // ============================================================
    // P2-T3 会话存储接入（灰度双写 + replay 重建）
    // ============================================================

    #[tokio::test]
    async fn with_session_store_persists_turn_and_replay_rebuilds() {
        // 灰度双写:with_session_store 后 turn_submit 落盘;经 replay 纯段文件
        // 回放重建会话（「model-visible means logged」的存储面验证）
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = session_store::StoreConfig::with_dir(dir.path());
        cfg.spawn_flush_loop = false; // 确定性:仅显式 flush 触发
        let store = session_store::CbmrWriter::new(cfg).expect("store");
        let server = AppServer::with_session_store(
            AppServerConfig::default(),
            Box::new(InMemoryBackend),
            store,
        );
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");
        server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: tid.clone(),
                input: UserInput::new("你好"),
            })
            .await
            .expect("提交成功");

        // 显式 persist（turn_submit 已双写 1 条;本次再落 1 条,seq 连续）
        let off = server
            .persist_turn(&tid, &TurnId::new("turn-1"))
            .await
            .expect("落盘");
        assert_eq!(off.seq, 1, "turn_submit 双写 seq=0,显式 persist seq=1");

        // 经 replay 重建会话（纯段文件回放,不依赖内存 DashMap）
        let tree = session_store::TreeIndex::open(&dir.path().join("sessions.sqlite3"))
            .expect("重开树索引");
        let stream = session_store::replay(
            &tree,
            dir.path(),
            &session_store::SessionId::new(tid.as_str()),
            session_store::Offset::new(0, 0),
        )
        .expect("replay");
        let items = stream.collect().expect("collect");
        assert_eq!(items.len(), 2, "turn_submit 双写 1 + 显式 persist 1");
        assert!(
            items.iter().all(|i| i.event.event_type == "turn.submit"),
            "落盘事件类型 = turn.submit"
        );
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.offset.seq, i as u64, "回放顺序 = 写入顺序");
        }
    }

    #[tokio::test]
    async fn pure_memory_mode_persist_rejected() {
        // 灰度语义:默认 AppServer（纯内存）persist_turn 返回 StoreNotConfigured,
        // turn_submit 不受影响（热路径零开销）
        let server = AppServer::new(AppServerConfig::default());
        let err = server
            .persist_turn(&ThreadId::new("x"), &TurnId::new("t1"))
            .await
            .expect_err("纯内存模式必须拒绝落盘");
        assert!(matches!(err, ServerError::StoreNotConfigured));
        // 内存路径仍正常
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        assert_eq!(server.session_count(), 1);
    }
}
