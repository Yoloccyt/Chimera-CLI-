//! 动作域注册 — 按功能职责分包(ADR-029,v3.1 §4.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **分域注册防上帝对象**:每个域一个文件,导出 `descriptors()` 返回该域的
//!   `ActionDescriptor` 列表;`ActionRegistry::with_builtin_domains` 聚合六域。
//! - **新增 Action 落到对应域文件**:评审聚焦单域,`ActionRegistry` 本身零改动。

use crate::actions::descriptor::ActionDescriptor;

pub mod config;
pub mod export;
pub mod quest;
pub mod system;
pub mod task;
pub mod view;

/// 聚合全部内建域的动作描述(注册表构造时调用)
///
/// WHY 集中聚合:`ActionRegistry` 通过本函数一次性纳入六域,新增域只需
/// 在此追加一行,不改动注册表内部逻辑。
pub fn all_builtin_descriptors() -> Vec<ActionDescriptor> {
    let mut out = Vec::new();
    out.extend(quest::descriptors());
    out.extend(task::descriptors());
    out.extend(export::descriptors());
    out.extend(view::descriptors());
    out.extend(system::descriptors());
    out.extend(config::descriptors());
    out
}
