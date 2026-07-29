//! 渲染方法 — TUI 面板布局与绘制
//!
//! 包含 [`TuiApp::render`]、布局计算、各区域渲染辅助方法。
//!
//! 对应架构层:L10 Interface

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};
use ratatui::Frame;
use std::time::Instant;

use super::TuiApp;
use crate::config::Theme;
use crate::types::{InputMode, LayoutMode};

impl TuiApp {
    /// 渲染当前帧:绘制所有面板 + FPS 统计 + 弹窗叠加。
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        // P4.4 FPS 统计:测量上一帧到本帧的真实耗时。
        // WHY 放在 render 开头:捕获两次渲染间的完整间隔(含事件处理与等待),
        // 这是用户实际感知到的帧率,比仅测量绘制耗时更具代表性。
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_time);
        self.last_frame_time = now;
        self.update_fps(delta);

        let area = frame.area();
        self.last_area = area;
        let chunks = self.layout(area);

        // P6.2:SinglePane 专注模式不渲染 tabs(全屏当前面板)
        // WHY 跳过渲染:SinglePane 的 layout 返回 chunks[0] = Rect::default()(空),
        // 在空 Rect 上渲染 Tabs widget 虽不 panic 但浪费 CPU,显式跳过更高效。
        if self.state.layout_mode != LayoutMode::SinglePane {
            self.render_tabs(frame, chunks[0]);
        }
        self.render_main_panel(frame, chunks[1]);

        // P6.2:SinglePane 专注模式不渲染 status_bar(全屏当前面板)
        // 但命令/搜索模式仍需渲染 command_palette(用户输入需可见)
        if self.state.input_mode != InputMode::Normal {
            self.command_palette
                .render(&self.state, chunks[2], frame.buffer_mut());
        } else if self.state.layout_mode == LayoutMode::SinglePane {
            // SinglePane:只渲染 hint_bar(快捷提示),不渲染 status_bar
            self.render_hint_bar(frame, chunks[2]);
        } else {
            // DualPane/TriplePane:将 bottom 区域拆分为 status_bar + hint_bar 双行
            let bottom_split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1)])
                .split(chunks[2]);
            self.render_status_bar(frame, bottom_split[0]);
            self.render_hint_bar(frame, bottom_split[1]);
        }

        // 弹窗叠加在最上层
        if !self.state.popup_stack.is_empty() {
            self.state.popup_stack.render(area, frame.buffer_mut());
        }

        // M2.2:统一命令面板作为居中 overlay,渲染在最上层(高于面板与状态栏)
        if self.palette.is_some() {
            self.render_palette(frame, area);
        }

        // P4.1:本帧渲染完成,重置 dirty 集合。下一帧的 `update` 会基于
        // 新一轮快照比较重新填充。
        self.state.clear_dirty();
    }

    /// 渲染统一命令面板 overlay(M2.2,用户北极星)
    ///
    /// WHY 复用自研布局引擎的 `centered_overlay`(M1.4):将 M1 的布局原语接线到
    /// M2 的实际渲染,overlay 尺寸为视口的 60%×60% 居中;标题/提示取自 i18n,
    /// 随 `Ctrl+L` 实时切换语言。渲染仍走 ratatui widget(引擎渲染路径切换属
    /// 后续 `v3-engine` 里程碑),此处只用引擎做几何计算。
    fn render_palette(&self, frame: &mut Frame<'_>, area: Rect) {
        let Some(model) = self.palette.as_ref() else {
            return;
        };
        // 用自研布局引擎计算居中 overlay 区域(engine::Rect → ratatui::Rect)
        let eng_overlay =
            crate::engine::layout::centered_overlay(crate::engine::from_ratatui_rect(area), 60, 60);
        let overlay = crate::engine::to_ratatui_rect(eng_overlay);
        // 视口过小(边框 + 三行内容至少需 3 行 4 列):放弃渲染,避免挤压
        if overlay.width < 4 || overlay.height < 3 {
            return;
        }

        // 先清空 overlay 区域,确保面板浮于底层内容之上
        frame.render_widget(Clear, overlay);

        // 外框:标题取自 i18n(随 Ctrl+L 实时切换)
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", crate::t!("palette.title")));
        let inner = block.inner(overlay);
        frame.render_widget(block, overlay);

        // 内部纵向切分:查询行(1)+ 候选列表(其余)+ 提示行(1)
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

        // 查询行:`> <query>`
        let query_line = Paragraph::new(format!("> {}", model.query()))
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(query_line, rows[0]);

        // 候选列表:滚动窗口保证选中项始终可见,选中项高亮 + ▶ 标记
        let list_area = rows[1];
        let visible = list_area.height as usize;
        let sel = model.selected_index();
        let entries = model.entries();
        // WHY 滚动偏移:当选中项超出可视高度时,下滑窗口使其贴底可见
        let offset = if visible > 0 && sel >= visible {
            sel + 1 - visible
        } else {
            0
        };
        let items: Vec<ListItem> = entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, e)| {
                let marker = if i == sel { "▶ " } else { "  " };
                let text = format!("{marker}{}  —  {}", e.title, e.subtitle);
                let style = if i == sel {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(text, style)))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);

        // 提示行:操作说明(随 locale 切换)
        let hint = Paragraph::new(crate::t!("palette.hint"))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(hint, rows[2]);
    }

    /// 计算当前布局,返回 [tabs, main, bottom] 三个区域
    ///
    /// WHY 独立方法:事件处理中需要知道各区域位置以响应鼠标点击,
    /// 与渲染复用同一套布局逻辑。
    ///
    /// # P6.2 布局模式
    /// - `SinglePane`:当前面板全屏,无 tabs,无 bottom(专注模式)
    /// - `DualPane`:默认布局(tabs 3 行 + main ratio% + bottom 剩余)
    /// - `TriplePane`:main 更小(70% × ratio),bottom 更大(预留 log_panel)
    pub(super) fn layout(&self, area: Rect) -> [Rect; 3] {
        match self.state.layout_mode {
            // P6.2 SinglePane:当前面板全屏,无 tabs,无 bottom
            // 返回 [空, 全屏, 空],render 时跳过 tabs/status_bar 渲染
            LayoutMode::SinglePane => [Rect::default(), area, Rect::default()],
            // P6.2 DualPane / M3d VimSplit:tabs + main + bottom
            // WHY 合并:VimSplit 的外层结构与 DualPane 一致(tabs + main + bottom),
            // 其左右双分屏在 main 区内由 `pane_rects` 按 PaneMode 切分,不影响外层布局。
            LayoutMode::DualPane | LayoutMode::VimSplit => {
                let tab_and_rest = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(0)])
                    .split(area);

                let main_and_bottom = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage((self.main_panel_ratio * 100.0) as u16),
                        // 双行状态栏(状态行 + 快捷提示行)至少占 4 行
                        Constraint::Min(4),
                    ])
                    .split(tab_and_rest[1]);

                [tab_and_rest[0], main_and_bottom[0], main_and_bottom[1]]
            }
            // P6.2 TriplePane:main 更小(70% × ratio),bottom 更大(预留 log_panel)
            // WHY 70%:DualPane 的 main 是 ratio(默认 70%),TriplePane 的 main 是
            // ratio × 0.7(默认约 49%),留出更多空间给 bottom(log_panel + status_bar)
            LayoutMode::TriplePane => {
                let tab_and_rest = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(0)])
                    .split(area);

                // TriplePane 的 main 占 ratio × 0.7,bottom 占剩余(更大)
                let triple_ratio = (self.main_panel_ratio * 0.7 * 100.0) as u16;
                let main_and_bottom = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(triple_ratio),
                        // 双行状态栏(状态行 + 快捷提示行)至少占 4 行
                        Constraint::Min(4),
                    ])
                    .split(tab_and_rest[1]);

                [tab_and_rest[0], main_and_bottom[0], main_and_bottom[1]]
            }
        }
    }

    /// 渲染面板标签栏
    fn render_tabs(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let titles: Vec<Line> = self
            .focus_manager
            .panels()
            .iter()
            .map(|&p| Line::from(format!(" {} ", p.as_str())))
            .collect();

        let focused = self.focus_manager.focused();
        let selected = self
            .focus_manager
            .panels()
            .iter()
            .position(|&p| p == focused)
            .unwrap_or(0);

        let tabs = Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(Color::Gray))
            .highlight_style(
                Style::default()
                    .fg(self.theme_fg())
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL).title(" Panels "));

        frame.render_widget(tabs, area);
    }

    /// 渲染主面板(当前激活面板的内容)
    fn render_main_panel(&mut self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let focused = self.focus_manager.focused();
        let focused_idx = self.panel_index(focused);

        // M1 清理项 #5:仅当焦点面板变化时才调用 focus 回调。
        if self.last_focused != Some(focused) {
            if let Some(idx) = focused_idx {
                self.panels[idx].focus(true);
            }
            for (i, panel) in self.panels.iter_mut().enumerate() {
                if Some(i) != focused_idx {
                    panel.focus(false);
                }
            }
            self.last_focused = Some(focused);
        }

        // M3d:按当前 PaneMode 计算可见窗格与区域,逐窗格渲染其面板。
        // 单窗格模式 / 窄视口下 panes/rects 均退化为主区一块(与既有 companion 行为等价)。
        let panes = self.pane_panels();
        let rects = self.pane_rects(area);
        for (panel_id, rect) in panes.iter().copied().zip(rects.iter().copied()) {
            if let Some(idx) = self.panel_index(panel_id) {
                self.panels[idx].render(&self.state, rect, frame.buffer_mut());
            }
        }

        // M3d:多窗格时(>1 区域)在活跃窗格边框叠加 accent 高亮,提示焦点所在。
        // WHY 仅多窗格:单窗格无需高亮(全屏即焦点),与既有单栏渲染逐字节一致。
        if rects.len() > 1 {
            let active_rect = rects.get(self.active_pane).copied().unwrap_or(rects[0]);
            let highlight = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.theme_accent()));
            frame.render_widget(highlight, active_rect);
        }
    }

    /// 渲染状态栏
    fn render_status_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let (status, fg) = match &self.state.status_message {
            Some((msg, severity)) => (
                format!(
                    " {}: {} | {}: {} | {}: {} | {} ",
                    crate::t!("status.panel"),
                    self.current_panel().as_str(),
                    crate::t!("status.tick"),
                    self.state.tick_mode.display(),
                    crate::t!("status.fps"),
                    self.state.fps,
                    msg
                ),
                severity.color(),
            ),
            None => (
                format!(
                    " {}: {} | {}: {} | {}: {} | {}: {} | {}: {:.0}% ",
                    crate::t!("status.panel"),
                    self.current_panel().as_str(),
                    crate::t!("status.tick"),
                    self.state.tick_mode.display(),
                    crate::t!("status.fps"),
                    self.state.fps,
                    crate::t!("status.frame"),
                    self.state.frame_count,
                    crate::t!("status.ratio"),
                    self.main_panel_ratio * 100.0
                ),
                Color::Black,
            ),
        };

        let span = Span::styled(
            status,
            Style::default()
                .fg(fg)
                .bg(self.theme_accent())
                .add_modifier(Modifier::BOLD),
        );
        let line = Line::from(span);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    /// 渲染键盘快捷提示栏
    ///
    /// WHY 独立方法:用户首次使用 TUI 时不知道有哪些快捷键,
    /// 底部提示栏可降低学习曲线,同时不会挤占状态信息空间。
    fn render_hint_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let span = Span::styled(crate::t!("hint.bar"), Style::default().fg(Color::DarkGray));
        let paragraph = Paragraph::new(Line::from(span)).alignment(Alignment::Right);
        frame.render_widget(paragraph, area);
    }

    /// 返回主题前景色
    pub(crate) fn theme_fg(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::White,
            Theme::Light => Color::Black,
            // P6.1:HighContrast 前景为白色(纯黑白最大对比度)
            Theme::HighContrast => Color::White,
        }
    }

    /// 返回主题强调色
    pub(crate) fn theme_accent(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::Cyan,
            Theme::Light => Color::Blue,
            // P6.1:HighContrast 强调色为亮黄(高饱和度,色盲友好)
            Theme::HighContrast => Color::LightYellow,
        }
    }
}
