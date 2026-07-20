//! RootOrchestrator 根协调器 — MAS 子系统全局任务编排入口(Task 12)
//!
//! 定义 `RootOrchestrator`,MAS 子系统的全局任务编排入口。
//! - ADR-026 决策 1: 最大委托深度 5(`MAX_AGENT_DEPTH`)
//! - ADR-026 决策 2: 复用 event-bus,不新建 AgentMessageBus
//!
//! ## 核心职责
//!
//! 1. `delegate(task)`: 根据 `task.complexity` 决定子 Agent 数量与类型,
//!    通过 `AgentFactory::create_agent` 创建子 Agent,返回 `AgentHandle` 列表
//! 2. `monitor()`: 订阅 `EventTopic::Agent`,收集 `NexusEvent::AgentHeartbeat`,
//!    更新内部心跳注册表(§4.4 反模式 3:subscribe 在 spawn 之前同步调用)
//! 3. 深度限制: `delegation_depth >= max_depth` 返回 `MaxDepthExceeded`
//!
//! ## 委托分发策略
//!
//! | TaskComplexity | 子 Agent 数量 | 说明 |
//! |----------------|--------------|------|
//! | Simple         | 1            | 快速响应,单 Agent 执行 |
//! | Medium         | 2            | 标准任务,双 Agent 并行 |
//! | Complex        | 3            | 多文件重构,三 Agent 协作 |
//! | VeryComplex    | 5            | 跨系统迁移,五 Agent 并行 |
//!
//! ## 架构合规
//!
//! - §4.4 反模式 3:`subscribe_filtered()` 在 `tokio::spawn()` 之前同步调用
//! - §6.2:`AgentTaskDelegated` 是 Normal 级,走 broadcast(非 mpsc)
//! - §4.1:`create_agent` 是 sync 方法,内部用 `publish_blocking` 发布事件
//! - `#![forbid(unsafe_code)]` 保持(chimera-mas crate 级)

use crate::agent::factory::AgentFactory;
use crate::agent::meta::AgentType;
use crate::delegation::{AgentTask, TaskComplexity};
use crate::error::{MasError, Result};
use crate::quadrant::QuadrantPlan;
use crate::stability::{StabilityGuard, TerminalState};
use chrono::{DateTime, Utc};
use event_bus::{EventBus, EventTopic, NexusEvent};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// 最大委托深度 — ADR-026 决策 1
///
/// RootOrchestrator=0 → MainAgent=1 → SubAgent=2 → GrandAgent=3 → ...
/// 超过此深度将触发 `MasError::MaxDepthExceeded`,防止递归爆炸。
pub const MAX_AGENT_DEPTH: usize = 5;

// ============================================================
// AgentHandle — Agent 句柄
// ============================================================

/// Agent 句柄 — 指向已创建 Agent 实例的轻量引用
///
/// 由 `RootOrchestrator::delegate()` 返回,携带子 Agent 的关键元数据,
/// 供调用方(如 `DelegationExecutor`)后续引用子 Agent。
///
/// ## 字段说明
///
/// - `agent_id`: Agent 唯一标识(用于 EventBus 通信、注册表查询)
/// - `agent_type`: Agent 层级类型(MainAgent / SubAgent / GrandAgent)
/// - `depth`: Agent 层级深度(= parent.delegation_depth + 1)
/// - `current_task_id`: 当前关联的任务 ID(委托时设置)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    /// Agent 唯一标识
    pub agent_id: String,
    /// Agent 层级类型
    pub agent_type: AgentType,
    /// Agent 层级深度(1=MainAgent, 2=SubAgent, 3=GrandAgent, ...)
    pub depth: usize,
    /// 当前关联的任务 ID(委托时设置,Agent 空闲后置 None)
    pub current_task_id: Option<String>,
}

// ============================================================
// HeartbeatInfo — 心跳信息
// ============================================================

/// 心跳信息 — `monitor()` 收集的 Agent 心跳快照
///
/// 由 `RootOrchestrator::monitor()` 后台任务从 `NexusEvent::AgentHeartbeat`
/// 事件中提取并存储。调用方可通过 `get_heartbeat()` 查询 Agent 最新状态。
///
/// ## 字段语义
///
/// - `agent_id`: 心跳来源 Agent ID
/// - `status`: Agent 运行时状态(event_bus::AgentStatus,与 chimera_mas::AgentStatus 变体一致)
/// - `current_task`: 当前任务 ID(空闲时为 None)
/// - `token_usage`: Token 使用量
/// - `memory_usage_mb`: 内存使用量(MB)
/// - `received_at`: 收到心跳的时间戳(UTC)
#[derive(Debug, Clone)]
pub struct HeartbeatInfo {
    /// 心跳来源 Agent ID
    pub agent_id: String,
    /// Agent 运行时状态
    pub status: event_bus::AgentStatus,
    /// 当前任务 ID(空闲时为 None)
    pub current_task: Option<String>,
    /// Token 使用量
    pub token_usage: u64,
    /// 内存使用量(MB)
    pub memory_usage_mb: u64,
    /// 收到心跳的时间戳(UTC)
    pub received_at: DateTime<Utc>,
}

// ============================================================
// RootOrchestrator — 根协调器
// ============================================================

/// RootOrchestrator — MAS 子系统全局任务编排入口
///
/// 持有 `AgentFactory`(创建 Agent) + `EventBus`(发布/订阅事件) +
/// 心跳注册表(`Arc<Mutex<HashMap>>`,monitor 后台任务写入)。
///
/// ## 线程安全
///
/// - `AgentFactory`: 内部 `DashMap` 并发安全,`&self` 方法可跨线程共享
/// - `EventBus`: `Clone`(Arc-based,廉价),`&self` 方法可跨线程共享
/// - `heartbeats`: `Arc<Mutex<HashMap>>`,`monitor()` 后台任务通过 `Arc::clone` 写入,
///   主线程通过 `heartbeat_count()` / `get_heartbeat()` 读取
///
/// ## 示例
///
/// ```no_run
/// use chimera_mas::prelude::*;
/// use chimera_mas::MAX_AGENT_DEPTH;
/// use event_bus::EventBus;
/// use nexus_core::{Task, TaskStatus};
/// use std::time::Duration;
///
/// # async fn run() -> chimera_mas::Result<()> {
/// let bus = EventBus::new();
/// let orchestrator = RootOrchestrator::new(bus.clone());
/// assert_eq!(orchestrator.max_depth(), MAX_AGENT_DEPTH);
///
/// let task = Task {
///     task_id: "t-1".into(),
///     description: "示例任务".into(),
///     status: TaskStatus::Pending,
///     dependencies: vec![],
/// };
/// let agent_task = AgentTask::new(
///     task,
///     TaskComplexity::Simple,
///     1000,
///     Duration::from_secs(60),
///     QualityLevel::Standard,
/// );
/// let handles = orchestrator.delegate(agent_task).await?;
/// assert_eq!(handles.len(), 1);
/// # Ok(())
/// # }
/// ```
pub struct RootOrchestrator {
    /// 最大委托深度(默认 `MAX_AGENT_DEPTH=5`)
    max_depth: usize,
    /// Agent 工厂(创建子 Agent + 发布 AgentTaskDelegated 事件)
    factory: AgentFactory,
    /// 事件总线(monitor 订阅 AgentHeartbeat)
    event_bus: EventBus,
    /// 心跳注册表(monitor 后台任务写入,Arc<Mutex> 跨任务共享)
    heartbeats: Arc<Mutex<HashMap<String, HeartbeatInfo>>>,
    /// 稳定性守护器(零孤儿终态保证 + 故障隔离,Task 19 §19.10)
    ///
    /// WHY Arc 包装:monitor() 后台任务需要共享 StabilityGuard,
    /// 后台任务在 spawn 时 Arc::clone,处理 AgentTaskCompleted/Failed 时
    /// 调用 record_terminal() 注册终态,调用 ensure_terminal_state() 校验。
    stability_guard: Arc<StabilityGuard>,
}

impl RootOrchestrator {
    /// 创建根协调器,使用默认最大深度(`MAX_AGENT_DEPTH=5`)
    ///
    /// ## 参数
    /// - `event_bus`: 事件总线(AgentFactory 用它创建 AgentContext + 发布事件,
    ///   monitor 用它订阅 AgentHeartbeat)
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_max_depth(event_bus, MAX_AGENT_DEPTH)
    }

    /// 创建根协调器,自定义最大深度
    ///
    /// ## 参数
    /// - `event_bus`: 事件总线
    /// - `max_depth`: 最大委托深度(传入 0 会被钳制为 1,避免所有委托立即失败)
    pub fn with_max_depth(event_bus: EventBus, max_depth: usize) -> Self {
        // WHY max_depth.max(1): 避免传入 0 导致所有委托立即失败(max_depth=0 时
        // delegation_depth=0 >= 0 恒成立,delegate 永远返回 MaxDepthExceeded)
        let clamped_depth = max_depth.max(1);
        let factory = AgentFactory::new(event_bus.clone());
        Self {
            max_depth: clamped_depth,
            factory,
            event_bus,
            heartbeats: Arc::new(Mutex::new(HashMap::new())),
            stability_guard: Arc::new(StabilityGuard::new()),
        }
    }

    /// 返回最大委托深度
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// 返回事件总线引用(供外部订阅事件或检查订阅者数)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 返回 Agent 工厂引用(供外部直接创建 Agent)
    pub fn factory(&self) -> &AgentFactory {
        &self.factory
    }

    /// 返回稳定性守护器引用(供外部校验零孤儿终态 / 故障隔离,Task 19 §19.10)
    ///
    /// ## 使用场景
    ///
    /// - 外部调用方主动校验任务终态:`orchestrator.stability_guard().ensure_terminal_state(task_id)`
    /// - 查询隔离子树:`orchestrator.stability_guard().is_isolated(subtree_id)`
    /// - 注册终态(通常由 monitor 后台任务自动处理,外部一般不需要手动调用)
    pub fn stability_guard(&self) -> &StabilityGuard {
        &self.stability_guard
    }

    /// 校验任务终态已注册(零孤儿校验,§6.1 红线 + Task 19 §19.10)
    ///
    /// ## 参数
    /// - `task_id`: 任务 ID
    ///
    /// ## 返回
    /// - `Ok(())`: 任务终态已注册(Completed 或 Failed)
    /// - `Err(MasError::Internal)`: 任务未注册终态(孤儿任务)
    ///
    /// ## 使用场景
    ///
    /// - Quest 收尾时校验所有子任务终态已注册
    /// - 测试中验证零孤儿不变量
    pub fn check_terminal_state(&self, task_id: &str) -> Result<()> {
        self.stability_guard.ensure_terminal_state(task_id)
    }

    /// 委托任务 — 根据 `task.complexity` 创建子 Agent 并返回句柄列表
    ///
    /// ## 分发策略(spec SubTask 12.2)
    ///
    /// | complexity | 子 Agent 数量 |
    /// |------------|--------------|
    /// | Simple     | 1            |
    /// | Medium     | 2            |
    /// | Complex    | 3            |
    /// | VeryComplex| 5            |
    ///
    /// ## 深度限制(spec SubTask 12.4)
    ///
    /// 若 `task.delegation_depth >= self.max_depth`,返回 `MaxDepthExceeded`。
    /// 子 Agent 的 `depth = task.delegation_depth + 1`。
    ///
    /// ## 事件发布
    ///
    /// `AgentFactory::create_agent` 内部用 `publish_blocking` 发布
    /// `NexusEvent::AgentTaskDelegated`(Normal 级,broadcast)。
    /// 订阅者需在调用 `delegate` 之前 `bus.subscribe()`(§4.4 反模式 3)。
    ///
    /// ## 错误
    /// - `MasError::MaxDepthExceeded`: `delegation_depth >= max_depth`
    /// - `MasError::AgentAlreadyExists`: agent_id 重复(不应发生,delegate 生成唯一 ID)
    /// - `MasError::AgentCreationFailed`: AgentContext 初始化或事件发布失败
    pub async fn delegate(&self, task: AgentTask) -> Result<Vec<AgentHandle>> {
        // 1. 深度检查(spec SubTask 12.4)
        self.check_depth(task.delegation_depth)?;

        // 2. 根据 complexity 决定子 Agent 数量
        let count = sub_agent_count(task.complexity);

        // 3. 子 Agent depth = parent.delegation_depth + 1
        let child_depth = task.delegation_depth + 1;

        // 4. 创建子 Agent,收集 AgentHandle
        let task_id = task.inner.task_id.clone();
        let mut handles = Vec::with_capacity(count);
        for index in 0..count {
            let agent_type = pick_agent_type(&task, child_depth, index);
            // WHY agent_id 格式 `{task_id}-sub-{index}`:保证同任务内子 Agent ID 唯一,
            // 且可从 handle.agent_id 反推所属任务(便于调试与日志关联)
            let agent_id = format!("{task_id}-sub-{index}");
            // create_agent 是 sync 方法,内部用 publish_blocking 发布事件(§4.4 反模式 8)
            let _agent = self.factory.create_agent(agent_type.clone(), &agent_id)?;
            handles.push(AgentHandle {
                agent_id,
                agent_type,
                depth: child_depth,
                current_task_id: Some(task_id.clone()),
            });
        }

        debug!(
            task_id = %task_id,
            complexity = ?task.complexity,
            child_count = count,
            child_depth = child_depth,
            "delegate 成功创建子 Agent"
        );

        Ok(handles)
    }

    /// 象限感知的孙层编排 (§3.3 / §5.4 / ADR-027 决策 1-3)
    ///
    /// 由子代理专家(SubAgent, depth 2)调用, 按任务复杂度激活四象限中的
    /// `≤ 4` 个, 为每个激活象限创建一个 `GrandAgent`(depth 3), 象限经
    /// `task_scope` 尾缀 `#Q1..#Q4` 编码, 强制 INV-3(扇出≤4) 与 INV-4(象限唯一)。
    ///
    /// ## 与 `delegate()` 的区别
    ///
    /// `delegate()` 按 `sub_agent_count` 创建同质子 Agent(主层/子层, 扇出无象限约束);
    /// 本方法在**孙层**按四象限稳定分工创建孙代理, 每个归属唯一象限, 扇出
    /// 恒 `≤ MAX_QUADRANT_FANOUT`。两者职责解耦, `delegate()` 语义保持不变。
    ///
    /// ## 参数
    /// - `sub_agent_id`: 发起编排的子代理专家 ID(作为孙代理 parent_id)
    /// - `task`: 待分工任务; 其 `complexity` 决定激活哪些象限(§3.4),
    ///   `delegation_depth` 为发起子代理的深度
    ///
    /// ## 返回
    /// - `Ok(Vec<AgentHandle>)`: 每个激活象限一个 `GrandAgent` 句柄(≤4),
    ///   句柄顺序遵循 Q1→Q2→Q3→Q4 稳定序
    /// - `Err(MasError::MaxDepthExceeded)`: `delegation_depth >= max_depth`(与 `delegate()` 一致)
    /// - `Err(MasError::AgentAlreadyExists)`: 同一 sub_agent_id 重复编排同象限
    ///
    /// ## 深度语义
    ///
    /// 与 `delegate()` 一致: 校验发起方深度 `check_depth(task.delegation_depth)`,
    /// 孙代理 `depth = task.delegation_depth + 1`。
    pub fn delegate_quadrants(
        &self,
        sub_agent_id: &str,
        task: &AgentTask,
    ) -> Result<Vec<AgentHandle>> {
        // 1. 深度校验(与 delegate 同语义: 发起方深度须 < max_depth)
        self.check_depth(task.delegation_depth)?;
        let grand_depth = task.delegation_depth + 1;

        // 2. 按复杂度构造象限分工计划(自动满足 INV-3/INV-4)
        let plan = QuadrantPlan::from_complexity(task.inner.task_id.clone(), task.complexity);

        // 3. 逐象限创建 GrandAgent, 象限编码进 task_scope
        let mut handles = Vec::with_capacity(plan.fanout());
        for (quadrant, scoped_scope) in plan.scoped_assignments() {
            let agent_type = AgentType::GrandAgent {
                parent_id: sub_agent_id.to_string(),
                task_scope: scoped_scope,
            };
            // WHY agent_id 含象限标签: 保证同一子代理下各象限孙代理 ID 唯一(天然满足 INV-4),
            // 且可从 agent_id 反推所属象限(便于调试与审计)。
            let agent_id = format!("{sub_agent_id}::{}", quadrant.tag().trim_start_matches('#'));
            let _agent = self.factory.create_agent(agent_type.clone(), &agent_id)?;
            handles.push(AgentHandle {
                agent_id,
                agent_type,
                depth: grand_depth,
                current_task_id: Some(task.inner.task_id.clone()),
            });
        }

        debug!(
            sub_agent_id = %sub_agent_id,
            task_id = %task.inner.task_id,
            complexity = ?task.complexity,
            quadrant_count = plan.fanout(),
            grand_depth = grand_depth,
            "delegate_quadrants 成功创建象限孙代理"
        );

        Ok(handles)
    }

    /// 监控 Agent 心跳 — 订阅 `EventTopic::Agent` 并收集 `AgentHeartbeat` 事件
    ///
    /// ## 实现细节(§4.4 反模式 3)
    ///
    /// `subscribe_filtered()` 必须在 `tokio::spawn()` **之前同步调用**,
    /// 确保不错过后续发布的心跳事件。spawn 后立即返回 `JoinHandle`,
    /// 后台任务持续运行直到通道关闭或被 abort。
    ///
    /// ## 心跳收集
    ///
    /// 后台任务从 `FilteredSubscriber::recv()` 接收事件:
    /// - `NexusEvent::AgentHeartbeat`: 提取字段,更新 `heartbeats` 注册表(同 agent_id 覆盖)
    /// - `NexusEvent::AgentTaskCompleted`: 注册终态 Completed 到 `stability_guard`,
    ///   并调用 `ensure_terminal_state()` 校验(零孤儿,Task 19 §19.10)
    /// - `NexusEvent::AgentTaskFailed`: 注册终态 Failed 到 `stability_guard`,
    ///   并调用 `ensure_terminal_state()` 校验(零孤儿,Task 19 §19.10)
    /// - 其他 Agent 主题事件(如 AgentTaskDelegated): 忽略,不记入注册表
    /// - 通道错误(ChannelClosed/SlowConsumerDropped): 记录 warn 日志,终止收集
    ///
    /// ## StabilityGuard 集成(Task 19 §19.10)
    ///
    /// 后台任务在处理 AgentTaskCompleted/Failed 时:
    /// 1. 调用 `record_terminal(task_id, state)` 注册终态
    /// 2. 调用 `ensure_terminal_state(task_id)` 校验注册成功(应返回 Ok)
    /// 3. 校验失败(不应发生)记 warn 日志,标记孤儿任务
    ///
    /// ## 返回
    /// - `Ok(JoinHandle)`: 后台任务已 spawn,调用方可 abort 或 await
    /// - `Err`: 不会返回(subscribe_filtered 不返回错误)
    pub async fn monitor(&self) -> Result<tokio::task::JoinHandle<()>> {
        // §4.4 反模式 3: subscribe 必须在 spawn 之前同步调用
        let mut rx = self
            .event_bus
            .subscribe_filtered(HashSet::from([EventTopic::Agent]));
        let heartbeats = self.heartbeats.clone();
        // WHY Arc::clone 而非 clone:StabilityGuard 内部 DashMap 已并发安全,
        // Arc::clone 共享同一实例,后台任务注册终态后主线程可立即通过
        // `stability_guard()` 访问器查询(零拷贝,无锁一致)
        let stability_guard = self.stability_guard.clone();

        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(NexusEvent::AgentHeartbeat {
                        from,
                        status,
                        current_task,
                        token_usage,
                        memory_usage_mb,
                        ..
                    }) => {
                        let info = HeartbeatInfo {
                            agent_id: from.clone(),
                            status,
                            current_task,
                            token_usage,
                            memory_usage_mb,
                            received_at: Utc::now(),
                        };
                        // WHY 先 await lock 再 insert:HashMap 无并发安全版本,
                        // 必须持锁修改。lock().await 不会跨 await 持锁(insert 后立即释放)
                        let mut guard = heartbeats.lock().await;
                        // 同 agent_id 覆盖(保留最新心跳状态)
                        guard.insert(from.clone(), info);
                        debug!(agent_id = %from, "monitor 收到 AgentHeartbeat");
                    }
                    Ok(NexusEvent::AgentTaskCompleted { task_id, .. }) => {
                        // Task 19 §19.10: 零孤儿终态注册 + 校验
                        stability_guard.record_terminal(task_id.clone(), TerminalState::Completed);
                        // 校验注册成功(应返回 Ok,失败则记 warn 标记孤儿)
                        if let Err(e) = stability_guard.ensure_terminal_state(&task_id) {
                            warn!(task_id = %task_id, error = %e, "零孤儿校验失败:AgentTaskCompleted 注册后仍报孤儿(不应发生)");
                        }
                        debug!(task_id = %task_id, "monitor 收到 AgentTaskCompleted,已注册终态");
                    }
                    Ok(NexusEvent::AgentTaskFailed { task_id, .. }) => {
                        // Task 19 §19.10: 零孤儿终态注册 + 校验
                        // AgentTaskFailed 是 Critical 级事件(§6.2 红线,走 mpsc),
                        // 但本订阅是 broadcast 旁路,仅用于注册终态,
                        // 不影响 Critical 事件的 mpsc 投递
                        stability_guard.record_terminal(task_id.clone(), TerminalState::Failed);
                        if let Err(e) = stability_guard.ensure_terminal_state(&task_id) {
                            warn!(task_id = %task_id, error = %e, "零孤儿校验失败:AgentTaskFailed 注册后仍报孤儿(不应发生)");
                        }
                        debug!(task_id = %task_id, "monitor 收到 AgentTaskFailed,已注册终态");
                    }
                    Ok(_) => {
                        // 非 AgentHeartbeat/AgentTaskCompleted/AgentTaskFailed 的 Agent 主题事件
                        // (如 AgentTaskDelegated/AgentConsultRequested 等),
                        // FilteredSubscriber 已过滤非 Agent 主题,此处仅跳过非终态变体
                        continue;
                    }
                    Err(e) => {
                        warn!(error = %e, "monitor 心跳订阅通道关闭,终止收集循环");
                        break;
                    }
                }
            }
        });

        Ok(handle)
    }

    /// 检查委托深度是否超限
    ///
    /// ## 语义
    ///
    /// `current_depth >= max_depth` 返回 `MaxDepthExceeded`,
    /// 否则返回 `Ok(())`。
    ///
    /// ## 使用场景
    ///
    /// - `delegate()` 内部调用:检查 `task.delegation_depth`
    /// - 外部调用:预检查某深度是否可委托
    pub fn check_depth(&self, current_depth: usize) -> Result<()> {
        if current_depth >= self.max_depth {
            Err(MasError::MaxDepthExceeded {
                current_depth,
                max_depth: self.max_depth,
            })
        } else {
            Ok(())
        }
    }

    /// 返回当前已收集的心跳数量
    ///
    /// 供测试与监控指标使用。async 方法因 `Mutex::lock().await`。
    pub async fn heartbeat_count(&self) -> usize {
        let guard = self.heartbeats.lock().await;
        guard.len()
    }

    /// 查询指定 Agent 的最新心跳信息
    ///
    /// ## 参数
    /// - `agent_id`: Agent ID
    ///
    /// ## 返回
    /// - `Some(HeartbeatInfo)`: 存在心跳记录
    /// - `None`: 未收到该 Agent 的心跳
    pub async fn get_heartbeat(&self, agent_id: &str) -> Option<HeartbeatInfo> {
        let guard = self.heartbeats.lock().await;
        guard.get(agent_id).cloned()
    }
}

impl std::fmt::Debug for RootOrchestrator {
    /// 手动实现 Debug,避免 `AgentFactory` 未派生 Debug 导致 derive 失败
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootOrchestrator")
            .field("max_depth", &self.max_depth)
            .field("factory", &self.factory)
            .field("subscriber_count", &self.event_bus.subscriber_count())
            .finish_non_exhaustive()
    }
}

// ============================================================
// 内部辅助函数
// ============================================================

/// 根据 `TaskComplexity` 返回子 Agent 数量
///
/// ## 分发策略(spec SubTask 12.2)
///
/// | complexity   | 数量 | 说明 |
/// |--------------|------|------|
/// | Simple       | 1    | 快速响应,单 Agent 执行 |
/// | Medium       | 2    | 标准任务,双 Agent 并行 |
/// | Complex      | 3    | 多文件重构,三 Agent 协作 |
/// | VeryComplex  | 5    | 跨系统迁移,五 Agent 并行 |
fn sub_agent_count(complexity: TaskComplexity) -> usize {
    match complexity {
        TaskComplexity::Simple => 1,
        TaskComplexity::Medium => 2,
        TaskComplexity::Complex => 3,
        TaskComplexity::VeryComplex => 5,
    }
}

/// 根据子 Agent 深度选择 `AgentType`
///
/// ## 深度→类型映射
///
/// | child_depth | AgentType     | 说明 |
/// |-------------|---------------|------|
/// | 1           | MainAgent     | RootOrchestrator 直接委托 |
/// | 2           | SubAgent      | MainAgent 委托 |
/// | 3+          | GrandAgent    | SubAgent 进一步委托(层级类型上限) |
///
/// WHY depth>=3 用 GrandAgent:`AgentType` 仅 4 种层级类型
/// (Root/Main/Sub/Grand),depth>=3 无更深层级类型,统一用 GrandAgent。
/// 实际深度由 `AgentHandle.depth` 字段精确记录,不依赖 AgentType。
fn pick_agent_type(task: &AgentTask, child_depth: usize, index: usize) -> AgentType {
    // parent_id:优先用 task.parent_agent_id,无则用 "root"(RootOrchestrator 直接委托)
    let parent_id = task
        .parent_agent_id
        .clone()
        .unwrap_or_else(|| "root".to_string());
    let task_scope = task.inner.task_id.clone();
    match child_depth {
        1 => AgentType::MainAgent {
            // WHY domain 用 index:AgentFactory::create_agent 无法从 task 推导领域,
            // 用 index 区分同任务的不同子 Agent(如 "domain-0" / "domain-1")
            domain: format!("domain-{index}"),
        },
        2 => AgentType::SubAgent {
            parent_id,
            task_scope,
        },
        _ => AgentType::GrandAgent {
            parent_id,
            task_scope,
        },
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_count_simple() {
        assert_eq!(sub_agent_count(TaskComplexity::Simple), 1);
    }

    #[test]
    fn test_sub_agent_count_medium() {
        assert_eq!(sub_agent_count(TaskComplexity::Medium), 2);
    }

    #[test]
    fn test_sub_agent_count_complex() {
        assert_eq!(sub_agent_count(TaskComplexity::Complex), 3);
    }

    #[test]
    fn test_sub_agent_count_very_complex() {
        assert_eq!(sub_agent_count(TaskComplexity::VeryComplex), 5);
    }

    #[test]
    fn test_max_agent_depth_constant() {
        assert_eq!(MAX_AGENT_DEPTH, 5);
    }
}
