//! nexus-subagent — 类型化 SubAgent 运行时 + Task Auction 市场（P3-T9，v4.0 WI-25）
//!
//! 对应架构层: **L7 Execution**（ADR-148 裁决：D-P5 层归属定案——执行层，同一引擎换参数）
//! 对应任务: **P3-T9**（手册 W17-18，WI-25：3 类型 + Arena + 禁嵌套 + 竞价）
//!
//! # 设计（v4.0 WI-25 规格）
//! - 类型化 SubAgent（coder/explore/plan）= 同一执行引擎换参数
//!   （模型/工具集/权限上下文/worktree）;
//! - **禁嵌套**:[`NestedSubAgentForbidden`]（L0 契约）编译期 + 运行期双断言;
//! - Arena 竞争 + Task Auction（bid → `min_by(cost/match)` 择胜）;
//! - 与 mas-sched 分工:Claim 管长任务租约 / Auction 管短任务派发（WI-29）;
//! - 取消经 CancellationToken 四因传播（用户取消/超时/配额耗尽/父级撤销）。
//!
//! # 门禁（ADR-148）
//! Swarm 规模上限 8;3 类型并行 E2E;竞价抖动/饿死测试;嵌套禁止断言。

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod auction;
pub mod cancel;
pub mod runtime;
pub mod types;

pub use auction::{Bid, TaskAuction, TaskOffer};
pub use cancel::{CancelReason, CancellationToken};
pub use runtime::{SubAgentHandle, SubAgentRuntime};
pub use types::{SubAgentKind, SubAgentProfile, SubAgentSpec, SWARM_LIMIT};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::{
        Bid, CancelReason, CancellationToken, SubAgentHandle, SubAgentKind, SubAgentProfile,
        SubAgentRuntime, SubAgentSpec,
    };
}
