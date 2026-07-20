//! 委托执行 — 占位实现(Task 11 + Task 13 将完善)
//!
//! 本文件定义 MAS 子系统的委托执行类型:
//! - `AgentTask`: 任务 wrapper,包装 `nexus_core::Task` 并扩展 MAS 特有字段
//! - `TaskComplexity`: 任务复杂度枚举(Simple/Medium/Complex/VeryComplex)
//! - `TaskResult`: 任务执行结果
//! - `DelegationExecutor`: 委托执行器,并行执行多个子任务
//!
//! ## ADR-026 决策 3: AgentTask wrapper(不修改核心类型)
//!
//! `AgentTask` 包装 `nexus_core::Task`,扩展 MAS 特有字段(complexity / estimated_tokens /
//! parent_agent_id / delegation_depth 等),**不修改 `nexus_core::Task` 的任何字段**
//! (遵守 §3.3.1 第 4 条领域类型稳定性)。
//!
//! ## ADR-026 决策 6: TaskComplexity → ThinkingMode 映射
//!
//! - `Simple` → `ThinkingMode::Fast`(快速响应)
//! - `Medium` → `ThinkingMode::Standard`(标准深度)
//! - `Complex` / `VeryComplex` → `ThinkingMode::Deep`(深度推理)

use crate::error::{MasError, Result};
use event_bus::{EventBus, EventMetadata, NexusEvent, TaskPriority};
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

/// 任务复杂度枚举 — 用于评估任务难度并映射到 ThinkingMode
///
/// ADR-026 决策 6: 通过 `From<TaskComplexity> for nexus_core::ThinkingMode` 映射
/// (Task 7.4 实现,本文件占位不实现 From impl 以避免循环依赖)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskComplexity {
    /// 简单任务:快速响应,如查询、格式化(→ ThinkingMode::Fast)
    Simple,
    /// 中等任务:标准深度,如单文件修改、单元测试(→ ThinkingMode::Standard)
    Medium,
    /// 复杂任务:深度推理,如多文件重构、架构设计(→ ThinkingMode::Deep)
    Complex,
    /// 极复杂任务:超深度推理,如跨系统迁移、性能调优(→ ThinkingMode::Deep)
    VeryComplex,
}

// ============================================================
// Task 7.4: TaskComplexity → ThinkingMode 映射(GREEN 阶段)
// ============================================================

/// 任务复杂度到思考模式的映射 — ADR-026 决策 6
///
/// WHY 此 impl 放在 delegation.rs 而非 meta.rs:
/// - TaskComplexity 定义在 delegation.rs,遵循"为类型实现 trait 时,impl 与类型同模块"惯例
/// - 避免在 meta.rs 中 import TaskComplexity 造成模块间耦合
/// - nexus_core::ThinkingMode 是 L1 Core 公共类型,可在任何上层 crate 实现 From
///
/// ## 映射规则(与 ADR-026 决策 6 + spec.md 一致)
///
/// | TaskComplexity | ThinkingMode | 适用场景 |
/// |----------------|--------------|----------|
/// | Simple         | Fast         | 查询、格式化等快速响应任务 |
/// | Medium         | Standard     | 单文件修改、单元测试等常规任务 |
/// | Complex        | Deep         | 多文件重构、架构设计等深度推理任务 |
/// | VeryComplex    | Deep         | 跨系统迁移、性能调优等超深度推理任务 |
///
/// ## 设计权衡
///
/// `Complex` 与 `VeryComplex` 均映射到 `Deep`,因为 nexus_core::ThinkingMode
/// 仅有三级(Fast/Standard/Deep,ADR-026 决策 6 不新建 Max)。
/// VeryComplex 与 Complex 的区别在 `estimated_tokens` / `acceptable_latency`
/// 等 AgentTask 字段中体现,而非 ThinkingMode。
impl From<TaskComplexity> for nexus_core::ThinkingMode {
    fn from(complexity: TaskComplexity) -> Self {
        match complexity {
            TaskComplexity::Simple => Self::Fast,
            TaskComplexity::Medium => Self::Standard,
            TaskComplexity::Complex | TaskComplexity::VeryComplex => Self::Deep,
        }
    }
}

/// 任务质量等级 — 定义可接受的结果质量
///
/// 用于 DelegationExecutor 决定是否需要重试或专家咨询。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualityLevel {
    /// 草稿质量:可包含错误,用于初步探索
    Draft,
    /// 标准质量:满足基本要求,可用于生产
    Standard,
    /// 高质量:经过验证,可用于关键路径
    High,
    /// 生产质量:经过完整测试与审查
    Production,
}

/// Agent 任务 wrapper — 包装 nexus_core::Task 并扩展 MAS 特有字段
///
/// ADR-026 决策 3: **不修改 nexus_core::Task**,通过 wrapper 模式扩展字段。
/// `inner` 字段持有原始 Task,MAS 特有字段在 wrapper 层。
///
/// ## trait 派生
///
/// - `Clone` / `Debug`: 标准调试与复制
/// - `Serialize` / `Deserialize`: serde 序列化(JSON + MessagePack,ADR-004)
/// - `PartialEq`: 字段级相等性比较(依赖 `nexus_core::Task: PartialEq`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentTask {
    /// 内部包装的 nexus_core::Task(不修改,§3.3.1 第 4 条领域类型稳定性)
    pub inner: nexus_core::Task,
    /// 任务复杂度(用于映射 ThinkingMode + 决定委托深度)
    pub complexity: TaskComplexity,
    /// 预估 Token 消耗(用于预算分配)
    pub estimated_tokens: usize,
    /// 可接受延迟(超时将触发 MasError::TaskTimeout)
    pub acceptable_latency: Duration,
    /// 质量要求(决定是否需要重试或专家咨询)
    pub quality_requirement: QualityLevel,
    /// 任务调度优先级(§8 调度一等公民,ADR-027 决策 4)
    ///
    /// WHY 复用 `event_bus::TaskPriority`(L1)而非新建:§2.2 依赖铁律,
    /// 轻量枚举下沉 L1 供 L9 向下复用;`new()` 默认 `Medium`,可经
    /// `with_priority()` builder 覆盖。
    pub priority: TaskPriority,
    /// 委托方 Agent ID(None 表示由 RootOrchestrator 直接发起)
    pub parent_agent_id: Option<String>,
    /// 委托深度(0=RootOrchestrator 直接执行,1=MainAgent 委托,...)
    pub delegation_depth: usize,
}

impl AgentTask {
    /// 创建新的 AgentTask wrapper(5 参数完整构造)
    ///
    /// ADR-026 决策 3: 包装 nexus_core::Task,不修改核心类型。
    /// `parent_agent_id` 和 `delegation_depth` 使用默认值(None / 0),
    /// 如需设置请用 `with_parent()` / `with_depth()` builder 方法。
    ///
    /// ## 参数
    /// - `task`: 原始 nexus_core::Task(不修改,§3.3.1 第 4 条领域类型稳定性)
    /// - `complexity`: 任务复杂度(用于映射 ThinkingMode + 决定委托深度)
    /// - `estimated_tokens`: 预估 Token 消耗(用于预算分配)
    /// - `acceptable_latency`: 可接受延迟(std::time::Duration,超时触发 MasError::TaskTimeout)
    /// - `quality_requirement`: 质量要求(决定是否需要重试或专家咨询)
    ///
    /// ## 复用 Task 7.4
    ///
    /// `complexity` 可通过 `From<TaskComplexity> for ThinkingMode` 转换为思考模式
    /// (Task 7.4 已实现,见本文件上方 impl 块)。
    pub fn new(
        task: nexus_core::Task,
        complexity: TaskComplexity,
        estimated_tokens: usize,
        acceptable_latency: Duration,
        quality_requirement: QualityLevel,
    ) -> Self {
        Self {
            inner: task,
            complexity,
            estimated_tokens,
            acceptable_latency,
            quality_requirement,
            // WHY 默认 Medium:多数任务为常规优先级;Critical/High/Low 经
            // with_priority() 显式设置,保持 new() 5 参数签名不变(非破坏性)。
            priority: TaskPriority::Medium,
            // 默认值:RootOrchestrator 直接发起,深度 0
            parent_agent_id: None,
            delegation_depth: 0,
        }
    }

    /// 设置委托方 Agent ID(builder 模式)
    ///
    /// ## 参数
    /// - `parent_id`: 委托方 Agent ID
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use nexus_core::{Task, TaskStatus};
    /// use std::time::Duration;
    ///
    /// let task = Task {
    ///     task_id: "t-1".into(),
    ///     description: "示例".into(),
    ///     status: TaskStatus::Pending,
    ///     dependencies: vec![],
    /// };
    /// let agent_task = AgentTask::new(
    ///     task,
    ///     TaskComplexity::Medium,
    ///     1000,
    ///     Duration::from_secs(60),
    ///     QualityLevel::Standard,
    /// )
    /// .with_parent("main-agent-1");
    /// ```
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_agent_id = Some(parent_id.into());
        self
    }

    /// 设置委托深度(builder 模式)
    ///
    /// ## 参数
    /// - `depth`: 委托深度(0=RootOrchestrator 直接执行,1=MainAgent 委托,...)
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.delegation_depth = depth;
        self
    }

    /// 设置任务调度优先级(builder 模式)— §8 调度一等公民
    ///
    /// ## 参数
    /// - `priority`: 任务优先级(Low/Medium/High/Critical)
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use event_bus::TaskPriority;
    /// use nexus_core::{Task, TaskStatus};
    /// use std::time::Duration;
    ///
    /// let task = Task {
    ///     task_id: "t-1".into(),
    ///     description: "示例".into(),
    ///     status: TaskStatus::Pending,
    ///     dependencies: vec![],
    /// };
    /// let agent_task = AgentTask::new(
    ///     task,
    ///     TaskComplexity::Medium,
    ///     1000,
    ///     Duration::from_secs(60),
    ///     QualityLevel::Standard,
    /// )
    /// .with_priority(TaskPriority::Critical);
    /// ```
    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }
}

/// 任务执行结果 — 子任务完成后返回
///
/// 用于 DelegationExecutor 聚集多个子任务结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// 关联的任务 ID
    pub task_id: String,
    /// 执行是否成功
    pub success: bool,
    /// 结果摘要(成功时为输出概要,失败时为错误信息)
    pub summary: String,
    /// 实际消耗 Token 数
    pub tokens_used: usize,
    /// 实际执行时长
    pub duration: Duration,
    /// 执行 Agent ID
    pub agent_id: String,
}

impl TaskResult {
    /// 创建成功结果
    pub fn success(
        task_id: impl Into<String>,
        agent_id: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            success: true,
            summary: summary.into(),
            tokens_used: 0,
            duration: Duration::ZERO,
            agent_id: agent_id.into(),
        }
    }

    /// 创建失败结果
    pub fn failure(
        task_id: impl Into<String>,
        agent_id: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            success: false,
            summary: error.into(),
            tokens_used: 0,
            duration: Duration::ZERO,
            agent_id: agent_id.into(),
        }
    }
}

/// 任务执行器 — 单个 AgentTask 的执行闭包类型(注入实际执行逻辑)
///
/// 解耦“并行调度/超时/事件发布”(DelegationExecutor 职责)与“任务执行”
/// (实际 Agent / LLM 调用)。闭包接收 `AgentTask`,返回 `Result<摘要, 错误>`。
///
/// WHY `Arc<dyn Fn ... + Send + Sync>`:
/// - `Send + Sync`:DelegationExecutor 可跨线程共享,闭包需线程安全
/// - `Arc`:每个 `tokio::spawn` 的子任务 `Arc::clone` 一份,廉价引用计数
/// - `BoxFuture<'static>`:spawn 的 future 需 `'static` 以脱离借用(§4.1)
pub type TaskRunner =
    Arc<dyn Fn(AgentTask) -> BoxFuture<'static, std::result::Result<String, String>> + Send + Sync>;

/// 委托执行器 — 并行执行多个子任务并聚集结果
///
/// ADR-026 决策 5: 用 `tokio::time::timeout` 包装超时(零孤儿调用,§6.1 红线)。
/// §4.1 规范: 并发收集用 `FuturesUnordered`(不用 `join_all`)。
/// §6.2 红线: 子任务失败/超时发布 `AgentTaskFailed`(Critical 级,走 mpsc 双通道)。
///
/// ## 超时决策(SubTask 13.4 + 13.5)
///
/// - 子任务 `acceptable_latency > 0` → `min(acceptable_latency, default_timeout)`
///   (子任务可自定义更短超时,但不超过框架上限)
/// - 子任务 `acceptable_latency == 0` → 回退到 `default_timeout`
/// - 超时使用 `std::time::Duration`(非 `chrono::Duration`,SubTask 13.5)
///
/// ## 示例
///
/// ```no_run
/// use chimera_mas::prelude::*;
/// use event_bus::EventBus;
/// use std::time::Duration;
///
/// let bus = EventBus::new();
/// let executor = DelegationExecutor::new(bus, Duration::from_secs(60));
/// // let results = executor.execute_delegation("parent-1", sub_tasks).await?;
/// ```
pub struct DelegationExecutor {
    /// 事件总线(发布 AgentTaskCompleted / AgentTaskFailed)
    event_bus: EventBus,
    /// 默认超时(子任务 acceptable_latency 为 0 时回退使用)
    default_timeout: Duration,
    /// 任务执行闭包(注入实际执行逻辑,默认总是成功)
    task_runner: TaskRunner,
}

impl DelegationExecutor {
    /// 创建新的委托执行器(使用默认成功 runner)
    ///
    /// 默认 runner 总是返回 `Ok("default-{task_id}")`,适用于:
    /// - 仅需验证委托调度/超时/事件发布逻辑的场景
    /// - 测试中通过 `with_runner` 替换为自定义 runner
    ///
    /// ## 参数
    /// - `event_bus`: 事件总线(发布 AgentTaskCompleted / AgentTaskFailed)
    /// - `default_timeout`: 默认超时(子任务 acceptable_latency 为 0 时回退使用)
    pub fn new(event_bus: EventBus, default_timeout: Duration) -> Self {
        Self {
            event_bus,
            default_timeout,
            task_runner: default_task_runner(),
        }
    }

    /// 创建新的委托执行器(注入自定义 TaskRunner)
    ///
    /// ## 参数
    /// - `event_bus`: 事件总线
    /// - `default_timeout`: 默认超时
    /// - `task_runner`: 任务执行闭包(注入实际执行逻辑)
    ///
    /// ## WHY 注入模式
    ///
    /// spec 原描述 `new(event_bus, default_timeout)` 签名无法测试失败/超时场景。
    /// 通过 `with_runner` 注入闭包,测试可控制子任务的成功/失败/超时行为,
    /// 验证聚集与事件发布逻辑,无需依赖真实 Agent / LLM 调用。
    pub fn with_runner(
        event_bus: EventBus,
        default_timeout: Duration,
        task_runner: TaskRunner,
    ) -> Self {
        Self {
            event_bus,
            default_timeout,
            task_runner,
        }
    }

    /// 获取默认超时
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 执行委托 — 并行执行多个子任务并聚集结果
    ///
    /// ## 参数
    /// - `parent_id`: 委托方 Agent ID(用于派生子任务 agent_id + 事件 `to` 字段)
    /// - `sub_tasks`: 子任务列表
    ///
    /// ## 返回
    /// - `Ok(Vec<TaskResult>)`: 所有子任务执行结果(成功/失败均包含,超时转为失败)
    /// - `Err(MasError::DelegationFailed)`: 委托执行框架错误(spawn JoinError)
    ///
    /// ## 设计约束(§4.1 + §6.1 + §6.2 红线)
    ///
    /// - 使用 `FuturesUnordered`(不用 `join_all`,§4.1)
    /// - 每个子任务通过 `tokio::time::timeout` 包装超时(零孤儿,§6.1)
    /// - 不持锁跨 `.await`(§6.2 红线,本模块无锁,天然满足)
    /// - 子任务完成发布 `NexusEvent::AgentTaskCompleted`(Normal 级)
    /// - 子任务失败/超时发布 `NexusEvent::AgentTaskFailed`(Critical 级,走 mpsc 双通道)
    ///
    /// ## 超时决策(SubTask 13.4 + 13.5)
    ///
    /// - 子任务 `acceptable_latency > 0` → `min(acceptable_latency, default_timeout)`
    ///   (子任务可自定义更短超时,但不超过框架上限)
    /// - 子任务 `acceptable_latency == 0` → 回退到 `default_timeout`
    /// - 超时使用 `std::time::Duration`(非 `chrono::Duration`,SubTask 13.5)
    pub async fn execute_delegation(
        &self,
        parent_id: &str,
        sub_tasks: Vec<AgentTask>,
    ) -> Result<Vec<TaskResult>> {
        // 空任务列表直接返回空 Vec(避免无意义的 spawn)
        if sub_tasks.is_empty() {
            return Ok(Vec::new());
        }

        // 并行 spawn 每个子任务,用 FuturesUnordered 收集 JoinHandle
        // WHY FuturesUnordered:§4.1 规范,优于 join_all,支持流式结果收集
        let mut futures: FuturesUnordered<tokio::task::JoinHandle<TaskResult>> =
            FuturesUnordered::new();

        for task in sub_tasks {
            // 每个 spawn 的子任务 Arc::clone 一份 task_runner + EventBus
            // WHY Arc::clone:§4.4 反模式 5,async 任务共享状态必须用 Arc::clone 而非 clone
            let runner = Arc::clone(&self.task_runner);
            let bus = self.event_bus.clone();
            // effective_timeout 在 spawn 之前同步调用(借用 task),之后 task 被 move
            let timeout = effective_timeout(&task, self.default_timeout);
            let agent_id = format!("{parent_id}::sub::{}", task.inner.task_id);
            let parent_id_owned = parent_id.to_string();

            let handle = tokio::spawn(execute_single_task(
                task,
                runner,
                bus,
                timeout,
                agent_id,
                parent_id_owned,
            ));
            futures.push(handle);
        }

        // 流式收集所有子任务结果
        // WHY while + next():任意子任务完成即取出,避免等待最慢任务才开始处理
        let mut results = Vec::with_capacity(futures.len());
        while let Some(join_result) = futures.next().await {
            match join_result {
                Ok(task_result) => results.push(task_result),
                Err(join_err) => {
                    // JoinError 仅在 spawn 的 task panic 或被 cancel 时出现
                    // 返回框架错误,因为无法关联具体 task_id
                    warn!(error = %join_err, "子任务 JoinHandle 异常");
                    return Err(MasError::DelegationFailed {
                        reason: format!("子任务 spawn 异常: {join_err}"),
                    });
                }
            }
        }

        Ok(results)
    }

    /// 批量执行委托 — 切块后的子任务批量并行执行(Task 16 §16.9)
    ///
    /// 与 `execute_delegation` 的语义区别:
    /// - `execute_delegation`: 通用委托执行(子任务未切块,来自 RootOrchestrator::delegate)
    /// - `execute_batch_delegation`: 接收 `TaskChunker::chunk` 切块后的 chunks,
    ///   agent_id 后缀 `::batch::` 标识批量切块路径,便于审计追溯
    ///
    /// # 设计约束
    ///
    /// - **保留 `execute_delegation` 语义不变**(非破坏性扩展)
    /// - **复用 `execute_single_task`**(零代码重复,与 `BatchExecutor::execute_batch`
    ///   代码相似但解耦,后者独立实现以支持 `metadata.source` 差异化)
    /// - **零孤儿包装**: `tokio::time::timeout` + `FuturesUnordered`(§6.1 + §4.1)
    /// - **Critical 事件走 mpsc**: `AgentTaskFailed` 经 `publish_critical`(§6.2 红线)
    ///
    /// # 参数
    ///
    /// - `parent_id`: 委托方 Agent ID
    /// - `chunks`: `TaskChunker::chunk` 产生的切块子任务列表
    ///
    /// # 返回
    ///
    /// - `Ok(Vec<TaskResult>)`: 各块执行结果(成功/失败均包含,超时转为失败)
    /// - `Err(MasError::DelegationFailed)`: 委托执行框架错误(spawn JoinError)
    ///
    /// # 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use event_bus::EventBus;
    /// use std::time::Duration;
    ///
    /// # async fn example(parent_id: &str, chunks: Vec<AgentTask>) -> chimera_mas::Result<Vec<TaskResult>> {
    /// let executor = DelegationExecutor::new(EventBus::new(), Duration::from_secs(60));
    /// executor.execute_batch_delegation(parent_id, chunks).await
    /// # }
    /// ```
    pub async fn execute_batch_delegation(
        &self,
        parent_id: &str,
        chunks: Vec<AgentTask>,
    ) -> Result<Vec<TaskResult>> {
        // 空切块列表直接返回(避免无意义的 spawn)
        if chunks.is_empty() {
            return Ok(Vec::new());
        }

        // 复用 execute_delegation 的 FuturesUnordered + tokio::time::timeout 模式
        // WHY 不依赖 BatchExecutor: DelegationExecutor 自包含,避免引入额外耦合;
        // BatchExecutor 提供独立的批量执行能力(事件 source 不同),两者解耦。
        let mut futures: FuturesUnordered<tokio::task::JoinHandle<TaskResult>> =
            FuturesUnordered::new();

        for task in chunks {
            let runner = Arc::clone(&self.task_runner);
            let bus = self.event_bus.clone();
            let timeout = effective_timeout(&task, self.default_timeout);
            // WHY agent_id 后缀 ::batch::: 与 execute_delegation 的 ::sub:: 区分,
            // 便于审计追溯区分批量切块与普通委托
            let agent_id = format!("{parent_id}::batch::{}", task.inner.task_id);
            let parent_id_owned = parent_id.to_string();

            let handle = tokio::spawn(execute_single_task(
                task,
                runner,
                bus,
                timeout,
                agent_id,
                parent_id_owned,
            ));
            futures.push(handle);
        }

        let mut results = Vec::with_capacity(futures.len());
        while let Some(join_result) = futures.next().await {
            match join_result {
                Ok(task_result) => results.push(task_result),
                Err(join_err) => {
                    warn!(error = %join_err, "批量委托子任务 JoinHandle 异常");
                    return Err(MasError::DelegationFailed {
                        reason: format!("批量委托 spawn 异常: {join_err}"),
                    });
                }
            }
        }

        Ok(results)
    }
}

// ============================================================
// 手动 trait 实现(TaskRunner 含 dyn Fn,不支持 derive)
// ============================================================

impl std::fmt::Debug for DelegationExecutor {
    /// 手动实现 Debug — EventBus 未派生 Debug,用 subscriber_count 替代
    /// (参考 RootOrchestrator / AgentContext 的 Debug 实现模式)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationExecutor")
            .field("default_timeout", &self.default_timeout)
            .field("subscriber_count", &self.event_bus.subscriber_count())
            .field("task_runner", &"<closure>")
            .finish()
    }
}

impl Default for DelegationExecutor {
    /// 默认构造 — EventBus::default() + 60s 超时 + 默认 runner
    fn default() -> Self {
        Self::new(EventBus::default(), Duration::from_secs(60))
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 计算子任务的有效超时(SubTask 13.5)
///
/// - `acceptable_latency == 0` → 回退到 `default_timeout`(未设置自定义超时)
/// - `acceptable_latency > 0` → `min(acceptable_latency, default_timeout)`
///   (子任务可自定义更短超时,但不超过框架上限)
///
/// WHY min:框架设定超时上限(default_timeout),子任务可在上限内自定义更短超时。
/// 测试验证三场景:
/// - acceptable_latency=10s, default_timeout=100ms → 用 100ms(框架上限更严)
/// - acceptable_latency=50ms, default_timeout=10s → 用 50ms(子任务自定义更短)
/// - acceptable_latency=0 → 用 default_timeout(回退)
fn effective_timeout(task: &AgentTask, default_timeout: Duration) -> Duration {
    if task.acceptable_latency > Duration::ZERO {
        task.acceptable_latency.min(default_timeout)
    } else {
        default_timeout
    }
}

/// 默认任务 runner — 总是返回成功(用于 `new()` 构造)
///
/// 返回固定摘要 `"default-{task_id}"`,不执行实际逻辑。
/// 生产场景应通过 `with_runner` 注入真实执行逻辑。
fn default_task_runner() -> TaskRunner {
    Arc::new(|task: AgentTask| {
        Box::pin(async move { Ok(format!("default-{}", task.inner.task_id)) })
    })
}

/// 执行单个子任务 — 包含超时包装 + 事件发布(SubTask 13.4 + 13.6)
///
/// 此函数被 `tokio::spawn` 调用,需满足 `Send + 'static`。
/// 所有参数均为 owned 类型(无借用),确保 spawn 后独立执行。
///
/// ## 执行流程
///
/// 1. 记录开始时间 + clone task_id(task 将被 runner 消费)
/// 2. `tokio::time::timeout(timeout, runner(task))` 包装执行(零孤儿,§6.1)
/// 3. 根据结果构造 `TaskResult` + 发布对应事件:
///    - `Ok(Ok(summary))` → 成功 + 发布 `AgentTaskCompleted`(Normal 级,broadcast)
///    - `Ok(Err(error))` → 失败 + 发布 `AgentTaskFailed`(Critical 级,mpsc 双通道)
///    - `Err(_elapsed)` → 超时 + 发布 `AgentTaskFailed`(Critical 级,mpsc 双通道)
///
/// ## 事件字段约定
///
/// - `from`: 执行子任务的 agent_id(`{parent_id}::sub::{task_id}`)
/// - `to`: 委托方 parent_id
/// - `metadata.source`: `"chimera-mas:DelegationExecutor"`
async fn execute_single_task(
    task: AgentTask,
    runner: TaskRunner,
    bus: EventBus,
    timeout: Duration,
    agent_id: String,
    parent_id: String,
) -> TaskResult {
    // 在 task 被 runner 消费前,先 clone 出 task_id 用于事件发布与结果构造
    let task_id = task.inner.task_id.clone();
    let start = std::time::Instant::now();

    // tokio::time::timeout 包装执行(零孤儿调用,§6.1 红线)
    // WHY tokio::time::timeout 而非 gqep_executor:
    // gqep_executor::gather() 返回 GatherResult(仅统计,不保留独立结果),
    // 语义不匹配 DelegationExecutor 需返回 Vec<TaskResult> 的需求(SubTask 13.4 决策)
    let outcome = tokio::time::timeout(timeout, runner(task)).await;

    let duration = start.elapsed();

    match outcome {
        // runner 在超时内完成且成功
        Ok(Ok(summary)) => {
            debug!(task_id = %task_id, duration = ?duration, "子任务执行成功");
            let event = NexusEvent::AgentTaskCompleted {
                metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
                from: agent_id.clone(),
                to: parent_id,
                task_id: task_id.clone(),
                result_summary: summary.clone(),
            };
            // Normal 级事件走 broadcast(非 Critical,用 publish 而非 publish_critical)
            let _ = bus.publish(event).await;
            TaskResult {
                task_id,
                success: true,
                summary,
                tokens_used: 0,
                duration,
                agent_id,
            }
        }
        // runner 在超时内完成但返回错误
        Ok(Err(error)) => {
            warn!(task_id = %task_id, error = %error, "子任务执行失败");
            let event = NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
                from: agent_id.clone(),
                to: parent_id,
                task_id: task_id.clone(),
                error: error.clone(),
                retry_count: 0,
            };
            // §6.2 红线:Critical 级事件走 mpsc 双通道(publish_critical)
            let _ = bus.publish_critical(event).await;
            TaskResult {
                task_id,
                success: false,
                summary: error,
                tokens_used: 0,
                duration,
                agent_id,
            }
        }
        // runner 超时未完成
        Err(_elapsed) => {
            warn!(task_id = %task_id, timeout = ?timeout, "子任务执行超时");
            let error_msg = format!("任务超时(限时 {timeout:?})");
            let event = NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
                from: agent_id.clone(),
                to: parent_id,
                task_id: task_id.clone(),
                error: error_msg.clone(),
                retry_count: 0,
            };
            // 超时也发布 AgentTaskFailed(Critical 级,走 mpsc 双通道)
            let _ = bus.publish_critical(event).await;
            TaskResult {
                task_id,
                success: false,
                summary: error_msg,
                tokens_used: 0,
                duration,
                agent_id,
            }
        }
    }
}
