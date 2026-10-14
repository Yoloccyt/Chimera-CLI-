//! MODULE-CONTRACT BEGIN
//! LAYER:    L3(cmt-tiering 内生子模块,ADR-193 架构减法:原独立 crate
//!           `lsct-tiering` 因唯一消费方即本 crate 而整并,消除跨 crate
//!           Tier 复制与双向转换守护层)
//! ROLE:     任务感知能力分层 — 按任务负载动态调整能力存储层级
//! BACKEND:  memory
//! PRODUCERS: 1 LsctTierSwitched
//! CONSUMERS: 1 cmt-tiering(同 crate:spawn_lsct_policy 装配 + 订阅腿)
//! MATURITY: TRUE
//! MODULE-CONTRACT END
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
//! - `Tier` 层级类型与 CMT 共用单一实现(`crate::types::Tier`,ADR-193 统一)
//! - 计算策略并发布事件,CMT 订阅事件做实际数据迁移
//! - 生产装配入口:`CmtCoordinator::spawn_lsct_policy`(cmt 侧)
//!
//! # 快速示例
//! ```no_run
//! use cmt_tiering::lsct::{LsctCoordinator, LsctConfig, TaskType, Tier};
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

pub mod config;
pub mod error;
pub mod tiering;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::LsctConfig;
pub use error::LsctError;
pub use tiering::{
    compute_target_tier, compute_target_tier_with_config, LsctCoordinator, LsctDemoter,
    LsctPromoter,
};
pub use types::{
    next_colder, next_warmer, tier_rank, TaskLoadProfile, TaskType, Tier, TierAssignment,
    TierSwitchDecision,
};
