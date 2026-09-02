//! 生命周期事件 — 13+ 挂载点（P3-T3，v4.0 WI-24）
//!
//! 对应架构层: L9 Quest（nexus-hook，ADR-146）
//!
//! 13+ 事件覆盖:会话/任务/轮次/工具/请求/命令/错误 七类生命周期。
//! 命名空间「hook.」供事件双轨注册（WI-21 EventNamespace::Hook 联动,Phase 3 T10）。

/// 生命周期事件 — 13+ 挂载点（v4.0 WI-24）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LifecycleEvent {
    /// 会话开始
    SessionStart,
    /// 会话结束
    SessionEnd,
    /// Quest 开始
    QuestStart,
    /// Quest 结束
    QuestEnd,
    /// 轮次开始前（可中断）
    PreQuestTurn,
    /// 轮次结束后
    PostQuestTurn,
    /// 工具调用前（可中断:非零退出码拒否）
    PreToolUse,
    /// 工具调用后
    PostToolUse,
    /// 请求发出前（可中断）
    PreRequest,
    /// 请求完成后
    PostRequest,
    /// 命令执行前（可中断）
    PreCommand,
    /// 命令执行后
    PostCommand,
    /// 错误发生（可观测,不中断）
    Error,
    /// 停止裁决（WI-32 记分卡联动,可观测）
    Stop,
}

impl LifecycleEvent {
    /// 全部事件（诊断/注册遍历）
    pub const ALL: [LifecycleEvent; 14] = [
        LifecycleEvent::SessionStart,
        LifecycleEvent::SessionEnd,
        LifecycleEvent::QuestStart,
        LifecycleEvent::QuestEnd,
        LifecycleEvent::PreQuestTurn,
        LifecycleEvent::PostQuestTurn,
        LifecycleEvent::PreToolUse,
        LifecycleEvent::PostToolUse,
        LifecycleEvent::PreRequest,
        LifecycleEvent::PostRequest,
        LifecycleEvent::PreCommand,
        LifecycleEvent::PostCommand,
        LifecycleEvent::Error,
        LifecycleEvent::Stop,
    ];

    /// 可中断事件 — 非零退出码触发拒否（Pre 类）
    #[must_use]
    pub const fn interruptible(self) -> bool {
        matches!(
            self,
            LifecycleEvent::PreQuestTurn
                | LifecycleEvent::PreToolUse
                | LifecycleEvent::PreRequest
                | LifecycleEvent::PreCommand
        )
    }

    /// TOML 节名（配置挂载键）
    #[must_use]
    pub const fn toml_section(self) -> &'static str {
        match self {
            LifecycleEvent::SessionStart => "session_start",
            LifecycleEvent::SessionEnd => "session_end",
            LifecycleEvent::QuestStart => "quest_start",
            LifecycleEvent::QuestEnd => "quest_end",
            LifecycleEvent::PreQuestTurn => "pre_quest_turn",
            LifecycleEvent::PostQuestTurn => "post_quest_turn",
            LifecycleEvent::PreToolUse => "pre_tool_use",
            LifecycleEvent::PostToolUse => "post_tool_use",
            LifecycleEvent::PreRequest => "pre_request",
            LifecycleEvent::PostRequest => "post_request",
            LifecycleEvent::PreCommand => "pre_command",
            LifecycleEvent::PostCommand => "post_command",
            LifecycleEvent::Error => "error",
            LifecycleEvent::Stop => "stop",
        }
    }

    /// 事件命名空间前缀（WI-21 EventNamespace::Hook 联动）
    #[must_use]
    pub const fn namespace(self) -> &'static str {
        "hook."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 14 事件齐备且唯一（13+ 规格）
    #[test]
    fn all_events_unique() {
        let mut set = std::collections::HashSet::new();
        for e in LifecycleEvent::ALL {
            assert!(set.insert(e), "事件重复: {e:?}");
        }
        assert_eq!(set.len(), 14, "13+ 规格:至少 13 个,实际 14");
    }

    /// 可中断事件分类 — 仅 Pre 类
    #[test]
    fn interruptible_classification() {
        assert!(LifecycleEvent::PreToolUse.interruptible());
        assert!(LifecycleEvent::PreQuestTurn.interruptible());
        assert!(LifecycleEvent::PreRequest.interruptible());
        assert!(LifecycleEvent::PreCommand.interruptible());
        assert!(!LifecycleEvent::PostToolUse.interruptible());
        assert!(!LifecycleEvent::SessionStart.interruptible());
        assert!(!LifecycleEvent::Error.interruptible());
    }

    /// TOML 节名 — 全部唯一且 snake_case
    #[test]
    fn toml_sections_unique() {
        let mut set = std::collections::HashSet::new();
        for e in LifecycleEvent::ALL {
            assert!(
                set.insert(e.toml_section()),
                "节名重复: {}",
                e.toml_section()
            );
        }
        assert_eq!(set.len(), 14);
    }

    /// 序列化往返 — serde 可编解码（配置持久化）
    #[test]
    fn serde_roundtrip() {
        for e in LifecycleEvent::ALL {
            let json = serde_json::to_string(&e).expect("编码必须成功");
            let back: LifecycleEvent = serde_json::from_str(&json).expect("解码必须成功");
            assert_eq!(back, e);
        }
    }
}
