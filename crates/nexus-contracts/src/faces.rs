//! 域导向聚合面 —— L0-C（additive，ADR-新-1 同批）
//!
//! 四个按消费域组织的便捷再导出面：**不改变既有路径**，`nexus_contracts::<leaf>`
//! 与 `nexus_contracts::<group>::<leaf>` 逐字不变；本模块只是让"按域找类型"不再
//! 依赖对 W3 拆分批次的记忆（组目录是历史拆分面，不是消费域面）。
//!
//! | 面 | 覆盖域 | 典型消费方 |
//! |---|---|---|
//! | [`domain`] | 核心领域与事件元数据（UserIntent/Quest/Checkpoint/EventMetadata…） | L1-L9 全层 |
//! | [`execution`] | 工具执行与调度契约（tool_plan/tool_dispatch/scheduler_contract…） | L7 gqep / L9 mas-sched / L10 |
//! | [`governance`] | 安全与治理契约（command_validation/adjudication/calibration…） | L4 seccore / L8 parliament / L9 auditor |
//! | [`learning`] | 学习/经验/RL 契约（experience_card/rl_hooks/S1-S9 接缝…） | L5 gsoe / L6 omega-learner |
//!
//! # 有意不收入任何面（仍走原路径）
//! - 稀疏掩码体系（`masks`/`omni_masks`/`ids`——`ids` 已收 [`execution`]）与向量数学
//!   （`vector`/`mcsm`/`recall_fusion`）——L2/L6 热路径，glob 导入反而扩大冲突面；
//! - MCA 亲和与消息协议（`affinity`/`omni_message`/`price_vector`/`context_capability`）——
//!   L10 mca-gateway 专用面；
//! - Harness spec 家族（`harness_spec`/`harness_dimensions`/`blueprint`/`variant`/
//!   `behavior_contract`/`platform_grounding`）——配方期一次性消费，非热路径。
//!
//! # 示例
//! ```
//! use nexus_contracts::faces::execution::ToolPlan;
//! use nexus_contracts::faces::domain::UserIntent;
//!
//! let _plan: Option<ToolPlan> = None;
//! let _intent: Option<UserIntent> = None;
//! ```

/// 核心领域与事件元数据面。
pub mod domain {
    pub use crate::checkpoint::Checkpoint;
    pub use crate::domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
    pub use crate::event_metadata::EventMetadata;
    pub use crate::event_payload::{AgentStatus, EventSeverity, TaskPriority};
    pub use crate::graph_identity::GraphIdentity;
    pub use crate::task::TaskStatus;
    pub use crate::temporal::{TemporalMeta, TransitionType};
}

/// 工具执行与调度契约面。
pub mod execution {
    pub use crate::ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
    pub use crate::scheduler_contract::{
        ClaimOutcome, DenyReason, Lease, Priority, Quota, RenewOutcome, ShouldRunVerdict,
        TodoClaim, HANDOFF,
    };
    pub use crate::tool_admission::{
        Admission, BatchPlan, BatchPolicy, BatchReport, CallOutcome, DenyReason as BatchDenyReason,
        EffectClass, PlannedCall, TruncationCause,
    };
    pub use crate::tool_dispatch::{
        DispatchError, DispatchFuture, DispatchOutcome, DispatchRequest, DispatchRequestRef,
        ToolDispatcher,
    };
    pub use crate::tool_plan::{guards, PlanEdge, PlanError, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
}

/// 安全与治理契约面。
pub mod governance {
    pub use crate::adjudication::{
        CertifyPayload, CertifyVerdict, ClaimPayload, DeclaimPayload, EpistemicType,
        FailureFixStatus, FailureRecord, VerifierKind,
    };
    pub use crate::archive_monotonicity::{
        assert_archive_monotonicity, ArchiveTier, InvariantViolation,
    };
    pub use crate::budget_tier::BudgetTier;
    pub use crate::calibration::{
        agreement_rate_milli, certifiable, CalibrationSample, GateVerdict, HumanLabel,
        DEFAULT_AGREEMENT_THRESHOLD_MILLI,
    };
    pub use crate::command_validation::{
        AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError,
        CommandValidator,
    };
    pub use crate::errors::{
        ErrorLayer, EventFailure, NexusError, Recoverable, RecoveryStrategy, ToolFailure,
    };
    pub use crate::formal_props::{
        EmptyFormalProvider, FormalProperty, FormalResultProvider, InvariantSpec,
        PropertyCategory, VerificationMethod, VerificationResult, VerifiedWithStrength,
    };
    pub use crate::quota::{NamespaceQuota, QuotaError, QuotaLimits};
    pub use crate::sensitive::{
        redact, scan, ScanHit, ScanPolicy, ScanVerdict, SensitiveDisposition, SensitiveKind,
    };
}

/// 学习/经验/RL 契约面（含 S1-S9 策略接缝全集）。
pub mod learning {
    pub use crate::capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
    pub use crate::decay_profile::{DecayPolicy, DecayProfile};
    pub use crate::density::{DensityPolicy, DensityTier};
    pub use crate::experience_card::{
        AtomicOperator, CardMetadata, EnvironmentInfo, ErrorSignature, ExecutionStatus,
        ExperienceCard, NormalizedThreeFactor, ThreeFactorScore,
    };
    pub use crate::memory_pyramid::{
        AtomicCardType, AtomicMemoryCard, MemoryPyramidLevel, PersonaSummary, SceneBlock,
    };
    pub use crate::memory_strategy::{MemoryStrategyProvider, MemoryTaskPhase};
    pub use crate::parliament_policy::{ActivationStrategy, ParliamentPolicy};
    pub use crate::policy::{SelectorPolicy, SelectorWeights};
    pub use crate::prefetch::{PrefetchPolicy, PrefetchStrategy};
    pub use crate::recall_quota::{RecallQuota, RecallQuotaPolicy};
    pub use crate::reward::{RewardLayer, RewardSignal, RewardSpec, SecuritySeverity};
    pub use crate::rl_hooks::{
        PolicyFormat, RLActionVector, RLHook, RLStateVector, RLTrajectory, SerializedPolicy,
    };
    pub use crate::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
    pub use crate::skill_lifecycle::{SkillLifecycleContract, SkillLifecycleState};
    pub use crate::strategy::{MemoryStrategy, MemoryStrategyPolicy};
    pub use crate::token_evidence::{
        SegmentCreationReason, SegmentMetadata, TokenLedgerEntry, ToolCallRecord,
    };
}
