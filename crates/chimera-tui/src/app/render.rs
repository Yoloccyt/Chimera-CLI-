//! 渲染方法 — TUI 面板布局与绘制
//!
//! 包含 [`TuiApp::render`]、布局计算、各区域渲染辅助方法。
//!
//! 对应架构层:L10 Interface

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Widget};
use ratatui::Frame;
use std::time::Instant;

use super::TuiApp;
use crate::config::Theme;
use crate::types::{InputMode, LayoutMode, PanelId};

impl TuiApp {
    /// 渲染当前帧:绘制所有面板 + FPS 统计 + 弹窗叠加。
    ///
    /// # v3-engine M2 切换(ADR-061)
    /// `v3-engine` feature 启用时(默认),走自研引擎布局路径(`v3_render`);
    /// 设置环境变量 `CHIMERA_NO_V3_ENGINE=1` 或编译时禁用 feature 时回退到
    /// ratatui 路径(`legacy_ratatui_render`)。双路径并存保证 51 集成测试
    /// 全绿且支持运行时回退验证(M2 切换后 2 个版本周期)。
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        // P4.4 FPS 统计:测量上一帧到本帧的真实耗时。
        // WHY 放在 render 开头:捕获两次渲染间的完整间隔(含事件处理与等待),
        // 这是用户实际感知到的帧率,比仅测量绘制耗时更具代表性。
        let now = Instant::now();
        // Task 1.15.4:last_frame_time 移至 fps_counter,经 fps_counter 字段访问
        let delta = now.duration_since(self.fps_counter.last_frame_time);
        self.fps_counter.last_frame_time = now;
        self.update_fps(delta);

        // Concord W3 T3.2:会话模式为第一默认视图(ADR-076);会话流全屏 +
        // composer 底栏 + statusline,不走 Dashboard 三块布局。
        if self.state.view_mode == crate::types::ViewMode::Chat {
            self.render_chat_mode(frame);
            return;
        }

        // v3-engine M2 分发:feature 开启且未通过 env var 禁用时走 v3 路径
        #[cfg(feature = "v3-engine")]
        if !Self::v3_engine_disabled_by_env() {
            self.v3_render(frame);
            return;
        }

        // 回退路径:ratatui Layout::split + frame.render_widget
        self.legacy_ratatui_render(frame);
    }

    /// 渲染会话模式视图(Concord W3 T3.2):会话流 + composer + statusline
    ///
    /// # 布局
    /// 三区域自上而下:ChatStream(弹性)复用已注册的 Chat 面板实例(滚动/
    /// 跟随状态跨模式保留);composer 底栏复用 CommandPalette 底栏渲染
    /// (Insert/Slash/Command 态各自前缀);Normal 态显示输入提示占位。
    ///
    /// WHY 单函数双路径共用:会话模式布局与 v3/legacy 无关(区域简单固定),
    /// 单实现避免双路径分岐;帧末同样清理 dirty 集合。
    pub(crate) fn render_chat_mode(&mut self, frame: &mut Frame<'_>) {
        // Concord W6 T6.1:Plan/Auto 态预留 ModeBanner 条件行;Normal 态
        // 布局与 W3 逐字节一致(零冲击面)
        let banner_visible = crate::mode_banner::banner_line(self.state.approval_mode).is_some();
        let parts = crate::chat_mode::split_chat_layout(frame.area(), banner_visible);
        let buf = frame.buffer_mut();

        // 会话流:复用已注册 Chat 面板实例(与 Dashboard 内同一实例,状态保留)
        let state = &self.state;
        if let Some(idx) = self.panel_index(PanelId::Chat) {
            self.panels[idx].render(state, parts.stream, buf);
        }

        // Concord W6 T6.1:ModeBanner 常驻横幅(stream 与 composer 之间)
        if let Some(banner_area) = parts.banner {
            crate::mode_banner::render_banner(state.approval_mode, banner_area, buf);
        }

        // composer 底栏:非 Normal 态复用 CommandPalette 底栏(Insert `>` /
        // Slash `/` / Command `:` 前缀);Normal 态显示输入提示占位
        if state.input_mode != InputMode::Normal {
            self.command_palette.render(state, parts.composer, buf);
        } else {
            let hint = Paragraph::new(Line::from(Span::styled(
                format!("> {}", crate::t!("chat.composer_hint")),
                Style::default().add_modifier(Modifier::DIM),
            )))
            .block(Block::default().borders(Borders::ALL));
            hint.render(parts.composer, buf);
        }

        // statusline:复用 Dashboard 状态栏(面板/tick/FPS/状态消息同源)
        self.render_status_bar(frame, parts.status);

        // Concord W3 T3.3:会话流内嵌卡片——复盘卡优先(失败告警),
        // 否则计划卡;附着于会话流区底部(先 Clear 再绘,不遮 composer)
        let reflection = crate::chat_cards::derive_reflection_card(&self.state);
        let plan = if reflection.is_none() {
            crate::chat_cards::derive_quest_plan_card(&self.state)
        } else {
            None
        };
        if reflection.is_some() || plan.is_some() {
            let card_h = if reflection.is_some() { 5u16 } else { 6u16 };
            if parts.stream.height > card_h + 2 {
                let card_area = Rect::new(
                    parts.stream.x,
                    parts.stream.y + parts.stream.height - card_h,
                    parts.stream.width,
                    card_h,
                );
                ratatui::widgets::Clear.render(card_area, frame.buffer_mut());
                if let Some(r) = &reflection {
                    crate::chat_cards::render_reflection_card(r, card_area, frame.buffer_mut());
                } else if let Some(p) = &plan {
                    crate::chat_cards::render_quest_plan_card(p, card_area, frame.buffer_mut());
                }
            }
        }

        // Concord W2/W3:Slash 态补全列表 overlay(会话流区底部靠栏)
        if self.state.input_mode == InputMode::Slash {
            self.render_slash_overlay(frame, parts.stream);
        }

        // 帧末清理 dirty(与 Dashboard 路径一致)
        self.state.clear_dirty();
    }

    /// 检查环境变量 `CHIMERA_NO_V3_ENGINE` 是否禁用 v3-engine(M2 回退验证用)
    #[cfg(feature = "v3-engine")]
    pub(crate) fn v3_engine_disabled_by_env() -> bool {
        // WHY 运行时 env var:feature flag 是编译期的,无法在已编译二进制中关闭;
        // env var 提供"逃生舱"用于生产回退验证(Task 0.4.6 --no-v3-engine flag 设置此变量)
        std::env::var("CHIMERA_NO_V3_ENGINE")
            .ok()
            .as_deref()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    /// ratatui 遗留渲染路径(feature 关闭或 env var 禁用时的回退)
    ///
    /// 保留 M2 切换前的完整渲染逻辑,确保随时可回滚(§3.4.1 第 5 条向后兼容)。
    fn legacy_ratatui_render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        // Task 1.15.4:last_area 移至 pane_manager,经 pane_manager 字段访问
        self.pane_manager.last_area = area;
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
        // Task 1.15.4:palette 移至 chat_session,经 chat_session 字段访问
        if self.chat_session.palette.is_some() {
            self.render_palette(frame, area);
        }

        // Concord W2:斜杠补全列表 overlay(遗留路径同步,双路径行为一致)
        if self.state.input_mode == InputMode::Slash {
            self.render_slash_overlay(frame, chunks[1]);
        }

        // P4.1:本帧渲染完成,重置 dirty 集合。下一帧的 `update` 会基于
        // 新一轮快照比较重新填充。
        self.state.clear_dirty();
    }

    // ========================================================================
    // v3-engine M2 渲染路径(ADR-061)
    // ------------------------------------------------------------------------
    // M2 启用策略(minimal):布局计算走自研 `engine::layout::flex::split`,
    // 面板 widget 渲染仍委托 ratatui `frame.render_widget`(经 v3_engine 抽象层)。
    // 这样可确保 51 集成测试全绿,后续 Task 1.15-1.17 逐步真正替换为
    // DoubleBuffer/DiffEngine/TerminalWriter 输出路径。
    // ========================================================================
    #[cfg(feature = "v3-engine")]
    fn v3_render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        // Task 1.15.4:last_area 移至 pane_manager,经 pane_manager 字段访问
        self.pane_manager.last_area = area;

        // 用自研布局引擎替换 ratatui Layout::split(M2 核心切换点)
        let chunks = self.v3_layout(area);

        // P6.2:SinglePane 专注模式不渲染 tabs(全屏当前面板)
        if self.state.layout_mode != LayoutMode::SinglePane {
            self.render_tabs(frame, chunks[0]);
        }
        self.render_main_panel(frame, chunks[1]);

        // bottom 区:command_palette / hint_bar / status_bar + hint_bar
        if self.state.input_mode != InputMode::Normal {
            self.command_palette
                .render(&self.state, chunks[2], frame.buffer_mut());
        } else if self.state.layout_mode == LayoutMode::SinglePane {
            self.render_hint_bar(frame, chunks[2]);
        } else {
            // DualPane/TriplePane:bottom 拆分为 status_bar + hint_bar 双行
            // 复用自研 flex::split 做行内切分(M2 一致性,不再走 ratatui Layout)
            let eng_bottom = crate::engine::from_ratatui_rect(chunks[2]);
            let bottom_parts = crate::engine::layout::flex::split(
                eng_bottom,
                crate::engine::layout::Direction::Vertical,
                &[
                    crate::engine::layout::Constraint::Fixed(1),
                    crate::engine::layout::Constraint::Fixed(1),
                ],
            );
            self.render_status_bar(frame, crate::engine::to_ratatui_rect(bottom_parts[0]));
            self.render_hint_bar(frame, crate::engine::to_ratatui_rect(bottom_parts[1]));
        }

        // 弹窗叠加在最上层
        if !self.state.popup_stack.is_empty() {
            self.state.popup_stack.render(area, frame.buffer_mut());
        }

        // M2.2:统一命令面板作为居中 overlay,渲染在最上层
        // Task 1.15.4:palette 移至 chat_session,经 chat_session 字段访问
        if self.chat_session.palette.is_some() {
            self.render_palette(frame, area);
        }

        // Concord W2:斜杠补全列表 overlay(v3 路径同步,双路径行为一致)
        if self.state.input_mode == InputMode::Slash {
            self.render_slash_overlay(frame, chunks[1]);
        }

        // P4.1:本帧渲染完成,重置 dirty 集合
        self.state.clear_dirty();
    }

    /// 斜杠命令补全 overlay(Concord W2 T2.2)
    ///
    /// 在主区底部绘制候选列表(输入栏上方):宽 min(64, 区宽),高随候选数
    /// 增长但不超 12 行;先 `Clear` 擦除底层再渲染,避免面板内容透出。
    /// WHY 底部靠栏而非居中:补全与输入栏视觉连续(主流 Agent CLI 交互惯例)。
    fn render_slash_overlay(&self, frame: &mut Frame<'_>, main_area: Rect) {
        use ratatui::widgets::Widget;
        let reg = crate::actions::SlashCommandRegistry::with_builtin_commands();
        let cands = crate::slash_surface::candidates(&reg, &self.state.input_buffer);
        if cands.is_empty() || main_area.height < 3 || main_area.width < 8 {
            return;
        }
        let rows = cands.len().min(crate::slash_surface::MAX_VISIBLE_ROWS) + 2; // 边框两行
        let height = (rows as u16).min(main_area.height);
        let width = 64u16.min(main_area.width);
        let overlay = Rect::new(
            main_area.x,
            main_area.y + main_area.height - height,
            width,
            height,
        );
        ratatui::widgets::Clear.render(overlay, frame.buffer_mut());
        let selected = crate::slash_surface::clamp_selected(self.state.slash_selected, cands.len());
        crate::slash_surface::render_candidates(&cands, selected, overlay, frame.buffer_mut());
    }

    /// v3 布局计算 — 用 `engine::layout::flex::split` 替换 ratatui `Layout::split`
    ///
    /// 返回 `[tabs, main, bottom]` 三块区域(ratatui Rect,供下游 widget 渲染)。
    /// 语义与 `layout()` 完全对齐,仅切换布局求解后端(M2 切换点)。
    #[cfg(feature = "v3-engine")]
    fn v3_layout(&self, area: Rect) -> [Rect; 3] {
        use crate::engine::layout::{
            split, Constraint as EngConstraint, Direction as EngDirection,
        };
        use crate::engine::{from_ratatui_rect, to_ratatui_rect};

        let eng_area = from_ratatui_rect(area);
        match self.state.layout_mode {
            // SinglePane:当前面板全屏,无 tabs,无 bottom
            LayoutMode::SinglePane => [Rect::default(), area, Rect::default()],
            // DualPane / VimSplit:tabs(3) + main(ratio%) + bottom(min 4)
            LayoutMode::DualPane | LayoutMode::VimSplit => {
                // 外层:tabs(固定 3 行)+ 剩余(Min 0,吸收余量)
                let outer = split(
                    eng_area,
                    EngDirection::Vertical,
                    &[EngConstraint::Fixed(3), EngConstraint::Min(0)],
                );
                // 内层:main(Percent ratio)+ bottom(Min 4)
                // Task 1.15.4:main_panel_ratio 移至 pane_manager
                let main_pct = (self.pane_manager.main_panel_ratio * 100.0) as u8;
                let inner = split(
                    outer[1],
                    EngDirection::Vertical,
                    &[EngConstraint::Percent(main_pct), EngConstraint::Min(4)],
                );
                [
                    to_ratatui_rect(outer[0]),
                    to_ratatui_rect(inner[0]),
                    to_ratatui_rect(inner[1]),
                ]
            }
            // TriplePane:main 占 ratio × 0.7,bottom 更大
            LayoutMode::TriplePane => {
                let outer = split(
                    eng_area,
                    EngDirection::Vertical,
                    &[EngConstraint::Fixed(3), EngConstraint::Min(0)],
                );
                let triple_pct = (self.pane_manager.main_panel_ratio * 0.7 * 100.0) as u8;
                let inner = split(
                    outer[1],
                    EngDirection::Vertical,
                    &[EngConstraint::Percent(triple_pct), EngConstraint::Min(4)],
                );
                [
                    to_ratatui_rect(outer[0]),
                    to_ratatui_rect(inner[0]),
                    to_ratatui_rect(inner[1]),
                ]
            }
        }
    }

    ///
    /// WHY 复用自研布局引擎的 `centered_overlay`(M1.4):将 M1 的布局原语接线到
    /// M2 的实际渲染,overlay 尺寸为视口的 60%×60% 居中;标题/提示取自 i18n,
    /// 随 `Ctrl+L` 实时切换语言。渲染仍走 ratatui widget(引擎渲染路径切换属
    /// 后续 `v3-engine` 里程碑),此处只用引擎做几何计算。
    fn render_palette(&self, frame: &mut Frame<'_>, area: Rect) {
        // Task 1.15.4:palette 移至 chat_session
        let Some(model) = self.chat_session.palette.as_ref() else {
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
                        Constraint::Percentage((self.pane_manager.main_panel_ratio * 100.0) as u16),
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
                let triple_ratio = (self.pane_manager.main_panel_ratio * 0.7 * 100.0) as u16;
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
        // Task 1.15.4:last_focused 移至 pane_manager
        if self.pane_manager.last_focused != Some(focused) {
            if let Some(idx) = focused_idx {
                self.panels[idx].focus(true);
            }
            for (i, panel) in self.panels.iter_mut().enumerate() {
                if Some(i) != focused_idx {
                    panel.focus(false);
                }
            }
            self.pane_manager.last_focused = Some(focused);
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
            // Task 1.15.4:active_pane 移至 pane_manager
            let active_rect = rects
                .get(self.pane_manager.active_pane)
                .copied()
                .unwrap_or(rects[0]);
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
                    // Task 1.15.4:main_panel_ratio 经 getter 方法读取(委托 pane_manager)
                    self.main_panel_ratio() * 100.0
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
        // Concord W4 T4.1:审批模式徽标(三态色彩区分,Chat/Dashboard 双视图共用;
        // yottacode banner 实证——模式状态常驻可见)
        let badge_fg = match self.state.approval_mode {
            crate::approval_mode::ApprovalMode::Normal => Color::Green,
            crate::approval_mode::ApprovalMode::Plan => Color::Yellow,
            crate::approval_mode::ApprovalMode::Auto => Color::Red,
        };
        let badge = Span::styled(
            format!(" {} ", crate::t!(self.state.approval_mode.label_key())),
            Style::default()
                .fg(badge_fg)
                .bg(self.theme_accent())
                .add_modifier(Modifier::BOLD),
        );
        let line = Line::from(vec![span, badge]);
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

// ============================================================================
// v3-engine M2 单元测试(ADR-061)
// ----------------------------------------------------------------------------
// 覆盖:
// 1. `v3_engine_disabled_by_env` 的 env var 解析("1"/"true"/"TRUE"/"0"/unset)
// 2. `v3_layout` 三种 LayoutMode 的布局正确性(与 `solve` 算法推导一致)
// 3. 布局不变量:子区域高度之和 == 父区域高度(无缝平铺)
// ============================================================================
#[cfg(all(test, feature = "v3-engine"))]
mod v3_engine_tests {
    use super::*;
    use crate::config::TuiConfig;
    use std::sync::Mutex;

    /// 序列化 env var 测试,防止并行执行导致的 race condition
    /// WHY Mutex:std::env::set_var 是进程级全局状态,并行测试会相互覆盖
    static ENV_TEST_MUTEX: Mutex<()> = Mutex::new(());

    /// 构造指定 LayoutMode 的 TuiApp(使用默认 StubDataSource)
    fn make_app(layout_mode: LayoutMode) -> TuiApp {
        let mut app = {
            let mut __app = TuiApp::new(TuiConfig::default()).expect("TuiApp construction failed");
            __app.state_mut().view_mode = crate::types::ViewMode::Dashboard;
            __app
        };
        app.state_mut().layout_mode = layout_mode;
        app
    }

    // ============================================================
    // v3_engine_disabled_by_env 解析测试
    // ============================================================

    #[test]
    fn v3_env_var_disabled_when_set_to_1() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        let original = std::env::var("CHIMERA_NO_V3_ENGINE").ok();

        std::env::set_var("CHIMERA_NO_V3_ENGINE", "1");
        assert!(
            TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=1 should disable v3-engine"
        );

        if let Some(val) = original {
            std::env::set_var("CHIMERA_NO_V3_ENGINE", val);
        } else {
            std::env::remove_var("CHIMERA_NO_V3_ENGINE");
        }
    }

    #[test]
    fn v3_env_var_disabled_when_set_to_true_case_insensitive() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        let original = std::env::var("CHIMERA_NO_V3_ENGINE").ok();

        // 小写 "true"
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "true");
        assert!(
            TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=true should disable v3-engine"
        );

        // 大写 "TRUE"(eq_ignore_ascii_case)
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "TRUE");
        assert!(
            TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=TRUE should disable v3-engine (case insensitive)"
        );

        // 混合大小写 "True"
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "True");
        assert!(
            TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=True should disable v3-engine (case insensitive)"
        );

        if let Some(val) = original {
            std::env::set_var("CHIMERA_NO_V3_ENGINE", val);
        } else {
            std::env::remove_var("CHIMERA_NO_V3_ENGINE");
        }
    }

    #[test]
    fn v3_env_var_not_disabled_when_unset_or_other_values() {
        let _guard = ENV_TEST_MUTEX.lock().unwrap();
        let original = std::env::var("CHIMERA_NO_V3_ENGINE").ok();

        // 未设置 → 不禁用
        std::env::remove_var("CHIMERA_NO_V3_ENGINE");
        assert!(
            !TuiApp::v3_engine_disabled_by_env(),
            "unset CHIMERA_NO_V3_ENGINE should not disable v3-engine"
        );

        // "0" → 不禁用(非 "1" 且非 "true")
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "0");
        assert!(
            !TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=0 should not disable v3-engine"
        );

        // 空字符串 → 不禁用
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "");
        assert!(
            !TuiApp::v3_engine_disabled_by_env(),
            "empty CHIMERA_NO_V3_ENGINE should not disable v3-engine"
        );

        // 任意字符串 → 不禁用
        std::env::set_var("CHIMERA_NO_V3_ENGINE", "false");
        assert!(
            !TuiApp::v3_engine_disabled_by_env(),
            "CHIMERA_NO_V3_ENGINE=false should not disable v3-engine"
        );

        if let Some(val) = original {
            std::env::set_var("CHIMERA_NO_V3_ENGINE", val);
        } else {
            std::env::remove_var("CHIMERA_NO_V3_ENGINE");
        }
    }

    // ============================================================
    // v3_layout 布局正确性测试
    // ============================================================

    #[test]
    fn v3_layout_single_pane_returns_fullscreen_with_empty_tabs_and_bottom() {
        let app = make_app(LayoutMode::SinglePane);
        let area = Rect::new(0, 0, 80, 24);
        let chunks = app.v3_layout(area);

        // SinglePane:tabs 与 bottom 为空,main 占满全屏
        assert_eq!(
            chunks[0],
            Rect::default(),
            "tabs should be empty in SinglePane"
        );
        assert_eq!(chunks[1], area, "main should be full screen in SinglePane");
        assert_eq!(
            chunks[2],
            Rect::default(),
            "bottom should be empty in SinglePane"
        );
    }

    #[test]
    fn v3_layout_dual_pane_splits_into_tabs_main_bottom() {
        // 默认 main_panel_ratio = 0.7(见 TuiConfig::default)
        let app = make_app(LayoutMode::DualPane);
        let area = Rect::new(0, 0, 80, 24);
        let chunks = app.v3_layout(area);

        // outer = [Fixed(3), Min(0)] → tabs=3行, rest=21行
        assert_eq!(chunks[0], Rect::new(0, 0, 80, 3), "tabs should be 3 rows");

        // inner = [Percent(70), Min(4)] on height=21
        // solve(21, [Percent(70), Min(4)]):
        //   base = [21*70/100, 4] = [14, 4], base_sum=18, leftover=3
        //   Min(4) grows by 3 → [14, 7]
        assert_eq!(
            chunks[1],
            Rect::new(0, 3, 80, 14),
            "main should be 14 rows (70% of 21, truncated)"
        );
        assert_eq!(
            chunks[2],
            Rect::new(0, 17, 80, 7),
            "bottom should be 7 rows (remaining after main)"
        );
    }

    #[test]
    fn v3_layout_triple_pane_has_smaller_main_than_dual() {
        let app = make_app(LayoutMode::TriplePane);
        let area = Rect::new(0, 0, 80, 24);
        let chunks = app.v3_layout(area);

        // outer 同 DualPane:tabs=3, rest=21
        assert_eq!(chunks[0], Rect::new(0, 0, 80, 3), "tabs should be 3 rows");

        // triple_pct = (0.7 * 0.7 * 100.0) as u8 = 49
        // solve(21, [Percent(49), Min(4)]):
        //   base = [21*49/100, 4] = [10, 4], base_sum=14, leftover=7
        //   Min(4) grows by 7 → [10, 11]
        assert_eq!(
            chunks[1],
            Rect::new(0, 3, 80, 10),
            "main should be 10 rows (49% of 21, truncated)"
        );
        assert_eq!(
            chunks[2],
            Rect::new(0, 13, 80, 11),
            "bottom should be 11 rows (larger than DualPane's 7)"
        );
    }

    #[test]
    fn v3_layout_vim_split_uses_same_outer_as_dual_pane() {
        // VimSplit 的外层结构与 DualPane 一致(tabs + main + bottom),
        // 左右双分屏在 main 区内由 pane_rects 切分,不影响外层布局。
        let app = make_app(LayoutMode::VimSplit);
        let area = Rect::new(0, 0, 80, 24);
        let chunks = app.v3_layout(area);

        assert_eq!(chunks[0], Rect::new(0, 0, 80, 3));
        assert_eq!(chunks[1], Rect::new(0, 3, 80, 14));
        assert_eq!(chunks[2], Rect::new(0, 17, 80, 7));
    }

    // ============================================================
    // 布局不变量:无缝平铺(子区域高度之和 == 父区域高度)
    // ============================================================

    #[test]
    fn v3_layout_tiles_cover_full_area_in_all_modes() {
        let area = Rect::new(0, 0, 80, 24);

        for &mode in &[
            LayoutMode::DualPane,
            LayoutMode::TriplePane,
            LayoutMode::VimSplit,
        ] {
            let app = make_app(mode);
            let chunks = app.v3_layout(area);

            let total_height: u16 = chunks[0].height + chunks[1].height + chunks[2].height;
            assert_eq!(
                total_height, area.height,
                "tiles must cover full area height for {:?} (got {}, expected {})",
                mode, total_height, area.height
            );
        }
    }

    #[test]
    fn v3_layout_works_with_small_terminal() {
        // 边界场景:极小终端(10x5)不应 panic
        let app = make_app(LayoutMode::DualPane);
        let area = Rect::new(0, 0, 10, 5);
        let chunks = app.v3_layout(area);

        // tabs=3, rest=2, main=Percent(70) of 2 = 1, bottom=Min(4) but only 1 left
        // solve(2, [Percent(70), Min(4)]):
        //   base = [2*70/100, 4] = [1, 4], base_sum=5 > total=2
        //   → shrink: sizes[0] = 1*2/5 = 0, sizes[1] = 4*2/5 = 1, assigned=1 < 2
        //   → sizes[0] += 1 → [1, 1]
        let total_height: u16 = chunks[0].height + chunks[1].height + chunks[2].height;
        assert_eq!(
            total_height, area.height,
            "small terminal must still tile fully"
        );
        assert_eq!(
            chunks[0].height, 3,
            "tabs still 3 rows even in small terminal"
        );
    }

    /// M3 端到端管线:真实 app 帧 → compat 转换 → V3Output diff 写出。
    ///
    /// 断言:默认中文帧输出含 CJK 字节;输出不含 NUL(宽字符续格哨兵被
    /// TerminalWriter 跳过,避免汉字右半格被空格覆盖的渲染破损)。
    #[test]
    fn v3_output_pipeline_renders_zh_frame_without_nul() {
        use ratatui::backend::TestBackend;

        // 与其它 locale 测试互斥,避免并行竞态把全局语言切到 En 导致断言失败
        let _locale_guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let mut app = {
            let mut __app = TuiApp::new(TuiConfig::default()).expect("TuiApp construction failed");
            __app.state_mut().view_mode = crate::types::ViewMode::Dashboard;
            __app
        };
        app.switch_panel_to(crate::types::PanelId::Quest);
        let backend = TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).expect("memory terminal init");
        term.draw(|f| app.render(f)).expect("frame draw");
        let rb = term.backend().buffer().clone();
        let back = crate::engine::from_ratatui_buffer(&rb);

        let mut out_state = crate::engine::output::V3Output::new();
        let mut sink = Vec::<u8>::new();
        out_state.render(back, &mut sink).expect("v3 output render");
        assert!(
            sink.windows(3)
                .any(|w| w == "任".as_bytes() || w == "务".as_bytes()),
            "默认中文帧应输出 CJK 字节"
        );
        assert!(!sink.contains(&0u8), "输出不应包含 NUL(续格哨兵被跳过)");
    }
}
