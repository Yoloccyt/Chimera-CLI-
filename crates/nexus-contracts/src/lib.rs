//! NEXUS-OMEGA L0 契约层 — 纯类型定义，零逻辑，零 crate 依赖
//!
//! 对应架构层: **L0 Contracts**（nexus-core 之下，十层架构新增最低层）
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C6
//!
//! # 核心职责
//!
//! 承载跨层共享的**纯类型契约**，消除 L1 `nexus-core` 的上帝 crate 问题（被 7+ crate 依赖）
//! 与 L6 `osa-coordinator` 的星型耦合问题（3 router 均依赖 OSA 取 `OmniSparseMasks`）。
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 零逻辑**: 仅类型定义与基础构造函数，不含业务逻辑
//! - **零 crate 依赖**: 禁止依赖任何 workspace crate（含 `nexus-core` / `event-bus`）
//! - **唯一外部依赖**: `serde`（仅 derive 宏，用于跨 crate 序列化）
//! - **依赖铁律扩展**: `L(N) → L(0)` 恒允许（任何层均可依赖 L0）
//!
//! # 承载类型
//!
//! | 类型 | 来源 | 消费层 |
//! |------|------|--------|
//! | `OmniSparseMasks` | 从 `osa-coordinator` 上提 | L2 HCW / L6 Router × 3 |
//! | `SparseMask<T>` | 从 `osa-coordinator` 上提 | L6 OSA / L2 HCW |
//! | `ToolId` / `FileId` / `MemoryId` / `OperationId` / `TaskId` | 从 `osa-coordinator` 上提 | L6 Router × 3 / L7 Execution |
//! | `HarnessSpec` / `ContractSpec` / `HopSpec` | 新建（P4 Harness-as-Spec） | L5 gsoe-evolution / L9 quest-engine |
//! | `TemporalMeta` / `TransitionType` | 新建（P3 时间扩展） | L2 mlc-engine |
//! | `NamespaceQuota` | 新建（命名空间配额上提） | L9 chimera-mas |
//! | `SelectorPolicy` / `SelectorWeights` | 新建（P3-W10.3 D1 修复） | L2 hcw-window / L6 omega-learner |
//! | `DensityPolicy` / `DensityTier` | 新建（P4-W13.2 S1 接缝） | L2 hcw-window / L6 omega-learner |
//! | `MemoryStrategy` / `MemoryStrategyPolicy` | 新建（P4-W14.1 S2 接缝） | L2 mlc-engine / L6 omega-learner |
//! | `MemoryTaskPhase` / `MemoryStrategyProvider` | 新建（Task 2 OSA S2 桥接） | L6 osa-coordinator / L6 omega-learner |
//! | `ActivationStrategy` / `ParliamentPolicy` | 新建（P4-W14.3 S5 接缝） | L8 parliament / L6 omega-learner |
//! | `DecayProfile` / `DecayPolicy` | 新建（P4-W14.4 S6 接缝） | L4 decay-engine / L6 omega-learner |
//! | `EventSeverity` / `TaskPriority` / `AgentStatus` | 从 `event-bus` 下沉（ADR-054 决策 6，P9-T7） | L1 event-bus / L9 chimera-mas |
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::{OmniSparseMasks, SparseMask, ToolId};
//!
//! let routing = SparseMask::select_top_k(
//!     &[ToolId::new("tool-1"), ToolId::new("tool-2")],
//!     &[0.9, 0.1],
//!     1,
//! );
//! assert!(routing.is_active(&ToolId::new("tool-1")));
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]
#![doc(html_root_url = "https://docs.rs/nexus-contracts")]

// ============================================================
// 模块声明
// ============================================================

/// ID newtype 类型 — 五维度稀疏化的统一标识
pub mod ids;

/// 稀疏掩码容器 — 泛型 `SparseMask<T>`
pub mod masks;

/// 全维稀疏掩码 — 五维度掩码聚合体 `OmniSparseMasks`
pub mod omni_masks;

/// Harness-as-Spec 契约类型 — P4 学习闭环规格
pub mod harness_spec;

/// 时间元数据 — P3 时间感知记忆扩展
pub mod temporal;

/// 命名空间配额 — chimera-mas 资源配额上提
pub mod quota;

/// 向量存储契约 — 跨层向量检索抽象（VectorStore trait + VectorStoreExt 扩展）
pub mod vector;

/// 选择器策略契约 — D1 病理修复（selector 权重外置，P3-W10.3）
pub mod policy;

/// 密度档位策略契约 — S1 接缝（DDR/HCW 密度档位，P4-W13.2）
pub mod density;

/// 记忆策略契约 — S2 接缝（mlc-engine 记忆策略选择，P4-W14.1）
pub mod strategy;

/// OSA memory 维度自适应记忆策略契约 — S2 桥接层（Task 2）
///
/// 承载 `MemoryTaskPhase` 任务阶段枚举与 `MemoryStrategyProvider` trait，
/// 供 OSA memory 维度通过 L0 trait 调用 omega-learner S2（依赖铁律 §2.2 合规）。
pub mod memory_strategy;

/// 预取策略契约 — S3 接缝（scc-cache 预取策略选择，P4-W14.2）
pub mod prefetch;

/// Parliament 激活策略契约 — S5 接缝（Parliament 激活策略选择，P4-W14.3）
pub mod parliament_policy;

/// 衰减参数策略契约 — S6 接缝（decay-engine 衰减档位选择，P4-W14.4）
pub mod decay_profile;

/// 能力场令牌契约 — 学习策略灰度授权载体（P4-W14.5 C4 合规）
///
/// 承载 CapabilityToken 类型，使编排器在注入 Learned 策略前查询授权等级，
/// 实现 C4 合规要求的"运行时灰度走能力场，而非散落运行时布尔旗"。
pub mod capability_token;

/// 召回配额策略契约 — S7 接缝（R1 离线 RL，P4-W16.2.2）
///
/// 承载 R1（召回配额 CQL/IQL）接缝的离散动作空间 `RecallQuota`
/// 与灰度授权载体 `RecallQuotaPolicy`，供 L6 omega-learner 与 L9 编排器共享。
pub mod recall_quota;

/// 行为契约 — 类型使用规范契约(polish-v2.7 P1-3,ADR-049)
///
/// 承载 BehaviorContract(前置/后置/不变量断言)与 ContractExample,
/// 供 L9 RuntimeAuditor 审计与 L5 AEGIS Evolver 约束输入。
pub mod behavior_contract;

/// 变体契约 — Harness 变体标识与性能契约(polish-v2.7 P3-2,ADR-051)
///
/// 承载 VariantId(spec 主键复用)与 VariantContract(任务类型适用域 +
/// 性能承诺),供 L8 parliament 变体池与 L5 AEGIS 变体登记共享。
pub mod variant;

/// 流程蓝图契约 — 隐性流程经验载体(polish-v2.7 P4-7,ADR-049)
///
/// 承载 ProceduralBlueprint(步骤/前置/成功率)与纯函数计划校验,
/// 供 L5 repo-wiki 轨迹提取与 L9 quest-engine 计划预检共享。
pub mod blueprint;

/// 形式化属性定义框架 — FormalVerifier L4 骨架基础类型(T6-2)
///
/// 承载 PropertyCategory / VerificationResult / InvariantSpec / FormalProperty,
/// 供 L4 formal-verifier 验证器实现与 L8 parliament 审议时查询属性满足状态共享。
pub mod formal_props;

/// 事件元数据契约 — L0 共享的事件追踪元信息(Task 3.10,ADR-033 扩展)
///
/// 承载 `EventMetadata`(event_id / timestamp / source),从 L1 `event-bus/src/payloads.rs`
/// 上提至 L0,缓解 L1 上帝 crate 病理(被 100+ 文件依赖)。
/// 依赖: chrono + uuid(ADR-033 Task 3.10 新增例外,基础类型库)。
pub mod event_metadata;

/// 任务状态契约 — L0 共享的 Task 生命周期枚举(Task 3.10,ADR-033 扩展)
///
/// 承载 `TaskStatus`(Pending/Running/Completed/Failed),从 L1 `nexus-core/src/types.rs`
/// 上提至 L0,缓解 L1 上帝 crate 病理(被 65+ 文件依赖)。纯 enum,仅 serde derive 依赖。
pub mod task;

/// 检查点契约 — L0 共享的 Quest 断点恢复快照(Task 3.10,ADR-033 扩展)
///
/// 承载 `Checkpoint`(quest_id / checkpoint_id / memory_snapshot_hash / serialized_state / created_at),
/// 从 L1 `nexus-core/src/types.rs` 上提至 L0,缓解 L1 上帝 crate 病理(被 42+ 文件依赖)。
/// 依赖: chrono(ADR-033 Task 3.10 新增例外,基础类型库)。
pub mod checkpoint;

/// 预算档位契约 — DECB 三档枚举上提(ADR-054 决策 3,P9-T3)
///
/// 承载 `BudgetTier`(HighTier/LowTier/Degraded),从 L8 `decb-governor` 上提至 L0,
/// 消除 L9 `quest-engine` 对 L8 的生产依赖边(L9→L8 违规方向,ADR-054 裁决)。
/// decb-governor 保留 re-export 保兼容;纯枚举,仅 serde derive 依赖(ADR-033)。
pub mod budget_tier;

/// 模型亲和契约 — MCA 体系 L0 类型(ADR-065,PANTHEON 计划)
///
/// 承载 ProviderId / ProtocolDialect / CapabilitySet / ModelAffinitySpec /
/// AffinityRequest / AffinityResponse 等"能力协商取代名字嗅探"（原则 P1）
/// 契约类型,供 L10 mca-gateway / L1 model-router / L6 omega-learner 共享。
pub mod affinity;

/// 命令验证契约 — 攻击类型/命令/策略/trait 上提(ADR-054 决策 3,P9-T4)
///
/// 承载 `AttackType` / `Command` / `BlockedPattern` / `CommandPolicy` /
/// `CommandValidationError` / `CommandValidator` trait,从 L4 `seccore` 上提至 L0,
/// 消除 L8 `parliament` 对 L4 `seccore` 的生产依赖违规边(ADR-054 裁决)。
/// seccore 保留 re-export 保兼容 + 实现 `CommandValidator`;parliament 改 L0 trait
/// 注入 + 未注入优雅降级(MemoryStrategyProvider 先例)。默认策略语义冻结迁移。
pub mod command_validation;

/// 纯领域类型契约 — 用户意图/长期任务/思考模式上提(ADR-054 决策 6,P9-T7)
///
/// 承载 `ThinkingMode` / `MultimodalInput` / `UserIntent` / `Quest` / `Task`,
/// 从 L1 `nexus-core` 上提至 L0,缓解 D1 上帝 crate 病理(nexus-core 被 30 依赖方引用)。
/// nexus-core 保留 re-export 保兼容;纯类型,仅 serde derive 依赖(ADR-033)。
pub mod domain;

/// 事件载荷契约 — 纯载荷枚举下沉(ADR-054 决策 6,P9-T7 Task 2)
///
/// 承载 `EventSeverity` / `TaskPriority` / `AgentStatus` 三个高价值纯载荷枚举,
/// 从 L1 `event-bus/src/payloads.rs` 下沉至 L0,缓解 L1 超级节点(34 依赖方)负担。
/// **severity() 判定逻辑不迁移**(架构红线:Critical 事件 mpsc 保障,判定逻辑留在
/// L1 event-bus);本模块仅承载纯枚举类型(变体/derive 与 L1 原定义逐字一致)。
pub mod event_payload;

/// P9-T2 测试等待缩放工具 — CHIMERA_TEST_TIMEOUT_SCALE 协议
///
/// 提供 `scale_timeout` / `scaled_timeout!` 宏,允许 PR fast 档通过环境变量
/// 收敛硬编码 2s+ 等待。是 nexus-contracts "纯类型 + 零逻辑" 约束的**唯一例外**:
/// 工具仅读 env,不引入任何领域逻辑;详见 module-level 文档。
pub mod test_scale;

// ============================================================
// 公开 API 导出
// ============================================================

pub use capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
// polish-v2.7 P1-3: 行为契约(BehaviorContract + ContractContext + ContractExample,ADR-049)
pub use behavior_contract::{BehaviorContract, ContractContext, ContractExample};
// P4-W16.2.2: R1 召回配额（RecallQuota + RecallQuotaPolicy，S7 接缝）
pub use density::{DensityPolicy, DensityTier};
pub use recall_quota::{RecallQuota, RecallQuotaPolicy};
// P4-W14.1: 记忆策略（MemoryStrategy + MemoryStrategyPolicy，S2 接缝）
// P4-W15.1.1: HarnessSpec 扩展（ImmutableSurface 不可进化面清单 + HarnessSpecError 校验错误）
pub use harness_spec::{
    ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface,
    RetryPolicy, REQUIRED_ACCEPTANCE_GATES,
};
pub use ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
pub use masks::SparseMask;
pub use omni_masks::OmniSparseMasks;
// P4-W14.3: Parliament 激活策略（ActivationStrategy + ParliamentPolicy，S5 接缝）
pub use parliament_policy::{ActivationStrategy, ParliamentPolicy};
// P4-W14.4: 衰减参数策略（DecayProfile + DecayPolicy，S6 接缝）
pub use decay_profile::{DecayPolicy, DecayProfile};
// P4-W14.2: 预取策略（PrefetchStrategy + PrefetchPolicy，S3 接缝）
pub use prefetch::{PrefetchPolicy, PrefetchStrategy};
pub use strategy::{MemoryStrategy, MemoryStrategyPolicy};
// Task 2: OSA memory 维度 S2 桥接契约（MemoryTaskPhase + MemoryStrategyProvider trait）
pub use memory_strategy::{MemoryStrategyProvider, MemoryTaskPhase};
// P3-W10.3: 选择器策略（SelectorPolicy + SelectorWeights，D1 修复）
pub use policy::{SelectorPolicy, SelectorWeights};
pub use quota::{NamespaceQuota, QuotaLimits};
pub use temporal::{TemporalMeta, TransitionType};
// polish-v2.7 P3-2: 变体契约(VariantId + VariantContract,ADR-051)
pub use variant::{VariantContract, VariantId};
// polish-v2.7 P4-7: 流程蓝图(ProceduralBlueprint + 计划校验,ADR-049)
pub use blueprint::{BlueprintSource, BlueprintStep, PlanViolation, ProceduralBlueprint};
// P2-W7.3: 向量存储契约（VectorStore trait + VectorHit + 扩展 trait）
pub use vector::{VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats};
// T6-2: 形式化属性定义框架（FormalVerifier L4 骨架基础类型）
pub use formal_props::{
    FormalProperty, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
};
// Task 3.10: L0 共享类型扩展(EventMetadata / TaskStatus / Checkpoint)
// 从 L1 nexus-core / event-bus 下沉,缓解 L1 上帝 crate 病理(100+/65+/42+ 文件依赖)
pub use checkpoint::Checkpoint;
pub use event_metadata::EventMetadata;
pub use task::TaskStatus;
// ADR-054 决策 3: DECB 预算档位(BudgetTier 从 L8 decb-governor 上提,P9-T3)
pub use budget_tier::BudgetTier;
// ADR-065: MCA 模型亲和契约(能力协商唯一事实源 + 统一请求/响应)
// WHY 不导出全部子类型: PricingSpec/EndpointSpec/QuirkRule 等仅 spec 装配方
// (L10 spec_loader)使用,走 affinity:: 路径引用,避免顶层命名空间膨胀。
pub use affinity::{
    sampling_bucket, AffinityRequest, AffinityResponse, CapabilitySet, ContentBlock,
    ModelAffinitySpec, NegotiationFidelity, OutputBudget, OutputFormat, ProtocolDialect,
    ProviderId, SamplingParams, ThinkingPreference, TokenCacheKey, UsageReport,
};
// ADR-054 决策 3:命令验证契约(AttackType/Command/CommandPolicy/CommandValidator 上提,P9-T4)
pub use command_validation::{
    AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError, CommandValidator,
};
// ADR-054 决策 6:纯领域类型(ThinkingMode/MultimodalInput/UserIntent/Quest/Task 上提,P9-T7)
// WHY 顶层导出: 与 nexus-core re-export 形成对等路径,依赖方可直接 `use nexus_contracts::Quest`
pub use domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
// ADR-054 决策 6:事件载荷契约(EventSeverity/TaskPriority/AgentStatus 下沉,P9-T7 Task 2)
// severity() 判定逻辑留在 L1 event-bus(架构红线:Critical 事件 mpsc 保障)
pub use event_payload::{AgentStatus, EventSeverity, TaskPriority};

/// 预导出模块 — 常用类型的便捷导入
///
/// # 示例
/// ```
/// use nexus_contracts::prelude::*;
///
/// let id = ToolId::new("tool-1");
/// let mask: SparseMask<ToolId> = SparseMask::empty();
/// ```
pub mod prelude {
    pub use crate::capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
    // P4-W16.2.2: R1 召回配额（S7 接缝）
    pub use crate::density::{DensityPolicy, DensityTier};
    pub use crate::recall_quota::{RecallQuota, RecallQuotaPolicy};
    // P4-W15.1.1: HarnessSpec 扩展（ImmutableSurface + HarnessSpecError）
    pub use crate::harness_spec::{
        ContractSpec, HarnessMeta, HarnessSpec, HarnessSpecError, HopSpec, ImmutableSurface,
        RetryPolicy, REQUIRED_ACCEPTANCE_GATES,
    };
    pub use crate::ids::{FileId, MemoryId, OperationId, TaskId, ToolId};
    pub use crate::masks::SparseMask;
    pub use crate::omni_masks::OmniSparseMasks;
    // P3-W10.3: 选择器策略（D1 修复）
    pub use crate::policy::{SelectorPolicy, SelectorWeights};
    // P4-W14.3: Parliament 激活策略（S5 接缝）
    pub use crate::parliament_policy::{ActivationStrategy, ParliamentPolicy};
    // P4-W14.4: 衰减参数策略（S6 接缝）
    pub use crate::decay_profile::{DecayPolicy, DecayProfile};
    // P4-W14.2: 预取策略（S3 接缝）
    pub use crate::prefetch::{PrefetchPolicy, PrefetchStrategy};
    pub use crate::quota::{NamespaceQuota, QuotaLimits};
    // P4-W14.1: 记忆策略（S2 接缝）
    pub use crate::strategy::{MemoryStrategy, MemoryStrategyPolicy};
    // Task 2: OSA memory 维度 S2 桥接契约
    pub use crate::memory_strategy::{MemoryStrategyProvider, MemoryTaskPhase};
    pub use crate::temporal::{TemporalMeta, TransitionType};
    pub use crate::vector::{
        VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats,
    };
    // T6-2: 形式化属性定义框架
    pub use crate::formal_props::{
        FormalProperty, InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
    };
    // Task 3.10: L0 共享类型扩展(EventMetadata / TaskStatus / Checkpoint)
    pub use crate::checkpoint::Checkpoint;
    pub use crate::event_metadata::EventMetadata;
    pub use crate::task::TaskStatus;
    // ADR-054 决策 3: DECB 预算档位(BudgetTier 上提,P9-T3)
    pub use crate::budget_tier::BudgetTier;
    // ADR-065: MCA 模型亲和契约(与顶层导出同集)
    pub use crate::affinity::{
        sampling_bucket, AffinityRequest, AffinityResponse, CapabilitySet, ContentBlock,
        ModelAffinitySpec, NegotiationFidelity, OutputBudget, OutputFormat, ProtocolDialect,
        ProviderId, SamplingParams, ThinkingPreference, TokenCacheKey, UsageReport,
    };
    // ADR-054 决策 3:命令验证契约(AttackType/Command/CommandPolicy/CommandValidator 上提,P9-T4)
    pub use crate::command_validation::{
        AttackType, BlockedPattern, Command, CommandPolicy, CommandValidationError,
        CommandValidator,
    };
    // ADR-054 决策 6:纯领域类型(与顶层导出同集)
    pub use crate::domain::{MultimodalInput, Quest, Task, ThinkingMode, UserIntent};
    // ADR-054 决策 6:事件载荷契约(EventSeverity/TaskPriority/AgentStatus 下沉,P9-T7 Task 2)
    // severity() 判定逻辑留在 L1 event-bus(架构红线:Critical 事件 mpsc 保障)
    pub use crate::event_payload::{AgentStatus, EventSeverity, TaskPriority};
}
