//! 核心状态与领域类型 — 维护 NexusState、UserIntent、CLV 等全局领域模型
//!
//! 对应架构层:L1 Core
//! 对应创新点:CLV(Context Latent Vector,512-dim 潜在语言)
//!
//! # 核心职责
//! - 定义 L1-L10 所有上层 crate 共享的领域类型(UserIntent、Quest、Task、Checkpoint)
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
pub mod error;
pub mod newtype;
pub mod path_util;
pub mod state;
pub mod storage_traits;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use clv::{cosine_similarity_slices, CLV};
pub use config::ChimeraConfig;
pub use error::NexusError;
pub use state::NexusState;
pub use storage_traits::{apply_performance_pragmas, PragmaCapable};
pub use types::{Checkpoint, MultimodalInput, Quest, Task, TaskStatus, ThinkingMode, UserIntent};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::clv::{cosine_similarity_slices, CLV};
    pub use crate::config::ChimeraConfig;
    pub use crate::error::NexusError;
    pub use crate::state::NexusState;
    pub use crate::storage_traits::{apply_performance_pragmas, PragmaCapable};
    pub use crate::types::{
        Checkpoint, MultimodalInput, Quest, Task, TaskStatus, ThinkingMode, UserIntent,
    };
}
