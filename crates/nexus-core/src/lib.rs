//! CRATE-CONTRACT BEGIN
//! LAYER:    L1
//! ROLE:     核心状态与领域类型 — 维护 NexusState、UserIntent、CLV 等全局领域模型
//! BACKEND:  memory
//! PRODUCERS: 1 ClvSnapshotReported
//! CONSUMERS: 19 chimera-cli,chimera-mas,chtc-bridge,cmt-tiering,csn-substitutor…
//! MATURITY: TRUE
//! CRATE-CONTRACT END
//! 核心状态与领域类型 — 维护 NexusState、UserIntent、CLV 等全局领域模型
//!
//! 对应架构层:L1 Core
//! 对应创新点:CLV(Context Latent Vector,512-dim 潜在语言)
//!
//! # 核心职责
//! - 领域类型(UserIntent、Quest、Task、Checkpoint 等)**已下沉 L0 nexus-contracts**
//!   (ADR-054 决策 6 / Task 3.10;ADR-222 D2 拆分为 `nexus-contracts-core`)。
//!   本 crate 的 `types` 纯 re-export 汇聚点已随 B4d/ADR-311 退役,
//!   **消费方一律直连 L0**:`use nexus_contracts::domain::{Quest, Task, ...}` /
//!   `use nexus_contracts::task::TaskStatus` / `use nexus_contracts::checkpoint::Checkpoint`
//! - 实现 CLV(512 维潜在向量),作为语义路由与记忆检索的统一表示
//! - 维护 NexusState(线程安全全局状态),支持 Quest 注册、查询、快照哈希
//!
//! # 快速示例
//! ```
//! use nexus_contracts::domain::{Quest, Task, ThinkingMode};
//! use nexus_contracts::task::TaskStatus;
//! use nexus_core::NexusState;
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
// NOTE(B3/ADR-313): config(ChimeraConfig 等 30 个 CLI 域配置类型)已迁回唯一
// 真实消费者 chimera-cli（L2-L9 生产代码零消费;`chimera_cli::config::X` 路径经
// 其 config.rs re-export 保持零 diff）。
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
// NOTE(B4d/ADR-311): types 纯 re-export 汇聚点已退役——全部领域类型
// （Quest/Task/UserIntent/Checkpoint/TaskStatus/ThinkingMode/MultimodalInput）
// 由消费方直连 L0 nexus_contracts（B4a-d 约 70 条 use 同批迁移）；其编译期
// 等价性守卫（reexported_types_are_identical_to_contracts）随模块退役，
// 类型一致性由 L0 自身 tests/type_consistency_test.rs 承担。

// === 关键类型重导出,简化外部导入 ===
pub use clv::{cosine_similarity_slices, CLV};
pub use error::NexusError;
pub use ids::{AgentId, CapabilityId, IntentId, ModelId, OperationId, QuestId, TaskId};
// P1-1 观察者接线:状态变更类型 + 监听器 trait(依赖倒置,适配层在 L9 quest-engine)
pub use state::{NexusState, StateChangeKind, StateChangeListener};
pub use storage_traits::{apply_performance_pragmas, PragmaCapable};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::clv::{cosine_similarity_slices, CLV};
    // NOTE(B1/ADR-309): compute 面已随 W4-1 迁出为 nexus-compute；shim 退役后
    // prelude 不再承载 compute 导出，消费方直接 `use nexus_compute::…`。
    // NOTE(B3/ADR-313): config 类型已迁回 chimera-cli，prelude 同步摘除。
    // NOTE(B4d/ADR-311): L0 领域类型 re-export 已退役，消费方直连 nexus_contracts。
    pub use crate::error::NexusError;
    pub use crate::ids::{AgentId, CapabilityId, IntentId, ModelId, OperationId, QuestId, TaskId};
    // P1-1:状态变更观察者类型加入 prelude
    pub use crate::state::{NexusState, StateChangeKind, StateChangeListener};
    pub use crate::storage_traits::{apply_performance_pragmas, PragmaCapable};
}
