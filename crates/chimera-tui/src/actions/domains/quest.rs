//! Quest 与 Agent 对话域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 Quest 域的全部动作描述
///
/// WHY agent.chat 归入 Quest 域:对话是驱动 Quest 的最高频入口,与 Quest
/// 生命周期控制同属"任务推进"语义域,统一管理便于三入口一致派发。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![
        // Agent 对话 — 核心功能,斜杠 /chat,亦经 i 键进入 Insert 触发
        ActionDescriptor {
            is_core: true,
            // F-5:需用户输入 query(palette 选中后进入参数输入态)
            requires_query: true,
            ..ActionDescriptor::new(
                "agent.chat",
                ActionDomain::Quest,
                "action.agent.chat",
                Some("chat"),
            )
        },
        // Quest 启动 — 需用户输入 query(与 agent.chat 同语义,F-5)
        ActionDescriptor {
            requires_query: true,
            ..ActionDescriptor::new(
                "quest.start",
                ActionDomain::Quest,
                "action.quest.start",
                Some("quest start"),
            )
        },
        // Quest 暂停 — 核心功能(Quest 启停)
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "quest.pause",
                ActionDomain::Quest,
                "action.quest.pause",
                Some("quest pause"),
            )
        },
        // Quest 恢复 — 核心功能
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            ..ActionDescriptor::new(
                "quest.resume",
                ActionDomain::Quest,
                "action.quest.resume",
                Some("quest resume"),
            )
        },
        // Quest 取消 — 破坏性,需上下文(焦点 Quest)
        ActionDescriptor {
            requires_context: true,
            ..ActionDescriptor::new(
                "quest.cancel",
                ActionDomain::Quest,
                "action.quest.cancel",
                Some("quest cancel"),
            )
        },
        // Quest 事件跳转
        ActionDescriptor {
            requires_context: true,
            ..ActionDescriptor::new(
                "quest.jump",
                ActionDomain::Quest,
                "action.quest.jump",
                Some("quest jump"),
            )
        },
    ]
}
