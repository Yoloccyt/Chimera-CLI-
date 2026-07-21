//! TUI OSA 稀疏度可视化面板 — OMEGA Ω-Sparse 定律的可视化展示
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:OsaSparse
//! 对应创新点:Ω-Sparse(全维稀疏 — 工具/上下文/记忆/审计/预算五维度)
//!
//! # 核心职责
//! - 顶部 Gauge 展示 `TuiState.osa_sparsity`(平均稀疏度 0.0-1.0)及颜色编码
//! - 中部虚拟滚动列表展示 `TuiState.osa_context_mask`(context 维度活跃文件 ID)
//! - 底部 sparkline 展示 `TuiState.osa_sparsity_history`(稀疏度历史趋势,容量 256 FIFO)
//!
//! # 交互
//! - J/Down: 向下滚动(选中下一个活跃文件)
//! - K/Up: 向上滚动(选中上一个活跃文件)
//! - g: 跳到列表顶部
//! - G: 跳到列表底部
//! - Enter: 空列表时返回 None(无详情弹窗定义)
//!
//! # 设计决策(WHY)
//! - **三段式纵向布局**:顶部 Gauge(3 行) + 中部列表(弹性) + 底部 sparkline(5 行),
//!   与 Timeline 面板的纵向布局模式一致 — 时间序列/趋势类数据纵向浏览更自然。
//!   Gauge 在顶部提供瞬时概览,sparkline 在底部提供历史趋势,列表在中部提供明细,
//!   形成"当前态 → 明细态 → 趋态势"的三时间维度视图。
//! - **颜色编码策略**:小于 0.3 绿色(低稀疏度,上下文保留充分)/ 0.3-0.7 黄色(适中)/
//!   大于 0.7 红色(高稀疏度,大量上下文被掩码,可能过度过滤)。
//!   WHY 此映射:稀疏度越高表示被 OSA 掩码的上下文比例越大,过高的稀疏度可能
//!   误过滤关键上下文导致信息丢失,因此用红色警示运维人员关注。
//! - **虚拟滚动**:context 活跃文件列表理论上可包含数百文件 ID,沿用
//!   `virtual_scroll_window` 保持与 Timeline/EventStream 面板一致的渲染模式,
//!   将渲染复杂度从 O(n) 降至 O(visible + 2×BUFFER)。
//! - **数据源**:`TuiState.osa_sparsity` / `osa_context_mask` / `osa_sparsity_history`
//!   — 由 `OsaSparseSync` 从 `OmniSparseMasksComputed` 事件同步填充。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::render::{self, virtual_scroll_window};
use crate::types::{PanelId, TuiCommand, TuiState};

/// OSA 稀疏度可视化面板
///
/// 展示 OMEGA Ω-Sparse 定律的平均稀疏度、context 维度活跃文件列表与稀疏度历史趋势。
///
/// WHY 持有 `selected` / `scroll_offset`:与 TimelinePanel / EventStreamPanel 保持一致,
/// 面板内部管理列表导航状态,`TuiApp` 通过 Panel trait 统一驱动。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OsaSparsePanel {
    /// 当前选中文件索引(context 列表内)
    selected: usize,
    /// 列表滚动偏移(可见区域起始行)
    scroll_offset: usize,
}

impl OsaSparsePanel {
    /// 创建新的 OSA 稀疏度面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中项索引(测试用,与 TimelinePanel 模式一致)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 返回当前滚动偏移(测试用)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// 根据稀疏度值返回对应的颜色编码
    ///
    /// WHY 独立函数:将颜色映射逻辑与渲染逻辑解耦,便于未来调整阈值或增加更多档位。
    /// 全程使用 f32 字面量,避免 f32→f64 隐式转换导致的精度膨胀(§4.4 #6 红线)。
    fn sparsity_color(sparsity: f32) -> Color {
        // 阈值边界:[0.0, 0.3) 绿色 / [0.3, 0.7] 黄色 / (0.7, 1.0] 红色
        if sparsity < 0.3_f32 {
            Color::Green
        } else if sparsity <= 0.7_f32 {
            Color::Yellow
        } else {
            Color::Red
        }
    }

    /// 渲染 context 活跃文件列表的可见窗口(中部区域)
    ///
    /// 使用 `virtual_scroll_window` 仅构造可见区域 + 上下缓冲的 Text,
    /// 确保即使文件数量增大,渲染复杂度仍为 O(visible + 2×BUFFER)。
    fn render_window(
        state: &TuiState,
        selected: usize,
        scroll_offset: usize,
        visible_rows: usize,
    ) -> Text<'static> {
        let files = &state.osa_context_mask;
        let total = files.len();

        let mut lines: Vec<Line<'static>> = Vec::new();

        if total == 0 {
            // 空列表:未收到 OSA 事件时显示等待提示
            lines.push(Line::from(Span::styled(
                "No active context files. Waiting for OSA coordinator...",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let (start, end) = virtual_scroll_window(total, scroll_offset, visible_rows);

            for idx in start..end {
                if let Some(file_id) = files.get(idx) {
                    let line = format_file_line(file_id, idx, idx == selected);
                    lines.push(line);
                }
            }

            // 虚拟滚动提示:总文件数 > 可见窗口时显示总数
            if total > visible_rows {
                lines.push(Line::from(Span::styled(
                    format!(
                        "... showing {} of {} files",
                        end.saturating_sub(start),
                        total
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        Text::from(lines)
    }
}

/// 格式化单个文件 ID 为列表行
///
/// WHY 独立函数:将"如何渲染单行"与"渲染哪些行"解耦,
/// 便于未来调整行格式(如增加文件大小列)时只需修改此处。
fn format_file_line(file_id: &str, index: usize, is_selected: bool) -> Line<'static> {
    let style = if is_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let prefix = if is_selected { "> " } else { "  " };

    Line::from(vec![
        Span::styled(format!("{}{:>3}. ", prefix, index + 1), style),
        Span::styled(
            file_id.to_string(),
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Cyan)
            },
        ),
    ])
}

impl Panel for OsaSparsePanel {
    fn id(&self) -> PanelId {
        PanelId::OsaSparse
    }

    fn title(&self) -> Line<'static> {
        Line::from(" OSA Sparse ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let files = &state.osa_context_mask;
        let count = files.len();
        // 钳位选中索引到有效范围(数据变化时同步)
        self.selected = list_state::clamp_selected(self.selected, count);

        // 构造面板边框
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        // 终端过小时不渲染内容,避免布局计算溢出
        if inner.height < 6 || inner.width < 20 {
            return;
        }

        // 三段式纵向布局:顶部 Gauge(3 行) + 中部列表(弹性) + 底部 sparkline(5 行)
        //
        // WHY 固定顶部 3 行:Gauge 需要边框 2 行 + 内容 1 行;
        // 底部 sparkline 5 行(边框 2 + 内容 3),与 Timeline 面板底部一致;
        // 中部列表取剩余空间,最少 5 行保证可读性。
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(5),
                Constraint::Length(5),
            ])
            .split(inner);

        // === 顶部:平均稀疏度 Gauge ===
        //
        // WHY 根据 osa_sparsity 是否为 None 区分渲染:
        // - Some(value):正常显示稀疏度百分比与颜色编码
        // - None:显示 0% + "N/A" 标签,表示尚未收到 OSA 事件
        let (percent, label, color) = match state.osa_sparsity {
            Some(sparsity) => {
                // 钳位到 [0.0, 1.0] 避免越界值导致 Gauge 异常
                let clamped = sparsity.clamp(0.0_f32, 1.0_f32);
                // 全程 f32 运算,避免隐式 f64 转换(§4.4 #6 红线)
                let pct = (clamped * 100.0_f32) as u16;
                let lbl = format!("Sparsity: {:.1}%", clamped * 100.0_f32);
                (pct, lbl, Self::sparsity_color(clamped))
            }
            None => {
                // 空状态:Gauge 显示 0% 并标注 N/A,颜色用灰色表示无数据
                (0u16, "N/A".to_string(), Color::DarkGray)
            }
        };

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Sparsity"))
            .gauge_style(Style::default().fg(color))
            .percent(percent)
            .label(label);
        gauge.render(chunks[0], buf);

        // === 中部:context 活跃文件列表(虚拟滚动) ===
        let content_height = chunks[1].height as usize;
        // 列表内容高度需扣除虚拟滚动提示行(1 行)
        let visible_rows = content_height.saturating_sub(1);
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, visible_rows);

        let list_content =
            Self::render_window(state, self.selected, self.scroll_offset, visible_rows);
        let list_paragraph = Paragraph::new(list_content);
        list_paragraph.render(chunks[1], buf);

        // === 底部:稀疏度历史 sparkline ===
        // WHY 直接使用 osa_sparsity_history 作为 sparkline 数据点:
        // 该字段已由 OsaSparseSync 将稀疏度 × 1000 转为整型存储,适合 sparkline 展示。
        let sparkline = render::sparkline(
            &state.osa_sparsity_history,
            "Sparsity History",
            Color::Magenta,
        );
        sparkline.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.osa_context_mask.len();

        // 优先处理方向键导航(复用 list_state 模块)
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        // WHY 额外处理 j/k:list_state::handle_key_navigation 只处理 Up/Down,
        // 不处理 j/k(后者不是所有面板都支持)。OsaSparse 面板遵循 vim 风格,
        // 额外映射 j/k 到 Down/Up,与 Timeline 面板保持一致。
        match key.code {
            KeyCode::Char('j') => {
                self.selected = list_state::move_selection(self.selected, 1, count);
                None
            }
            KeyCode::Char('k') => {
                self.selected = list_state::move_selection(self.selected, -1, count);
                None
            }
            // Enter:空列表时返回 None;非空列表也返回 None(本面板未定义详情弹窗)
            KeyCode::Enter => None,
            KeyCode::Char('g') => {
                self.scroll_to_top(state);
                None
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom(state);
                None
            }
            // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let count = state.osa_context_mask.len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.osa_context_mask.len();
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("R", "刷新")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn test_osa_sparse_panel_id() {
        let panel = OsaSparsePanel::new();
        assert_eq!(panel.id(), PanelId::OsaSparse);
    }

    #[test]
    fn test_osa_sparse_panel_title() {
        let panel = OsaSparsePanel::new();
        assert_eq!(panel.title().to_string(), " OSA Sparse ");
    }

    #[test]
    fn test_osa_sparse_panel_empty_state() {
        // 空状态(无 context 文件、无稀疏度)时 handle_key 不应 panic
        let mut panel = OsaSparsePanel::new();
        let mut state = TuiState::new();
        assert!(state.osa_context_mask.is_empty());
        assert!(state.osa_sparsity.is_none());

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = panel.handle_key(key, &mut state);
        assert!(result.is_none(), "Down on empty list should return None");
        assert_eq!(
            panel.selected(),
            0,
            "selected should stay at 0 on empty list"
        );
    }

    #[test]
    fn test_osa_sparse_panel_navigation() {
        let mut panel = OsaSparsePanel::new();
        let mut state = TuiState::new();
        state.osa_context_mask = vec!["file1.rs".into(), "file2.rs".into(), "file3.rs".into()];

        // j 向下
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 1);

        // j 继续向下
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 2);

        // k 向上
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 1);
    }

    #[test]
    fn test_osa_sparse_panel_clamp_selected() {
        let mut panel = OsaSparsePanel::new();
        let mut state = TuiState::new();
        // 初始 selected=0,无文件
        assert_eq!(panel.selected(), 0);
        // 在空列表上按 Down,应保持 0 不越界
        panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
        assert_eq!(panel.selected(), 0);

        // 添加 3 个文件后连续按 j 超过列表长度,应钳位在最后一项
        state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];
        for _ in 0..10 {
            panel.handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &mut state,
            );
        }
        assert_eq!(panel.selected(), 2, "selected should clamp at last index");
    }

    #[test]
    fn test_osa_sparse_panel_sparsity_color_thresholds() {
        // 验证颜色编码阈值边界:[0.0, 0.3) 绿 / [0.3, 0.7] 黄 / (0.7, 1.0] 红
        assert_eq!(OsaSparsePanel::sparsity_color(0.0_f32), Color::Green);
        assert_eq!(OsaSparsePanel::sparsity_color(0.29_f32), Color::Green);
        assert_eq!(OsaSparsePanel::sparsity_color(0.3_f32), Color::Yellow);
        assert_eq!(OsaSparsePanel::sparsity_color(0.5_f32), Color::Yellow);
        assert_eq!(OsaSparsePanel::sparsity_color(0.7_f32), Color::Yellow);
        assert_eq!(OsaSparsePanel::sparsity_color(0.71_f32), Color::Red);
        assert_eq!(OsaSparsePanel::sparsity_color(1.0_f32), Color::Red);
    }

    #[test]
    fn test_osa_sparse_panel_render_window_empty() {
        let state = TuiState::new();
        let text = OsaSparsePanel::render_window(&state, 0, 0, 20).to_string();
        assert!(text.contains("No active context files"));
    }

    #[test]
    fn test_osa_sparse_panel_render_window_with_data() {
        let mut state = TuiState::new();
        state.osa_context_mask = vec!["main.rs".into(), "lib.rs".into()];
        let text = OsaSparsePanel::render_window(&state, 0, 0, 20).to_string();
        assert!(text.contains("main.rs"));
        assert!(text.contains("lib.rs"));
    }

    #[test]
    fn test_osa_sparse_panel_scroll_to_top_bottom() {
        let mut panel = OsaSparsePanel::new();
        let mut state = TuiState::new();
        state.osa_context_mask = vec!["a".into(), "b".into(), "c".into()];

        panel.scroll_to_bottom(&mut state);
        assert_eq!(panel.selected(), 2);

        panel.scroll_to_top(&mut state);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_osa_sparse_panel_help_key_returns_none() {
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        let mut panel = OsaSparsePanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(panel.handle_key(key, &mut state), None);
    }
}
