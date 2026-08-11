//! actions — TUI 统一 Action 层(ADR-029,v3.1 §4.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 模块职责
//! - `descriptor` 模块:`ActionDescriptor` / `ActionDomain` — 单个动作的元描述
//! - `registry` 模块:`ActionRegistry` — 单一事实源,索引 + 查询 + 模糊匹配
//! - `domains` 模块:六个功能域的动作声明(quest/task/export/view/system/config)
//! - `codegen` 模块:从注册表生成斜杠命令 / 命令面板条目 / 帮助内容(防漂移)
//! - `slash_registry` 模块:斜杠命令独立注册表(Concord 重构 T1.1,R9 解法:
//!   命令表不占 ActionRegistry 40 项预算;三分层 tier 供 W2 解析器分流)
//!
//! # 三入口一致性(§2.3 铁律)
//! Chat 斜杠命令 / 命令面板 / 面板上下文动作三入口共享本层,同一 Action 无论
//! 从哪个入口触发,行为与结果完全一致——由 `ActionRegistry` 单源保证。
//! 派发统一经 `NexusEvent::TuiActionRequested`(见 event-bus),编排在 chimera-cli。

pub mod codegen;
pub mod descriptor;
pub mod domains;
pub mod panel_menu;
pub mod registry;
pub mod slash_parser;
pub mod slash_registry;

pub use codegen::{HelpLine, PaletteEntry, SlashCommand};
pub use descriptor::{ActionDescriptor, ActionDomain};
pub use panel_menu::panel_context_actions;
pub use registry::{ActionRegistry, MAX_ACTIONS};
pub use slash_parser::{
    parse as parse_slash, plan as plan_slash, DispatchPlan, ParsedSlash, SlashEffect,
};
pub use slash_registry::{SlashCommandDesc, SlashCommandRegistry, SlashDomain, SlashTier};
