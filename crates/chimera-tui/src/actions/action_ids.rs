//! action_ids — 编排域动作 ID 单一事实源(B3,2026-09-06 复评)
//!
//! 对应架构层:L10 Interface
//!
//! # WHY 独立常量模块
//! 编排域动作 ID 在三处被字面量引用:①`domains/*.rs` 的 ActionDescriptor
//! 注册;②chimera-tui 派发层(dispatch 本地臂 / 斜杠计划);③chimera-cli
//! `action_orchestrator::route_action` 的域路由。任何一侧重命名都会静默
//! 断链(评估报告 I-F)。本模块是唯一的字符串事实源;ID 值一经发布即为
//! 协议契约(进入 NexusEvent::TuiActionRequested.payload 之外的动作标识),
//! 重命名 = 破坏性变更,需走 ADR。

/// Agent 对话(编排域:经 QueryLoop 生成回复)
pub const AGENT_CHAT: &str = "agent.chat";
/// 创建 Quest
pub const QUEST_START: &str = "quest.start";
/// 暂停 Quest
pub const QUEST_PAUSE: &str = "quest.pause";
/// 恢复 Quest
pub const QUEST_RESUME: &str = "quest.resume";
/// 取消 Quest
pub const QUEST_CANCEL: &str = "quest.cancel";
/// Quest 事件跳转(本地即时臂)
pub const QUEST_JUMP: &str = "quest.jump";
/// 保存 Quest 检查点(B1 接线:engine.save_checkpoint 真实能力)
pub const QUEST_CHECKPOINT: &str = "quest.checkpoint";
/// 超窗兜底检索(ADR-072)
pub const OVERWINDOW_RUN: &str = "overwindow.run";
/// `/compact` 上下文策展(ADR-081)
pub const COMPACT: &str = "compact";

/// 向后兼容别名:FC-2 批次先以 `COMPACT_ACTION_ID` 落地,保留导出路径
pub const COMPACT_ACTION_ID: &str = COMPACT;

#[cfg(test)]
mod tests {
    use super::*;

    /// ID 值锁定测试:重命名常量名自由,但字符串值 = 协议契约,不可漂移
    #[test]
    fn action_id_values_are_protocol_frozen() {
        assert_eq!(AGENT_CHAT, "agent.chat");
        assert_eq!(QUEST_START, "quest.start");
        assert_eq!(QUEST_PAUSE, "quest.pause");
        assert_eq!(QUEST_RESUME, "quest.resume");
        assert_eq!(QUEST_CANCEL, "quest.cancel");
        assert_eq!(QUEST_CHECKPOINT, "quest.checkpoint");
        assert_eq!(OVERWINDOW_RUN, "overwindow.run");
        assert_eq!(COMPACT, "compact");
        assert_eq!(COMPACT_ACTION_ID, COMPACT);
    }
}
