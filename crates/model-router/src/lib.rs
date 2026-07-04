//! 多模型分层路由 — 按任务特征将请求路由至最适配的底层模型
//!
//! 对应架构层:L1 Core
//! 对应创新点:无(基础设施)
//!
//! # 核心职责
//! - 维护模型注册表(`ModelRegistry`),支持动态注册/注销
//! - 提供三种路由策略:Lite(成本优先)、Efficient(延迟优先)、Auto(综合评分)
//! - 路由成功后发布 `ModelRouteSelected` 事件,供 Quest Engine 等订阅
//!
//! # 快速示例
//! ```
//! use event_bus::EventBus;
//! use model_router::{ModelRouter, ModelRegistry, RouterConfig, RoutingRequest, RoutingStrategy};
//! use nexus_core::{UserIntent, MultimodalInput};
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let registry = ModelRegistry::from_config(&RouterConfig::default());
//! let router = ModelRouter::new(registry, bus);
//!
//! let req = RoutingRequest {
//!     quest_id: "q-1".into(),
//!     intent: UserIntent {
//!         intent_id: "i-1".into(),
//!         raw_text: "hello".into(),
//!         multimodal_inputs: vec![MultimodalInput::Text("hello".into())],
//!         risk_level: 10,
//!     },
//!     estimated_tokens: 1000,
//!     strategy: RoutingStrategy::Auto,
//! };
//! let decision = router.route(req).await.unwrap();
//! assert!(!decision.model_id.is_empty());
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod cacr;
pub mod config;
pub mod error;
pub mod registry;
pub mod router;
pub mod strategies;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use cacr::{CacrConfig, CacrDecision, CacrGuard};
pub use config::RouterConfig;
pub use error::RouterError;
pub use registry::ModelRegistry;
pub use router::ModelRouter;
pub use types::{ModelInfo, RoutingDecision, RoutingRequest, RoutingStrategy};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::cacr::{CacrConfig, CacrDecision, CacrGuard};
    pub use crate::config::RouterConfig;
    pub use crate::error::RouterError;
    pub use crate::registry::ModelRegistry;
    pub use crate::router::ModelRouter;
    pub use crate::types::{ModelInfo, RoutingDecision, RoutingRequest, RoutingStrategy};
}
