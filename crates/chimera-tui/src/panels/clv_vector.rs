//! TUI CLV 向量可视化面板 — 512 维潜在向量摘要展示
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:ClvVector
//!
//! # 核心职责
//! - 展示 `TuiState.clv_summary` 中的 CLV (Context Latent Vector) 摘要
//! - 三段式纵向布局:L2 范数 Gauge + 8 分块 heat_bar 热图 + Top-8 维度表
//! - 支持方向键/J/K 滚动 Top-8 维度表,g/G 跳顶/跳底
//!
//! # 交互
//! - Up/Down: 滚动 Top-8 维度表
//! - j/k: vim 风格上下滚动(与 Timeline 面板一致)
//! - g: 跳到顶部
//! - G: 跳到底部
//!
//! # 设计决策(WHY)
//! - **三段式纵向布局**:顶部 L2 范数 Gauge(标量概览) + 中部 8 分块热图(空间分布)
//!   + 底部 Top-8 维度表(细粒度详情),由粗到细呈现 CLV 信息,与 Timeline 面板
//!     的"摘要+列表+sparkline"模式互补。
//! - **复用 heat_bar**:与 OsaSparse 面板共享 `render::heat_bar`,保持热图风格一致。
//! - **双颜色编码**:heat_bar 自身按值域 25%/75% 分桶着色表达强度;块均值文本用
//!   `color_for_block_mean` 按 0 阈值对称着色表达"激活/抑制"语义,两者互补。
//! - **数据源**:`TuiState.clv_summary` — 由 `DataPipeline` 从
//!   `NexusEvent::ClvSnapshotReported` 同步而来。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::render;
use crate::types::{PanelId, TuiCommand, TuiState};

/// CLV 向量可视化面板
///
/// 展示 CLV (Context Latent Vector, 512 维潜在向量) 的摘要信息:
/// L2 范数、8 分块均值热图、Top-8 维度。
///
/// WHY 持有 `selected` / `scroll_offset`:与 TimelinePanel / QuestPanel 保持一致,
/// 面板内部管理 Top-8 维度表的导航状态,`TuiApp` 通过 Panel trait 统一驱动。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClvVectorPanel {
    /// 当前选中的 Top-8 维度索引
    selected: usize,
    /// 列表滚动偏移(可见区域起始行)
    scroll_offset: usize,
}

impl ClvVectorPanel {
    /// 创建新的 ClvVector 面板
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

    /// 根据块均值返回颜色编码
    ///
    /// WHY 单独提取:heat_bar 自身按值域 25%/75% 分桶着色(蓝/灰/红),
    /// 但块均值的"激活/抑制"语义需要按 0 阈值对称着色 — 负值(抑制)用蓝,
    /// 正值(激活)用红,中性用灰。此函数用于块均值数值文本的着色,
    /// 与 heat_bar 的强度着色互补(前者表达语义,后者表达强度)。
    ///
    /// # 颜色编码规则
    /// - mean < -0.5: 蓝色(显著抑制)
    /// - -0.5 ≤ mean ≤ 0.5: 灰色(中性)
    /// - mean > 0.5: 红色(显著激活)
    fn color_for_block_mean(mean: f32) -> Color {
        if mean < -0.5 {
            Color::Blue
        } else if mean > 0.5 {
            Color::Red
        } else {
            Color::Gray
        }
    }

    /// 格式化 Top-8 维度行
    ///
    /// # 参数
    /// - `index`: 列表行索引(从 0 开始,用于显示"第 N 项")
    /// - `dim`: CLV 维度索引(0-511)
    /// - `value`: 该维度的值
    /// - `is_selected`: 是否为当前选中行(决定是否高亮)
    ///
    /// # 返回
    /// 格式如 `> 1. dim=127  value=+0.8321` 的 Line,
    /// 选中时整行黄底黑字高亮,未选中时维度索引青色、数值按符号着色。
    fn format_top_dim_line(
        index: usize,
        dim: usize,
        value: f32,
        is_selected: bool,
    ) -> Line<'static> {
        let highlight_style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let prefix = if is_selected { "> " } else { "  " };

        // WHY 数值按符号着色:正激活红/负抑制蓝/近零灰,
        // 与 color_for_block_mean 阈值一致,保持语义连贯
        let value_color = if value > 0.5 {
            Color::Red
        } else if value < -0.5 {
            Color::Blue
        } else {
            Color::Gray
        };

        Line::from(vec![
            Span::styled(format!("{}{:>2}. ", prefix, index + 1), highlight_style),
            Span::styled(
                format!("dim={:<3}", dim),
                if is_selected {
                    highlight_style
                } else {
                    Style::default().fg(Color::Cyan)
                },
            ),
            Span::raw("  "),
            Span::styled(
                format!("value={:+.4}", value),
                if is_selected {
                    highlight_style
                } else {
                    Style::default().fg(value_color)
                },
            ),
        ])
    }
}

impl Panel for ClvVectorPanel {
    fn id(&self) -> PanelId {
        PanelId::ClvVector
    }

    fn title(&self) -> Line<'static> {
        Line::from(" CLV Vector ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        // 构造面板边框
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        // 终端过小时不渲染内容,避免布局计算溢出
        if inner.height < 6 || inner.width < 20 {
            return;
        }

        // 三段式纵向布局:顶部 L2 范数 Gauge(3行) + 中部 8 分块热图(9行) + 底部 Top-8 维度表(弹性)
        //
        // WHY 固定顶部 3 行:Gauge 边框 2 + 内容 1;
        // 中部 9 行:1 标题行 + 8 个块行;
        // 底部取剩余空间展示 Top-8 维度表(可滚动)。
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(9),
                Constraint::Min(4),
            ])
            .split(inner);

        // 提取 CLV 摘要(可能为 None — 未收到 NMC 编码器事件)
        let summary = state.clv_summary.as_ref();

        // === 顶部:L2 范数 Gauge ===
        //
        // WHY 范数上限 10.0:CLV 为 512 维潜在向量,L2 范数典型范围 [0, 10],
        // 超过 10 视为异常(render::gauge 内部会 clamp 到 100%)。
        // 颜色按强度梯度:绿(<3)→黄(3-7)→红(>7),一眼识别范数量级。
        let (l2_norm, l2_color) = match summary {
            Some(s) => {
                let norm = s.l2_norm;
                let color = if norm < 3.0 {
                    Color::Green
                } else if norm < 7.0 {
                    Color::Yellow
                } else {
                    Color::Red
                };
                (norm, color)
            }
            None => (0.0, Color::DarkGray),
        };
        let l2_label = if summary.is_some() {
            format!(" L2 Norm: {:.4} ", l2_norm)
        } else {
            " L2 Norm: -- ".to_string()
        };
        let l2_gauge = render::gauge(l2_norm as f64, 10.0, &l2_label, l2_color);
        l2_gauge.render(chunks[0], buf);

        // === 中部:8 分块 heat_bar 热图 ===
        let mut heatmap_lines: Vec<Line<'static>> = Vec::with_capacity(9);
        heatmap_lines.push(Line::from(Span::styled(
            "Block Means (8 x 64-dim blocks):",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        match summary {
            Some(s) => {
                // WHY 安全遍历 8 块:ClvSummary.block_means 理论上为 8 元素,
                // 但运行时可能因同步延迟少于 8 个,用 .get(i) 防御性访问避免越界。
                for i in 0..8usize {
                    let mean = s.block_means.get(i).copied().unwrap_or(0.0);
                    // heat_bar 返回 Line<'static>(含 2 个 Span:filled + empty),
                    // 需用 .spans 展开到当前行的 Span 列表中,不能直接放入 vec![Span, ...]。
                    let heat = render::heat_bar(mean as f64, -1.0, 1.0, 20);
                    let mean_color = Self::color_for_block_mean(mean);
                    let mut spans = vec![Span::styled(
                        format!("B{} ", i),
                        Style::default().fg(Color::Cyan),
                    )];
                    spans.extend(heat.spans);
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("{:+.4}", mean),
                        Style::default().fg(mean_color),
                    ));
                    let row = Line::from(spans);
                    heatmap_lines.push(row);
                }
            }
            None => {
                // 空状态:8 个灰色空块,提示等待 NMC 编码器
                for i in 0..8usize {
                    let row = Line::from(vec![
                        Span::styled(format!("B{} ", i), Style::default().fg(Color::DarkGray)),
                        Span::styled("·".repeat(20), Style::default().fg(Color::DarkGray)),
                        Span::raw("  "),
                        Span::styled("waiting...", Style::default().fg(Color::DarkGray)),
                    ]);
                    heatmap_lines.push(row);
                }
            }
        }
        let heatmap = Paragraph::new(Text::from(heatmap_lines));
        heatmap.render(chunks[1], buf);

        // === 底部:Top-8 维度表(可滚动) ===
        let content_height = chunks[2].height as usize;
        // 列表内容高度需扣除标题行(1行)
        let visible_rows = content_height.saturating_sub(1);

        // WHY 提取 top_dims 切片:避免在闭包中持有对 state.clv_summary 的引用,
        // 同时统一空状态与有数据状态的处理路径
        let top_dims: &[(usize, f32)] = summary.map(|s| s.top_dims.as_slice()).unwrap_or(&[]);
        let count = top_dims.len();

        // 钳位选中索引到有效范围(数据变化时同步)
        self.selected = list_state::clamp_selected(self.selected, count);
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, visible_rows);

        let mut dim_lines: Vec<Line<'static>> = Vec::new();
        dim_lines.push(Line::from(Span::styled(
            "Top Dimensions (by |value|):",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        if count == 0 {
            // 空状态:无 Top 维度数据
            dim_lines.push(Line::from(Span::styled(
                if summary.is_some() {
                    "No top dimensions available."
                } else {
                    "Waiting for NMC encoder..."
                },
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            // WHY 复用 virtual_scroll_window:Top-8 最多 8 项,虽不需要虚拟滚动性能优化,
            // 但沿用 Timeline/EventStream 模式保持一致性,且为未来扩展更多维度预留
            let (start, end) =
                render::virtual_scroll_window(count, self.scroll_offset, visible_rows);
            for idx in start..end {
                if let Some((dim, value)) = top_dims.get(idx) {
                    dim_lines.push(Self::format_top_dim_line(
                        idx,
                        *dim,
                        *value,
                        idx == self.selected,
                    ));
                }
            }
            // 虚拟滚动提示:总维度数 > 可见窗口时显示总数
            if count > visible_rows {
                dim_lines.push(Line::from(Span::styled(
                    format!(
                        "... showing {} of {} dimensions",
                        end.saturating_sub(start),
                        count
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        let dim_table = Paragraph::new(Text::from(dim_lines));
        dim_table.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        // 获取 Top-8 维度列表长度(count=0 时空列表,导航操作保持 selected=0)
        let count = state
            .clv_summary
            .as_ref()
            .map(|s| s.top_dims.len())
            .unwrap_or(0);

        // 优先处理方向键导航(复用 list_state 模块)
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        // WHY 额外处理 j/k:list_state::handle_key_navigation 只处理 Up/Down,
        // 不处理 j/k(后者不是所有面板都支持)。ClvVector 面板遵循 vim 风格,
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
        let count = state
            .clv_summary
            .as_ref()
            .map(|s| s.top_dims.len())
            .unwrap_or(0);
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state
            .clv_summary
            .as_ref()
            .map(|s| s.top_dims.len())
            .unwrap_or(0);
        if let Some(new_selected) =
            list_state::handle_mouse_scroll(mouse.kind, self.selected, count)
        {
            self.selected = new_selected;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_clv_vector_panel_id() {
        let panel = ClvVectorPanel::new();
        assert_eq!(panel.id(), PanelId::ClvVector);
    }

    #[test]
    fn test_clv_vector_panel_title() {
        let panel = ClvVectorPanel::new();
        assert_eq!(panel.title().to_string(), " CLV Vector ");
    }

    #[test]
    fn test_clv_vector_panel_empty_state() {
        // 空状态(clv_summary = None)下 handle_key 不应 panic,返回 None
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();
        assert!(state.clv_summary.is_none());

        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let result = panel.handle_key(key, &mut state);
        assert!(result.is_none(), "Down on empty list should return None");
        assert_eq!(panel.selected(), 0, "selected should stay at 0 on empty");
    }

    #[test]
    fn test_clv_vector_panel_navigation() {
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();
        // 构造含 Top-3 维度的 CLV 摘要
        state.clv_summary = Some(event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(10, 0.5), (20, -0.3), (30, 0.8)],
        });

        // j 向下:0 → 1
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 1);

        // k 向上:1 → 0
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_clv_vector_panel_color_for_block_mean() {
        // 蓝色:显著抑制(mean < -0.5)
        assert_eq!(ClvVectorPanel::color_for_block_mean(-0.6), Color::Blue);
        assert_eq!(ClvVectorPanel::color_for_block_mean(-1.0), Color::Blue);

        // 灰色:中性(-0.5 ≤ mean ≤ 0.5)
        assert_eq!(ClvVectorPanel::color_for_block_mean(0.0), Color::Gray);
        assert_eq!(ClvVectorPanel::color_for_block_mean(-0.5), Color::Gray);
        assert_eq!(ClvVectorPanel::color_for_block_mean(0.5), Color::Gray);

        // 红色:显著激活(mean > 0.5)
        assert_eq!(ClvVectorPanel::color_for_block_mean(0.6), Color::Red);
        assert_eq!(ClvVectorPanel::color_for_block_mean(1.0), Color::Red);
    }

    #[test]
    fn test_clv_vector_panel_default() {
        let panel = ClvVectorPanel::default();
        assert_eq!(panel.id(), PanelId::ClvVector);
        assert_eq!(panel.selected(), 0);
        assert_eq!(panel.scroll_offset(), 0);
    }

    #[test]
    fn test_clv_vector_panel_scroll_to_top_bottom() {
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();
        state.clv_summary = Some(event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(0, 0.5), (1, 0.4), (2, 0.3), (3, 0.2), (4, 0.1)],
        });

        // 跳到底部:selected 应为最后一项索引
        panel.scroll_to_bottom(&mut state);
        assert_eq!(panel.selected(), 4);

        // 跳到顶部:selected 应归零
        panel.scroll_to_top(&mut state);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_clv_vector_panel_scroll_to_bottom_empty() {
        // 空状态跳底应保持 0,不 panic
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();

        panel.scroll_to_bottom(&mut state);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_clv_vector_panel_format_top_dim_line_selected() {
        let line = ClvVectorPanel::format_top_dim_line(0, 127, 0.8321, true);
        let text = line.to_string();
        // 选中时前缀为 ">"
        assert!(text.contains(">"), "selected line should have '>' prefix");
        assert!(text.contains("dim=127"));
        assert!(text.contains("0.8321"));
    }

    #[test]
    fn test_clv_vector_panel_format_top_dim_line_not_selected() {
        let line = ClvVectorPanel::format_top_dim_line(2, 64, -0.5, false);
        let text = line.to_string();
        // 未选中时无 ">" 前缀
        assert!(
            !text.contains("> 3."),
            "unselected line should not have '>' prefix"
        );
        assert!(text.contains("dim=64"));
        assert!(text.contains("-0.5000"));
    }

    #[test]
    fn test_clv_vector_panel_help_key_returns_none() {
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(panel.handle_key(key, &mut state), None);
    }

    #[test]
    fn test_clv_vector_panel_navigation_clamp_at_bottom() {
        // 连续按 j 超过列表长度,应钳位在最后一项
        let mut panel = ClvVectorPanel::new();
        let mut state = TuiState::new();
        state.clv_summary = Some(event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: vec![(0, 0.5), (1, 0.4), (2, 0.3)],
        });

        for _ in 0..10 {
            panel.handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                &mut state,
            );
        }
        assert_eq!(
            panel.selected(),
            2,
            "selected should clamp at last index (2)"
        );
    }
}
