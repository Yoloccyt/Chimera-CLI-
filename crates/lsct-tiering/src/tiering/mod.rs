//! LSCT 分层引擎 — 任务负载画像、升降温器与协调器
//!
//! 对应架构层:L3 Storage
//!
//! # 模块职责
//! - `profile`:任务负载画像,从 Quest 标题推断任务类型与强度,计算目标层级
//! - `promoter`:升温器,逐级提升层级,HashSet 防级联升温
//! - `demoter`:降温器,逐级降级层级,HashSet 防级联降级
//! - `coordinator`:协调器,聚合画像与升降温,集成 EventBus 发布事件

pub mod coordinator;
pub mod demoter;
pub mod profile;
pub mod promoter;

pub use coordinator::LsctCoordinator;
pub use demoter::LsctDemoter;
pub use profile::{compute_target_tier, compute_target_tier_with_config};
pub use promoter::LsctPromoter;
