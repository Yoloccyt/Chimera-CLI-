//! Chat 会话 — 用户交互会话状态(会话 ID + 命令面板 overlay)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! Task 1.15 拆分:原 TuiApp 持有 chat_session_id / palette 两字段,集中到
//! ChatSession 便于单一职责(用户交互会话)与后续扩展(如多会话、命令历史)。

use crate::command_palette::CommandPaletteModel;

/// Chat 会话 — 持有会话标识与命令面板 overlay 状态
///
/// `chat_session_id` 随 `TuiChatSubmitted` 发布,供 M3c 编排器多轮关联;
/// `palette` 是 Ctrl+P 唤起的统一命令面板 overlay(M2.2 用户北极星)。
#[derive(Debug)]
pub struct ChatSession {
    /// 当前 Chat 会话标识(M3b,uuid v7 时间有序,构造时生成)
    ///
    /// WHY TuiApp 持有:Submit 时随 `TuiChatSubmitted` 发布,供 M3c 编排器多轮关联;
    /// 单会话生命周期与 TuiApp 一致(`/clear` 重置留后续)。
    pub chat_session_id: String,
    /// 统一命令面板 overlay 状态(M2.2,用户北极星)
    ///
    /// WHY `Option` 表达开关:`Some` = 面板已打开(键盘路由与渲染都据此分流),
    /// `None` = 关闭。模型自持 `ActionRegistry` 副本,复用同一实例避免每次
    /// 打开都重建注册表;候选项经 `codegen::palette_entries` 与斜杠命令/帮助同源。
    pub palette: Option<CommandPaletteModel>,
}

impl ChatSession {
    /// 创建 ChatSession(生成新的 uuid v7 会话 ID)
    pub fn new() -> Self {
        Self {
            chat_session_id: uuid::Uuid::now_v7().to_string(),
            palette: None,
        }
    }
}

impl Default for ChatSession {
    fn default() -> Self {
        Self::new()
    }
}
