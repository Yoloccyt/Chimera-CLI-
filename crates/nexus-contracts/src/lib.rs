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
//! | `ActivationStrategy` / `ParliamentPolicy` | 新建（P4-W14.3 S5 接缝） | L8 parliament / L6 omega-learner |
//! | `DecayProfile` / `DecayPolicy` | 新建（P4-W14.4 S6 接缝） | L4 decay-engine / L6 omega-learner |
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

// ============================================================
// 公开 API 导出
// ============================================================

pub use capability_token::{CapabilityToken, CapabilityTokenStatus, SeamId};
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
// P3-W10.3: 选择器策略（SelectorPolicy + SelectorWeights，D1 修复）
pub use policy::{SelectorPolicy, SelectorWeights};
pub use quota::{NamespaceQuota, QuotaLimits};
pub use temporal::{TemporalMeta, TransitionType};
// P2-W7.3: 向量存储契约（VectorStore trait + VectorHit + 扩展 trait）
pub use vector::{VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats};

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
    pub use crate::temporal::{TemporalMeta, TransitionType};
    pub use crate::vector::{
        VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats,
    };
}
