//! 任务生命周期域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 Task 域的全部动作描述
///
/// WHY 任务域独立于 Quest:Quest 是长期任务编排,Task 是可交互调度单元
/// (创建/暂停/恢复/取消/优先级),对应任务管理与调度中心面板的操作集。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![
        // 创建任务 — 核心功能
        ActionDescriptor {
            is_core: true,
            ..ActionDescriptor::new(
                "task.create",
                ActionDomain::Task,
                "action.task.create",
                Some("task new"),
            )
        },
        // 暂停任务 — 核心功能,需焦点任务上下文
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "task.pause",
                ActionDomain::Task,
                "action.task.pause",
                Some("task pause"),
            )
        },
        // 恢复任务 — 核心功能
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "task.resume",
                ActionDomain::Task,
                "action.task.resume",
                Some("task resume"),
            )
        },
        // 取消任务 — 核心功能
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "task.cancel",
                ActionDomain::Task,
                "action.task.cancel",
                Some("task cancel"),
            )
        },
        // 调整优先级(P0–P3)— 核心功能
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "task.set_priority",
                ActionDomain::Task,
                "action.task.set_priority",
                Some("task pri"),
            )
        },
    ]
}
