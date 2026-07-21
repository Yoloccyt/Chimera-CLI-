//! components — 自研引擎组件系统(ADR-029,v3.1 自研渲染引擎 L5)
//!
//! 对应架构层:L10 Interface
//!
//! # 模块职责
//! - `traits` 模块:`ComponentPanel` 声明式面板契约 + `ViewContext` + `LayoutNode`
//!
//! # 迁移路径
//! M2 增加 `builtins`(内建组件,替代 render.rs/viz widget)与 `adapter`
//! (`LegacyPanelAdapter` 包装旧 `panels::Panel`);逐面板从旧 trait 迁移到
//! `ComponentPanel`,M5 移除旧 trait 与适配层。

pub mod traits;

pub use traits::{ComponentPanel, LayoutNode, ViewContext};
