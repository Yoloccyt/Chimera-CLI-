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
//! use nexus_contracts::affinity::ThinkingPreference;
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
//!     thinking_pref: ThinkingPreference::Standard,
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
pub mod history;
/// MCA M3: 通道化路由决策单元(ADR-065,替代旧的 model_id 字符串)
pub mod model_route;
pub mod moe;
pub mod registry;
/// MCA M3: 路由亲和元数据(ADR-065,能力协商结果在路由决策中的投射)
pub mod route_affinity;
/// MCA M3: 通道化路由目标三元组(ADR-065,不动 RoutingDecision)
pub mod route_target;
pub mod router;
pub mod strategies;
pub mod trajectory;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use cacr::{CacrConfig, CacrDecision, CacrGuard};
pub use config::{HistoryPersistence, RouterConfig};
pub use error::RouterError;
// v1.4.0 P1:HistoryStore/InMemoryHistoryStore/SqliteHistoryStore 从 history 模块重导出
// HistoryRecord/MoeGate 仍从 moe 模块导出(权威定义源)
pub use history::{HistoryStore, InMemoryHistoryStore, SqliteHistoryStore};
pub use history::{RouteHistoryStore, RouteRecord};
pub use model_route::ModelRoute;
pub use moe::{HistoryRecord, MoeGate};
pub use registry::ModelRegistry;
pub use route_affinity::RouteAffinity;
pub use route_target::RouteTarget;
pub use router::ModelRouter;
// P4-W16.1.1: RouteHook trait + TrajectoryEvent/Outcome 轨迹捕获类型重导出
// P4-W16.1.2: RecordingHook 生产级实现 + TrajectoryStats 统计快照重导出
pub use trajectory::{
    RecordingHook, RouteHook, TrajectoryEvent, TrajectoryOutcome, TrajectoryStats,
};
pub use types::{ModelInfo, RoutingDecision, RoutingRequest, RoutingStrategy};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::cacr::{CacrConfig, CacrDecision, CacrGuard};
    pub use crate::config::{HistoryPersistence, RouterConfig};
    pub use crate::error::RouterError;
    pub use crate::history::{HistoryStore, InMemoryHistoryStore, SqliteHistoryStore};
    pub use crate::history::{RouteHistoryStore, RouteRecord};
    pub use crate::model_route::ModelRoute;
    pub use crate::moe::{HistoryRecord, MoeGate};
    pub use crate::registry::ModelRegistry;
    pub use crate::route_affinity::RouteAffinity;
    pub use crate::route_target::RouteTarget;
    pub use crate::router::ModelRouter;
    // P4-W16.1.1 + P4-W16.1.2: 轨迹捕获类型 + 生产级 RecordingHook 加入 prelude
    pub use crate::trajectory::{
        RecordingHook, RouteHook, TrajectoryEvent, TrajectoryOutcome, TrajectoryStats,
    };
    pub use crate::types::{ModelInfo, RoutingDecision, RoutingRequest, RoutingStrategy};
}
