//! actions — TUI 统一 Action 层(ADR-029,v3.1 §4.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 模块职责
//! - `descriptor` 模块:`ActionDescriptor` / `ActionDomain` — 单个动作的元描述
//! - `registry` 模块:`ActionRegistry` — 单一事实源,索引 + 查询 + 模糊匹配
//! - `domains` 模块:六个功能域的动作声明(quest/task/export/view/system/config)
//! - `codegen` 模块:从注册表生成斜杠命令 / 命令面板条目 / 帮助内容(防漂移)
//!
//! # 三入口一致性(§2.3 铁律)
//! Chat 斜杠命令 / 命令面板 / 面板上下文动作三入口共享本层,同一 Action 无论
//! 从哪个入口触发,行为与结果完全一致——由 `ActionRegistry` 单源保证。
//! 派发统一经 `NexusEvent::TuiActionRequested`(见 event-bus),编排在 chimera-cli。

pub mod codegen;
pub mod descriptor;
pub mod domains;
pub mod registry;

pub use codegen::{HelpLine, PaletteEntry, SlashCommand};
pub use descriptor::{ActionDescriptor, ActionDomain};
pub use registry::{ActionRegistry, MAX_ACTIONS};
