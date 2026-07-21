//! 配置域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 Config 域的全部动作描述
///
/// WHY 配置编辑独立成域:TuiBible 配置项(主题/键位/阈值/布局)的交互编辑
/// 是可达性 100% 的最后一块——原本只能改配置文件的能力,经此 Action 进入 TUI。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![ActionDescriptor::new(
        "config.edit",
        ActionDomain::Config,
        "action.config.edit",
        Some("config"),
    )]
}
