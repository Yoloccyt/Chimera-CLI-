//! engine — 自研渲染引擎(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 分层(对应 AETHER TUI v3.0 §1.2)
//! - L2 边界(`writer`):`TerminalWriter` 经 crossterm 输出 ANSI(保留终端后端)
//! - L3 渲染(`buffer` / `diff` / `style`):双缓冲 + 逐格差异 + 样式池
//! - L4 布局(`layout`):Flexbox 约束求解 + 四模式预设 + 命令面板 overlay
//! - 兼容桥接(`compat`):ratatui Buffer ↔ 自研 Buffer 边界翻译(M1,渐进迁移)
//!
//! # 设计边界(WHY 不重写 L1/L2)
//! `#![forbid(unsafe_code)]` 铁律下,raw mode / TTY / ANSI 跨平台差异由 crossterm
//! 承担;自研范围是 L3 双缓冲差异渲染与 L4 布局(纯 safe Rust)。这是"自研渲染
//! 引擎从零重建"在安全铁律下的正确解读——重写重绘策略而非终端 IO。
//!
//! # 渐进迁移(v3-engine feature)
//! M0-M1 引擎默认编译供 CI 类型检查;M2 起在 `v3-engine` 开启后经 `compat` 桥接
//! 逐面板切换到本引擎,M5 移除 ratatui 与兼容层,引擎成为唯一渲染路径。

pub mod buffer;
pub mod compat;
pub mod diff;
pub mod layout;
pub mod output;
pub mod rect;
pub mod style;
pub mod writer;

pub use buffer::{Buffer, Cell, DirtyTracker, DoubleBuffer};
pub use compat::{
    from_ratatui_buffer, from_ratatui_buffer_diffed, from_ratatui_rect, to_ratatui_rect,
};
pub use diff::{Change, DiffEngine};
pub use layout::{Constraint, Direction, LayoutEngine, LayoutTree, PaneMode, Regions};
pub use rect::{Position, Rect, Size};
pub use style::{Color, Modifier, Style, StylePool};
pub use writer::TerminalWriter;
