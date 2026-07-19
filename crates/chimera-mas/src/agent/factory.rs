//! Agent 工厂与 Agent 实例 — 创建 Agent 并管理其生命周期(Task 8)
//!
//! 本模块定义 `AgentFactory`(创建 Agent)与 `Agent`(Agent 实例),
//! 是 MAS 子系统的核心入口。`AgentFactory` 负责校验 agent_id 唯一性、
//! 组装 Agent 三组件(AgentMeta + AgentContext + AgentLifecycle),
//! 并在创建完成后发布 `NexusEvent::AgentTaskDelegated` 事件。
//!
//! ## 架构合规
//!
//! - **§4.4 反模式 3**:`bus.subscribe()` 必须在 `create_agent` 之前同步调用,
//!   否则事件静默丢失。`create_agent` 是 sync 方法,内部用 `publish_blocking`
//!   同步发布事件,订阅者只需在调用前 `bus.subscribe()` 即可接收。
//! - **§4.4 反模式 8**:sync 方法用 `publish_blocking` 发布事件,
//!   不用 `publish().await`(create_agent 非 async)。
//! - **§6.2**:AgentTaskDelegated 是 Normal 级事件,走 broadcast(非 mpsc)。
//! - **ADR-026 决策 3**:AgentTask wrapper 包装 `nexus_core::Task`,不修改核心领域类型。
//! - **1M Token 等效**:context_window = 1_048_576(128K 实际 + 8× 稀疏压缩)。
//!
//! ## Agent 组件关系
//!
//! ```text
//! AgentFactory ──create_agent──▶ Agent
//!                                  ├── AgentMeta      (静态元数据 + 运行时 status)
//!                                  ├── AgentContext   (1M Token 等效上下文, HCW 稀疏化)
//!                                  └── AgentLifecycle (状态机: Idle→Running→...→Terminated)
//! ```
//!
//! `Agent` 的生命周期方法(start/pause/resume/complete/fail/crash/destroy/restart)
//! 委托给 `AgentLifecycle` 状态机,并同步更新 `AgentMeta.status`(对外可见状态)。
//! 这样 `AgentMeta.status` 与 `AgentLifecycle.state`(状态机真实状态)保持一致,
//! 避免双重状态源不一致。

use chrono::Utc;
use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent, TaskPriority};

use crate::agent::lifecycle::{AgentLifecycle, LifecycleState};
use crate::agent::meta::{AgentMeta, AgentStatus, AgentType, ModelConfig};
use crate::context::manager::AgentContext;
use crate::error::{MasError, Result};

// ============================================================
// AgentFactory — Agent 工厂
// ============================================================

/// Agent 工厂 — 创建 Agent 实例并发布 `AgentTaskDelegated` 事件
///
/// ## 职责
///
/// 1. 校验 `agent_id` 唯一性(重复创建触发 `MasError::AgentAlreadyExists`)
/// 2. 根据 `AgentType` 构造 `AgentMeta`(depth/parent_id/上下文窗口)
/// 3. 构造 `AgentContext`(1M Token 等效,经 HCW 稀疏化)
/// 4. 构造 `AgentLifecycle`(初始状态 `Idle`)
/// 5. 组装 `Agent` 并注册 agent_id 到内部注册表
/// 6. 发布 `NexusEvent::AgentTaskDelegated` 事件(Normal 级,broadcast)
///
/// ## 并发安全
///
/// `registry` 使用 `DashMap`(分片锁),读多写少场景下并发性能优于 `RwLock<HashMap>`。
/// `EventBus` 是 `Clone`(Arc-based,廉价),`AgentFactory` 可跨线程共享。
///
/// ## 示例
///
/// ```no_run
/// use chimera_mas::prelude::*;
/// use event_bus::EventBus;
///
/// let factory = AgentFactory::new(EventBus::new());
/// let agent = factory.create_agent(AgentType::RootOrchestrator, "agent-1")?;
/// # Ok::<(), chimera_mas::MasError>(())
/// ```
pub struct AgentFactory {
    /// 事件总线(创建 AgentContext + 发布 AgentTaskDelegated 事件)
    event_bus: EventBus,
    /// Agent ID 注册表(检测重复创建,DashMap 并发安全)
    registry: DashMap<String, ()>,
}

impl AgentFactory {
    /// 创建新的 Agent 工厂实例
    ///
    /// ## 参数
    /// - `event_bus`: 事件总线(AgentContext 内部 HCW 通信 + AgentTaskDelegated 发布)
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            registry: DashMap::new(),
        }
    }

    /// 返回工厂持有的事件总线引用
    ///
    /// 供外部调用方订阅事件(必须在 `create_agent` 之前 `bus.subscribe()`,
    /// §4.4 反模式 3)或检查订阅者数。
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 创建 Agent 实例并发布 `AgentTaskDelegated` 事件
    ///
    /// ## 参数
    /// - `agent_type`: Agent 层级类型(RootOrchestrator / MainAgent / SubAgent / ...)
    /// - `agent_id`: Agent 唯一标识(重复将触发 `AgentAlreadyExists`)
    ///
    /// ## 返回
    /// - `Ok(Agent)`: 创建成功,返回组装好的 Agent 实例
    /// - `Err(MasError::AgentAlreadyExists)`: agent_id 已存在于注册表
    /// - `Err(MasError::AgentCreationFailed)`: AgentContext 初始化或事件发布失败
    ///
    /// ## 事件发布(§4.4 反模式 8)
    ///
    /// `create_agent` 是 sync 方法,内部用 `publish_blocking` 同步发布
    /// `NexusEvent::AgentTaskDelegated`(Normal 级,broadcast)。
    /// 订阅者需在调用前 `bus.subscribe()`,否则事件被静默丢弃(无订阅者不报错)。
    pub fn create_agent(&self, agent_type: AgentType, agent_id: &str) -> Result<Agent> {
        // 1. 校验 agent_id 唯一性(跨 AgentType 也检测,避免 ID 冲突)
        if self.registry.contains_key(agent_id) {
            return Err(MasError::AgentAlreadyExists {
                agent_id: agent_id.to_string(),
            });
        }

        // 2. 根据 AgentType 构造 AgentMeta(depth/parent_id/上下文窗口)
        let meta = build_meta(agent_type, agent_id);

        // 3. 构造 AgentContext(EventBus 是 Clone/Arc-based,clone 廉价)
        let context = AgentContext::new(agent_id, meta.context_window, self.event_bus.clone())
            .map_err(|e| MasError::AgentCreationFailed {
                reason: format!("AgentContext 初始化失败: {e}"),
            })?;

        // 4. 构造 AgentLifecycle(初始状态 Idle)
        let lifecycle = AgentLifecycle::new();

        // 5. 组装 Agent
        let agent = Agent {
            meta,
            context,
            lifecycle,
        };

        // 6. 注册 agent_id 到注册表(后续重复创建将被拒绝)
        self.registry.insert(agent_id.to_string(), ());

        // 7. 发布 AgentTaskDelegated 事件(§4.4 反模式 8:sync 用 publish_blocking)
        //    WHY 先注册再发布:确保事件发布后,agent_id 已在注册表中,
        //    避免订阅者收到事件后查询注册表时 agent_id 尚未注册的竞态。
        let event = NexusEvent::AgentTaskDelegated {
            metadata: EventMetadata::new("chimera-mas:AgentFactory"),
            from: "AgentFactory".to_string(),
            to: agent_id.to_string(),
            task_id: format!("task-{agent_id}"),
            // deadline 默认 1 小时后(创建时设定合理上限,调度器据此排序)
            deadline: Utc::now() + chrono::Duration::hours(1),
            priority: TaskPriority::Medium,
        };
        self.event_bus
            .publish_blocking(event)
            .map_err(|e| MasError::AgentCreationFailed {
                reason: format!("AgentTaskDelegated 事件发布失败: {e}"),
            })?;

        Ok(agent)
    }
}

impl std::fmt::Debug for AgentFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFactory")
            .field("registered_count", &self.registry.len())
            .finish_non_exhaustive()
    }
}

impl Default for AgentFactory {
    /// 默认工厂 — 使用新建的 EventBus(无订阅者时事件静默丢弃)
    fn default() -> Self {
        Self::new(EventBus::default())
    }
}

// ============================================================
// Agent — Agent 实例
// ============================================================

/// Agent 实例 — 持有 AgentMeta + AgentContext + AgentLifecycle 三组件
///
/// 由 `AgentFactory::create_agent` 创建,不可直接构造(保证 agent_id 唯一性校验)。
///
/// ## 状态管理
///
/// `Agent` 的生命周期方法(start/pause/resume/complete/fail/crash/destroy/restart)
/// 委托给内部的 `AgentLifecycle` 状态机,每次成功转换后同步更新 `AgentMeta.status`。
/// 这样 `AgentMeta.status`(对外可见)与 `AgentLifecycle.state`(状态机真实状态)
/// 保持一致,避免双重状态源不一致。
///
/// ## 状态机(ADR-026 决策 5)
///
/// ```text
///     create ─▶ Idle ─start─▶ Running ─pause─▶ Paused
///                 ▲                 │              │
///                 │ restart         │ complete     │ resume
///                 │                 ▼              ▼
///                 │            Completed       Running
///                 │                 │
///                 │           fail  │  crash(非终态)
///                 │                 ▼
///                 └─────────── Failed
///                 │                 │
///                 │    destroy(任意)│
///                 │                 ▼
///                 └─── restart ── Terminated
/// ```
pub struct Agent {
    /// Agent 元数据(静态信息 + 运行时 status,对外可见)
    meta: AgentMeta,
    /// Agent 独立上下文(1M Token 等效,经 HCW 稀疏化)
    context: AgentContext,
    /// Agent 生命周期状态机(真实状态源)
    lifecycle: AgentLifecycle,
}

impl Agent {
    /// 返回 Agent 元数据引用(对外可见的静态信息 + 运行时 status)
    pub fn meta(&self) -> &AgentMeta {
        &self.meta
    }

    /// 返回 Agent 上下文引用(1M Token 等效上下文)
    pub fn context(&self) -> &AgentContext {
        &self.context
    }

    /// 返回 Agent 生命周期状态机引用
    pub fn lifecycle(&self) -> &AgentLifecycle {
        &self.lifecycle
    }

    /// 返回当前生命周期状态(便捷访问,等价于 `lifecycle().current_state()`)
    pub fn state(&self) -> LifecycleState {
        self.lifecycle.current_state()
    }

    /// 启动 Agent: Idle → Running
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Idle`
    pub fn start(&mut self) -> Result<()> {
        // disjoint borrows: &self.meta.agent_id 与 &mut self.lifecycle 是不同字段
        self.lifecycle.start(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 暂停 Agent: Running → Paused
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Running`
    pub fn pause(&mut self) -> Result<()> {
        self.lifecycle.pause(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 恢复 Agent: Paused → Running
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Paused`
    pub fn resume(&mut self) -> Result<()> {
        self.lifecycle.resume(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 标记任务成功: Running → Completed
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Running`
    pub fn complete(&mut self) -> Result<()> {
        self.lifecycle.complete(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 标记任务失败: Running | Failed → Failed(幂等)
    ///
    /// `Failed → Failed` 幂等(允许重复调用 fail 而不报错)。
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态既非 `Running` 也非 `Failed`
    pub fn fail(&mut self) -> Result<()> {
        self.lifecycle.fail(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 标记 Agent 崩溃: 非终态 → Failed(静默,无错误返回)
    ///
    /// 语义:Agent 发生 panic 或不可恢复错误,强制转为 `Failed`。
    /// 已终态(`Completed`/`Failed`/`Terminated`)调用 crash 为 no-op(静默忽略)。
    ///
    /// WHY 无错误返回:崩溃是不可恢复事件,调用方无法处理"崩溃失败"的情况。
    pub fn crash(&mut self) {
        self.lifecycle.crash();
        // crash 无论是否实际转换状态都同步 status(幂等,终态 crash 为 no-op)
        self.sync_status();
    }

    /// 销毁 Agent: 任意状态 → Terminated(幂等)
    ///
    /// 语义:主动终止 Agent 并回收资源。任意状态均可 destroy,
    /// 已 `Terminated` 再次 destroy 幂等成功(不重复计数)。
    ///
    /// ## 返回
    /// - `Ok(())`: 总是成功(destroy 是幂等终态转换)
    pub fn destroy(&mut self) -> Result<()> {
        self.lifecycle.destroy(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 重启 Agent: 终态 → Idle
    ///
    /// 语义:将 Agent 从终态(`Completed`/`Failed`/`Terminated`)重置为 `Idle`,
    /// 允许重新 start。非终态调用 restart 为非法转换。
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是终态
    pub fn restart(&mut self) -> Result<()> {
        self.lifecycle.restart(&self.meta.agent_id)?;
        self.sync_status();
        Ok(())
    }

    /// 分解 Agent 为三组件(meta, context, lifecycle)
    ///
    /// 供外部获取组件所有权(如将 context 传给 ContextIsolationGuard,
    /// 或将 meta 持久化)。分解后 Agent 不再可用。
    pub fn into_parts(self) -> (AgentMeta, AgentContext, AgentLifecycle) {
        (self.meta, self.context, self.lifecycle)
    }

    /// 同步 `AgentMeta.status` 与 `AgentLifecycle.current_state()`
    ///
    /// WHY 单一状态源原则:`AgentLifecycle` 是状态机真实状态源,
    /// `AgentMeta.status` 是对外可见的镜像。每次生命周期转换后调用此方法,
    /// 确保两者一致,避免双重状态源不一致问题。
    fn sync_status(&mut self) {
        self.meta.status = self.lifecycle.current_state().as_agent_status();
    }
}

impl std::fmt::Debug for Agent {
    /// 手动实现 Debug,避免 `AgentContext` 未派生 Debug 导致 derive 失败
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("agent_id", &self.meta.agent_id)
            .field("agent_type", &self.meta.agent_type)
            .field("state", &self.lifecycle.current_state())
            .field("transition_count", &self.lifecycle.transition_count())
            .field("context_max_tokens", &self.context.max_tokens)
            .finish_non_exhaustive()
    }
}

// ============================================================
// build_meta — 根据 AgentType 构造 AgentMeta
// ============================================================

/// 根据 `AgentType` 与 `agent_id` 构造 `AgentMeta`
///
/// 处理 5 种 AgentType 的 depth/parent_id/name/description:
///
/// | AgentType | depth | parent_id | 说明 |
/// |-----------|-------|-----------|------|
/// | RootOrchestrator | 0 | None | 全局编排入口 |
/// | MainAgent { domain } | 1 | None | 域级执行(工厂无法获知父 ID) |
/// | SubAgent { parent_id, task_scope } | 2 | Some(parent_id) | 子任务执行 |
/// | GrandAgent { parent_id, task_scope } | 3 | Some(parent_id) | 孙任务执行 |
/// | ExpertAgent { specialty } | 0 | None | 专家咨询(不参与层级委托) |
///
/// WHY MainAgent parent_id=None:`AgentFactory::create_agent(agent_type, agent_id)`
/// 仅接收 agent_type 与 agent_id,无父 ID 参数。MainAgent 的 parent_id 应由
/// `RootOrchestrator::delegate()` 在委托时设置(Task 12 实现),工厂创建阶段
/// 留空。测试 `test_create_main_agent` 也未验证 MainAgent 的 parent_id。
///
/// context_window 统一为 1_048_576(1M Token 等效 = 128K 实际 + 8× 稀疏压缩)。
fn build_meta(agent_type: AgentType, agent_id: &str) -> AgentMeta {
    let depth = agent_type.depth();
    let parent_id = match &agent_type {
        AgentType::SubAgent { parent_id, .. } | AgentType::GrandAgent { parent_id, .. } => {
            Some(parent_id.clone())
        }
        // RootOrchestrator / MainAgent / ExpertAgent 均无父 ID(工厂创建阶段)
        _ => None,
    };
    let (name, description) = match &agent_type {
        AgentType::RootOrchestrator => (
            "RootOrchestrator".to_string(),
            "全局任务编排入口".to_string(),
        ),
        AgentType::MainAgent { domain } => (
            "MainAgent".to_string(),
            format!("域级任务执行 Agent (domain={domain})"),
        ),
        AgentType::SubAgent { task_scope, .. } => (
            "SubAgent".to_string(),
            format!("子任务执行 Agent (scope={task_scope})"),
        ),
        AgentType::GrandAgent { task_scope, .. } => (
            "GrandAgent".to_string(),
            format!("孙任务执行 Agent (scope={task_scope})"),
        ),
        AgentType::ExpertAgent { specialty } => (
            "ExpertAgent".to_string(),
            format!("专家咨询 Agent (specialty={:?})", specialty),
        ),
    };

    AgentMeta {
        agent_id: agent_id.to_string(),
        agent_type,
        name,
        description,
        model_config: ModelConfig::default(),
        // 1M Token 等效 = 128K 实际 + 8× 稀疏压缩(ADR-026 决策 7)
        context_window: 1_048_576,
        parent_id,
        children_ids: Vec::new(),
        created_at: chrono::Utc::now(),
        status: AgentStatus::Idle,
        depth,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_meta_root_orchestrator() {
        let meta = build_meta(AgentType::RootOrchestrator, "root-1");
        assert_eq!(meta.agent_id, "root-1");
        assert_eq!(meta.depth, 0);
        assert!(meta.parent_id.is_none());
        assert_eq!(meta.context_window, 1_048_576);
        assert_eq!(meta.status, AgentStatus::Idle);
    }

    #[test]
    fn test_build_meta_sub_agent_parent_id() {
        let agent_type = AgentType::SubAgent {
            parent_id: "main-1".into(),
            task_scope: "impl".into(),
        };
        let meta = build_meta(agent_type, "sub-1");
        assert_eq!(meta.depth, 2);
        assert_eq!(meta.parent_id.as_deref(), Some("main-1"));
    }

    #[test]
    fn test_build_meta_expert_agent_no_parent() {
        let agent_type = AgentType::ExpertAgent {
            specialty: vec!["sec".into()],
        };
        let meta = build_meta(agent_type, "expert-1");
        assert_eq!(meta.depth, 0);
        assert!(meta.parent_id.is_none());
    }

    #[test]
    fn test_agent_factory_default() {
        let factory = AgentFactory::default();
        assert_eq!(factory.event_bus().subscriber_count(), 0);
    }
}
