//! 系统域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 System 域的全部动作描述
///
/// WHY 系统域聚合全局性交互:中英切换、上下文帮助、监控采样控制、可视化维度
/// 切换。这些动作跨面板生效,不绑定单一业务域,故统一归入 System。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![
        // 中英切换 — 核心功能,Ctrl+L 全局快捷键
        ActionDescriptor {
            is_core: true,
            default_key: Some("Ctrl+L"),
            ..ActionDescriptor::new(
                "system.toggle_locale",
                ActionDomain::System,
                "action.system.toggle_locale",
                Some("lang"),
            )
        },
        // 上下文帮助 — ? 键,按当前焦点面板动态生成
        ActionDescriptor {
            default_key: Some("?"),
            ..ActionDescriptor::new(
                "system.open_help",
                ActionDomain::System,
                "action.system.open_help",
                Some("help"),
            )
        },
        // 暂停/恢复监控采样 — 监控面板 Space 触发
        ActionDescriptor {
            requires_context: true,
            ..ActionDescriptor::new(
                "monitor.pause_sampling",
                ActionDomain::System,
                "action.monitor.pause_sampling",
                Some("monitor pause"),
            )
        },
        // 切换统计时间窗
        ActionDescriptor {
            requires_context: true,
            ..ActionDescriptor::new(
                "monitor.time_window",
                ActionDomain::System,
                "action.monitor.time_window",
                Some("monitor win"),
            )
        },
        // 可视化维度切换
        ActionDescriptor {
            requires_context: true,
            ..ActionDescriptor::new(
                "viz.switch_dimension",
                ActionDomain::System,
                "action.viz.switch_dimension",
                Some("viz dim"),
            )
        },
    ]
}
