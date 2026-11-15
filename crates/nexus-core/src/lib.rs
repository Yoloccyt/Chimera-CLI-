//! CRATE-CONTRACT BEGIN
//! LAYER:    L1
//! ROLE:     核心状态与领域类型 — 维护 NexusState、UserIntent、CLV 等全局领域模型
//! BACKEND:  memory
//! PRODUCERS: 1 ClvSnapshotReported
//! CONSUMERS: 21 chimera-cli,chimera-mas,chimera-tui,chtc-bridge,cmt-tiering…
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! 核心状态与领域类型 — 维护 NexusState、UserIntent、CLV 等全局领域模型
//!
//! 对应架构层:L1 Core
//! 对应创新点:CLV(Context Latent Vector,512-dim 潜在语言)
//!
//! # 核心职责
//! - 提供 L1-L10 共享领域类型的 re-export 汇聚点(UserIntent、Quest、Task、Checkpoint
//!   等已下沉 L0 nexus-contracts,ADR-054 决策 6 / Task 3.10;本 crate 保留
//!   `use nexus_core::Quest` 路径兼容 30 依赖方)
//! - 实现 CLV(512 维潜在向量),作为语义路由与记忆检索的统一表示
//! - 维护 NexusState(线程安全全局状态),支持 Quest 注册、查询、快照哈希
//!
//! # 快速示例
//! ```
//! use nexus_core::{NexusState, Quest, Task, TaskStatus, ThinkingMode};
//!
//! let state = NexusState::new();
//! let quest = Quest {
//!     quest_id: "q-1".into(),
//!     title: "示例".into(),
//!     tasks: vec![Task {
//!         task_id: "t-1".into(),
//!         description: "首步".into(),
//!         status: TaskStatus::Pending,
//!         dependencies: vec![],
//!     }],
//!     thinking_mode: ThinkingMode::Standard,
//!     checkpoint_id: None,
//!     priority: 128,
//! };
//! state.register_quest(quest).unwrap();
//! assert_eq!(state.list_quests().len(), 1);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod clv;
pub mod config;
pub mod decay;
pub mod error;
#[macro_use]
pub mod newtype;
pub mod ids;
pub mod path_util;
// NOTE(B2/ADR-311): resilience / rl_client / stat_learning 三个 RL/学习预留面已随
// 2026-11-15 破坏式重构退役（生产代码零消费者,仅自身 tests 引用;决策与重建触发
// 条件登记 ADR-311）。RL 闸门开启时按 L0 rl_hooks 契约（nexus-contracts-seams）重建。
pub mod state;
pub mod storage_traits;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use clv::{cosine_similarity_slices, CLV};
pub use config::ChimeraConfig;
pub use error::NexusError;
pub use ids::{AgentId, CapabilityId, IntentId, ModelId, OperationId, QuestId, TaskId};
// P1-1 观察者接线:状态变更类型 + 监听器 trait(依赖倒置,适配层在 L9 quest-engine)
pub use state::{NexusState, StateChangeKind, StateChangeListener};
pub use storage_traits::{apply_performance_pragmas, PragmaCapable};
pub use types::{Checkpoint, MultimodalInput, Quest, Task, TaskStatus, ThinkingMode, UserIntent};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::clv::{cosine_similarity_slices, CLV};
    // NOTE(B1/ADR-309): compute 面已随 W4-1 迁出为 nexus-compute；shim 退役后
    // prelude 不再承载 compute 导出，消费方直接 `use nexus_compute::…`。
    pub use crate::config::ChimeraConfig;
    pub use crate::error::NexusError;
    pub use crate::ids::{AgentId, CapabilityId, IntentId, ModelId, OperationId, QuestId, TaskId};
    // P1-1:状态变更观察者类型加入 prelude
    pub use crate::state::{NexusState, StateChangeKind, StateChangeListener};
    pub use crate::storage_traits::{apply_performance_pragmas, PragmaCapable};
    pub use crate::types::{
        Checkpoint, MultimodalInput, Quest, Task, TaskStatus, ThinkingMode, UserIntent,
    };
}
