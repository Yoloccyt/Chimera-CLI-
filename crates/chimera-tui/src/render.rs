//! TUI 渲染辅助函数 — 统一 Sparkline/Gauge/进度条等可视化组件
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 将常用可视化组件抽取为纯函数,避免各面板重复构造 ratatui widget。
//! - 辅助函数接收原始数值与主题色,返回可直接 `render` 的 widget,
//!   保持面板代码聚焦于业务布局。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Sparkline};

/// 通用帮助页脚文本
///
/// WHY 提取常量:Quest 面板与 Parliament 面板原使用相同文案,
/// 避免两处维护导致不一致。
pub const FOOTER_TEXT: &str = "Press Tab to switch panels, ':' for commands, 'q' to quit.";

/// 构造 Sparkline widget
///
/// # 参数
/// - `data`: 历史数据点
/// - `title`: 图表标题
/// - `color`: 折线颜色
pub fn sparkline(data: &[u64], title: &str, color: Color) -> Sparkline<'static> {
    Sparkline::default()
        .data(data)
        .style(Style::default().fg(color))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
}

/// 构造 Gauge widget
///
/// # 参数
/// - `value`: 当前值
/// - `max`: 最大值(必须 > 0,否则按 0 处理)
/// - `label`: 中心标签文本
/// - `color`: 填充颜色
pub fn gauge(value: f64, max: f64, label: &str, color: Color) -> Gauge<'static> {
    let ratio = if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Gauge::default()
        .percent((ratio * 100.0) as u16)
        .label(label.to_string())
        .gauge_style(Style::default().fg(color))
        .block(Block::default().borders(Borders::ALL))
}

/// 构造利用率进度条文本行
///
/// # 参数
/// - `value`: 当前值
/// - `max`: 最大值(必须 > 0)
/// - `width`: 进度条内部宽度(不含中括号与标签)
///
/// 返回形如 `[====------] 40.0%` 的 `Line`,已用部分为青色,未用部分为灰色。
pub fn utilization_bar(value: f64, max: f64, width: usize) -> Line<'static> {
    let ratio = if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let used = ((ratio * width as f64).round() as usize).min(width);
    let remaining = width.saturating_sub(used);
    let pct = ratio * 100.0;

    Line::from(vec![
        Span::from("["),
        Span::styled("=".repeat(used), Style::default().fg(Color::Cyan)),
        Span::styled("-".repeat(remaining), Style::default().fg(Color::Gray)),
        Span::from(format!("] {:.1}%", pct)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utilization_bar_full() {
        let line = utilization_bar(100.0, 100.0, 10);
        let text = line.to_string();
        assert!(text.contains("100.0%"));
        assert!(text.contains("=========="));
    }

    #[test]
    fn test_utilization_bar_zero() {
        let line = utilization_bar(0.0, 100.0, 10);
        let text = line.to_string();
        assert!(text.contains("0.0%"));
        assert!(text.contains("----------"));
    }

    #[test]
    fn test_utilization_bar_clamped() {
        // 超过 max 应该被钳位到 100%
        let line = utilization_bar(150.0, 100.0, 10);
        let text = line.to_string();
        assert!(text.contains("100.0%"));
    }

    #[test]
    fn test_gauge_full() {
        let g = gauge(100.0, 100.0, "full", Color::Green);
        // Gauge 没有直接公开内部 percent,通过构造不 panic 即可
        let _ = g;
    }

    #[test]
    fn test_sparkline_empty() {
        let s = sparkline(&[], "empty", Color::Yellow);
        let _ = s;
    }

    #[test]
    fn test_footer_text_constant() {
        assert!(FOOTER_TEXT.contains("Tab"));
        assert!(FOOTER_TEXT.contains(":"));
        assert!(FOOTER_TEXT.contains("q"));
    }
}
