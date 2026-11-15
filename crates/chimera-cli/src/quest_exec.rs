//! `QuestExecutor` — 任务级租约 + DAG 就绪队列 + 工具计划执行（Phase 5，ADR-231）
//!
//! 对应架构层: L10 Interface（chimera-cli 组合面）
//!
//! # 为什么落在 L10
//! 本模块同时消费 L9 `quest-engine`（任务状态机）、L9 `mas-sched`（租约控制面）与
//! L7 `gqep-executor::tool_plane`（工具执行面）。依赖铁律 §2.2 禁止 L7 → L9/L10，
//! 故"同时看得见三者"的唯一合法位置是 L10：**工具级并发留在 L7，任务级租约留在 L10**。
//!
//! # 调度语义
//! - 就绪队列：`Task.dependencies` 全部 `Completed` 的任务才进入 ready；
//! - 并发窗口：`DagMode::Linear` ⇒ 恒 1（与既有伪链等价，保守默认）；
//!   `DagMode::MultiRoot` ⇒ `max_concurrent_tasks`；
//! - 租约：`claim`（准入）→ 心跳 `renew_lease` → `handoff(HANDOFF)`（释放）；
//! - 级联：前置 `Failed` ⇒ 后继标 `Failed`；前置无计划（`Skipped`）⇒ 后继 `Skipped`
//!   且**不写状态**（保持 `Pending`，补计划后可重跑）；
//! - 取消：父 token 派生每任务子 token；取消即在任务边界收敛为 `Cancelled`，
//!   已排空的结果不丢弃，未启动的任务收尾标 `Cancelled`；
//! - panic 隔离：`JoinError` 带回 `task_id` 并只失败该任务（与 `chimera-mas` 委托面同口径）。

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use gqep_executor::tool_plane::ToolRuntime;
use gqep_executor::{PlanRunError, PlanRunOptions};
use mas_sched::{
    ClaimOutcome, PeerScheduler, Priority, Quota, RenewOutcome, SimplePeerScheduler, TaskId,
    TodoClaim, HANDOFF,
};
use nexus_contracts::tool_plan::ToolPlan;
use nexus_contracts::{Quest, Task, TaskStatus};
use quest_engine::{DagMode, QuestEngine, QuestError};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// 灰度开关：默认 `Off`（与 `ToolPlaneMode` 同族三态纪律，但执行面无 observe 档）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuestExecMode {
    /// 拒绝执行并提示启用方式（默认）
    #[default]
    Off,
    /// 真实执行任务计划
    Apply,
}

/// 执行面配置（§5.4 契约 + `mode` 灰度门）
#[derive(Debug, Clone)]
pub struct QuestExecConfig {
    /// 灰度开关
    pub mode: QuestExecMode,
    /// MultiRoot 下同时在飞的任务数上界（钳到 ≥1）
    pub max_concurrent_tasks: usize,
    /// 单任务计划内并发（透传 `PlanRunOptions::max_parallel`）
    pub max_concurrent_tools: usize,
    /// 单任务超时（ms,同时决定租约时长与心跳间隔）
    pub task_timeout_ms: u64,
    /// DAG 形态：Linear 恒串行；MultiRoot 才开并发窗口
    pub dag_mode: DagMode,
}

impl Default for QuestExecConfig {
    fn default() -> Self {
        Self {
            mode: QuestExecMode::Off,
            max_concurrent_tasks: 4,
            max_concurrent_tools: 8,
            task_timeout_ms: 120_000,
            dag_mode: DagMode::Linear,
        }
    }
}

impl QuestExecConfig {
    /// 从 `CHIMERA_QUEST_*` env 读取（非法值回退默认，不 panic）
    #[must_use]
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let mode = match env::var("CHIMERA_QUEST_EXEC").ok().as_deref() {
            Some("apply") | Some("on") => QuestExecMode::Apply,
            _ => QuestExecMode::Off,
        };
        let dag_mode = match env::var("CHIMERA_QUEST_DAG").ok().as_deref() {
            Some("multi_root") | Some("multi-root") => DagMode::MultiRoot,
            _ => DagMode::Linear,
        };
        Self {
            mode,
            dag_mode,
            max_concurrent_tasks: env_usize(
                "CHIMERA_QUEST_EXEC_MAX",
                defaults.max_concurrent_tasks,
            ),
            max_concurrent_tools: env_usize(
                "CHIMERA_QUEST_TOOL_MAX",
                defaults.max_concurrent_tools,
            ),
            task_timeout_ms: env_u64("CHIMERA_QUEST_TASK_TIMEOUT_MS", defaults.task_timeout_ms),
        }
    }

    /// 实际并发窗口 — Linear 恒 1（保守默认），MultiRoot 才用配置值
    #[must_use]
    pub fn window(&self) -> usize {
        match self.dag_mode {
            DagMode::Linear => 1,
            DagMode::MultiRoot => self.max_concurrent_tasks.max(1),
        }
    }
}

/// 计划来源错误 — 库层 thiserror（§4.1）
#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    /// 该任务没有可用计划（执行面记 Skipped，不记 Failed）
    #[error("no plan for task {0}")]
    NoPlan(String),
    /// 计划本身非法
    #[error("invalid plan for task {0}: {1}")]
    InvalidPlan(String, String),
}

/// 任务 → 工具计划映射（Phase 5 只有 JSON 实现；LLM 规划器属 Phase 7）
#[async_trait]
pub trait TaskPlanner: Send + Sync {
    /// 返回 `Ok(None)` = 无计划（该任务跳过）；`Err` = 有计划但取不到（该任务失败）
    async fn plan(&self, task: &Task) -> Result<Option<ToolPlan>, PlannerError>;
}

/// 从 `task_id -> ToolPlan` JSON 表读取的计划来源
#[derive(Debug, Clone, Default)]
pub struct JsonPlanPlanner {
    plans: HashMap<String, ToolPlan>,
}

impl JsonPlanPlanner {
    /// 解析 JSON 文本（顶层必须是对象：键为 task_id）
    pub fn from_json_str(text: &str) -> Result<Self, PlannerError> {
        let plans: HashMap<String, ToolPlan> = serde_json::from_str(text)
            .map_err(|e| PlannerError::InvalidPlan("<file>".into(), e.to_string()))?;
        Ok(Self { plans })
    }

    /// 从文件加载（IO 失败报 `InvalidPlan`,调用方转 CLI 错误码）
    pub fn from_path(path: &Path) -> Result<Self, PlannerError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| PlannerError::InvalidPlan(path.display().to_string(), e.to_string()))?;
        let mut planner = Self::from_json_str(&text)?;
        // 让错误里的 task_id 指向真实路径（而非占位符）
        for plan in planner.plans.values_mut() {
            if plan.id.is_empty() {
                plan.id = format!("plan-{}", path.display());
            }
        }
        Ok(planner)
    }

    /// 是否含该任务的计划（`--strict` 前置校验用）
    #[must_use]
    pub fn contains(&self, task_id: &str) -> bool {
        self.plans.contains_key(task_id)
    }

    /// 计划条目数（诊断）
    #[must_use]
    pub fn len(&self) -> usize {
        self.plans.len()
    }

    /// 是否空表（空表 = 全部任务跳过，值得单独提示）
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }
}

#[async_trait]
impl TaskPlanner for JsonPlanPlanner {
    async fn plan(&self, task: &Task) -> Result<Option<ToolPlan>, PlannerError> {
        Ok(self.plans.get(&task.task_id).cloned())
    }
}

/// 执行汇总 — total 恒等于 completed+failed+skipped+cancelled
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QuestExecReport {
    /// Quest 内任务总数
    pub total: usize,
    /// 计划跑通的任务数
    pub completed: usize,
    /// 失败任务数（含因前置失败被级联标失败的）
    pub failed: usize,
    /// 跳过任务数（无计划 / 租约拒绝 / 前置被跳过）
    pub skipped: usize,
    /// 取消任务数
    pub cancelled: usize,
    /// 整个执行耗时（ms）
    pub duration_ms: u64,
}

/// 任务终局分类（执行器视角）
///
/// WHY 与 `TaskStatus` 分开：`Skipped` 刻意**不写状态**（任务保持 `Pending`），
/// 这样补上计划后同一个 Quest 可重跑；其余三态一一映射到 `TaskStatus`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskDone {
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl TaskDone {
    /// 需要落到 quest-engine 的终态；`Skipped` 返回 `None`（不改状态）
    const fn status(self) -> Option<TaskStatus> {
        match self {
            Self::Completed => Some(TaskStatus::Completed),
            Self::Failed => Some(TaskStatus::Failed),
            Self::Cancelled => Some(TaskStatus::Cancelled),
            Self::Skipped => None,
        }
    }
}

/// spawned 任务的回报
#[derive(Debug)]
struct Executed {
    task_id: String,
    done: TaskDone,
}

/// 一次 run 的共享依赖（移入 spawned task 用 Arc 而非 clone，§4.4 反模式 5）
struct TaskDeps {
    engine: Arc<QuestEngine>,
    runtime: Arc<ToolRuntime>,
    planner: Arc<dyn TaskPlanner>,
    sched: Arc<SimplePeerScheduler>,
    quest_id: String,
    peer_id: String,
    task_timeout_ms: u64,
    lease_ms: u64,
    max_concurrent_tools: usize,
    window: usize,
}

/// Quest 执行面入口
pub struct QuestExecutor {
    engine: Arc<QuestEngine>,
    runtime: Arc<ToolRuntime>,
    planner: Arc<dyn TaskPlanner>,
    sched: Arc<SimplePeerScheduler>,
    cfg: QuestExecConfig,
}

impl QuestExecutor {
    /// 装配执行面 —— 五个部件全部注入（无内部默认，便于测试替换）
    ///
    /// `runtime` 用 `Arc` 是 `ToolRuntime::run_plan` 的 `self: &Arc<Self>` 签名要求，
    /// 不是本层选择。
    #[must_use]
    pub fn new(
        engine: Arc<QuestEngine>,
        runtime: Arc<ToolRuntime>,
        planner: Arc<dyn TaskPlanner>,
        sched: Arc<SimplePeerScheduler>,
        cfg: QuestExecConfig,
    ) -> Self {
        Self {
            engine,
            runtime,
            planner,
            sched,
            cfg,
        }
    }

    /// 当前配置（诊断/测试断言窗口用）
    #[must_use]
    pub fn config(&self) -> &QuestExecConfig {
        &self.cfg
    }

    /// 执行一个 Quest — 返回计数汇总；Quest 不存在报 `QuestError::QuestNotFound`
    pub async fn run(
        &self,
        quest_id: &str,
        cancel: CancellationToken,
    ) -> Result<QuestExecReport, QuestError> {
        let quest = self
            .engine
            .get_quest(quest_id)
            .ok_or_else(|| QuestError::QuestNotFound(quest_id.to_string()))?;
        let started = Instant::now();
        let window = self.cfg.window();
        let deps = Arc::new(TaskDeps {
            engine: Arc::clone(&self.engine),
            runtime: Arc::clone(&self.runtime),
            planner: Arc::clone(&self.planner),
            sched: Arc::clone(&self.sched),
            quest_id: quest_id.to_string(),
            peer_id: "chimera-cli:quest-exec".to_string(),
            task_timeout_ms: self.cfg.task_timeout_ms,
            lease_ms: self.cfg.task_timeout_ms.saturating_mul(2).max(1),
            max_concurrent_tools: self.cfg.max_concurrent_tools,
            window,
        });

        let mut state = QuestState::from_quest(&quest);
        let semaphore = Arc::new(Semaphore::new(window));
        // wrapper future 只有一种类型（同一个私有 async fn）——避免 E0308
        let mut running: FuturesUnordered<_> = FuturesUnordered::new();
        let mut cancelled = false;

        loop {
            flush_status_writes(&self.engine, quest_id, &mut state).await;
            while !cancelled {
                let Some(task_id) = state.next_ready() else {
                    break;
                };
                // 先取 permit 再 spawn：峰值在飞 = window，而非任务总数
                let permit = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => { cancelled = true; break; }
                    acquired = Arc::clone(&semaphore).acquire_owned() => {
                        match acquired {
                            Ok(permit) => permit,
                            // 信号量只可能因 Arc 全部释放而关闭；本函数持有一个 ⇒ 不可达，
                            // 但若真发生只能记该任务失败并停止准入（不 panic、不静默）
                            Err(closed) => {
                                warn!(quest_id, error = %closed, "任务并发信号量已关闭,停止准入");
                                cancelled = true;
                                break;
                            }
                        }
                    }
                };
                let Some(task) = quest.tasks.iter().find(|t| t.task_id == task_id).cloned() else {
                    state.settle(&task_id, TaskDone::Failed);
                    continue;
                };
                let task_cancel = cancel.child_token();
                let handle = tokio::spawn({
                    let deps = Arc::clone(&deps);
                    async move {
                        let _permit = permit;
                        execute_one(deps, task, task_cancel).await
                    }
                });
                running.push(join_tagged(task_id, handle));
            }
            if running.is_empty() {
                break;
            }
            let Some((task_id, joined)) = running.next().await else {
                break;
            };
            match joined {
                Ok(executed) => state.settle(&executed.task_id, executed.done),
                Err(join_err) => {
                    // panic / 取消：结果仍归属到具体 task_id，不丢批次
                    warn!(quest_id, task_id = %task_id, error = %join_err, "任务执行异常");
                    state.settle(&task_id, TaskDone::Failed);
                }
            }
            state.propagate();
        }

        if cancelled {
            state.cancel_rest();
        } else {
            // 未取消却仍有未收敛任务 ⇒ 其前置永远等不到 Completed（悬空依赖/环外阻塞），记跳过
            state.skip_rest();
        }
        flush_status_writes(&self.engine, quest_id, &mut state).await;

        Ok(state.report(quest.tasks.len(), started.elapsed()))
    }
}

/// 把 `JoinHandle` 包成"带 task_id 的结果"：panic 也能定位归属任务
async fn join_tagged(
    task_id: String,
    handle: tokio::task::JoinHandle<Executed>,
) -> (String, Result<Executed, tokio::task::JoinError>) {
    (task_id, handle.await)
}

/// 单任务全链路：plan → claim → Running → 心跳 + run_plan → handoff → 分类
async fn execute_one(deps: Arc<TaskDeps>, task: Task, cancel: CancellationToken) -> Executed {
    let task_id = task.task_id.clone();
    let done = |done| Executed {
        task_id: task_id.clone(),
        done,
    };

    // 1. 计划
    let plan = match deps.planner.plan(&task).await {
        Ok(Some(plan)) => plan,
        Ok(None) => return done(TaskDone::Skipped),
        Err(error) => {
            warn!(task_id, error = %error, "任务计划不可用");
            return done(if cancel.is_cancelled() {
                TaskDone::Cancelled
            } else {
                TaskDone::Failed
            });
        }
    };

    // 2. 租约准入（控制面拒绝 ⇒ 跳过，不误报失败）
    let claim = TodoClaim {
        task_id: TaskId::from(task_id.clone()),
        peer_id: deps.peer_id.clone(),
        priority: Priority::Medium,
        est_duration_ms: deps.task_timeout_ms,
        quota: Quota {
            max_concurrent: deps.window,
            max_duration_ms: deps.lease_ms,
            priority_weight: 1.0,
        },
    };
    match deps.sched.claim(&claim) {
        ClaimOutcome::Granted(_) => {}
        ClaimOutcome::Denied(reason) => {
            warn!(task_id, ?reason, "租约被控制面拒绝,跳过该任务");
            return done(TaskDone::Skipped);
        }
    }

    // 3. 状态机：Pending/Running → Running（同态幂等合法）
    if let Err(error) = deps
        .engine
        .update_task_status(&deps.quest_id, &task_id, TaskStatus::Running)
        .await
    {
        release_lease(&deps, &task_id);
        warn!(task_id, error = %error, "任务状态无法置 Running");
        return done(if cancel.is_cancelled() {
            TaskDone::Cancelled
        } else {
            TaskDone::Failed
        });
    }

    // 4. 心跳续租（租约 half-life 续一次；失败只停心跳,不级联——本地单写者不会抢走租约）
    let heartbeat_stop = CancellationToken::new();
    let heartbeat = {
        let sched = Arc::clone(&deps.sched);
        let peer_id = deps.peer_id.clone();
        let id = task_id.clone();
        let every = Duration::from_millis((deps.task_timeout_ms / 2).max(25));
        let stop = heartbeat_stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(every) => {}
                }
                match sched.renew_lease(&id, &peer_id) {
                    Ok(RenewOutcome::Renewed) => {}
                    other => {
                        warn!(task_id = %id, ?other, "租约心跳停止");
                        break;
                    }
                }
            }
        })
    };

    // 5. 执行计划（fail_fast：任一节点失败即任务失败；审批面未接线 ⇒ approve_writes=false 保持 fail-closed）
    let opts = PlanRunOptions {
        max_parallel: deps.max_concurrent_tools,
        timeout_ms: deps.task_timeout_ms,
        fail_fast: true,
        approve_writes: false,
        cancel: Some(cancel.clone()),
    };
    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(PlanRunError::Cancelled { completed: 0 }),
        result = deps.runtime.run_plan(&plan, opts) => result,
    };
    heartbeat_stop.cancel();
    heartbeat.abort();

    // 6. 释放租约（HANDOFF 哨兵：移除租约 + 归还 peer 并发额度）
    release_lease(&deps, &task_id);

    match outcome {
        Ok(_summary) => done(TaskDone::Completed),
        Err(PlanRunError::Cancelled { .. }) => done(TaskDone::Cancelled),
        Err(error) => {
            warn!(task_id, error = %error, "任务计划执行失败");
            done(
                if cancel.is_cancelled() && !matches!(error, PlanRunError::TimedOut { .. }) {
                    TaskDone::Cancelled
                } else {
                    TaskDone::Failed
                },
            )
        }
    }
}

fn release_lease(deps: &TaskDeps, task_id: &str) {
    if let Err(error) = deps.sched.handoff(task_id, HANDOFF) {
        warn!(task_id, error = %error, "租约释放失败（将由 reap_expired 兜底）");
    }
}

/// 把状态机攒下的终态写入 quest-engine
async fn flush_status_writes(engine: &Arc<QuestEngine>, quest_id: &str, state: &mut QuestState) {
    let writes = std::mem::take(&mut state.status_writes);
    for (task_id, status) in writes {
        if let Err(error) = engine.update_task_status(quest_id, &task_id, status).await {
            // 写失败只告警：报告计数取自执行器自己的 outcome 账本，不与引擎状态混淆
            warn!(quest_id, task_id, ?status, error = %error, "任务终态写入失败");
        }
    }
}

/// DAG 就绪队列 + 级联账本
struct QuestState {
    /// 挂载序（report 计数与收尾遍历的稳定序）
    order: Vec<String>,
    dependents: HashMap<String, Vec<String>>,
    /// 尚未满足的前置数
    unsatisfied: HashMap<String, usize>,
    ready: VecDeque<String>,
    outcome: HashMap<String, TaskDone>,
    /// 待落盘的状态写
    status_writes: Vec<(String, TaskStatus)>,
    /// 级联传播队列
    cascade: VecDeque<(String, TaskDone)>,
}

impl QuestState {
    fn from_quest(quest: &Quest) -> Self {
        let known: HashSet<&str> = quest.tasks.iter().map(|t| t.task_id.as_str()).collect();
        let mut state = Self {
            order: quest.tasks.iter().map(|t| t.task_id.clone()).collect(),
            dependents: HashMap::new(),
            unsatisfied: HashMap::new(),
            ready: VecDeque::new(),
            outcome: HashMap::new(),
            status_writes: Vec::new(),
            cascade: VecDeque::new(),
        };
        for task in &quest.tasks {
            // 只认 Quest 内存在的前置（外部生产者可能留下悬空依赖，忽略而非炸）
            let deps: Vec<String> = task
                .dependencies
                .iter()
                .filter(|d| known.contains(d.as_str()))
                .cloned()
                .collect();
            for dep in &deps {
                state
                    .dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(task.task_id.clone());
            }
            state.unsatisfied.insert(task.task_id.clone(), deps.len());
            // 初就绪：无前置且尚未终态的任务，第一轮就要被准入 —— 级联传播只在
            // "前置 Completed 使计数归零"时入队，零前置任务没有任何前置会来触发它
            if deps.is_empty() && !terminal_status(task.status) {
                state.ready.push_back(task.task_id.clone());
            }
        }
        // 重跑/断点恢复：已是终态的任务作为种子，不重复执行
        for task in &quest.tasks {
            let seed = match task.status {
                TaskStatus::Completed => Some(TaskDone::Completed),
                TaskStatus::Failed => Some(TaskDone::Failed),
                TaskStatus::Cancelled => Some(TaskDone::Cancelled),
                TaskStatus::Paused => Some(TaskDone::Skipped),
                TaskStatus::Pending | TaskStatus::Running => None,
            };
            if let Some(done) = seed {
                state.settle(&task.task_id, done);
            }
        }
        state.propagate();
        state
    }

    /// 登记终局并排队级联（同Id 只登记一次）
    fn settle(&mut self, task_id: &str, done: TaskDone) {
        if self.outcome.contains_key(task_id) {
            return;
        }
        self.outcome.insert(task_id.to_string(), done);
        if let Some(status) = done.status() {
            self.status_writes.push((task_id.to_string(), status));
        }
        self.cascade.push_back((task_id.to_string(), done));
    }

    /// 处理级联：Completed 递减后继的未满足计数；其余终态直接传染
    fn propagate(&mut self) {
        while let Some((id, done)) = self.cascade.pop_front() {
            let Some(dependents) = self.dependents.get(&id).cloned() else {
                continue;
            };
            for dep in dependents {
                if self.outcome.contains_key(&dep) {
                    continue;
                }
                match done {
                    TaskDone::Completed => {
                        if let Some(left) = self.unsatisfied.get_mut(&dep) {
                            *left = left.saturating_sub(1);
                            if *left == 0 {
                                self.ready.push_back(dep);
                            }
                        }
                    }
                    other => self.settle(&dep, other),
                }
            }
        }
    }

    fn next_ready(&mut self) -> Option<String> {
        while let Some(id) = self.ready.pop_front() {
            if !self.outcome.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    /// 取消收尾：未收敛的任务一律标 Cancelled
    fn cancel_rest(&mut self) {
        let order = self.order.clone();
        for id in order {
            if !self.outcome.contains_key(&id) {
                self.settle(&id, TaskDone::Cancelled);
            }
        }
        self.propagate();
    }

    /// 阻塞收尾：前置永不满足（如被跳过）的任务标 Skipped 且不写状态
    fn skip_rest(&mut self) {
        let order = self.order.clone();
        for id in order {
            if !self.outcome.contains_key(&id) {
                self.settle(&id, TaskDone::Skipped);
            }
        }
        self.propagate();
    }

    fn report(&self, total: usize, elapsed: Duration) -> QuestExecReport {
        let mut report = QuestExecReport {
            total,
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            ..QuestExecReport::default()
        };
        for id in &self.order {
            match self.outcome.get(id).copied() {
                Some(TaskDone::Completed) => report.completed += 1,
                Some(TaskDone::Failed) => report.failed += 1,
                Some(TaskDone::Skipped) => report.skipped += 1,
                Some(TaskDone::Cancelled) => report.cancelled += 1,
                None => report.skipped += 1,
            }
        }
        report
    }
}

/// 任务是否已处于"不由本执行面推进"的状态（终态或 Paused）
const fn terminal_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Paused
    )
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
