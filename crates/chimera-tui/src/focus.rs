//! TUI 焦点管理 — 维护面板顺序与当前焦点
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 将"面板顺序"与"当前焦点"抽离成独立结构,`TuiApp` 无需硬编码切换逻辑。
//! - `jump_to` 返回 bool:调用者可据此决定是否消费该按键事件。

use crate::types::PanelId;

/// 焦点管理器 — 跟踪有序面板列表中的当前焦点
#[derive(Debug, Clone, PartialEq)]
pub struct FocusManager {
    /// 有序面板列表
    panels: Vec<PanelId>,
    /// 当前焦点在 `panels` 中的索引
    current: usize,
}

impl FocusManager {
    /// 创建焦点管理器
    ///
    /// # Panics
    /// 当 `panels` 为空时 panic;TUI 至少应包含一个面板。
    pub fn new(panels: Vec<PanelId>) -> Self {
        assert!(
            !panels.is_empty(),
            "FocusManager requires at least one panel"
        );
        Self { panels, current: 0 }
    }

    /// 返回当前焦点面板
    pub fn focused(&self) -> PanelId {
        // current 始终位于 panels 范围内(由 next/prev/jump_to 维护)。
        self.panels[self.current]
    }

    /// 焦点切换到下一个面板(循环)
    pub fn next(&mut self) {
        self.current = (self.current + 1) % self.panels.len();
    }

    /// 焦点切换到上一个面板(循环)
    pub fn prev(&mut self) {
        self.current = if self.current == 0 {
            self.panels.len() - 1
        } else {
            self.current - 1
        };
    }

    /// 跳转到指定面板
    ///
    /// 返回 `true` 表示跳转成功;若目标不在面板列表中则返回 `false`。
    pub fn jump_to(&mut self, panel: PanelId) -> bool {
        if let Some(idx) = self.panels.iter().position(|&p| p == panel) {
            self.current = idx;
            true
        } else {
            false
        }
    }

    /// 返回当前有序面板列表
    pub fn panels(&self) -> &[PanelId] {
        &self.panels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_manager_new() {
        let mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Budget]);
        assert_eq!(mgr.focused(), PanelId::Quest);
    }

    #[test]
    #[should_panic(expected = "FocusManager requires at least one panel")]
    fn test_focus_manager_empty_panics() {
        FocusManager::new(vec![]);
    }

    #[test]
    fn test_focus_manager_next_cycles() {
        let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget]);
        mgr.next();
        assert_eq!(mgr.focused(), PanelId::Parliament);
        mgr.next();
        assert_eq!(mgr.focused(), PanelId::Budget);
        mgr.next();
        assert_eq!(mgr.focused(), PanelId::Quest);
    }

    #[test]
    fn test_focus_manager_prev_cycles() {
        let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Parliament, PanelId::Budget]);
        mgr.prev();
        assert_eq!(mgr.focused(), PanelId::Budget);
        mgr.prev();
        assert_eq!(mgr.focused(), PanelId::Parliament);
        mgr.prev();
        assert_eq!(mgr.focused(), PanelId::Quest);
    }

    #[test]
    fn test_focus_manager_jump_to_existing() {
        let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Budget]);
        assert!(mgr.jump_to(PanelId::Budget));
        assert_eq!(mgr.focused(), PanelId::Budget);
    }

    #[test]
    fn test_focus_manager_jump_to_missing() {
        let mut mgr = FocusManager::new(vec![PanelId::Quest, PanelId::Budget]);
        assert!(!mgr.jump_to(PanelId::Help));
        assert_eq!(mgr.focused(), PanelId::Quest);
    }
}
