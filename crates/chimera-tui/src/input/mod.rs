//! input — TUI 输入路由(ADR-029,v3.1 §4.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 模块职责
//! - `router` 模块:`InputRouter` 三态路由状态机 + `RouteTarget` 路由目标枚举。
//!   决定每个按键在 Normal/Insert/Command 模式下应交由谁处理。
//!
//! # 与既有输入处理的关系
//! M0 提供路由骨架与三态路由表(D 类快照测试覆盖);M2 正式接线,替换
//! `app.rs` 内联的按键分发逻辑,是 `app.rs` 主循环拆分(EventLoop/Renderer/
//! InputRouter)的第一步。

pub mod router;

pub use router::{InputRouter, PaneDir, RouteTarget, RouterMode};
