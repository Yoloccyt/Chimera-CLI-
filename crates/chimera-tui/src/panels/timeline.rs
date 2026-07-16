//! TUI Timeline 历史回放面板 — 系统运行历史周期快照展示
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:Timeline
//!
//! # 核心职责
//! - 展示 `TuiState.timeline_snapshots` 中的周期快照列表
//! - 支持时间轴滚动(J/K 或方向键)与详情查看(Enter 键)
//! - 底部显示事件速率历史 sparkline
//!
//! # 交互
//! - J/Down: 向下滚动(选中下一个快照)
//! - K/Up: 向上滚动(选中上一个快照)
//! - Enter: 弹出详情 `PopupKind::Detail`
//! - g: 跳到顶部
//! - G: 跳到底部
//!
//! # 设计决策(WHY)
//! - **三段式布局**:顶部摘要(选中快照详情) + 中部列表(虚拟滚动) + 底部 sparkline,
//!   与 Health 面板的"信息+可视化"双栏模式互补 — Timeline 面板纵向布局更适合
//!   时间序列数据的浏览。
//! - **虚拟滚动**:快照容量上限 100(max_snapshots),虽然不算海量,但沿用
//!   `virtual_scroll_window` 保持与 EventStream 面板一致的渲染模式,
//!   为未来扩展更大历史窗口预留性能余量。
//! - **数据源**:`TuiState.timeline_snapshots` — 由 `DataPipeline` 按
//!   `snapshot_interval_s` 周期生成,FIFO 丢弃最旧快照。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::list_state;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::render::{self, virtual_scroll_window};
use crate::types::{PanelId, TimelineSnapshot, TuiCommand, TuiState};

/// Timeline 历史回放面板
///
/// 展示系统运行历史的周期快照,支持时间轴滚动与详情查看。
///
/// WHY 持有 `selected` / `scroll_offset`:与 QuestPanel / EventStreamPanel 保持一致,
/// 面板内部管理列表导航状态,`TuiApp` 通过 Panel trait 统一驱动。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TimelinePanel {
    /// 当前选中快照索引
    selected: usize,
    /// 列表滚动偏移(可见区域起始行)
    scroll_offset: usize,
}

impl TimelinePanel {
    /// 创建新的 Timeline 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中项索引(测试用,与 QuestPanel/EventStreamPanel 模式一致)
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 返回当前滚动偏移(测试用)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// 构建选中快照的摘要文本(顶部区域)
    ///
    /// WHY 独立方法:与 `render_window` 解耦,便于未来单元测试直接验证摘要内容。
    fn summary_text(state: &TuiState, selected: usize) -> Text<'static> {
        let snapshots = &state.timeline_snapshots;
        if snapshots.is_empty() {
            return Text::from(Line::from(Span::styled(
                "No snapshots yet. Waiting for first snapshot...",
                Style::default().fg(Color::DarkGray),
            )));
        }

        let snap = &snapshots[selected];
        let bold = Style::default().add_modifier(Modifier::BOLD);

        Text::from(vec![
            Line::from(vec![
                Span::styled("Selected: ", bold),
                Span::styled(
                    format!("#{} of {}", selected + 1, snapshots.len()),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Line::from(vec![
                Span::styled("Time: ", bold),
                Span::from(snap.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
            ]),
            Line::from(vec![
                Span::styled("Events: ", bold),
                Span::from(format!(
                    "{} total, {}/s rate",
                    snap.event_count, snap.event_rate
                )),
            ]),
            Line::from(vec![
                Span::styled("Budget: ", bold),
                Span::styled(
                    format!("{:.1}%", snap.budget_utilization * 100.0),
                    Style::default().fg(Color::Green),
                ),
                Span::raw("  "),
                Span::styled("Health: ", bold),
                Span::styled(
                    format!("{}/100", snap.health_score),
                    Style::default().fg(Color::Blue),
                ),
                Span::raw("  "),
                Span::styled("Decay: ", bold),
                Span::styled(
                    format!("{:.3}", snap.decay_coefficient),
                    Style::default().fg(Color::Magenta),
                ),
            ]),
        ])
    }

    /// 渲染快照列表的可见窗口(中部区域)
    ///
    /// 使用 `virtual_scroll_window` 仅构造可见区域 + 上下缓冲的 Text,
    /// 确保即使快照数量增大,渲染复杂度仍为 O(visible + 2×BUFFER)。
    fn render_window(
        state: &TuiState,
        selected: usize,
        scroll_offset: usize,
        visible_rows: usize,
    ) -> Text<'static> {
        let snapshots = &state.timeline_snapshots;
        let total = snapshots.len();

        let mut lines: Vec<Line<'static>> = Vec::new();

        if total == 0 {
            lines.push(Line::from(Span::styled(
                "No snapshots yet. Waiting for first snapshot...",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            let (start, end) = virtual_scroll_window(total, scroll_offset, visible_rows);

            for idx in start..end {
                if let Some(snap) = snapshots.get(idx) {
                    let line = format_snapshot_line(snap, idx, idx == selected);
                    lines.push(line);
                }
            }

            // 虚拟滚动提示:总快照数 > 可见窗口时显示总数
            if total > visible_rows {
                lines.push(Line::from(Span::styled(
                    format!(
                        "... showing {} of {} snapshots",
                        end.saturating_sub(start),
                        total
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        Text::from(lines)
    }

    /// 构建快照详情弹窗内容(Enter 键触发)
    fn detail_content(snap: &TimelineSnapshot) -> String {
        format!(
            "Timestamp: {}\nEvent Count: {}\nEvent Rate: {}/s\nBudget Utilization: {:.1}%\nHealth Score: {}/100\nDecay Coefficient: {:.3}",
            snap.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            snap.event_count,
            snap.event_rate,
            snap.budget_utilization * 100.0,
            snap.health_score,
            snap.decay_coefficient,
        )
    }
}

/// 格式化单个快照为列表行
///
/// WHY 独立函数:将"如何渲染单行"与"渲染哪些行"解耦,
/// 便于未来调整行格式(如增加列)时只需修改此处。
fn format_snapshot_line(snap: &TimelineSnapshot, index: usize, is_selected: bool) -> Line<'static> {
    let style = if is_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let prefix = if is_selected { "> " } else { "  " };
    let time = snap.timestamp.format("%H:%M:%S").to_string();

    Line::from(vec![
        Span::styled(format!("{}{:>3}. ", prefix, index + 1), style),
        Span::styled(
            time,
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Cyan)
            },
        ),
        Span::styled(
            format!("  ev/s:{:>4}", snap.event_rate),
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Yellow)
            },
        ),
        Span::styled(
            format!("  bud:{:>3.0}%", snap.budget_utilization * 100.0),
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Green)
            },
        ),
        Span::styled(
            format!("  hp:{:>3}", snap.health_score),
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Blue)
            },
        ),
        Span::styled(
            format!("  dec:{:.2}", snap.decay_coefficient),
            if is_selected {
                style
            } else {
                Style::default().fg(Color::Magenta)
            },
        ),
    ])
}

impl Panel for TimelinePanel {
    fn id(&self) -> PanelId {
        PanelId::Timeline
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Timeline ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let snapshots = &state.timeline_snapshots;
        let count = snapshots.len();
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

        // 三段式纵向布局:顶部摘要(4行) + 中部列表(弹性) + 底部 sparkline(5行)
        //
        // WHY 固定顶部 4 行:摘要固定 4 行(选中信息/时间/事件/指标行),
        // 底部 sparkline 5 行(边框 2 + 内容 3),中部列表取剩余空间。
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(5),
                Constraint::Length(5),
            ])
            .split(inner);

        // === 顶部:选中快照摘要 ===
        let summary = Paragraph::new(Self::summary_text(state, self.selected));
        summary.render(chunks[0], buf);

        // === 中部:快照列表(虚拟滚动) ===
        let content_height = chunks[1].height as usize;
        // 列表内容高度需扣除虚拟滚动提示行(1行)
        let visible_rows = content_height.saturating_sub(1);
        self.scroll_offset =
            list_state::adjust_scroll(self.selected, self.scroll_offset, visible_rows);

        let list_content =
            Self::render_window(state, self.selected, self.scroll_offset, visible_rows);
        let list_paragraph = Paragraph::new(list_content);
        list_paragraph.render(chunks[1], buf);

        // === 底部:事件速率 sparkline ===
        // WHY 从 timeline_snapshots 提取 event_rate 作为 sparkline 数据点:
        // 展示历史事件速率趋势,与顶部摘要的"当前速率"互补。
        let rate_data: Vec<u64> = snapshots.iter().map(|s| s.event_rate).collect();
        let sparkline = render::sparkline(&rate_data, "Event Rate History", Color::Yellow);
        sparkline.render(chunks[2], buf);
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.timeline_snapshots.len();

        // 优先处理方向键/JK 导航(复用 list_state 模块)
        if let Some(new_selected) =
            list_state::handle_key_navigation(key.code, self.selected, count)
        {
            self.selected = new_selected;
            return None;
        }

        // WHY 额外处理 j/k:list_state::handle_key_navigation 只处理 Up/Down,
        // 不处理 j/k(后者不是所有面板都支持)。Timeline 面板遵循 vim 风格,
        // 额外映射 j/k 到 Down/Up。
        match key.code {
            KeyCode::Char('j') => {
                self.selected = list_state::move_selection(self.selected, 1, count);
                None
            }
            KeyCode::Char('k') => {
                self.selected = list_state::move_selection(self.selected, -1, count);
                None
            }
            KeyCode::Enter => {
                // 弹出选中快照的详情弹窗;空列表时返回 None
                state.timeline_snapshots.get(self.selected).map(|snap| {
                    let content = Self::detail_content(snap);
                    TuiCommand::OpenPopup(PopupKind::Detail {
                        title: format!("Timeline Snapshot #{}", self.selected + 1),
                        content,
                        scroll: 0,
                    })
                })
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
        let count = state.timeline_snapshots.len();
        self.selected = if count == 0 { 0 } else { count - 1 };
        self.scroll_offset = self.selected;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, state: &mut TuiState) -> Option<TuiCommand> {
        let count = state.timeline_snapshots.len();
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
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyModifiers};

    /// 构造测试用 TimelineSnapshot
    fn make_snapshot(event_count: u64, event_rate: u64) -> TimelineSnapshot {
        TimelineSnapshot {
            timestamp: Utc::now(),
            event_count,
            event_rate,
            budget_utilization: 0.35,
            health_score: 90,
            decay_coefficient: 0.85,
        }
    }

    #[test]
    fn test_timeline_panel_id() {
        let panel = TimelinePanel::new();
        assert_eq!(panel.id(), PanelId::Timeline);
    }

    #[test]
    fn test_timeline_panel_title() {
        let panel = TimelinePanel::new();
        assert_eq!(panel.title().to_string(), " Timeline ");
    }

    #[test]
    fn test_timeline_panel_empty_summary() {
        let state = TuiState::new();
        let text = TimelinePanel::summary_text(&state, 0).to_string();
        assert!(text.contains("No snapshots yet"));
    }

    #[test]
    fn test_timeline_panel_summary_with_data() {
        let mut state = TuiState::new();
        state.timeline_snapshots.push(make_snapshot(42, 10));
        let text = TimelinePanel::summary_text(&state, 0).to_string();
        assert!(text.contains("42"));
        assert!(text.contains("10"));
        assert!(text.contains("35.0%"));
    }

    #[test]
    fn test_timeline_panel_render_window_empty() {
        let state = TuiState::new();
        let text = TimelinePanel::render_window(&state, 0, 0, 20).to_string();
        assert!(text.contains("No snapshots yet"));
    }

    #[test]
    fn test_timeline_panel_render_window_with_data() {
        let mut state = TuiState::new();
        for i in 0..5 {
            state.timeline_snapshots.push(make_snapshot(i, i * 10));
        }
        let text = TimelinePanel::render_window(&state, 0, 0, 20).to_string();
        // 应包含第一个快照的事件速率(0)
        assert!(text.contains("ev/s:"));
    }

    #[test]
    fn test_timeline_panel_detail_content() {
        let snap = make_snapshot(42, 10);
        let content = TimelinePanel::detail_content(&snap);
        assert!(content.contains("42"));
        assert!(content.contains("10"));
        assert!(content.contains("35.0%"));
        assert!(content.contains("90"));
        assert!(content.contains("0.850"));
    }

    #[test]
    fn test_timeline_panel_navigation_j_k() {
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();
        for i in 0..3 {
            state.timeline_snapshots.push(make_snapshot(i, i * 10));
        }

        // j 向下
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 1);

        // k 向上
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_timeline_panel_enter_returns_detail() {
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();
        state.timeline_snapshots.push(make_snapshot(42, 10));

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
        );
        match cmd {
            Some(TuiCommand::OpenPopup(PopupKind::Detail { title, content, .. })) => {
                assert!(title.contains("Timeline"));
                assert!(content.contains("42"));
            }
            other => panic!("expected OpenPopup(Detail), got {:?}", other),
        }
    }

    #[test]
    fn test_timeline_panel_enter_empty_returns_none() {
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();

        let cmd = panel.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
        );
        assert!(cmd.is_none());
    }

    #[test]
    fn test_timeline_panel_scroll_to_top_bottom() {
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();
        for i in 0..5 {
            state.timeline_snapshots.push(make_snapshot(i, i * 10));
        }

        panel.scroll_to_bottom(&mut state);
        assert_eq!(panel.selected(), 4);

        panel.scroll_to_top(&mut state);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_timeline_panel_clamp_selected() {
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();
        // 初始 selected=0,无快照
        assert_eq!(panel.selected(), 0);
        // render 会钳位 selected,空列表时保持 0
        // (通过 handle_key 间接验证不 panic)
        panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
        assert_eq!(panel.selected(), 0);
    }

    #[test]
    fn test_timeline_panel_help_key_returns_none() {
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        let mut panel = TimelinePanel::new();
        let mut state = TuiState::new();
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(panel.handle_key(key, &mut state), None);
    }
}
