//! 任务感知能力分层 — 按任务负载动态调整能力存储层级
//!
//! 对应架构层:L3 Storage
//! 对应创新点:LSCT(Load-aware Semantic Capability Tiering)
//!
//! # 核心职责
//! - 按任务负载画像(编译/调试/测试/运行)动态决定能力存储的目标层级
//! - 维护 TierAssignment 映射(capability_id → 当前/目标层级)
//! - 通过升降温器逐级迁移(只能相邻层级,防跨级跳跃)
//! - 发布 LsctTierSwitched 事件,供 CMT 等存储组件订阅执行实际迁移
//!
//! # 与 CMT 的关系
//! LSCT 是 CMT 之上的"任务感知策略层",不直接操作 CMT 存储:
//! - 复用 CMT 的 Tier enum(类型重用,非实现重用)
//! - 计算策略并发布事件,CMT 订阅事件做实际数据迁移
//! - 符合 §2.2 依赖铁律:同层 L3 互引 + 跨层走 EventBus
//!
//! # 快速示例
//! ```no_run
//! use lsct_tiering::{LsctCoordinator, LsctConfig, TaskType, compute_target_tier};
//! use cmt_tiering::Tier;
//!
//! let coordinator = LsctCoordinator::new(LsctConfig::default());
//! coordinator.register_capability("cap-1", Tier::Warm);
//! ```
//!
//! # 架构红线
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 锁中毒场景使用 `unwrap_or_else(|p| p.into_inner())`
//! - 所有 async fn 满足 `Send + 'static` 约束
//! - 跨层通信只能走 EventBus(§2.2 依赖铁律)

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod tiering;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::LsctConfig;
pub use error::LsctError;
pub use tiering::{compute_target_tier, LsctCoordinator, LsctDemoter, LsctPromoter};
pub use types::{
    next_colder, next_warmer, tier_rank, TaskLoadProfile, TaskType, TierAssignment,
    TierSwitchDecision,
};
