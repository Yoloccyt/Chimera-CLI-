//! FocusManager 集成测试 — 验证面板焦点导航

#![forbid(unsafe_code)]

use chimera_tui::{FocusManager, PanelId};

#[test]
fn focus_manager_starts_at_first_panel() {
    let mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget]);
    assert_eq!(mgr.focused(), PanelId::Quest);
}

#[test]
fn focus_manager_next_cycles_through_panels() {
    let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget]);

    mgr.next();
    assert_eq!(mgr.focused(), PanelId::Parliament);

    mgr.next();
    assert_eq!(mgr.focused(), PanelId::Budget);

    mgr.next();
    assert_eq!(mgr.focused(), PanelId::Quest);
}

#[test]
fn focus_manager_prev_cycles_backwards() {
    let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget]);

    mgr.prev();
    assert_eq!(mgr.focused(), PanelId::Budget);

    mgr.prev();
    assert_eq!(mgr.focused(), PanelId::Parliament);

    mgr.prev();
    assert_eq!(mgr.focused(), PanelId::Quest);
}

#[test]
fn focus_manager_jump_to_switches_focus() {
    let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Budget]);

    assert!(mgr.jump_to(PanelId::Budget));
    assert_eq!(mgr.focused(), PanelId::Budget);
}

#[test]
fn focus_manager_jump_to_unknown_returns_false() {
    let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Budget]);

    assert!(!mgr.jump_to(PanelId::Help));
    assert_eq!(mgr.focused(), PanelId::Quest);
}

#[test]
fn focus_manager_preserves_panel_order() {
    let panels = vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget];
    let mgr = FocusManager::new(panels.clone());
    assert_eq!(mgr.panels(), panels.as_slice());
}
