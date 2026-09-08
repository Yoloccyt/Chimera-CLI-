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
        // I-6(2026-09-06 评估):Chat 视图全屏渲染会话流,无 Dashboard 三块
        // 布局可命中 —— 仍按 Dashboard 切分会把 composer 区点击误判为
        // 遗留命令入口。Chat 视图的输入走键盘链(i/\/:),鼠标仅滚轮滚动。
        if self.state.view_mode == crate::types::ViewMode::Chat {
            if let MouseEventKind::ScrollUp | MouseEventKind::ScrollDown = mouse.kind {
                // WHY 直取 Chat 面板:会话流即 Chat 面板实例,与焦点面板无关
                if let Some(idx) = self.panel_index(crate::types::PanelId::Chat) {
                    if let Some(cmd) = self.panels[idx].handle_mouse(mouse, &mut self.state) {
                        self.apply_command(cmd);
                    }
                }
            }
            return;
        }
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
                    // I-B(2026-09-06 复评):底栏点击改入 Slash 模式 —— 此前
                    // 进遗留 InputMode::Command,与 `:`/`/` 的斜杠入口双轨
                    // (体验不一致:无补全列表/三分层);Command 输入能力已
                    // 被 Slash 的 Legacy 回退完整承接。
                    self.state.input_mode = InputMode::Slash;
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
        // IT-02(2026-09-06 评估):窄终端(宽度 < 面板数)时 tab_width 整除为 0,
        // `column / tab_width` 触发整数除零 panic —— 无可用标签位时静默忽略点击
        // (与 `tab_area_width == 0` 守卫同语义,覆盖"有宽度但每位不足 1 列"情形)。
        if tab_width == 0 {
            return;
        }
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
