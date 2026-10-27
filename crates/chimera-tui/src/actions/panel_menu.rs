//! panel_menu — 面板上下文动作菜单的动作来源(ADR-029,v3.1 §4.5)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **精选静态映射,非改 Panel trait**:每个 `PanelId` 显式声明其上下文动作,
//!   单一可审源、零 per-panel 样板,不触动既有面板的旧 `Panel` trait。
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
        PanelId::ResourceMonitor => vec!["monitor.pause_sampling", "monitor.time_window"],
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::actions::ActionRegistry;

    /// 菜单覆盖的全集 —— **直接派生自 `PanelId::ALL`**（C-80 / M60）。
    ///
    /// WHY 不再手抄一份定长清单：本文件原先写死 `const ALL_PANELS: [PanelId; 27]`，
    /// 而实际枚举是 30，漏了 `Bench`/`ReviewQueue`/`Blackboard` —— 于是四个
    /// “守卫”测试只遍历 27 个面板（自循环断言），覆盖静默降到 27/30，
    /// 而注释还声称“新增变体时会编译失败”（假的：定长数组不会因枚举增变体而报错）。
    /// 改为派生后，新变体只要进 `PanelId::ALL`（它自带 30 的锁）就自动被本套测试覆盖。
    const ALL_PANELS: &[PanelId] = PanelId::ALL;

    /// 菜单覆盖必须等于枚举全集，且不得有重复项。
    ///
    /// WHY 两条都要：`ALL` 是切片而非集合，误加一个重复项会让某个变体
    /// 静默不被测到（覆盖又降），而长度断言看不出来。
    #[test]
    fn menu_coverage_equals_panel_enum() {
        use std::collections::HashSet;
        let uniq: HashSet<PanelId> = ALL_PANELS.iter().copied().collect();
        assert_eq!(
            uniq.len(),
            ALL_PANELS.len(),
            "PanelId::ALL 不应含重复项（重复会让菜单覆盖静默缩水）"
        );
        // 与 types.rs 的 30 锁互证：两处一旦分叉，这里先红
        assert_eq!(uniq.len(), 30, "菜单覆盖应等于 PanelId 枚举全集");
    }

    #[test]
    fn every_panel_has_at_least_drill_down() {
        // 无只读死面板:每个 PanelId 至少含 panel.drill_down
        for &p in ALL_PANELS {
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
        for &p in ALL_PANELS {
            for id in panel_context_actions(p) {
                assert!(
                    reg.get(id).is_some(),
                    "菜单动作 {id} 应存在于 ActionRegistry"
                );
            }
        }
    }
}
