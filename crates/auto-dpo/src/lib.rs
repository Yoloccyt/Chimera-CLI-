//! 自动偏好对生成 — 自动构造 DPO 训练所需的偏好对样本
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块,服务于 GSOE 进化闭环)
//!
//! # 核心职责
//! - 从模型输出候选中构造偏好对(chosen / rejected)
//! - 基于质量评分进行样本门控(过滤低质量样本)
//! - 通过 EventBus 发布 `DpoPairGenerated` 事件,供 GSOE/Parliament 消费
//!
//! # 依赖方向(§2.2 依赖铁律)
//! AutoDPO 是 L5 层,向下依赖 L1 的 event-bus。不向上依赖 L8 Parliament,
//! 议会共识通过订阅 `ConsensusReached` 事件传入(由调用方驱动)。
//!
//! # 事件集成(Ω-Event 定律)
//! 已集成 event-bus,发布以下事件:
//! - 偏好对生成 → `DpoPairGenerated` 事件(携带 pair_id / chosen / rejected)
//!
//! # 快速示例
//! ```
//! use auto_dpo::{PreferencePairGenerator, AutoDpoConfig, ModelOutput};
//!
//! let generator = PreferencePairGenerator::new(AutoDpoConfig::default()).unwrap();
//! let outputs = vec![
//!     ModelOutput::new("output-a", 0.9),
//!     ModelOutput::new("output-b", 0.3),
//! ];
//! let pair = generator.generate(&outputs).unwrap();
//! println!("chosen: {}, rejected: {}", pair.chosen, pair.rejected);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod generator;
pub mod trainer;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::AutoDpoConfig;
pub use error::AutoDpoError;
pub use generator::PreferencePairGenerator;
pub use trainer::DpoTrainer;
pub use types::{ModelOutput, PreferencePair, SampleQuality};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::AutoDpoConfig;
    pub use crate::error::AutoDpoError;
    pub use crate::generator::PreferencePairGenerator;
    pub use crate::trainer::DpoTrainer;
    pub use crate::types::{ModelOutput, PreferencePair, SampleQuality};
}
