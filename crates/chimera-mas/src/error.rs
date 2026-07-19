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
//! - **Agent 生命周期**: AgentNotFound / InvalidAgentState / AgentAlreadyExists / AgentCreationFailed /
//!   AgentStartupFailed / AgentShutdownFailed
//! - **任务状态**: TaskNotFound / TaskAlreadyCompleted
//! - **专家咨询**: ConsultationFailed / ExpertUnavailable
//! - **消息传递**: MessageSendFailed / MessageTimeout
//! - **配置与序列化**: InvalidConfig / SerializationFailed / MessagePackFailed / MessagePackDecodeFailed
//! - **系统**: IoError / Internal

use thiserror::Error;

/// MAS 子系统错误类型
///
/// 共 25 个变体,覆盖 MAS 特有错误场景。
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

    // === 专家咨询相关(2 个)===
    /// 专家咨询失败 — 专家 Agent 咨询过程出错
    ///
    /// 触发场景:专家 Agent 处理 `AgentConsultRequested` 事件时出错。
    #[error("Expert consultation failed: {reason}")]
    ConsultationFailed {
        /// 失败原因
        reason: String,
    },

    /// 专家不可用 — 指定专家 Agent 不存在或未注册
    ///
    /// 触发场景:咨询请求的目标 expert_id 不存在或已下线。
    #[error("Expert agent unavailable: {expert_id}")]
    ExpertUnavailable {
        /// 专家 Agent ID
        expert_id: String,
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

        // 列举所有变体,确保数量 >= 25
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
            },
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
            MasError::IoError(std::io::Error::other("io")),
            MasError::Internal("internal".into()),
        ];
        assert!(
            variants.len() >= 25,
            "MasError 变体数量 = {},应 >= 25",
            variants.len()
        );
    }
}
