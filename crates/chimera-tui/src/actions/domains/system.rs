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
        // 超窗兜底检索(P1,ADR-072):经 TuiActionRequested → Action 编排器 → OverWindowBridge
        // 真实执行两级检索(kvbsr→repo-wiki→hcw)。需 query 参数(命令栏 `:overwindow run <词>`),
        // palette 选中后进入参数输入态(F-5),提交以 {"query": text} 派发。
        ActionDescriptor {
            requires_query: true,
            ..ActionDescriptor::new(
                "overwindow.run",
                ActionDomain::System,
                "action.overwindow.run",
                Some("overwindow run"),
            )
        },
    ]
}
