//! TUI Health 面板 — 显示事件速率、慢消费者、平均延迟与健康评分
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 数据驱动:从 `TuiState.health_metrics` 与 `event_rate_history` 读取。
//! - 健康评分使用 Gauge 直观展示 0-100 区间。
//! - 事件速率使用 Sparkline 展示近期趋势。

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::render::{self, FOOTER_TEXT};
use crate::types::{PanelId, SystemMetrics, TuiCommand, TuiState};

/// Health 面板
#[derive(Debug, Clone)]
pub struct HealthPanel {
    /// 上一 tick 的系统指标，用于计算趋势箭头
    last_sys_metrics: SystemMetrics,
}

impl HealthPanel {
    /// 创建新的 Health 面板
    pub fn new() -> Self {
        Self {
            last_sys_metrics: SystemMetrics::default(),
        }
    }
}

impl Default for HealthPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthPanel {
    /// 根据健康评分返回阈值颜色
    ///
    /// WHY 提取辅助函数:消除 `info_text` 与 `render` 中重复的分段阈值逻辑,
    /// 避免未来调整阈值时遗漏一处。
    fn health_score_color(score: u8) -> Color {
        if score >= 80 {
            Color::Green
        } else if score >= 50 {
            Color::Yellow
        } else {
            Color::Red
        }
    }

    /// 计算趋势箭头 — 与上一 tick 的值比较
    ///
    /// ↑ 上升（资源压力增大），↓ 下降（资源压力减小），→ 持平（变化 < 5%）
    fn trend_arrow(current: f32, previous: f32) -> &'static str {
        if previous == 0.0 {
            return "→"; // 首次采样
        }
        let change = (current - previous).abs();
        let pct = if previous > 0.0 {
            change / previous
        } else {
            1.0
        };
        if pct < 0.05 {
            "→"
        } else if current > previous {
            "↑"
        } else {
            "↓"
        }
    }

    /// 阈值着色 — 根据值与阈值决定颜色
    fn threshold_color(value: f32, warn: f32, crit: f32) -> Color {
        if value >= crit {
            Color::Red
        } else if value >= warn {
            Color::Yellow
        } else {
            Color::Green
        }
    }

    /// 构建左侧信息文本
    fn info_text(state: &TuiState) -> Text<'static> {
        let hm = &state.health_metrics;
        let health_color = Self::health_score_color(hm.health_score);

        let lines = vec![
            Line::from(vec![
                Span::styled(
                    "Events/sec: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1}", hm.events_per_second)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Slow Consumers: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(hm.slow_consumer_count.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    "Avg Latency: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(format!("{:.1} ms", hm.average_latency_ms)),
            ]),
            // Active/Paused Quests 指标:从 quest_list 与 paused_quest_count 派生,
            // 反映系统当前 Quest 负载与暂停状态(不新增事件,复用已有 QuestPaused/QuestResumed)
            Line::from(vec![
                Span::styled(
                    "Active Quests: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(state.quest_list.len().to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    "Paused Quests: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::from(state.paused_quest_count.to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    "Health Score: ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    hm.health_score.to_string(),
                    Style::default().fg(health_color),
                ),
            ]),
            Line::from(""),
            Line::from(FOOTER_TEXT),
        ];
        Text::from(lines)
    }

    /// 渲染系统资源摘要行 — CPU/RAM/Disk/Net 趋势 + 颜色
    fn render_sys_summary(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let sys = &state.sys_metrics;
        let last = &self.last_sys_metrics;

        // 构造各段 Span
        let mut spans: Vec<Span<'static>> = Vec::new();

        // CPU: global_usage + 趋势箭头
        {
            let trend = Self::trend_arrow(sys.cpu.global_usage, last.cpu.global_usage);
            let color = Self::threshold_color(sys.cpu.global_usage, 60.0, 80.0);
            spans.push(Span::styled(
                format!("CPU {:.0}%{}", sys.cpu.global_usage, trend),
                Style::default().fg(color),
            ));
        }

        // RAM: used/total GB + 趋势箭头
        {
            let ram_used_gb = sys.memory.used_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
            let ram_total_gb = sys.memory.total_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
            let trend = Self::trend_arrow(sys.memory.usage_percent, last.memory.usage_percent);
            let color = Self::threshold_color(sys.memory.usage_percent, 70.0, 90.0);
            spans.push(Span::styled(
                format!("RAM {:.1}/{:.1} GB{}", ram_used_gb, ram_total_gb, trend),
                Style::default().fg(color),
            ));
        }

        // Disk: 读写 MB/s
        {
            let disk_r_mb = sys.disk.read_bytes_per_sec as f64 / 1024.0 / 1024.0;
            let disk_w_mb = sys.disk.write_bytes_per_sec as f64 / 1024.0 / 1024.0;
            let current_io = (sys.disk.read_bytes_per_sec + sys.disk.write_bytes_per_sec) as f32;
            let last_io = (last.disk.read_bytes_per_sec + last.disk.write_bytes_per_sec) as f32;
            let trend = Self::trend_arrow(current_io, last_io);
            spans.push(Span::styled(
                format!("Disk R{:.1} W{:.1} MB/s{}", disk_r_mb, disk_w_mb, trend),
                Style::default().fg(Color::Gray),
            ));
        }

        // Net: ↓RX ↑TX MB/s
        {
            let net_rx_mb = sys.network.rx_bytes_per_sec as f64 / 1024.0 / 1024.0;
            let net_tx_mb = sys.network.tx_bytes_per_sec as f64 / 1024.0 / 1024.0;
            let trend = Self::trend_arrow(
                sys.network.rx_bytes_per_sec as f32,
                last.network.rx_bytes_per_sec as f32,
            );
            spans.push(Span::styled(
                format!("Net ↓{:.1} ↑{:.1} MB/s{}", net_rx_mb, net_tx_mb, trend),
                Style::default().fg(Color::Gray),
            ));
        }

        // 用 " | " 拼接:构建含分隔符的 Line
        let mut line_spans: Vec<Span<'static>> = Vec::with_capacity(spans.len() * 2);
        for (i, span) in spans.into_iter().enumerate() {
            if i > 0 {
                line_spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }
            line_spans.push(span);
        }
        let line = Line::from(line_spans);
        let summary = Paragraph::new(line)
            .style(Style::default())
            .alignment(ratatui::layout::Alignment::Center);

        summary.render(area, buf);

        // 更新上一 tick 缓存
        self.last_sys_metrics = state.sys_metrics.clone();
    }
}

impl Panel for HealthPanel {
    fn id(&self) -> PanelId {
        PanelId::Health
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Health ")
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 6 || inner.width < 20 {
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        // 左:文字指标
        let info = Paragraph::new(Self::info_text(state));
        info.render(chunks[0], buf);

        // 右:健康评分 Gauge + 事件速率 Sparkline
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(5)])
            .split(chunks[1]);

        let score_color = Self::health_score_color(state.health_metrics.health_score);
        let gauge = render::gauge(
            state.health_metrics.health_score as f64,
            100.0,
            &format!("{} / 100", state.health_metrics.health_score),
            score_color,
        );
        gauge.render(right_chunks[0], buf);

        let sparkline =
            render::sparkline(&state.event_rate_history, "Event Rate History", Color::Cyan);
        sparkline.render(right_chunks[1], buf);

        // 底部系统资源摘要行
        if inner.height > 2 {
            let summary_y = inner.bottom().saturating_sub(1);
            let summary_area = Rect::new(inner.x, summary_y, inner.width, 1);
            self.render_sys_summary(state, summary_area, buf);
        }
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("R", "刷新")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::HealthMetrics;
    use crate::types::TuiState;

    #[test]
    fn test_health_panel_id() {
        let panel = HealthPanel::new();
        assert_eq!(panel.id(), PanelId::Health);
    }

    #[test]
    fn test_health_panel_renders_metrics() {
        let mut state = TuiState::new();
        state.health_metrics = HealthMetrics {
            events_per_second: 42.0,
            slow_consumer_count: 1,
            average_latency_ms: 15.5,
            health_score: 90,
        };
        state.event_rate_history = vec![30, 35, 40, 38, 42, 45, 42];

        let content = HealthPanel::info_text(&state).to_string();
        assert!(content.contains("42.0"));
        assert!(content.contains("1"));
        assert!(content.contains("15.5 ms"));
        assert!(content.contains("90"));
    }

    #[test]
    fn test_health_panel_score_colors() {
        let mut state = TuiState::new();
        state.health_metrics.health_score = 30;
        let content = HealthPanel::info_text(&state).to_string();
        assert!(content.contains("30"));
    }
}
