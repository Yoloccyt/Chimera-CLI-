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
//!
//! # threads 404 回放桥（ADR-292 D4，P3 桥——读面从预留转生产）
//!
//! ADR-292 修订 ADR-290 D4（追溯 ADR-259 D3/U-08 的预留裁决）：Anthropic
//! Message threads 的 `continue` 返回 404（服务端线程 best-effort，空闲过期）
//! 时，harness 必须重新 `create`——按 Thread 段**完整回放重建对话**。
//! 本文件承载桥的两端：
//!
//! - **写面**：`thread_start` 落 `thread.started`（流首事件契约）、`turn_submit`
//!   落 `item.append`（D1 schema，append-only，不动 `turn.submit` 既有路径）；
//! - **读面**：[`AppServer::replay_thread_from_store`] 线程级回放入口（经
//!   `CbmrWriter::replay_session` 段文件权威源 → `rebuild_items` 纯函数重构），
//!   生产调用点为 [`AppServer::replay_since`] 的 store 兜底分支——内存态线程
//!   丢失（进程重启后 DashMap 为空，即 404 的本侧形态）时从磁盘权威源重建。
//!
//! 落盘均为 N-4 best-effort（失败仅 warn，不影响内存语义）；ledger_parity_locks
//! 锁 5 已按 ADR-292 D5 翻转为接生产行为锁（读面调用缺失即判红）。

use async_trait::async_trait;
use nexus_contracts::app::{
    AppEvent, AppOp, AppTokenUsage, ApprovalDecision, ApprovalRequest, Item, ItemId, ItemStatus,
    PermissionMode, ReqId, Thread, ThreadId, ThreadStartParams, TurnId, UserInput,
};
use session_store::{
    item_append_event, rebuild_items, thread_started_event, to_model_view, CbmrWriter,
    ModelVisibleEvent, Offset, SessionEvent, SessionId,
};
use thiserror::Error;

/// 核心后端错误 — `CoreBackend` seam 的结构化错误契约
///
/// # WHY 不用 `String`（原实现即 `Result<_, String>`）
/// 该 seam 是 L10 宿主与核心驱动层之间的边界。把错误降级为 `String` 后，调用方
/// **只能打印、无法分类**：CLI 无法区分"后端不可用（该提示用户装配引擎）"与
/// "回合被中断（用户主动取消，属正常路径、不该报错）"，两者会落到同一条提示上。
/// 结构化后即可 `match`，且底层引擎的错误描述仍完整保留在 `Internal` 内。
///
/// # 为何不挂 `#[source]`
/// 底层错误类型（`quest_engine::QuestError` 等）由 L9 自行保留并写日志，
/// L10 只需"分类 + 可读描述"。挂 `Box<dyn Error>` 会连带失去
/// `Clone`/`PartialEq`（测试与跨线程传递都要用），代价大于收益。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// 后端不可用（引擎未装配 / 连接断开）
    #[error("core backend unavailable: {0}")]
    Unavailable(String),
    /// 回合被中断（用户取消，属正常路径而非故障）
    #[error("turn interrupted: {0}")]
    Interrupted(String),
    /// 后端内部错误（保留底层引擎的错误描述，不丢失上下文）
    #[error("core backend internal error: {0}")]
    Internal(String),
}

impl BackendError {
    /// 从底层引擎错误构造，保留其 `Display` 描述
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<String> for BackendError {
    /// 平滑迁移通道：实现方旧写法 `Err("...".to_string())` 仍可编译
    fn from(msg: String) -> Self {
        Self::Internal(msg)
    }
}

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
    /// 核心后端错误（结构化：保留底层分类与描述）
    #[error("core backend error: {0}")]
    Backend(#[from] BackendError),
    /// 会话数达上限（`AppServerConfig::max_sessions` 限制）
    ///
    /// WHY 单列:原先此错误被塞进 `Backend(String)`,但它是**本地配置约束**而非
    /// 后端故障——归类错误会让「后端是否健康」的判断被污染。
    #[error("session limit reached: max={0}")]
    SessionLimit(usize),
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
    ) -> Result<Vec<Item>, BackendError>;

    /// 取消回合（中断信号）
    async fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), BackendError>;
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
    ) -> Result<Vec<Item>, BackendError> {
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

    async fn interrupt_turn(&self, turn_id: &TurnId) -> Result<(), BackendError> {
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
    ///
    /// # store 兜底分支（ADR-292 D4——读面从预留转生产的生产调用点）
    /// 内存面未命中（actor 不在 DashMap——进程重启 / 线程丢失,即 threads 404
    /// 的本侧形态）且 store 已配置时,经 [`AppServer::replay_thread_from_store`]
    /// 从磁盘权威源重建后按游标过滤。重建为空（旧数据仅 `turn.submit` / 线程
    /// 从未落盘）→ 维持原 `None` 语义（ADR-292 旧数据容忍验收线,不 panic）。
    pub async fn replay_since(
        &self,
        thread_id: &ThreadId,
        last_item_id: &ItemId,
    ) -> Option<Vec<Item>> {
        // 内存面（原语义零改动:纯内存模式 / actor 在场,热路径不触盘）
        if let Some(actor) = self.sessions.get(thread_id) {
            let items: Vec<Item> = actor
                .items
                .iter()
                .filter(|i| i.item_id.as_str() > last_item_id.as_str())
                .cloned()
                .collect();
            return Some(items);
        }
        // store 兜底（ADR-292 D4:threads 404 安全网）;重建非空才接管——
        // 空重建说明磁盘无该线程的有效 Item 流,保持「线程未知 = None」原语义
        if self.store.is_some() {
            if let Ok(items) = self.replay_thread_from_store(thread_id).await {
                if !items.is_empty() {
                    return Some(
                        items
                            .into_iter()
                            .filter(|i| i.item_id.as_str() > last_item_id.as_str())
                            .collect(),
                    );
                }
            }
            // 重建失败/为空:落盘读错误与 404 场景同姿（线程不可用）→ None;
            // 不向调用方上抛Err——本 API 的错误面一直是 Option（线程未知）
        }
        None
    }

    /// 线程级回放入口（ADR-292 D4）— 从 session-store 磁盘权威源重建整个 Thread
    ///
    /// # 语义（threads 404 安全网）
    /// Anthropic Message threads 的 `continue` 返回 404（服务端线程 best-effort
    /// 过期）时，harness 重新 `create` 前按 Thread 段完整回放重建对话。本方法经
    /// [`CbmrWriter::replay_session`]（纯段文件 k-way 归并,ADR-109——恢复路径
    /// 必须走权威源而非可能陈旧的 SQLite 索引）读回事件流,再经
    /// [`session_store::rebuild_items`] 纯函数重构 Item 序列（D2）。
    ///
    /// # thread_id 归一
    /// 重建 Item 的 thread_id 以**请求的线程身份**为准:流内 `thread.started`
    /// 可能因 N-4 降级（落盘失败仅 warn）缺席,重构器的流上下文退化占位
    /// （空串）不应泄漏给协议面。
    ///
    /// # 旧数据容忍（ADR-292 验收线）
    /// 仅含 `turn.submit` 的历史会话流 → 空 `Vec`（不 panic）。
    ///
    /// # 错误
    /// store 未配置（纯内存模式）→ [`ServerError::StoreNotConfigured`];
    /// store 读失败 → [`ServerError::Store`]。
    pub async fn replay_thread_from_store(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Vec<Item>, ServerError> {
        let store = self.store.as_ref().ok_or(ServerError::StoreNotConfigured)?;
        let session_id = SessionId::new(thread_id.as_str());
        let replayed = store
            .replay_session(&session_id, Offset::new(0, 0))
            .await
            .map_err(|e| ServerError::Store(format!("线程回放失败: {e}")))?;
        let events: Vec<SessionEvent> = replayed.into_iter().map(|ri| ri.event).collect();
        let mut rebuilt = rebuild_items(&events);
        for item in &mut rebuilt.items {
            item.thread_id = thread_id.clone();
        }
        Ok(rebuilt.items)
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
        let store = self.store.as_ref().ok_or(ServerError::StoreNotConfigured)?;
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

    /// 线程元数据落盘 — `thread.started` 事件（ADR-292 D1 写面接线）
    ///
    /// # WHY 写面接线（本批裁决,ADR-292 §背景）
    /// 回放桥（[`AppServer::replay_thread_from_store`]）的消费场景是 threads
    /// 404 重建——流内若无 `thread.started`/`item.append`,重构恒为空、安全网
    /// 沦为装饰品。写面接线是 D4 读面语义非空的必要条件（消费场景真实性由
    /// ADR-292 裁决,非为绿造消费者）。
    ///
    /// # 流序契约
    /// `thread.started` 是会话流**首事件**——重构器以流上下文解析 Item 的
    /// thread_id,先于任何 turn/item 事件落盘（调用方 `thread_start` 保证）。
    /// `turn.submit` 既有写入路径不动（D1 append-only 纪律）。
    pub async fn persist_thread_started(&self, thread: &Thread) -> Result<(), ServerError> {
        let store = self.store.as_ref().ok_or(ServerError::StoreNotConfigured)?;
        let session_id = SessionId::new(thread.thread_id.as_str());
        let event = thread_started_event(thread.thread_id.as_str(), thread.created_at_ms);
        store
            .append(&session_id, event)
            .await
            .map_err(|e| ServerError::Store(format!("线程元数据入队失败: {e}")))?;
        store
            .flush()
            .await
            .map_err(|e| ServerError::Store(format!("线程元数据落盘失败: {e}")))
    }

    /// Item 内容落盘 — 逐条 `item.append` 事件（ADR-292 D1 写面接线）
    ///
    /// # 语义
    /// 回放桥重建 `Vec<Item>` 的数据源:载荷为 Item 协议字段完整镜像
    /// （`ItemAppendPayload`）。批量 append 后单次 flush（微批写收益;
    /// 与 persist_turn 的 flush 分离——不动其既有路径与 Offset 返回语义）。
    /// N-4 best-effort 姿态由调用方维持（失败仅 warn,内存语义保留）。
    pub async fn persist_items(&self, thread_id: &ThreadId, items: &[Item]) -> Result<(), ServerError> {
        let store = self.store.as_ref().ok_or(ServerError::StoreNotConfigured)?;
        let session_id = SessionId::new(thread_id.as_str());
        for item in items {
            let event = item_append_event(item);
            store
                .append(&session_id, event)
                .await
                .map_err(|e| ServerError::Store(format!("Item 事件入队失败: {e}")))?;
        }
        store
            .flush()
            .await
            .map_err(|e| ServerError::Store(format!("Item 事件落盘失败: {e}")))
    }

    /// 模型可见上下文 — 白名单投影出口（ADR-201 D2;WI-G11 首个生产消费方）
    ///
    /// # 语义（「model-visible means logged」生产出口）
    /// 读回该会话**已落盘**事件（[`CbmrWriter::read_events`],含 fork 前缀合并),
    /// 逐条经 [`to_model_view`] 投影（白名单 + 投影时映射 + 敏感剔除）。
    /// 宿主（`chimera serve` / `acp`）拼装模型请求上下文**必须取此出口**——
    /// logged ⇒ 不泄漏 / 已泄漏面必已记录（不变量在生产路径可强制）。
    ///
    /// # 投影语义（ADR-201 D1，唯一出口）
    /// - `turn.submit` → 输出 `event_type = "model.request"`（映射表
    ///   [`session_store::model_view::MODEL_VISIBLE_MAPPING`]）;
    /// - 非白名单类型剔除;敏感载荷剔除**优先于**映射（`payload = None`）;
    /// - 存储面零改动（append-only 权威源不动;被剔除载荷仍可审计回看）。
    ///
    /// # 错误
    /// store 未配置 → [`ServerError::StoreNotConfigured`]（与 `persist_turn` 同语义）。
    pub async fn model_visible_context(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Vec<ModelVisibleEvent>, ServerError> {
        let store = self.store.as_ref().ok_or(ServerError::StoreNotConfigured)?;
        let session_id = SessionId::new(thread_id.as_str());
        let stored = store
            .read_events(&session_id, None)
            .await
            .map_err(|e| ServerError::Store(format!("模型视图读回失败: {e}")))?;
        Ok(stored
            .iter()
            .filter_map(|s| to_model_view(&s.event, s.offset))
            .collect())
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
            return Err(ServerError::SessionLimit(self.config.max_sessions));
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
        self.sessions
            .insert(thread.thread_id.clone(), SessionActor::new(thread.clone()));
        // ADR-292 D1 写面接线:thread.started 为会话流首事件（重构器流上下文
        // 依赖,须先于 initial_input 触发的任何 turn/item 事件）;N-4 best-effort
        // ——落盘失败仅 warn,不影响内存语义（与 persist_turn 同姿态）
        if self.store.is_some() {
            if let Err(e) = self.persist_thread_started(&thread).await {
                tracing::warn!(
                    thread = %thread.thread_id.as_str(),
                    "线程元数据落盘失败(仅内存语义保留): {e}"
                );
            }
        }
        let mut events = Vec::new();
        // 初始输入（可选）→ 首回合
        if let Some(input) = &params.initial_input {
            events.extend(self.turn_submit(&thread.thread_id, input).await?);
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
        // 阶段 1:占用回合号 —— 只在纯内存记账期间持写锁，出块即释放。
        // WHY 绝不持 guard 进入阶段 2:DashMap 的 `RefMut` 锁住的是**整个分片**
        // 而非单个 key（`sessions` 是分片哈希表），持 guard 跨 `submit_turn` 的
        // await 会让哈希到同一分片的其他会话一起排队——一次慢 LLM 调用就能阻塞
        // 无关会话的 `thread_start` / `turn_submit`。（旧实现即如此，且注释误称
        // "guard 已由 NLL 释放"：419 行在 await 之后仍读 `actor`，NLL 不可能提前
        // 释放 guard，注释与代码语义矛盾。）
        let (turn_id, thread) = {
            let mut actor = self
                .sessions
                .get_mut(thread_id)
                .ok_or_else(|| ServerError::ThreadNotFound(thread_id.as_str().into()))?;
            let turn_id = TurnId::new(format!("turn-{}", actor.turns.len() + 1));
            actor.turns.push(turn_id.clone());
            (turn_id, actor.thread.clone())
        }; // guard 在此 drop，慢路径开始前会话表已完全解锁

        // 阶段 2:驱动核心 —— 慢路径（LLM / 工具调用）期间不持有任何会话锁
        // 后端产出 Item 流（单向驱动核心）
        let items = self.backend.submit_turn(&thread, &turn_id, input).await?;

        // 阶段 3:回填 Item 历史（再次短暂取锁，锁内无 await）
        {
            let mut actor = self
                .sessions
                .get_mut(thread_id)
                .ok_or_else(|| ServerError::ThreadNotFound(thread_id.as_str().into()))?;
            actor.items.extend(items.iter().cloned());
        }

        // 灰度双写:store 启用时回合落盘（此刻会话锁已释放，落盘的 await 不阻塞
        // 任何其他会话）;落盘失败仅记日志——内存语义保留,审计由 rebuild 兜底
        if self.store.is_some() {
            if let Err(e) = self.persist_turn(thread_id, &turn_id).await {
                tracing::warn!(
                    thread = %thread_id.as_str(),
                    "回合落盘失败(仅内存语义保留): {e}"
                );
            }
            // ADR-292 D1 写面接线:Item 内容逐条落盘（item.append），回放桥
            // （replay_thread_from_store）重建 Vec<Item> 的数据源;N-4 best-effort
            // WHY 在 into_iter 消费 items 之前:persist_items 按引用取 Item 镜像
            if let Err(e) = self.persist_items(thread_id, &items).await {
                tracing::warn!(
                    thread = %thread_id.as_str(),
                    "Item 内容落盘失败(仅内存语义保留): {e}"
                );
            }
        }
        // 组装事件流: TurnCompleted 兜底（含 Token 用量——MVP 无真实用量，
        // 全零；WI-03 命中率埋点接入后由 L1 填充）
        let mut events: Vec<AppEvent> = items
            .into_iter()
            .map(|item| AppEvent::ItemChanged { item })
            .collect();
        events.push(AppEvent::TurnCompleted {
            turn_id,
            usage: AppTokenUsage::new(0, 0, 0, 0),
        });
        Ok(events)
    }

    async fn turn_interrupt(&self, turn_id: &TurnId) -> Result<Vec<AppEvent>, ServerError> {
        self.backend.interrupt_turn(turn_id).await?;
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
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::Arc;

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

    /// 门控后端 — 首个回合在 `submit_turn` 内挂起，直到测试显式放行
    ///
    /// 用于构造"慢 LLM 调用"的**确定性**时序（不依赖 sleep/超时竞速），从而验证
    /// [`AppServer::turn_submit`] 在 await 期间不持有会话锁。
    struct GateBackend {
        /// 慢回合已进入 `submit_turn`（测试据此推进，无需 sleep 猜测）
        entered: Arc<tokio::sync::Notify>,
        /// 放行慢回合的信号
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl CoreBackend for GateBackend {
        async fn submit_turn(
            &self,
            thread: &Thread,
            turn_id: &TurnId,
            _input: &UserInput,
        ) -> Result<Vec<Item>, BackendError> {
            // 仅首个回合挂起；后续回合立即返回
            if turn_id.as_str() == "turn-1" {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(vec![Item::new(
                ItemId::new(format!("{}-1", turn_id.as_str())),
                thread.thread_id.clone(),
                turn_id.clone(),
                "message",
                ItemStatus::Completed,
                "ok",
            )])
        }

        async fn interrupt_turn(&self, _turn_id: &TurnId) -> Result<(), BackendError> {
            Ok(())
        }
    }

    /// 回归守卫:`turn_submit` 不得持有会话锁跨越 await
    ///
    /// # 为什么用"同一会话的第二回合"来断言
    /// DashMap 的 `RefMut` 锁住整个分片且不可重入。若 `turn_submit` 在
    /// `backend.submit_turn(...).await` 期间仍持有 guard，则同一会话（必然哈希到
    /// 同一分片）的第二回合会在 `get_mut` 处阻塞，而第一回合又在等 `release` —— 双方
    /// 互等，2 秒超时必然触发。该判据**确定性**，不依赖线程调度或 sleep 竞速。
    ///
    /// # 负向验证
    /// 回退到"单 guard 跨 await"的旧实现后本用例必定失败（超时 panic）。
    #[tokio::test]
    async fn turn_submit_does_not_hold_session_lock_across_await() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let server = Arc::new(AppServer::with_backend(
            AppServerConfig::default(),
            Box::new(GateBackend {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        ));
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let tid = ThreadId::new("goal-1::run-1");

        // 回合 1: 进入 backend 后挂起（模拟慢 LLM 调用）
        let slow = Arc::clone(&server);
        let slow_tid = tid.clone();
        let slow_handle = tokio::spawn(async move {
            slow.handle_op(&AppOp::TurnSubmit {
                thread_id: slow_tid,
                input: UserInput::new("慢回合"),
            })
            .await
        });
        entered.notified().await;

        // 核心断言:非阻塞探测同一会话的写锁此刻是否仍被持有。
        //
        // WHY 用 `try_get_mut` 而不是"再发一个回合":DashMap 的 `RefMut` 不可重入，
        // 再次 `get_mut` 会**同步阻塞**而非 await。`#[tokio::test]` 默认是
        // current_thread 运行时，该阻塞会连带卡死整个 runtime 的调度（timer 也跑不了），
        // 于是 `tokio::time::timeout` 永不触发 —— 用例变成**挂死**而不是失败
        // （实测:回退到旧实现后 300s 超时仍未返回，退出码 124）。
        // `try_get_mut` 在分片被写锁占用时立即返回 `TryResult::Locked`，是既确定
        // 又不依赖调度的判据。
        let probe = server.sessions.try_get_mut(&tid);
        assert!(
            probe.is_present(),
            "慢路径 await 期间会话写锁应已释放（旧实现会返回 TryResult::Locked），实际得到 {probe:?}"
        );
        drop(probe);

        // 放行慢回合并回收任务，避免悬挂
        release.notify_one();
        let first = slow_handle.await.expect("慢回合任务不应 panic");
        assert!(first.is_ok(), "第一回合应正常完成: {first:?}");
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
        let replay = server
            .replay_since(&tid, last_id)
            .await
            .expect("回放成功");
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

        // 显式 persist（turn_submit 已双写 turn.submit + 2 条 item.append;
        // 本次再落 1 条 turn.submit,seq 连续）
        let off = server
            .persist_turn(&tid, &TurnId::new("turn-1"))
            .await
            .expect("落盘");
        assert_eq!(off.seq, 4, "流 = thread.started(0)+turn.submit(1)+item.append(2,3),显式 persist seq=4");

        // 经 replay 重建会话（纯段文件回放,不依赖内存 DashMap）
        // ADR-292 D1 schema 后流构成:thread.started + turn.submit + item.append×2
        // + 显式 turn.submit——D1 事件类型 append-only 追加,turn.submit 序不变
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
        assert_eq!(items.len(), 5, "thread.started(1) + turn.submit(2) + item.append(2)");
        let expected_types = [
            session_store::EVENT_TYPE_THREAD_STARTED,
            "turn.submit",
            session_store::EVENT_TYPE_ITEM_APPEND,
            session_store::EVENT_TYPE_ITEM_APPEND,
            "turn.submit",
        ];
        for (i, item) in items.iter().enumerate() {
            assert_eq!(item.offset.seq, i as u64, "回放顺序 = 写入顺序");
            assert_eq!(
                item.event.event_type, expected_types[i],
                "流序 = thread.started → turn.submit → item.append×2 → turn.submit"
            );
        }

        // 回放桥（ADR-292 D4）:store 事件流 → rebuild → Item 序列与内存快照一致
        let snap = server.snapshot(&tid).expect("快照存在");
        let rebuilt = server.replay_thread_from_store(&tid).await.expect("回放桥");
        assert_eq!(rebuilt.len(), snap.items.len(), "重构条数 == 内存 Item 数");
        for (a, b) in rebuilt.iter().zip(&snap.items) {
            assert_eq!(a, b, "回放桥重构与内存 Item 逐条一致(零丢失)");
        }
    }

    // ============================================================
    // ADR-201 D2 — model_visible_context 生产消费方（WI-G11）
    // ============================================================

    /// V2:persist_turn 双写后 model_visible_context 输出 = 白名单交集
    #[tokio::test]
    async fn model_visible_context_projects_whitelist_intersection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = session_store::StoreConfig::with_dir(dir.path());
        cfg.spawn_flush_loop = false;
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
        server
            .persist_turn(&tid, &TurnId::new("turn-1"))
            .await
            .expect("落盘");

        let views = server.model_visible_context(&tid).await.expect("模型视图");
        assert_eq!(views.len(), 2, "两条 turn.submit 均映射进白名单交集");
        // ADR-292 D1:流内 thread.started(流首)+ item.append(逐条 Item 镜像)
        // 穿插于 turn.submit 之间——「offset = 段文件 seq 镜像」的本意是
        // **seq 单调性**(offset 与落盘序一致),起点/间隔由 schema 组成决定,
        // 不硬编码具体值(schema 演进时本断言零修改)
        for pair in views.windows(2) {
            assert!(
                pair[1].offset > pair[0].offset,
                "offset = 段文件 seq 镜像(单调递增): {} → {}",
                pair[0].offset,
                pair[1].offset
            );
        }
        assert!(
            views[0].offset >= 1,
            "thread.started 流首事件先于任何 turn.submit(流序契约)"
        );
        for v in views.iter() {
            assert_eq!(
                v.event_type, "model.request",
                "投影时映射 canonical（ADR-201 D1）"
            );
            assert!(v.payload.is_some(), "turn_id JSON 非敏感载荷保留");
        }

        // 纯内存模式:与 persist_turn 同语义拒绝（StoreNotConfigured）
        let pure = AppServer::new(AppServerConfig::default());
        let err = pure
            .model_visible_context(&tid)
            .await
            .expect_err("纯内存模式必须拒绝");
        assert!(matches!(err, ServerError::StoreNotConfigured));
    }

    /// V2 附录:persist_turn 写入类型 ↔ 映射表一致性锁（跨 crate 常量对拍）
    #[test]
    fn persist_turn_written_type_is_mapped() {
        // persist_turn 写入 "turn.submit"（本文件字面量）;该类型必须存在于
        // session-store 映射表——否则模型视图永远看不到落盘回合（静默缺席）
        let written = "turn.submit";
        assert!(
            session_store::model_view::MODEL_VISIBLE_MAPPING
                .iter()
                .any(|(raw, _)| *raw == written),
            "persist_turn 写入类型 {written} 必须存在于 MODEL_VISIBLE_MAPPING"
        );
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

    #[tokio::test]
    async fn pure_memory_mode_replay_bridge_rejected() {
        // ADR-292 D4:回放桥与 persist_turn 同灰度语义——纯内存模式拒绝,
        // replay_since 内存面不受影响（actor 在场照常回放）
        let server = AppServer::new(AppServerConfig::default());
        let err = server
            .replay_thread_from_store(&ThreadId::new("x"))
            .await
            .expect_err("纯内存模式必须拒绝回放桥");
        assert!(matches!(err, ServerError::StoreNotConfigured));
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
        let snap = server.snapshot(&tid).expect("快照存在");
        let replay = server
            .replay_since(&tid, &snap.items[0].item_id)
            .await
            .expect("内存面回放不受桥拒绝影响");
        assert_eq!(replay.len(), 1, "内存面增量回放语义不变");
    }

    // ============================================================
    // 方向 4:错误契约统一 — CoreBackend seam 结构化错误
    // ============================================================

    /// 结构化错误可按变体 match —— 这是 `Result<_, String>` 给不了的能力
    #[test]
    fn backend_error_is_matchable_by_variant() {
        let unavailable = BackendError::Unavailable("engine not wired".into());
        let interrupted = BackendError::Interrupted("user cancel".into());
        let internal = BackendError::Internal("boom".into());

        // 三者原先都被压成同一种 String,调用方无法区分"故障"与"正常取消"
        assert!(matches!(unavailable, BackendError::Unavailable(_)));
        assert!(matches!(interrupted, BackendError::Interrupted(_)));
        assert!(matches!(internal, BackendError::Internal(_)));
    }

    /// 底层引擎错误的描述不得丢失（原实现经 `format!` 拼接后类型与来源信息全无）
    #[test]
    fn backend_error_internal_preserves_source_description() {
        let e = BackendError::internal("quest create failed: DAG cycle detected");
        assert_eq!(
            e.to_string(),
            "core backend internal error: quest create failed: DAG cycle detected"
        );
    }

    /// 旧写法 `Err("...".to_string())` 经 `From<String>` 平滑迁移为 Internal
    #[test]
    fn backend_error_from_string_maps_to_internal() {
        let e: BackendError = "legacy message".to_string().into();
        assert!(
            matches!(e, BackendError::Internal(ref s) if s == "legacy message"),
            "实际 {e:?}"
        );
    }

    /// `ServerError` 经 `#[from]` 吸收底层变体 —— 调用点的 `?` 依赖此 impl
    #[test]
    fn server_error_absorbs_backend_variant() {
        let converted: ServerError = BackendError::Unavailable("down".into()).into();
        match converted {
            ServerError::Backend(BackendError::Unavailable(msg)) => assert_eq!(msg, "down"),
            other => panic!("底层变体应保留, 实际 {other:?}"),
        }
    }

    /// 端到端:后端故障经 `handle_op` 上抛后**分类信息仍可识别**
    ///
    /// 本方向的核心验收点——结构化之前,这里只能拿到一个字符串。
    #[tokio::test]
    async fn backend_failure_propagates_with_classification_intact() {
        struct DownBackend;

        #[async_trait]
        impl CoreBackend for DownBackend {
            async fn submit_turn(
                &self,
                _thread: &Thread,
                _turn_id: &TurnId,
                _input: &UserInput,
            ) -> Result<Vec<Item>, BackendError> {
                Err(BackendError::Unavailable("core engine not wired".into()))
            }

            async fn interrupt_turn(&self, _turn_id: &TurnId) -> Result<(), BackendError> {
                Err(BackendError::Unavailable("core engine not wired".into()))
            }
        }

        let server = AppServer::with_backend(AppServerConfig::default(), Box::new(DownBackend));
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("启动成功");
        let err = server
            .handle_op(&AppOp::TurnSubmit {
                thread_id: ThreadId::new("goal-1::run-1"),
                input: UserInput::new("hi"),
            })
            .await
            .expect_err("后端不可用应上抛");

        assert!(
            matches!(err, ServerError::Backend(BackendError::Unavailable(_))),
            "分类应穿越 seam 保留, 实际 {err:?}"
        );
    }

    /// 会话数达上限单列为 `SessionLimit`（此前被误归入 `Backend`）
    #[tokio::test]
    async fn session_limit_is_its_own_error_class() {
        let server = AppServer::new(AppServerConfig {
            keep_item_history: true,
            max_sessions: 1,
        });
        server
            .handle_op(&AppOp::ThreadStart(sample_params()))
            .await
            .expect("首个会话应成功");
        let err = server
            .handle_op(&AppOp::ThreadStart(ThreadStartParams::new(
                "goal-2", "run-2",
            )))
            .await
            .expect_err("超限应报错");
        assert!(matches!(err, ServerError::SessionLimit(1)), "实际 {err:?}");
    }

    proptest! {
        /// 任意错误描述经 `From<String>` 往返:不 panic、不截断、恒为 Internal
        #[test]
        fn prop_backend_error_from_string_roundtrip(msg in "\\PC*") {
            let e: BackendError = msg.clone().into();
            match e {
                BackendError::Internal(inner) => prop_assert_eq!(inner, msg),
                other => prop_assert!(false, "From<String> 必须产出 Internal, 实际 {:?}", other),
            }
        }
    }
}
