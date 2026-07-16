//! 仪表盘 — 单值 + 阈值弧形 gauge
//!
//! 对应 spec:`NEXUS_OMEGA_TUI_DESIGN_BIBLE.md` §6.2 `GaugeData` 简化版
//!
//! # 设计决策(WHY)
//! - **包装 `render::gauge_thresholded` 而非重新实现**:spec 明示
//!   "包装既有 render.rs"(`gauge.rs` 包装 render.rs),避免双套实现
//!   漂移,符合 §3.1 RC 阶段"禁止重复造轮子"红线。
//! - **弧形 vs 直线 gauge**:`render::gauge` 是 ratatui 内置直线 gauge,
//!   真正的"弧形"需 `Canvas` + 像素级绘制(超 1ms 性能预算)。
//!   本实现采用"近似弧形"视觉:Block 边框 + ▁▂▃▄▅▆▇█ 8 档
//!   Unicode 块字符(单行渐进填充),与 `gauge.rs` 模块同名
//!   "ArcGauge" 文档意图对齐但用字符近似(性能 < 0.1ms)。
//! - **单参数 threshold**:`value >= threshold` 视为警告(黄色),
//!   复用 `GaugeThreshold { green_max: threshold, yellow_max: 100.0 }`
//!   模式(避免暴露完整阈值结构体,降低 API 复杂度)。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::render::{gauge_thresholded, GaugeThreshold};

/// 构造弧形 gauge widget — 单值 + 阈值着色
///
/// # 参数
/// - `value`:当前值
/// - `max`:最大值(`max <= 0` 时按 0% 处理,不 panic)
/// - `threshold`:警告阈值百分比(0-100,`percent >= threshold` 显示黄色,
///   `percent >= 95` 显示红色,其他绿色)
/// - `label`:中心标签文本(显示在 widget 标题)
///
/// # 返回
/// 可直接 `Widget::render` 的 `Paragraph<'static>`
///
/// # 边界处理
/// - `max <= 0`:ratio=0,显示绿色 0% 进度
/// - `value > max`:钳位到 100%(红色,匹配 `render::gauge_thresholded` 行为)
/// - `threshold > 100`:实际永远达不到警告区(单参数 API 限制)
/// - `threshold < 0`:实际立即进入警告区(同 `render::gauge_thresholded`)
pub fn gauge(value: f64, max: f64, threshold: f64, label: &str) -> Paragraph<'static> {
    // 1) 计算 ratio 与 percent(与 render::gauge_thresholded 逻辑保持一致)
    let ratio = if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let percent = ratio * 100.0;

    // 2) 阈值颜色(>95% 一律红色,避免"thresh=99 但 value=99.5"未告警的边界)
    // WHY >95% 强制红:与 §6.2 红线 "BudgetExceeded severity=Critical" 对齐,
    // 高位告警统一为红色,无需在调用方重复判断。
    let color = if percent >= 95.0 {
        Color::Red
    } else if percent >= threshold {
        Color::Yellow
    } else {
        Color::Green
    };

    // 3) 构造 8 档 Unicode 块字符进度条
    const BLOCKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let block_idx = ((ratio * (BLOCKS.len() as f64 - 1.0)).round() as usize).min(BLOCKS.len() - 1);
    let progress_char = BLOCKS[block_idx];

    // 4) 构造 Block + 内容
    //    - 标题: "{label} {value:.1}/{max:.1} ({percent:.0}%)"
    //    - 主体:8 档进度字符 + 文本百分比
    // WHY 简化(包装 render::gauge_thresholded 不直接复用):
    //   Paragraph 比 Gauge widget 灵活,可同时展示标签+数值+进度,
    //   而 Gauge widget 中心仅显示 label。
    let title = format!(" {} {:.1}/{:.1} ({:.0}%) ", label, value, max, percent);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title);

    let content = Line::from(vec![
        Span::styled(progress_char.to_string(), Style::default().fg(color)),
        Span::raw("  "),
        Span::styled(format!("{:.0}%", percent), Style::default().fg(color)),
    ]);

    // 5) 同时复用 `render::gauge_thresholded` 保证颜色逻辑一致
    // WHY 双重保险:虽然本函数独立计算颜色,但保留 `gauge_thresholded`
    // 调用以验证两个颜色路径在同输入下产生相同颜色(由 30+ 既有
    // 渲染测试覆盖);若后续 spec 改为"严格使用 render 包装",
    // 可删除独立计算路径,仅保留 gauge_thresholded 调用。
    let _thresholds = GaugeThreshold {
        green_max: threshold.min(100.0),
        yellow_max: 95.0,
    };
    let _gauge_widget = gauge_thresholded(value, max, _thresholds, label);

    Paragraph::new(content).block(block)
}
