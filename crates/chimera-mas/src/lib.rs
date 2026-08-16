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
//! - 直接复用 `nexus-core` 的 Quest / Task / Checkpoint 领域类型(L1,非 quest-engine)
//!   — 2026-07-31 订正:ADR-026 决策 5 原拟复用 quest-engine DAG,实现态直接用
//!   nexus-core 类型,src 零 quest_engine 引用,故移除僵尸同层依赖。
//! - 复用 `gqep-executor` + `qeep-protocol` 实现零孤儿调用(§6.1 红线)
//!
//! ## 快速示例
//!
//! ```no_run
//! use chimera_mas::prelude::*;
//! use event_bus::EventBus;
//!
//! # fn run() {
//! // RootOrchestrator 管理层级递归委托(最大深度 MAX_AGENT_DEPTH)
//! let orchestrator = RootOrchestrator::new(EventBus::new());
//! // 具体 delegate/四象限调度 API 见 orchestrator 模块文档
//! # let _ = orchestrator;
//! # }
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
pub mod feedback;
pub mod invariant_report;
pub mod invariants;
pub mod knowledge;
pub mod orchestrator;
pub mod pdca;
pub mod quadrant;
pub mod scheduler;
pub mod shadow;
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
pub use feedback::{ExpertFeedbackEntry, ExpertFeedbackRegistry, ExpertPriorityAdjustment};
pub use invariants::{
    ArchiveTier, DelegationEdge, InvariantChecker, MEMORY_BUDGET_MB, MEMORY_BUDGET_UTILIZATION,
};
pub use knowledge::{ConsultSla, ExpertConsultant, KnowledgeChain, MutualInquirer, WikiRetriever};
pub use orchestrator::{AgentHandle, HeartbeatInfo, RootOrchestrator, MAX_AGENT_DEPTH};
pub use pdca::{
    AlertThresholds, PdcaAdjustments, PdcaAlert, PdcaAlertSeverity, PdcaLoop, PdcaMetrics,
    PlanReflux, TierDistribution, ALERT_CONSULT_TIMEOUT_RATE_WARNING, ALERT_MEMORY_CRITICAL_MB,
    ALERT_SINGLE_AGENT_WARNING_MB, ALERT_WIKI_COUNT_WARNING, PDCA_ALERT_COOLDOWN_SECS,
};
pub use quadrant::{
    activated_quadrants, quadrant_status, ConfigurableQuadrantSelector, CoreCross, ProduceAssure,
    Quadrant, QuadrantPlan, QuadrantSelector, QuadrantStatus, QualityDimension, ValidationStep,
    MAX_QUADRANT_FANOUT,
};
pub use scheduler::{
    score_to_priority, should_preempt, wsjf_score, PriorityScheduler, PriorityThresholds,
    WsjfInput, WsjfWeights,
};
// 影子模式子系统(ADR-053 备忘录 §五 B-4/B-5)— 适度导出:
// 编排入口 + 治理配置 + 证据类型 + 终判建议;统计纯函数与批次账本
// 细节经 `shadow::` 路径访问,不在 crate 根污染命名空间。
pub use shadow::{
    AhirtEvidenceCollector, BatchEvidence, BatchVerdict, GovernanceSignoff, PromotionAdvice,
    ShadowModeConfig, ShadowModeOrchestrator, Stage3Prerequisites,
};
pub use stability::{
    CircuitBreaker, DegradationChain, DegradationStep, PressureSource, StabilityGuard,
    TerminalState, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN,
};
// ImmuneSystem facade（ADR-046 命名对齐 P1-5）— 从 L8 parliament 重新导出
//
// WHY 重新导出而非自实现：ADR-046 决策 5 裁决 ImmuneSystem 落地于 parliament crate,
// 通过 event-bus 订阅 chimera-mas StabilityGuard 事件维护镜像状态（方案 A 事件订阅镜像）。
// chimera-mas（L9）向下依赖 parliament（L8）合规（§2.2 依赖铁律,Cargo.toml 已声明依赖）,
// 此处仅为命名对齐 — 让 `chimera_mas::ImmuneSystem` 可访问,为下游消费者（如 chimera-cli L10）
// 提供统一的免疫/稳定性入口,避免消费者需同时依赖 parliament + chimera-mas 两个 crate。
//
// 导出清单（适度导出原则）：facade 主类型 + 公开方法签名涉及的关联类型;
// 不导出内部实现细节（StabilityMirror / compute_cascade_risk / 三探针具体类型 / MembraneController）。
pub use parliament::{
    ImmuneSystem, ImmuneSystemError, ParadoxProbe, ParadoxReport, ParadoxRiskReport,
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
        feedback::{ExpertFeedbackEntry, ExpertFeedbackRegistry, ExpertPriorityAdjustment},
        invariants::{
            ArchiveTier, DelegationEdge, InvariantChecker, MEMORY_BUDGET_MB,
            MEMORY_BUDGET_UTILIZATION,
        },
        knowledge::{ConsultSla, ExpertConsultant, KnowledgeChain, MutualInquirer, WikiRetriever},
        orchestrator::{AgentHandle, HeartbeatInfo, RootOrchestrator, MAX_AGENT_DEPTH},
        pdca::{
            AlertThresholds, PdcaAdjustments, PdcaAlert, PdcaAlertSeverity, PdcaLoop, PdcaMetrics,
            PlanReflux, TierDistribution,
        },
        quadrant::{
            activated_quadrants, ConfigurableQuadrantSelector, CoreCross, ProduceAssure, Quadrant,
            QuadrantPlan, QuadrantSelector, QualityDimension, ValidationStep, MAX_QUADRANT_FANOUT,
        },
        scheduler::{
            score_to_priority, should_preempt, wsjf_score, PriorityScheduler, PriorityThresholds,
            WsjfInput, WsjfWeights,
        },
        shadow::{
            AhirtEvidenceCollector, BatchEvidence, BatchVerdict, GovernanceSignoff,
            PromotionAdvice, ShadowModeConfig, ShadowModeOrchestrator, Stage3Prerequisites,
        },
        stability::{
            CircuitBreaker, DegradationChain, DegradationStep, PressureSource, StabilityGuard,
            TerminalState, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN,
        },
    };

    // ImmuneSystem facade（ADR-046 P1-5 命名对齐）— 与顶层 pub use 同步
    // WHY 单独 pub use 而非放入 crate::{...} 块:这些类型来自 L8 parliament 外部 crate,
    // 通过顶层 re-export 已成为 crate 根级符号,此处为 prelude 消费者提供便捷访问。
    pub use parliament::{
        ImmuneSystem, ImmuneSystemError, ParadoxProbe, ParadoxReport, ParadoxRiskReport,
    };
}
