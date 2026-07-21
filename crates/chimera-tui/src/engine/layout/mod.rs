//! engine::layout — 自研 Flexbox 布局引擎(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface(自研渲染引擎 L4 布局层)
//!
//! # 模块职责
//! - `constraint`:`Constraint`/`Direction` + 一维 `solve`(和恒等于总长)
//! - `flex`:`split` —— 沿方向把区域二维无缝切分为子区域
//! - `node`:`LayoutTree`(arena)—— 动态/嵌套布局的递归区域计算
//! - `presets`:四种模式(IDE/Chat/Vim/Focus)+ 默认 2 栏 + 命令面板 overlay
//! - `engine`:`LayoutEngine` 门面 —— 持模式 + 视口,输出命名区域与 overlay
//!
//! # 两条使用路径
//! - **内建固定布局**:`presets`/`LayoutEngine` 用 `flex::split` 直接切分(最清晰)。
//! - **动态用户布局**:`node::LayoutTree` 以 arena 表达任意嵌套,共享 `split` 原语。
//!
//! # 迁移状态(M1.4)
//! 布局引擎默认编译供 CI 类型检查与测试;M2 起在 `v3-engine` 路径接线到 app.rs
//! 的区域计算(替换现有 ratatui `Layout::split`)。

pub mod constraint;
pub mod engine;
pub mod flex;
pub mod node;
pub mod presets;

pub use constraint::{solve, Constraint, Direction};
pub use engine::LayoutEngine;
pub use flex::split;
pub use node::{BoxNode, LayoutTree, NodeId};
pub use presets::{centered_overlay, regions_for, PaneMode, Regions};
