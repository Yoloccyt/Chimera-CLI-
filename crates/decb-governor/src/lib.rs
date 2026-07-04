//! 双档认知预算治理 — 高低双档切换的认知预算治理器
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! # 核心职责
//! - 连续可调预算系数计算([0.0, 1.0],基于复杂度/紧急度/剩余预算)
//! - 高低档自动切换(HighTier / LowTier / Degraded,带滞后机制)
//! - 预算溢出检测与降级(后台监控 + 自动降级链路)
//! - 预算消耗统计(累计消耗、利用率、剩余预算)
//!
//! # 依赖方向(§2.2 依赖铁律)
//! DECB 是 L8 层,不能向上依赖 L9 Quest Engine。Quest 信息通过
//! `QuestBudgetInput` 值对象传入,实现层间解耦。
//!
//! # 事件集成(Week 5 Task 37)
//! 已集成 event-bus,发布以下事件:
//! - 档位切换 → `BudgetAdjusted` 事件
//! - 溢出降级 → `BudgetExceeded` 事件 `[Critical]`
//! - 统计报告 → `BudgetStatsReported` 事件
//!
//! # 快速示例
//! ```
//! use decb_governor::{DecbGovernor, DecbConfig, QuestBudgetInput};
//!
//! let governor = DecbGovernor::new(DecbConfig::default()).unwrap();
//! let quest = QuestBudgetInput::simple("quest-1");
//! let coefficient = governor.compute_budget(&quest);
//! println!("budget coefficient: {}", coefficient);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod error;
pub mod governor;
pub mod overflow;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::DecbConfig;
pub use error::DecbError;
pub use governor::DecbGovernor;
pub use overflow::OverflowDetector;
pub use types::{BudgetCoefficient, BudgetConsumption, BudgetStats, BudgetTier, QuestBudgetInput};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::DecbConfig;
    pub use crate::error::DecbError;
    pub use crate::governor::DecbGovernor;
    pub use crate::overflow::OverflowDetector;
    pub use crate::types::{
        BudgetCoefficient, BudgetConsumption, BudgetStats, BudgetTier, QuestBudgetInput,
    };
}
