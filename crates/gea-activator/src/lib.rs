//! 门控专家激活 — 基于门控机制的专家网络激活调度
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 核心职责
//! - 维护专家注册表(注册/注销专家画像)
//! - 计算门控值(sigmoid 门控,输出 ∈ `[0,1]`)
//! - 冲突消解(功能重叠检测 + Top-K 选择)
//! - 动态激活阈值(基于负载因子调整)
//! - 激活缓存(LRU,TTL 5 秒)
//! - 发布 `ExpertActivated` 事件通知订阅者
//!
//! # 快速示例
//! ```
//! use gea_activator::{GeaActivator, GeaConfig, ExpertProfile, TaskProfile};
//! use event_bus::EventBus;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let activator = GeaActivator::new(GeaConfig::default(), bus).unwrap();
//! activator.register_expert(ExpertProfile::new(
//!     "e-1", vec![0.5; 64], 0.8, vec!["code-gen".into()],
//! ));
//! let task = TaskProfile::new(0.9, "code-gen", 30, vec![0.5; 64]);
//! let result = activator.activate(&task).await.unwrap();
//! println!("activated: {:?}", result.activated);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod activator;
pub mod config;
pub mod conflict;
pub mod error;
pub mod gating;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use activator::GeaActivator;
pub use config::GeaConfig;
pub use conflict::{resolve_conflicts, Candidate};
pub use error::GeaError;
pub use gating::compute_gate_value;
pub use types::{ActivationResult, ExpertId, ExpertProfile, GateValue, TaskProfile};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::activator::GeaActivator;
    pub use crate::config::GeaConfig;
    pub use crate::error::GeaError;
    pub use crate::gating::compute_gate_value;
    pub use crate::types::{ActivationResult, ExpertId, ExpertProfile, GateValue, TaskProfile};
}
