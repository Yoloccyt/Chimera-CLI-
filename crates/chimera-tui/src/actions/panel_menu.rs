//! panel_menu — 面板上下文动作菜单的动作来源(ADR-029,v3.1 §4.5)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **精选静态映射,非改 Panel trait**:每个 `PanelId` 显式声明其上下文动作,
//!   单一可审源、零 per-panel 样板,不触动 20 个面板的旧 `Panel` trait。
//!   (M5 面板迁移到 `ComponentPanel` 后可由 `actions()` 取代本映射。)
//! - **无只读死面板铁律(§2.3)**:每个面板末尾统一追加 `panel.drill_down`,
//!   保证任意面板至少暴露一个交互动作(下钻查看详情)。
//! - **动作须存在于 ActionRegistry**:返回的 id 均为已注册 action_id,菜单展示
//!   时经 Registry 取 i18n 标题,派发经 `dispatch_action` 统一执行(三入口一致)。

use crate::types::PanelId;

/// 通用动作 — 每个面板都暴露(无只读死面板铁律)
const UNIVERSAL: &str = "panel.drill_down";

/// 返回指定面板的上下文动作 id 列表(末尾恒含 `panel.drill_down`)
///
/// 功能面板追加其域动作;纯展示面板仅有通用下钻。返回顺序即菜单展示顺序
/// (域动作在前,通用下钻在后)。返回的 id 均须存在于 `ActionRegistry`
/// (由单测 `all_menu_actions_exist_in_registry` 守护)。
pub fn panel_context_actions(panel: PanelId) -> Vec<&'static str> {
    let mut actions: Vec<&'static str> = match panel {
        // Quest 面板:Quest 生命周期控制(暂停/恢复/取消;不含需文本输入的 chat/start)
        PanelId::Quest => vec!["quest.pause", "quest.resume", "quest.cancel"],
        // 资源监控 / 系统信息:采样控制与统计时间窗
        PanelId::ResourceMonitor | PanelId::Sysinfo => {
            vec!["monitor.pause_sampling", "monitor.time_window"]
        }
        // 可视化面板:维度切换
        PanelId::OsaSparse | PanelId::ClvVector | PanelId::MetricsDashboard => {
            vec!["viz.switch_dimension"]
        }
        // 其余为展示型面板:仅通用下钻
        _ => Vec::new(),
    };
    actions.push(UNIVERSAL);
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::ActionRegistry;

    /// 全部 21 个 PanelId(与 types.rs 保持同步)
    const ALL_PANELS: [PanelId; 21] = [
        PanelId::Quest,
        PanelId::Parliament,
        PanelId::Budget,
        PanelId::Memory,
        PanelId::Security,
        PanelId::Health,
        PanelId::Log,
        PanelId::Help,
        PanelId::Decay,
        PanelId::EventStream,
        PanelId::Router,
        PanelId::McpNodes,
        PanelId::Chtc,
        PanelId::Timeline,
        PanelId::OsaSparse,
        PanelId::ClvVector,
        PanelId::ResourceMonitor,
        PanelId::MetricsDashboard,
        PanelId::Sysinfo,
        PanelId::Chat,
        // polish-v2.7 P1-5:自评仪表盘面板
        PanelId::SelfAssessment,
    ];

    #[test]
    fn every_panel_has_at_least_drill_down() {
        // 无只读死面板:每个 PanelId 至少含 panel.drill_down
        for p in ALL_PANELS {
            let acts = panel_context_actions(p);
            assert!(!acts.is_empty(), "{p:?} 应至少含一个动作");
            assert!(
                acts.contains(&UNIVERSAL),
                "{p:?} 应含通用下钻 panel.drill_down"
            );
        }
    }

    #[test]
    fn functional_panels_expose_domain_actions() {
        assert!(panel_context_actions(PanelId::Quest).contains(&"quest.pause"));
        assert!(panel_context_actions(PanelId::ResourceMonitor).contains(&"monitor.pause_sampling"));
        assert!(panel_context_actions(PanelId::OsaSparse).contains(&"viz.switch_dimension"));
    }

    #[test]
    fn display_panel_has_only_drill_down() {
        // 纯展示面板(如 Budget)仅暴露通用下钻
        assert_eq!(panel_context_actions(PanelId::Budget), vec![UNIVERSAL]);
    }

    #[test]
    fn all_menu_actions_exist_in_registry() {
        // 菜单动作 id 必须都在 Registry:展示取 i18n 标题、派发经 dispatch 均依赖此
        let reg = ActionRegistry::with_builtin_domains();
        for p in ALL_PANELS {
            for id in panel_context_actions(p) {
                assert!(
                    reg.get(id).is_some(),
                    "菜单动作 {id} 应存在于 ActionRegistry"
                );
            }
        }
    }
}
