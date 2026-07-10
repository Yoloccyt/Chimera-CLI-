//! 自适应认知预算治理 — L0-L3 四级自适应认知预算控制
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! # 核心职责
//! - 四级离散预算分级(L0 降级 / L1 基础 / L2 标准 / L3 充足)
//! - 基于当前消耗率与剩余预算自动调整级别
//! - 预算分配与检查(请求是否在当前级别预算内)
//! - 预算超限检测与降级(消耗超过阈值时自动降级)
//!
//! # 依赖方向(§2.2 依赖铁律)
//! ACB 是 L8 层,向下依赖 L1 的 event-bus(跨层通信唯一通道)。
//! 不向上依赖 L9 Quest Engine,Quest 信息通过值对象传入。
//!
//! # 事件集成(Ω-Event 定律)
//! 已集成 event-bus,发布以下事件:
//! - 级别切换 → `BudgetAdjusted` 事件
//! - 预算超限 → `BudgetExceeded` 事件
//!
//! # 快速示例
//! ```
//! use acb_governor::{AcbGovernor, AcbGovernorConfig, BudgetRequest};
//!
//! let governor = AcbGovernor::new(AcbGovernorConfig::default()).unwrap();
//! let request = BudgetRequest::new("quest-1", 100);
//! let result = governor.check_budget(&request);
//! println!("budget check: {:?}", result);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod budget_coordinator;
pub mod config;
pub mod error;
pub mod governor;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use budget_coordinator::{BudgetCoordinator, UnifiedBudgetDecision, UnifiedBudgetStatus};
pub use config::AcbGovernorConfig;
pub use error::AcbError;
pub use governor::AcbGovernor;
pub use types::{BudgetAllocation, BudgetRequest, BudgetStatus, BudgetTier, TierSwitchResult};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::budget_coordinator::{BudgetCoordinator, UnifiedBudgetDecision, UnifiedBudgetStatus};
    pub use crate::config::AcbGovernorConfig;
    pub use crate::error::AcbError;
    pub use crate::governor::AcbGovernor;
    pub use crate::types::{
        BudgetAllocation, BudgetRequest, BudgetStatus, BudgetTier, TierSwitchResult,
    };
}
