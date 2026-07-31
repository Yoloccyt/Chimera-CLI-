//! Task 2.5:Panel shortcuts 自动生成集成测试
//!
//! 验证 `panels::shortcuts_from_domain` helper 与 `Panel::shortcuts_with_registry`
//! 默认实现能从 ActionRegistry 按 domain 自动派生快捷键,新增面板无需手写 shortcuts。
//!
//! 对应架构层:L10 Interface(`chimera-tui`)

#![forbid(unsafe_code)]

use chimera_tui::{panels, ActionDescriptor, ActionDomain, ActionRegistry, Panel, QuestPanel};

/// 构造测试用 action 描述符(简化样板)
fn make_action(
    id: &'static str,
    domain: ActionDomain,
    key: Option<&'static str>,
) -> ActionDescriptor {
    ActionDescriptor {
        id,
        domain,
        title_key: id,
        desc_key: id,
        slash: None,
        default_key: key,
        requires_context: false,
        is_core: false,
    }
}

/// 注册 3 个 action 到同一 domain,断言 shortcuts_from_domain 返回 3 个元素
#[test]
fn test_shortcuts_from_domain_returns_expected() {
    let mut reg = ActionRegistry::new();
    assert!(reg.register(make_action("test.a", ActionDomain::Task, Some("Ctrl+A"),)));
    assert!(reg.register(make_action("test.b", ActionDomain::Task, Some("Ctrl+B"),)));
    assert!(reg.register(make_action("test.c", ActionDomain::Task, None,)));

    let shortcuts = panels::shortcuts_from_domain(&reg, ActionDomain::Task);
    assert_eq!(shortcuts.len(), 3, "应返回 3 个 Task 域 action 的快捷键");

    // 验证 key 选择规则:default_key 优先,无 default_key 回退到 id
    assert_eq!(shortcuts[0].0, "Ctrl+A");
    assert_eq!(shortcuts[1].0, "Ctrl+B");
    assert_eq!(
        shortcuts[2].0, "test.c",
        "无 default_key 时应回退到 action id"
    );

    // title_key 应保留为 action 的 title_key
    assert_eq!(shortcuts[0].1, "test.a");
}

/// QuestPanel 的 shortcuts_with_registry 应包含 Quest 域的自动派生 action
#[test]
fn test_quest_panel_uses_auto_shortcuts() {
    let registry = ActionRegistry::with_builtin_domains();
    let panel = QuestPanel::new();

    // shortcuts() 仅返回手写 UI 键位(4 条:导航/详情/跳顶/跳底)
    let ui_only = panel.shortcuts();
    assert_eq!(ui_only.len(), 4, "QuestPanel 手写 UI 键位应为 4 条");

    // shortcuts_with_registry 应合并 UI 键位 + Quest 域 action(6 条)
    let merged = panel.shortcuts_with_registry(&registry);
    assert!(
        merged.len() > ui_only.len(),
        "shortcuts_with_registry 应比 shortcuts 多出 Quest 域 action,{} > {}",
        merged.len(),
        ui_only.len()
    );

    // 验证 Quest 域 action 出现在合并结果中(如 quest.pause 的斜杠词 "quest pause")
    let has_quest_action = merged
        .iter()
        .any(|(key, _)| *key == "quest pause" || *key == "agent.chat");
    assert!(
        has_quest_action,
        "合并后的 shortcuts 应包含 Quest 域 action(agent.chat / quest.pause 等)"
    );

    // 验证 action_domain 声明正确
    assert_eq!(
        panel.action_domain(),
        Some(ActionDomain::Quest),
        "QuestPanel 应声明 Quest 域"
    );
}

/// 空 registry + 任意 domain 返回空 Vec(未知/未注册 domain 场景)
#[test]
fn test_unknown_domain_returns_empty() {
    let empty_reg = ActionRegistry::new();
    // 空 registry 对任何 domain 都应返回空 Vec
    let shortcuts = panels::shortcuts_from_domain(&empty_reg, ActionDomain::Task);
    assert!(shortcuts.is_empty(), "空 registry 应返回空 shortcuts");

    // 验证 with_builtin_domains 但查询未注册的 domain 组合(Task 已注册,但验证空 reg 兜底)
    let builtin_reg = ActionRegistry::with_builtin_domains();
    // Task 域在内建注册表中存在 action,应返回非空
    let task_shortcuts = panels::shortcuts_from_domain(&builtin_reg, ActionDomain::Task);
    assert!(!task_shortcuts.is_empty(), "Task 域应有内建 action");
}

/// 验证默认 action_domain() 返回 None 的面板(如 HelpPanel)shortcuts_with_registry 回退到 shortcuts()
#[test]
fn test_panel_without_domain_falls_back_to_shortcuts() {
    use chimera_tui::HelpPanel;

    let registry = ActionRegistry::with_builtin_domains();
    let panel = HelpPanel::new();

    // HelpPanel 未覆写 action_domain,默认 None
    assert_eq!(panel.action_domain(), None);

    // shortcuts_with_registry 应等于 shortcuts()(无自动派生)
    let with_reg = panel.shortcuts_with_registry(&registry);
    let without_reg = panel.shortcuts();
    assert_eq!(
        with_reg.len(),
        without_reg.len(),
        "无 action_domain 的面板应回退到 shortcuts(),不追加自动派生"
    );
}
