//! 视图与下钻域动作(ADR-029,v3.1)
//!
//! 对应架构层:L10 Interface

use crate::actions::descriptor::{ActionDescriptor, ActionDomain};

/// 返回 View 域的全部动作描述
///
/// WHY 视图域承载"一屏一事"的呈现控制:布局模式切换、保存视图应用、
/// 面板下钻(Enter 进入 Focus 全屏详情),对应 §4.6 三级信息层级的 L3。
pub fn descriptors() -> Vec<ActionDescriptor> {
    vec![
        // 切换布局模式(IDE/Chat/Vim/Focus)— 核心功能
        ActionDescriptor {
            is_core: true,
            ..ActionDescriptor::new(
                "view.switch_layout",
                ActionDomain::View,
                "action.view.switch_layout",
                Some("layout"),
            )
        },
        // 切换伴随面板(主区右侧并排渲染最近使用面板)— M2 增量3 Stage 1
        ActionDescriptor {
            default_key: Some("\\"),
            ..ActionDescriptor::new(
                "view.toggle_companion",
                ActionDomain::View,
                "action.view.toggle_companion",
                Some("companion"),
            )
        },
        // 循环绑定伴随面板— M2 增量3 Stage 2
        ActionDescriptor {
            default_key: Some("]"),
            ..ActionDescriptor::new(
                "view.cycle_companion",
                ActionDomain::View,
                "action.view.cycle_companion",
                Some("companion-cycle"),
            )
        },
        // 切换主/伴随窗格焦点— M2 增量3 Stage 2
        ActionDescriptor {
            default_key: Some("w"),
            ..ActionDescriptor::new(
                "view.focus_pane",
                ActionDomain::View,
                "action.view.focus_pane",
                Some("focus-pane"),
            )
        },
        // 应用已保存的视图(TuiBible.views)
        ActionDescriptor::new(
            "view.apply_saved",
            ActionDomain::View,
            "action.view.apply_saved",
            Some("view"),
        ),
        // 面板下钻查看详情 — 核心功能,焦点面板 Enter 触发(需上下文)
        ActionDescriptor {
            is_core: true,
            requires_context: true,
            default_key: Some("Enter"),
            ..ActionDescriptor::new(
                "panel.drill_down",
                ActionDomain::View,
                "action.panel.drill_down",
                None,
            )
        },
    ]
}
