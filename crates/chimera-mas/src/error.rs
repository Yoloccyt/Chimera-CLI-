//! MAS 子系统错误类型
//!
//! 遵循 §4.1 规范:库层用自定义 `thiserror` enum,不使用 `anyhow`。
//! 这些错误覆盖 MAS 子系统在 Agent 生命周期、委托执行、上下文隔离、
//! 专家咨询等过程中的失败场景。
//!
//! ## 错误分类
//!
//! - **上下文隔离违规**: ContextIsolationViolation / TokenBudgetExceeded / ContextCompressionFailed
//! - **委托深度与执行**: MaxDepthExceeded / DelegationFailed / NoAvailableSubAgent / TaskTimeout / TaskFailed
//! - **象限约束**: QuadrantFanoutExceeded(INV-3) / QuadrantConflict(INV-4)
//! - **归档单调性**: ArchiveMonotonicityViolated(INV-8,Task 21 §21.2) / ArchiveTierInvalid(Task 17 §17.2)
//! - **Agent 生命周期**: AgentNotFound / InvalidAgentState / AgentAlreadyExists / AgentCreationFailed /
//!   AgentStartupFailed / AgentShutdownFailed
//! - **任务状态**: TaskNotFound / TaskAlreadyCompleted
//! - **专家咨询**: ConsultationFailed / ExpertUnavailable(扩展 reason 字段,Task 18 §18.3)/ KnowledgeRetrievalFailed(Task 18 §18.10)
//! - **消息传递**: MessageSendFailed / MessageTimeout
//! - **配置与序列化**: InvalidConfig / SerializationFailed / MessagePackFailed / MessagePackDecodeFailed
//! - **稳定性(Task 19 §19.7)**: CircuitBreakerOpen
//! - **分块调度(Task 16 §16.10)**: ChunkingFailed
//! - **系统**: IoError / Internal

use thiserror::Error;

/// MAS 子系统错误类型
///
/// 共 33 个变体,覆盖 MAS 特有错误场景。
/// 所有变体均通过 `#[error("...")]` 提供人类可读的 Display 实现。
#[derive(Debug, Error)]
pub enum MasError {
    // === 上下文隔离相关(3 个)===
    /// 上下文隔离违规 — Agent 试图访问其他 Agent 的上下文
    ///
    /// 触发场景:`ContextIsolationGuard::verify_access()` 检测到跨 Agent 访问。
    /// 处理策略:立即终止违规 Agent,记录 Critical 告警。
    #[error(
        "Context isolation violation: agent {agent_id} attempted to access context {context_id}"
    )]
    ContextIsolationViolation {
        /// 违规 Agent ID
        agent_id: String,
        /// 被访问的上下文 ID
        context_id: String,
    },

    /// Token 预算超限 — Agent 上下文超过 max_tokens
    ///
    /// 触发场景:`TokenBudget::check()` 检测到 current_tokens > max_tokens。
    /// 处理策略:发布 `NexusEvent::AgentContextOverflow`(Critical,走 mpsc),
    /// 触发上下文压缩或归档。
    #[error("Token budget exceeded: agent {agent_id} used {current_tokens}/{max_tokens} tokens")]
    TokenBudgetExceeded {
        /// Agent ID
        agent_id: String,
        /// 当前已用 Token 数
        current_tokens: usize,
        /// 最大 Token 预算
        max_tokens: usize,
    },

    /// 上下文压缩失败 — HCW 稀疏化或归档过程失败
    ///
    /// 触发场景:`hcw_window::HierarchicalWindow::select()` 返回错误,
    /// 或 `osa_coordinator::OmniSparseCoordinator::compute_masks()` 失败。
    #[error("Context compression failed for agent {agent_id}: {reason}")]
    ContextCompressionFailed {
        /// Agent ID
        agent_id: String,
        /// 失败原因
        reason: String,
    },

    // === 委托深度与执行相关(5 个)===
    /// 委托深度超限 — 递归委托深度超过 max_agent_depth(默认 5)
    ///
    /// 触发场景:`RootOrchestrator::delegate()` 检测到 depth > max_agent_depth。
    /// 处理策略:拒绝委托,返回错误,防止递归爆炸。
    #[error("Max delegation depth exceeded: current {current_depth}, max {max_depth}")]
    MaxDepthExceeded {
        /// 当前委托深度
        current_depth: usize,
        /// 最大允许深度
        max_depth: usize,
    },

    /// 委托执行失败 — DelegationExecutor 执行过程中出错
    ///
    /// 触发场景:`DelegationExecutor::execute_delegation()` 内部子任务执行失败,
    /// 且不满足其他具体错误变体(如 TaskTimeout / TaskFailed)。
    #[error("Delegation execution failed: {reason}")]
    DelegationFailed {
        /// 失败原因
        reason: String,
    },

    /// 无可用子 Agent — 任务无法委托
    ///
    /// 触发场景:`RootOrchestrator::delegate()` 找不到匹配任务复杂度的可用子 Agent。
    #[error("No available sub-agent for task: {task_id}")]
    NoAvailableSubAgent {
        /// 任务 ID
        task_id: String,
    },

    /// 任务超时 — 任务执行超过 deadline
    ///
    /// 触发场景:`gqep_executor` 包装的超时检测到任务超过 deadline。
    /// 处理策略:发布 `NexusEvent::AgentTaskFailed`(Critical,走 mpsc)。
    #[error("Task timeout: task {task_id} exceeded deadline {deadline:?}")]
    TaskTimeout {
        /// 任务 ID
        task_id: String,
        /// 截止时间(UTC)
        deadline: chrono::DateTime<chrono::Utc>,
    },

    /// 任务失败 — 重试次数耗尽
    ///
    /// 触发场景:子任务重试达到上限仍失败。
    /// 处理策略:发布 `NexusEvent::AgentTaskFailed`(Critical,走 mpsc)。
    #[error("Task failed after {retry_count} retries: {error}")]
    TaskFailed {
        /// 失败原因
        error: String,
        /// 已重试次数
        retry_count: u32,
    },

    // === 象限约束相关(2 个,ADR-027 决策 1)===
    /// 象限扇出超限 — 孙代理扇出超过四象限上界(INV-3)
    ///
    /// 触发场景:`QuadrantPlan::from_quadrants()` 检测到象限数 > `MAX_QUADRANT_FANOUT`(4)。
    /// 处理策略:拒绝构造分工计划,防止无界委托爆炸(回应推理悖论)。
    #[error("Quadrant fanout exceeded: requested {requested}, max {max}")]
    QuadrantFanoutExceeded {
        /// 请求的象限数
        requested: usize,
        /// 允许的最大象限数(恒为 4)
        max: usize,
    },

    /// 象限冲突 — 同一子代理下象限重复(INV-4)
    ///
    /// 触发场景:`QuadrantPlan::from_quadrants()` 检测到重复象限。
    /// 处理策略:拒绝构造分工计划,保证每个象限至多一个活跃孙代理。
    #[error("Quadrant conflict: quadrant {quadrant} already active")]
    QuadrantConflict {
        /// 冲突的象限名称(如 "Implementation")
        quadrant: String,
    },

    /// 归档单调性违反 — INV-8(Task 21 §21.2 / §17.5)
    ///
    /// 触发场景:`InvariantChecker::check_inv8_archive_monotonicity()` 检测到
    /// 记忆试图沿非 Hot→Warm→Cold→Ice 方向移动(如反向 Ice→Hot 或同层 Hot→Hot)。
    ///
    /// 处理策略:拒绝归档操作,返回带 `from_tier`/`to_tier` 的诊断信息。
    /// 调用方应发布 Critical 事件(§6.2 红线)并保留原 tier 不变。
    ///
    /// WHY 复用 String 而非 ArchiveTier:错误类型应自描述,避免要求调用方
    /// 导入 `ArchiveTier` 类型即可理解错误上下文。`from_tier`/`to_tier`
    /// 存储 `"Hot"`/`"Warm"`/`"Cold"`/`"Ice"` 字符串。
    #[error("Archive monotonicity violated: cannot demote from {from_tier:?} to {to_tier:?}")]
    ArchiveMonotonicityViolated {
        /// 源归档级(如 "Hot"/"Warm"/"Cold"/"Ice")
        from_tier: String,
        /// 目标归档级(如 "Warm"/"Cold"/"Ice"/"Hot")
        to_tier: String,
    },

    /// 归档层级无效 — Task 17 §17.2 SubTask 17.11
    ///
    /// 触发场景:调用方传入未定义的归档层级字符串(如 `"Unknown"`)。
    /// 处理策略:拒绝操作并返回带 `tier` 字段的诊断信息,调用方应回滚或使用默认层级。
    ///
    /// WHY 复用 String 而非 ArchiveTier:错误类型应自描述,避免要求调用方
    /// 导入 `ArchiveTier` 类型即可理解错误上下文。`tier` 存储原始字符串。
    #[error("Archive tier invalid: {tier:?} is not a recognized archive tier")]
    ArchiveTierInvalid {
        /// 未识别的归档层级字符串
        tier: String,
    },

    /// 派生准入闸拒绝 — Task 15 §15.3 派生准入闸
    ///
    /// 触发场景:`AdmissionGate::check()` 检测到派生新 Agent 后全局内存预算
    /// 超出 `MEMORY_BUDGET_MB × MEMORY_BUDGET_UTILIZATION`(130MB × 0.9 = 117MB)。
    ///
    /// 处理策略:拒绝派生,调用方应发布 `NexusEvent::AgentContextOverflow`
    /// Critical 事件(走 mpsc,§6.2 红线),并降级 tier 或等待内存释放。
    ///
    /// WHY 独立变体而非复用 TokenBudgetExceeded:INV-7 失败时复用 TokenBudgetExceeded,
    /// 但 AdmissionGate 作为派生准入语义层,失败原因可能是单 Agent 驻留超限或全局
    /// 预算超限两种,需要明确区分。`reason` 字段保留 INV-7 原始错误信息用于诊断。
    #[error(
        "Admission gate denied: m_total={m_total}MB exceeds m_budget={m_budget}MB, new_agent_tier={new_agent_tier:?}, reason: {reason}"
    )]
    AdmissionGateDenied {
        /// 当前全 Agent 池聚合内存(MB)
        m_total: usize,
        /// 全局内存预算上限(MB,通常为 130)
        m_budget: usize,
        /// 新 Agent 的 tier 名称(如 "L0"/"L1"/"L2"/"L3")
        new_agent_tier: String,
        /// INV-7 失败原因(诊断用)
        reason: String,
    },

    // === Agent 生命周期相关(6 个)===
    /// Agent 未找到 — Agent ID 不存在
    ///
    /// 触发场景:查询 / 操作不存在的 Agent。
    #[error("Agent not found: {agent_id}")]
    AgentNotFound {
        /// Agent ID
        agent_id: String,
    },

    /// Agent 状态无效 — 当前状态不允许该操作
    ///
    /// 触发场景:对已终止 Agent 执行 start,或对 Running Agent 执行 start。
    #[error("Invalid agent state: agent {agent_id} is in state {current_state}, expected {expected_state}")]
    InvalidAgentState {
        /// Agent ID
        agent_id: String,
        /// 当前状态
        current_state: String,
        /// 期望状态
        expected_state: String,
    },

    /// Agent 已存在 — 创建重复 Agent
    ///
    /// 触发场景:`AgentFactory::create_agent()` 使用已存在的 agent_id。
    #[error("Agent already exists: {agent_id}")]
    AgentAlreadyExists {
        /// Agent ID
        agent_id: String,
    },

    /// Agent 工厂创建失败 — 创建 Agent 过程出错
    ///
    /// 触发场景:`AgentFactory::create_agent()` 内部初始化失败
    /// (如 ModelConfig 校验失败、上下文分配失败)。
    #[error("Agent factory failed to create agent: {reason}")]
    AgentCreationFailed {
        /// 失败原因
        reason: String,
    },

    /// Agent 启动失败 — 启动 Agent 过程出错
    ///
    /// 触发场景:`AgentLifecycle::start()` 启动 Agent 任务失败
    /// (如 EventBus 订阅失败、初始上下文加载失败)。
    #[error("Agent startup failed for agent {agent_id}: {reason}")]
    AgentStartupFailed {
        /// Agent ID
        agent_id: String,
        /// 失败原因
        reason: String,
    },

    /// Agent 终止失败 — 停止 Agent 过程出错
    ///
    /// 触发场景:`AgentLifecycle::shutdown()` 终止 Agent 任务失败
    /// (如资源回收失败、状态持久化失败)。
    #[error("Agent shutdown failed for agent {agent_id}: {reason}")]
    AgentShutdownFailed {
        /// Agent ID
        agent_id: String,
        /// 失败原因
        reason: String,
    },

    // === 任务状态相关(2 个)===
    /// 任务未找到 — Task ID 不存在
    ///
    /// 触发场景:查询 / 操作不存在的任务。
    #[error("Task not found: {task_id}")]
    TaskNotFound {
        /// 任务 ID
        task_id: String,
    },

    /// 任务已完成 — 不能对已完成任务执行操作
    ///
    /// 触发场景:对 Completed / Failed 状态的任务执行 start / delegate / cancel。
    #[error("Task already completed: {task_id}")]
    TaskAlreadyCompleted {
        /// 任务 ID
        task_id: String,
    },

    // === 专家咨询相关(3 个)===
    /// 专家咨询失败 — 专家 Agent 咨询过程出错
    ///
    /// 触发场景:专家 Agent 处理 `AgentConsultRequested` 事件时出错。
    #[error("Expert consultation failed: {reason}")]
    ConsultationFailed {
        /// 失败原因
        reason: String,
    },

    /// 专家不可用 — 指定专家 Agent 不存在、未注册或咨询超时(Task 18 §18.3)
    ///
    /// 触发场景:
    /// - 咨询请求的目标 expert_id 不存在或已下线
    /// - 咨询超过 SLA 时限(Critical < 5s / High < 15s / Medium < 30s)
    ///
    /// 处理策略:发布 `NexusEvent::AgentTaskFailed`(Critical,走 mpsc,§6.2 红线),
    /// 由 Parliament 进行补救决策(重试 / 降级 / 转交其他专家)。
    ///
    /// WHY 扩展 reason 字段(Task 18):原变体仅含 expert_id,无法区分"未注册"与
    /// "咨询超时"两种失败模式。扩展 reason 字段携带诊断信息("unregistered" /
    /// "timeout after Xs" / "overloaded"),便于 Parliament 决策与告警图表分类。
    #[error("Expert unavailable: expert_id={expert_id}, reason={reason}")]
    ExpertUnavailable {
        /// 专家 Agent ID
        expert_id: String,
        /// 不可用原因(超时 / 未注册 / 过载)
        reason: String,
    },

    /// 知识检索失败 — Task 18 §18.10
    ///
    /// 触发场景:`WikiRetriever::search()` 调用 `wiki.search_fulltext()` 失败,
    /// 或 `MutualInquirer::inquire()` 同僚互询全部失败且无可用兜底。
    ///
    /// 处理策略:调用方应降级到本地 mlc L0/L1 检索(三级检索链短路),
    /// 并发布 Normal 级事件记录检索失败原因,供 PDCA 度量统计。
    ///
    /// WHY 独立变体而非复用 ConsultationFailed:知识检索包含 Wiki 全文检索 +
    /// 内存 KNN + 同僚互询三种语义,与专家咨询语义不同,需独立分类便于
    /// PDCA §20.8 度量"咨询超时率"与"知识检索失败率"两个独立指标。
    #[error("Knowledge retrieval failed: {reason}")]
    KnowledgeRetrievalFailed {
        /// 失败原因
        reason: String,
    },

    // === 消息传递相关(2 个)===
    /// 消息发送失败 — Agent 间消息传递失败
    ///
    /// 触发场景:`EventBus::publish()` 发布 Agent 事件失败,
    /// 或点对点 `recv_matching` 通道异常。
    #[error("Message send failed from {from} to {to}: {reason}")]
    MessageSendFailed {
        /// 发送方 Agent ID
        from: String,
        /// 接收方 Agent ID
        to: String,
        /// 失败原因
        reason: String,
    },

    /// 消息超时 — 等待响应消息超时
    ///
    /// 触发场景:`AgentConsultRequested` 等待 `AgentConsultResponded` 超时,
    /// 或 `AgentTaskDelegated` 等待 `AgentTaskCompleted` 超时。
    #[error("Message timeout: {reason}")]
    MessageTimeout {
        /// 失败原因
        reason: String,
    },

    // === 配置与序列化相关(5 个)===
    /// 配置无效 — 配置参数非法
    ///
    /// 触发场景:ModelConfig 字段校验失败(如 temperature 越界、max_tokens 为 0)。
    #[error("Invalid config: {field} = {value}")]
    InvalidConfig {
        /// 非法字段名
        field: String,
        /// 非法值
        value: String,
    },

    /// JSON 序列化失败
    ///
    /// 触发场景:Agent 元数据或任务状态 JSON 序列化 / 反序列化失败。
    #[error("Serialization failed: {0}")]
    SerializationFailed(#[from] serde_json::Error),

    /// MessagePack 序列化失败
    ///
    /// 触发场景:Agent 状态持久化时 MessagePack 编码失败(ADR-004)。
    #[error("MessagePack serialization failed: {0}")]
    MessagePackFailed(#[from] rmp_serde::encode::Error),

    /// MessagePack 反序列化失败
    ///
    /// 触发场景:Agent 状态恢复时 MessagePack 解码失败(ADR-004)。
    #[error("MessagePack deserialization failed: {0}")]
    MessagePackDecodeFailed(#[from] rmp_serde::decode::Error),

    // === 稳定性相关(1 个,Task 19 §19.7)===
    /// 熔断器已打开 — CircuitBreaker 处于 Open 态,拒绝新请求通过
    ///
    /// 触发场景:`CircuitBreaker::record_failure()` 累计失败次数达到 `threshold`
    /// 后熔断器从 Closed → Open,后续调用方检查到 Open 态时返回本错误,
    /// 拒绝继续派发任务直到 `reset_timeout_ms` 后切换到 HalfOpen 试探。
    ///
    /// 处理策略:调用方应触发 `DegradationChain::apply(PressureSource::*)`
    /// 降级链(如 HCW 压缩 / tier 降级 / 拒新 Agent 排队),并发布
    /// `NexusEvent::AgentContextOverflow` Critical 事件(走 mpsc,§6.2 红线)。
    ///
    /// WHY 独立变体而非复用 DelegationFailed:CircuitBreaker 是稳定性子系统
    /// 的状态机错误,与单次委托执行失败语义不同,需明确区分以便降级链精准响应。
    /// `failure_count` / `threshold` 字段保留诊断信息供告警与图表使用。
    #[error("Circuit breaker open: failures={failure_count}, threshold={threshold}")]
    CircuitBreakerOpen {
        /// 当前累计失败次数
        failure_count: u32,
        /// 触发熔断的失败次数阈值
        threshold: u32,
    },

    // === 分块调度相关(1 个,Task 16 §16.10)===
    /// 任务切块失败 — Task 16 §16.10
    ///
    /// 触发场景:`TaskChunker::chunk()` 检测到无法安全切块的条件:
    /// - `delegation_depth >= MAX_AGENT_DEPTH`(5):继续切块会突破委托深度上限,
    ///   违反 §6.2 红线(递归爆炸防护)
    /// - 后续可能扩展:零字节任务、负数 token 等边界场景
    ///
    /// 处理策略:调用方应停止切块,改用降级策略(如直接执行原任务或返回错误)。
    /// `reason` 字段携带诊断信息便于告警与图表分类。
    ///
    /// WHY 独立变体而非复用 DelegationFailed:切块是分块调度子系统的语义,
    /// 与单次委托执行失败不同,需明确区分以便 §16 调度链精准响应。
    #[error("Chunking failed: {reason}")]
    ChunkingFailed {
        /// 失败原因(如 "delegation_depth 5 >= MAX_AGENT_DEPTH 5")
        reason: String,
    },

    // === 系统级(2 个)===
    /// IO 错误 — 文件读写、网络等底层 IO 错误
    ///
    /// 触发场景:Agent 状态持久化到磁盘、检查点文件读写。
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// 内部错误 — 兜底变体,用于未分类的内部错误
    ///
    /// 触发场景:不应发生的内部不变量违反、状态机不一致等。
    /// WHY 保留兜底:避免遗漏错误场景导致 panic,符合 §4.1 "避免 unwrap/expect" 规范。
    #[error("Internal error: {0}")]
    Internal(String),
}

/// MAS 子系统 Result 类型
///
/// 所有 MAS 公共 API 的返回类型,统一使用 `Result<T>` 简写。
pub type Result<T> = std::result::Result<T, MasError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_isolation_violation_display() {
        let e = MasError::ContextIsolationViolation {
            agent_id: "agent-1".into(),
            context_id: "ctx-2".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("agent-1"));
        assert!(msg.contains("ctx-2"));
    }

    #[test]
    fn test_max_depth_exceeded_display() {
        let e = MasError::MaxDepthExceeded {
            current_depth: 6,
            max_depth: 5,
        };
        let msg = format!("{e}");
        assert!(msg.contains("6"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_agent_not_found_display() {
        let e = MasError::AgentNotFound {
            agent_id: "missing-agent".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("missing-agent"));
    }

    #[test]
    fn test_token_budget_exceeded_display() {
        let e = MasError::TokenBudgetExceeded {
            agent_id: "agent-1".into(),
            current_tokens: 2000000,
            max_tokens: 1000000,
        };
        let msg = format!("{e}");
        assert!(msg.contains("2000000"));
        assert!(msg.contains("1000000"));
    }

    #[test]
    fn test_serde_json_error_from_conversion() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let mas_err: MasError = json_err.into();
        assert!(matches!(mas_err, MasError::SerializationFailed(_)));
    }

    #[test]
    fn test_io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let mas_err: MasError = io_err.into();
        assert!(matches!(mas_err, MasError::IoError(_)));
    }

    /// 验证 MasError 变体数量 >= 25(任务清单要求 25+ 变体)
    ///
    /// WHY 静态断言:错误变体数量是 Task 6 验收标准,变体增减需 ADR 评审。
    /// 通过遍历所有变体确保数量充足。
    #[test]
    fn test_error_variant_count_at_least_25() {
        // 引入 serde ser/de Error trait,用于构造 rmp_serde 错误实例
        use serde::de::Error as _;
        use serde::ser::Error as _;

        // 列举所有变体,确保数量 >= 33(当前实际 33 个变体,Task 16 新增 ChunkingFailed)
        let variants: Vec<MasError> = vec![
            MasError::ContextIsolationViolation {
                agent_id: "a".into(),
                context_id: "c".into(),
            },
            MasError::TokenBudgetExceeded {
                agent_id: "a".into(),
                current_tokens: 0,
                max_tokens: 0,
            },
            MasError::ContextCompressionFailed {
                agent_id: "a".into(),
                reason: "r".into(),
            },
            MasError::MaxDepthExceeded {
                current_depth: 0,
                max_depth: 0,
            },
            MasError::DelegationFailed { reason: "r".into() },
            MasError::NoAvailableSubAgent {
                task_id: "t".into(),
            },
            MasError::TaskTimeout {
                task_id: "t".into(),
                deadline: chrono::Utc::now(),
            },
            MasError::TaskFailed {
                error: "e".into(),
                retry_count: 0,
            },
            MasError::QuadrantFanoutExceeded {
                requested: 5,
                max: 4,
            },
            MasError::QuadrantConflict {
                quadrant: "Implementation".into(),
            },
            MasError::ArchiveMonotonicityViolated {
                from_tier: "Ice".into(),
                to_tier: "Hot".into(),
            },
            MasError::ArchiveTierInvalid {
                tier: "Unknown".into(),
            },
            MasError::AdmissionGateDenied {
                m_total: 120,
                m_budget: 130,
                new_agent_tier: "L3".into(),
                reason: "r".into(),
            },
            MasError::AgentNotFound {
                agent_id: "a".into(),
            },
            MasError::InvalidAgentState {
                agent_id: "a".into(),
                current_state: "s".into(),
                expected_state: "s".into(),
            },
            MasError::AgentAlreadyExists {
                agent_id: "a".into(),
            },
            MasError::AgentCreationFailed { reason: "r".into() },
            MasError::AgentStartupFailed {
                agent_id: "a".into(),
                reason: "r".into(),
            },
            MasError::AgentShutdownFailed {
                agent_id: "a".into(),
                reason: "r".into(),
            },
            MasError::TaskNotFound {
                task_id: "t".into(),
            },
            MasError::TaskAlreadyCompleted {
                task_id: "t".into(),
            },
            MasError::ConsultationFailed { reason: "r".into() },
            MasError::ExpertUnavailable {
                expert_id: "e".into(),
                reason: "timeout".into(),
            },
            MasError::KnowledgeRetrievalFailed { reason: "r".into() },
            MasError::MessageSendFailed {
                from: "a".into(),
                to: "b".into(),
                reason: "r".into(),
            },
            MasError::MessageTimeout { reason: "r".into() },
            MasError::InvalidConfig {
                field: "f".into(),
                value: "v".into(),
            },
            MasError::SerializationFailed(
                serde_json::from_str::<serde_json::Value>("x").unwrap_err(),
            ),
            MasError::MessagePackFailed(rmp_serde::encode::Error::custom("test encode")),
            MasError::MessagePackDecodeFailed(rmp_serde::decode::Error::custom("test decode")),
            MasError::CircuitBreakerOpen {
                failure_count: 5,
                threshold: 5,
            },
            MasError::ChunkingFailed {
                reason: "delegation_depth 5 >= MAX_AGENT_DEPTH 5".into(),
            },
            MasError::IoError(std::io::Error::other("io")),
            MasError::Internal("internal".into()),
        ];
        assert!(
            variants.len() >= 33,
            "MasError 变体数量 = {},应 >= 33",
            variants.len()
        );
    }
}
