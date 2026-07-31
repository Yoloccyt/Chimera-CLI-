//! 鼠标事件处理 — 点击、滚动与命中测试
//!
//! 对应架构层:L10 Interface

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::TuiApp;
use crate::types::InputMode;

impl TuiApp {
    /// 处理鼠标事件,按当前布局区域路由到面板交互(滚动/点击/拖拽)。
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        // Task 1.15.4:last_area 移至 pane_manager,经 pane_manager 字段访问
        let area = self.pane_manager.last_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let chunks = self.layout(area);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if is_inside(mouse.column, mouse.row, chunks[0]) {
                    self.handle_tab_click(mouse.column, chunks[0].width);
                } else if is_inside(mouse.column, mouse.row, chunks[2]) {
                    self.state.input_mode = InputMode::Command;
                    self.state.input_buffer.clear();
                }
                // 主面板点击已在焦点上,无需额外处理
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if !self.state.popup_stack.is_empty() {
                    let delta = if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    };
                    self.state.popup_stack.scroll_current(delta);
                } else if is_inside(mouse.column, mouse.row, chunks[1]) {
                    let focused = self.focus_manager.focused();
                    if let Some(idx) = self.panel_index(focused) {
                        if let Some(cmd) = self.panels[idx].handle_mouse(mouse, &mut self.state) {
                            self.apply_command(cmd);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// 处理标签栏点击,切换到对应面板
    fn handle_tab_click(&mut self, column: u16, tab_area_width: u16) {
        let panel_count = self.focus_manager.panels().len() as u16;
        if panel_count == 0 || tab_area_width == 0 {
            return;
        }
        let tab_width = tab_area_width / panel_count;
        let index = (column / tab_width) as usize;
        if let Some(&panel) = self.focus_manager.panels().get(index) {
            self.switch_panel_to(panel);
        }
    }
}

/// 判断坐标是否落在指定区域内
fn is_inside(column: u16, row: u16, area: Rect) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}
