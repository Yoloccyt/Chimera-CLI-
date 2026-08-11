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
        // Concord T1.3(INV-K-B 修复):'l' 键历史上被路由表硬编码派发但未声明,
        // 现补入 default_key 声明,键位以本声明为单一事实源(codegen 派生路由)。
        ActionDescriptor {
            is_core: true,
            default_key: Some("l"),
            ..ActionDescriptor::new(
                "view.switch_layout",
                ActionDomain::View,
                "action.view.switch_layout",
                Some("layout"),
            )
        },
        // 切换伴随面板(主区右侧并排渲染最近使用面板)— M2 增量3 Stage 1
        // Concord W3 T3.4:移除默认键 `\`(已复用为 Chat⇄Dashboard 互切);
        // 动作保留,改经命令面板(/companion)访问,零功能丢失。
        ActionDescriptor::new(
            "view.toggle_companion",
            ActionDomain::View,
            "action.view.toggle_companion",
            Some("companion"),
        ),
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
            // Enter 由焦点面板 handle_key 内部消费(InputRouter 路由为 FocusPanel),
            // 非全局路由键——双源不变量测试据此豁免(Concord T1.2)
            global_route: false,
            ..ActionDescriptor::new(
                "panel.drill_down",
                ActionDomain::View,
                "action.panel.drill_down",
                None,
            )
        },
    ]
}
