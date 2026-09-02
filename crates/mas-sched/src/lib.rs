//! mas-sched — 多代理调度器控制面（P3-T2，v4.0 WI-29）
//!
//! 对应架构层: **L9 Quest**（ADR-145 裁决：从 chimera-mas 拆出，D-P3 层归属定案）
//! 对应任务: **P3-T2**（手册 W16，WI-29 strangler：1 拆 2 控制面/执行面分离）
//!
//! # 职责
//! **控制面纯调度,不碰工具执行**（v4.0 WI-29 契约）:
//! - [`PeerScheduler`] trait:claim / renew_lease / handoff / should_run 四原语
//! - [`SimplePeerScheduler`]:内存实现（租约表 + 配额 + 优先级）
//! - [`ShadowScheduler`]:影子模式包装（只决策不执行,决策日志 100% 可回放,ADR-145）
//!
//! # 与 chimera-mas 的分工（v4.0 WI-25）
//! - **Claim 管长任务租约**（本 crate:TodoClaim/Lease/Quota/Handoff）;
//! - **Auction 管短任务派发**（nexus-subagent WI-25,Phase 3 T9）。
//!
//! # 影子模式（W16 门禁）
//! 影子决策日志 100% 可回放——`ShadowScheduler` 记录每条决策输入与输出,
//! [`ShadowLog::replay`] 逐条重放且决策结果与原始逐位一致（Ω₂ 确定性）。
//!
//! # 红线
//! `#![forbid(unsafe_code)]` 由 crate 顶层保证;依赖仅 L0/L1（内部 3 个 ≤6 门禁）;
//! 无自旋（租约判定用 Instant 时间戳,非忙等）;禁 feature 标志（影子经构造参数开启）。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod error;
pub mod scheduler;
pub mod shadow;
pub mod types;

pub use error::SchedError;
pub use scheduler::{PeerScheduler, SimplePeerScheduler};
pub use shadow::{ShadowDecision, ShadowLog, ShadowScheduler};
pub use types::{
    ClaimOutcome, DenyReason, Lease, Priority, Quota, RenewOutcome, ShouldRunVerdict, TaskId,
    TodoClaim, HANDOFF,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::{
        ClaimOutcome, DenyReason, Lease, PeerScheduler, Priority, Quota, RenewOutcome,
        ShouldRunVerdict, SimplePeerScheduler, TodoClaim,
    };
}
