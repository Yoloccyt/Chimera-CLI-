//! CHIMERA Multi-Agent Synergy (MAS) 子系统
//!
//! 架构层归属: L9 Quest(与 quest-engine / gea-activator / efficiency-monitor 同层)
//! 核心职责: 层级化递归委托编排、独立上下文隔离、Agent 生命周期管理
//!
//! ## 设计来源
//!
//! 基于 `CHIMERA_MULTI_AGENT_协同工作系统_终极设计文档.md`(v5.0.0-omega)设计,
//! 经 3 位 10+ 年经验专家(chimera-release-analyst + architecture-optimization-analyst
//! + rust-architecture-expert)分布式深度分析,识别 17 项 P0 阻断级差异后精简实现。
//!
//! ## 核心能力(ADR-026 决策 5:精简 3 子模块)
//!
//! - **层级递归委托**: RootOrchestrator → MainAgent → SubAgent → GrandAgent(最大深度 5)
//! - **独立上下文隔离**: 每个 Agent 拥有独立 1M Token 等效上下文(128K 实际 + 8× 稀疏压缩)
//! - **Agent 生命周期管理**: Idle → Running → Paused → Completed/Failed/Crashed
//! - **AgentTask wrapper**: 包装 `nexus_core::Task`,扩展 MAS 特有字段,**不修改核心类型**
//!
//! ## 相关 ADR
//!
//! - [ADR-026](../../../docs/architecture/ADR-026-chimera-mas-subsystem.md): MAS 子系统架构决策
//!
//! ## 与现有 crate 的关系(80% 能力复用)
//!
//! - 复用 `hcw-window` 实现 1M 上下文分层加载(不自实现压缩,Ω-Compress 单一实现)
//! - 复用 `osa-coordinator` 计算稀疏掩码(Ω-Sparse 单一实现)
//! - 复用 `event-bus` 的 NexusEvent(新增 7 个 Agent 变体,**不新建 AgentMessageBus**)
//! - 复用 `quest-engine` 的 Quest DAG + Checkpoint
//! - 复用 `gqep-executor` + `qeep-protocol` 实现零孤儿调用(§6.1 红线)
//!
//! ## 快速示例
//!
//! ```no_run
//! // Task 7-13 将填充具体实现,当前为骨架占位
//! use chimera_mas::prelude::*;
//!
//! // RootOrchestrator 将在 Task 12 实现
//! // let orchestrator = RootOrchestrator::new();
//! // let result = orchestrator.delegate(task).await?;
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod agent;
pub mod archive;
pub mod chunker;
pub mod context;
pub mod delegation;
pub mod error;
pub mod experts;
pub mod invariants;
pub mod knowledge;
pub mod orchestrator;
pub mod pdca;
pub mod quadrant;
pub mod scheduler;
pub mod stability;

// === 关键类型重导出,简化外部导入 ===
pub use agent::{
    Agent, AgentFactory, AgentLifecycle, AgentMeta, AgentStatus, AgentType, LifecycleState,
    ModelConfig,
};
pub use chunker::{BatchConfig, BatchExecutor, BatchResult, ChunkOutput, TaskChunker};
pub use context::{
    should_compress_at, AdmissionGate, AgentContext, ContextBlock, ContextIsolationGuard,
    ContextPriority, ContextTier, MemoryBudgetModel, TokenBudget, COMPRESSION_THRESHOLD,
    SPARSE_FACTOR,
};
pub use delegation::{
    AgentTask, DelegationExecutor, QualityLevel, TaskComplexity, TaskResult, TaskRunner,
};
pub use error::{MasError, Result};
pub use experts::{ExpertProfile, ExpertRegistry, PermissionTier, ToolPermission};
pub use invariants::{ArchiveTier, InvariantChecker, MEMORY_BUDGET_MB, MEMORY_BUDGET_UTILIZATION};
pub use knowledge::{ConsultSla, ExpertConsultant, KnowledgeChain, MutualInquirer, WikiRetriever};
pub use orchestrator::{AgentHandle, HeartbeatInfo, RootOrchestrator, MAX_AGENT_DEPTH};
pub use pdca::{
    AlertThresholds, PdcaAdjustments, PdcaAlert, PdcaAlertSeverity, PdcaLoop, PdcaMetrics,
    PlanReflux, TierDistribution, ALERT_CONSULT_TIMEOUT_RATE_WARNING, ALERT_MEMORY_CRITICAL_MB,
    ALERT_SINGLE_AGENT_WARNING_MB, ALERT_WIKI_COUNT_WARNING, PDCA_ALERT_COOLDOWN_SECS,
};
pub use quadrant::{
    activated_quadrants, CoreCross, ProduceAssure, Quadrant, QuadrantPlan, QualityDimension,
    ValidationStep, MAX_QUADRANT_FANOUT,
};
pub use scheduler::{
    score_to_priority, should_preempt, wsjf_score, PriorityScheduler, PriorityThresholds,
    WsjfInput, WsjfWeights,
};
pub use stability::{
    CircuitBreaker, DegradationChain, DegradationStep, PressureSource, StabilityGuard,
    TerminalState, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN,
};

/// 预导入模块 — 提供最常用类型
///
/// 使用方式:`use chimera_mas::prelude::*;`
pub mod prelude {
    pub use crate::{
        agent::{
            Agent, AgentFactory, AgentLifecycle, AgentMeta, AgentStatus, AgentType, LifecycleState,
            ModelConfig,
        },
        chunker::{BatchConfig, BatchExecutor, BatchResult, ChunkOutput, TaskChunker},
        context::{
            should_compress_at, AdmissionGate, AgentContext, ContextBlock, ContextIsolationGuard,
            ContextPriority, ContextTier, MemoryBudgetModel, TokenBudget, COMPRESSION_THRESHOLD,
            SPARSE_FACTOR,
        },
        delegation::{
            AgentTask, DelegationExecutor, QualityLevel, TaskComplexity, TaskResult, TaskRunner,
        },
        error::{MasError, Result},
        experts::{ExpertProfile, ExpertRegistry, PermissionTier, ToolPermission},
        invariants::{ArchiveTier, InvariantChecker, MEMORY_BUDGET_MB, MEMORY_BUDGET_UTILIZATION},
        knowledge::{ConsultSla, ExpertConsultant, KnowledgeChain, MutualInquirer, WikiRetriever},
        orchestrator::{AgentHandle, HeartbeatInfo, RootOrchestrator, MAX_AGENT_DEPTH},
        pdca::{
            AlertThresholds, PdcaAdjustments, PdcaAlert, PdcaAlertSeverity, PdcaLoop, PdcaMetrics,
            PlanReflux, TierDistribution,
        },
        quadrant::{
            activated_quadrants, CoreCross, ProduceAssure, Quadrant, QuadrantPlan,
            QualityDimension, ValidationStep, MAX_QUADRANT_FANOUT,
        },
        scheduler::{
            score_to_priority, should_preempt, wsjf_score, PriorityScheduler, PriorityThresholds,
            WsjfInput, WsjfWeights,
        },
        stability::{
            CircuitBreaker, DegradationChain, DegradationStep, PressureSource, StabilityGuard,
            TerminalState, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN,
        },
    };
}
