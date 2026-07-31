//! 任务状态契约 — L0 共享的 Task 生命周期枚举(Task 3.10,ADR-033 扩展)
//!
//! 对应架构层: **L0 Contracts**(从 L1 `nexus-core` 上提,缓解 L1 上帝 crate)
//! 对应 ADR: **ADR-033**(L0 nexus-contracts 契约层建立,本模块为 Task 3.10 类型扩展)
//!
//! # 核心职责
//!
//! 承载 Task 节点的生命周期状态枚举(4 变体:Pending/Running/Completed/Failed)。
//! 原定义于 `nexus-core/src/types.rs`,被 65+ 文件依赖(L1 上帝 crate 病理),
//! 下沉到 L0 共享契约层,供 L1-L10 所有上层 crate 直接导入。
//!
//! # 设计约束(ADR-033)
//!
//! - **纯类型 + 零逻辑**: 仅枚举定义,不含业务逻辑
//! - **零外部依赖**(serde derive 例外): `TaskStatus` 是纯 enum,仅依赖 serde derive
//! - **向后兼容**: `nexus-core/src/types.rs` 保留 `pub use nexus_contracts::TaskStatus`
//!   re-export,65+ 文件现有 `use nexus_core::types::TaskStatus` 路径不破坏

use serde::{Deserialize, Serialize};

/// 任务状态 — Task 生命周期
///
/// WHY 6 变体(非 spec 原始 4 变体): 第三阶段深化需要完整生命周期表达,
/// Cancelled/Paused 是 Quest 级任务编排的必要状态(ADR-029 三入口统一协议)。
/// 新增变体不影响现有 4 变体序列化兼容性(serde enum 按名匹配,新增变体只增不解)。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    /// 待执行:尚未开始
    Pending,
    /// 执行中:已启动但未完成
    Running,
    /// 已完成:成功结束
    Completed,
    /// 已失败:执行出错或被中止
    Failed,
    /// 已取消:用户或编排器主动取消,不同于 Failed(非错误中止)
    Cancelled,
    /// 已暂停:执行被挂起,可恢复,不同于 Cancelled(可恢复 vs 不可恢复)
    Paused,
}
