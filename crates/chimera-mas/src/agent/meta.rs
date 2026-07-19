//! Agent 元数据与类型定义 — 占位实现(Task 7 将完善)
//!
//! 本文件定义 MAS 子系统的核心 Agent 类型体系:
//! - `AgentType`: Agent 层级类型(RootOrchestrator / MainAgent / SubAgent / GrandAgent / ExpertAgent)
//! - `AgentStatus`: Agent 生命周期状态(Idle / Running / Paused / Completed / Failed / Crashed)
//! - `ModelConfig`: Agent 使用的模型配置(provider / model / temperature / max_tokens / thinking_mode)
//! - `AgentMeta`: Agent 元数据(agent_id / agent_type / model_config / parent_id / children_ids / ...)
//!
//! ## ADR-026 决策 6: 复用 nexus_core::ThinkingMode
//!
//! `ModelConfig.thinking_mode` 使用 `nexus_core::ThinkingMode`(Fast/Standard/Deep),
//! 不新建 `ThinkingMode::Max`。TaskComplexity 通过 `From` impl 映射到 ThinkingMode
//! (Task 7.4 实现)。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Agent 层级类型 — ADR-026 决策 1: L9 Quest 层级递归委托
///
/// 最大深度 5(RootOrchestrator=0 → MainAgent=1 → SubAgent=2 → GrandAgent=3 → ...),
/// 超过将触发 `MasError::MaxDepthExceeded`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentType {
    /// 根协调器(depth=0)— 全局任务编排入口
    RootOrchestrator,
    /// 主 Agent(depth=1)— 域级任务执行
    MainAgent {
        /// 所属领域(如 "frontend" / "backend" / "database")
        domain: String,
    },
    /// 子 Agent(depth=2)— 主 Agent 委托的子任务
    SubAgent {
        /// 父 Agent ID
        parent_id: String,
        /// 任务范围(如 "implement-api" / "write-tests")
        task_scope: String,
    },
    /// 孙 Agent(depth=3)— SubAgent 进一步委托的孙任务
    GrandAgent {
        /// 父 Agent ID
        parent_id: String,
        /// 任务范围
        task_scope: String,
    },
    /// 专家 Agent — 专家咨询(不参与层级委托,提供领域知识)
    ExpertAgent {
        /// 专长领域列表(如 ["security", "cryptography"])
        specialty: Vec<String>,
    },
}

/// Agent 生命周期状态 — 状态机:Idle → Running → Paused → Completed/Failed/Crashed
///
/// 状态转换规则(Task 8 将实现):
/// - Idle → Running(start)
/// - Running → Paused(pause)
/// - Paused → Running(resume)
/// - Running → Completed(成功)
/// - Running → Failed(失败)
/// - 任意 → Crashed(panic / 不可恢复错误)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    /// 空闲:已创建但未启动
    Idle,
    /// 运行中:正在执行任务
    Running,
    /// 已暂停:被外部暂停,可恢复
    Paused,
    /// 已完成:任务成功结束
    Completed,
    /// 已失败:任务执行失败
    Failed,
    /// 已崩溃:不可恢复错误(panic / 资源耗尽)
    Crashed,
}

/// Agent 模型配置 — 定义 Agent 使用的 LLM 模型与生成参数
///
/// ADR-026 决策 6: `thinking_mode` 复用 `nexus_core::ThinkingMode`,
/// 不新建 `ThinkingMode::Max`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// 模型提供方(如 "openai" / "anthropic" / "local")
    pub provider: String,
    /// 模型名称(如 "gpt-4" / "claude-3-opus")
    pub model: String,
    /// 采样温度(0.0-2.0,0=确定性,2=高随机性)
    pub temperature: f32,
    /// 最大 Token 数(单次生成上限)
    pub max_tokens: usize,
    /// 思考模式(TTG 三级切换),复用 nexus_core::ThinkingMode
    pub thinking_mode: nexus_core::ThinkingMode,
}

/// Agent 元数据 — 描述 Agent 实例的全部静态与运行时状态
///
/// 该结构是 MAS 子系统的核心数据载体,贯穿 Agent 创建、委托、监控、终止全生命周期。
/// `depth` 字段用于 ADR-026 决策 1 的最大深度限制(默认 5)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMeta {
    /// Agent 唯一标识(UUIDv7,时间有序,便于因果追踪)
    pub agent_id: String,
    /// Agent 层级类型
    pub agent_type: AgentType,
    /// 人类可读名称(用于日志与 TUI 展示)
    pub name: String,
    /// 人类可读描述(Agent 职责说明)
    pub description: String,
    /// 模型配置(provider / model / temperature / max_tokens / thinking_mode)
    pub model_config: ModelConfig,
    /// 上下文窗口大小(Token 数,1M 等效 = 128K 实际 + 8× 稀疏压缩)
    pub context_window: usize,
    /// 父 Agent ID(RootOrchestrator 为 None)
    pub parent_id: Option<String>,
    /// 子 Agent ID 列表(层级委托链)
    pub children_ids: Vec<String>,
    /// 创建时间(UTC,自动生成)
    pub created_at: DateTime<Utc>,
    /// 当前状态(运行时动态变化)
    pub status: AgentStatus,
    /// 委托深度(RootOrchestrator=0,MainAgent=1,SubAgent=2,GrandAgent=3,...)
    pub depth: usize,
}

// ============================================================
// Task 7.2: 方法实现(GREEN 阶段)
// ============================================================

impl AgentType {
    /// 返回该 Agent 层级类型对应的委托深度
    ///
    /// ## ADR-026 决策 1: L9 Quest 层级递归委托
    ///
    /// 深度对应关系:
    /// - `RootOrchestrator`: 0(全局编排入口)
    /// - `MainAgent`: 1(域级任务执行)
    /// - `SubAgent`: 2(主 Agent 委托的子任务)
    /// - `GrandAgent`: 3(SubAgent 进一步委托的孙任务)
    /// - `ExpertAgent`: 0(专家咨询,不参与层级委托)
    ///
    /// 超过 `MAX_AGENT_DEPTH`(默认 5)将触发 `MasError::MaxDepthExceeded`。
    pub fn depth(&self) -> usize {
        match self {
            Self::RootOrchestrator => 0,
            Self::MainAgent { .. } => 1,
            Self::SubAgent { .. } => 2,
            Self::GrandAgent { .. } => 3,
            // ExpertAgent 不参与层级委托,作为咨询角色独立存在
            Self::ExpertAgent { .. } => 0,
        }
    }
}

impl Default for ModelConfig {
    /// 默认模型配置 — 提供合理的生产可用默认值
    ///
    /// WHY 这些默认值:
    /// - `provider = "openai"`: OpenAI 是最广泛兼容的 API 协议(很多本地模型也兼容)
    /// - `model = "gpt-4"`: GPT-4 作为通用默认模型,平衡能力与可用性
    /// - `temperature = 0.7`: 0.7 是 OpenAI 推荐的通用温度(创造性 + 可控性平衡)
    /// - `max_tokens = 8192`: 8K 输出上限覆盖大多数代码生成场景
    /// - `thinking_mode = Standard`: 平衡速度与深度,适合常规任务
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            temperature: 0.7,
            max_tokens: 8192,
            thinking_mode: nexus_core::ThinkingMode::Standard,
        }
    }
}

impl AgentMeta {
    /// 创建 RootOrchestrator 元数据(depth=0)
    ///
    /// RootOrchestrator 是 MAS 子系统的全局任务编排入口,无父 Agent。
    /// 默认上下文窗口为 1M Token 等效(128K 实际 + 8x 稀疏压缩,ADR-026 决策 7)。
    ///
    /// ## 参数
    /// - `agent_id`: Agent 唯一标识(UUIDv7,时间有序)
    pub fn new_root_orchestrator(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_type: AgentType::RootOrchestrator,
            name: "RootOrchestrator".to_string(),
            description: "全局任务编排入口".to_string(),
            model_config: ModelConfig::default(),
            // 1M Token 等效 = 128K 实际 + 8x 稀疏压缩(ADR-026 决策 7)
            context_window: 1_048_576,
            parent_id: None,
            children_ids: Vec::new(),
            created_at: chrono::Utc::now(),
            status: AgentStatus::Idle,
            depth: 0,
        }
    }

    /// 创建 MainAgent 元数据(depth=1)
    ///
    /// MainAgent 是域级任务执行 Agent,由 RootOrchestrator 委托创建。
    ///
    /// ## 参数
    /// - `agent_id`: Agent 唯一标识
    /// - `parent_id`: 父 Agent ID(通常为 RootOrchestrator 的 ID)
    /// - `domain`: 所属领域(如 "frontend" / "backend" / "database")
    /// - `model_config`: 模型配置
    pub fn new_main_agent(
        agent_id: impl Into<String>,
        parent_id: impl Into<String>,
        domain: impl Into<String>,
        model_config: ModelConfig,
    ) -> Self {
        let parent_id_str = parent_id.into();
        Self {
            agent_id: agent_id.into(),
            agent_type: AgentType::MainAgent {
                domain: domain.into(),
            },
            name: "MainAgent".to_string(),
            description: "域级任务执行 Agent".to_string(),
            model_config,
            context_window: 1_048_576,
            parent_id: Some(parent_id_str),
            children_ids: Vec::new(),
            created_at: chrono::Utc::now(),
            status: AgentStatus::Idle,
            depth: 1,
        }
    }

    /// 创建 SubAgent 元数据(depth=2)
    ///
    /// SubAgent 是 MainAgent 委托的子任务执行 Agent。
    ///
    /// ## 参数
    /// - `agent_id`: Agent 唯一标识
    /// - `parent_id`: 父 Agent ID(MainAgent 的 ID)
    /// - `task_scope`: 任务范围(如 "implement-api" / "write-tests")
    /// - `model_config`: 模型配置
    pub fn new_sub_agent(
        agent_id: impl Into<String>,
        parent_id: impl Into<String>,
        task_scope: impl Into<String>,
        model_config: ModelConfig,
    ) -> Self {
        let parent_id_str = parent_id.into();
        let scope = task_scope.into();
        Self {
            agent_id: agent_id.into(),
            agent_type: AgentType::SubAgent {
                parent_id: parent_id_str.clone(),
                task_scope: scope,
            },
            name: "SubAgent".to_string(),
            description: "子任务执行 Agent".to_string(),
            model_config,
            context_window: 1_048_576,
            parent_id: Some(parent_id_str),
            children_ids: Vec::new(),
            created_at: chrono::Utc::now(),
            status: AgentStatus::Idle,
            depth: 2,
        }
    }
}
