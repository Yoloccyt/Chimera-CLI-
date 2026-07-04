//! SSRA 黏液式快速适配 — 预编译模板 + 运行时低延迟融合
//!
//! 对应架构层:L7 Execution
//! 对应创新点:SSRA(Slime-Style Rapid Adaptation)
//! 设计来源:GLM 5.2 slime 机制(2 天合并专家)+ ADR-022
//!
//! ## 核心机制
//! - 预编译适配器模板(`SlimeTemplate`),缓存于 `TemplateRegistry`
//! - 运行时低延迟融合(p95 ≤ 20ms),支持 WeightedAverage / TopK / MeanField 三种策略
//! - 通过 EventBus 订阅 `ConsensusReached` / `RedTeamAudit` 事件触发防御性适配
//!
//! ## 快速示例
//! ```no_run
//! use ssra_fusion::{
//!     SsraConfig, SlimeFusionEngine, FusionRequest, FusionStrategy,
//!     TemplateSpec, precompile,
//! };
//!
//! # async fn run() {
//! let config = SsraConfig::default();
//! let engine = SlimeFusionEngine::new(config);
//!
//! // 注册预编译模板
//! let template = precompile(TemplateSpec::new(
//!     "cap-1", vec!["x".into()], FusionStrategy::TopK,
//! ));
//! engine.registry().register(template).unwrap();
//!
//! let request = FusionRequest::new("q-1", vec!["cap-1".into()], "target", 20, 8);
//! let result = engine.fuse(request).await.unwrap();
//! assert!(result.confidence >= 0.0 && result.confidence <= 1.0);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod fusion;
pub mod templates;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::SsraConfig;
pub use error::SsraError;
pub use fusion::SlimeFusionEngine;
pub use templates::{precompile, TemplateRegistry, TemplateSpec};
pub use types::{FusionRequest, FusionResult, FusionStrategy, SlimeTemplate};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::SsraConfig;
    pub use crate::error::SsraError;
    pub use crate::fusion::SlimeFusionEngine;
    pub use crate::templates::{precompile, TemplateRegistry, TemplateSpec};
    pub use crate::types::{FusionRequest, FusionResult, FusionStrategy, SlimeTemplate};
}
