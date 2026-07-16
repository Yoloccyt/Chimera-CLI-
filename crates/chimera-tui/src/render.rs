//! TUI 渲染辅助函数 — 统一 Sparkline/Gauge/进度条等可视化组件
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 将常用可视化组件抽取为纯函数,避免各面板重复构造 ratatui widget。
//! - 辅助函数接收原始数值与主题色,返回可直接 `render` 的 widget,
//!   保持面板代码聚焦于业务布局。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Sparkline};

/// 通用帮助页脚文本
///
/// WHY 提取常量:Quest 面板与 Parliament 面板原使用相同文案,
/// 避免两处维护导致不一致。
pub const FOOTER_TEXT: &str = "Press Tab to switch panels, ':' for commands, 'q' to quit.";

/// 虚拟滚动缓冲行数 — 上下各保留 5 行
///
/// WHY 5 行:过小会导致快速滚动时出现空白闪烁(用户视线下移时缓冲已耗尽),
/// 过大则削弱虚拟滚动的性能优势(渲染行数 = visible + 2×BUFFER)。
/// 5 行在 120x40 终端下约占 12% 额外渲染量,既能吸收单次滚轮多行事件,
/// 又保持 O(visible + 2×BUFFER) 复杂度。参考 Claude Code 的 heightCache
/// 缓冲策略(流式输出场景验证)。
pub const VIRTUAL_SCROLL_BUFFER: usize = 5;

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

/// 构造延迟统计行(P50/P95/P99 三列横向对比)
///
/// # 参数
/// - `label`: 行前缀标签(如路由器名称 "KVBSR"/"SESA"/"FaaE")
/// - `p50`: P50 延迟(微秒)— 中位数,反映典型体验
/// - `p95`: P95 延迟(微秒)— 多数用户的上限
/// - `p99`: P99 延迟(微秒)— 尾部异常
///
/// # 设计决策(WHY)
/// P50/P95/P99 三列横向对比:运维常需同时观察 P50(中位数)与 P95/P99(尾部)
/// 的差距来判断长尾延迟严重程度,横排比纵排更易快速扫读。P50 反映典型体验,
/// P95 反映多数用户的上限,P99 反映尾部异常,三者并列可一眼识别延迟分布形态
/// (如 P99 远大于 P50 表示长尾问题)。同时复用此函数避免各面板重复拼字符串。
pub fn latency_line(label: &str, p50: u64, p95: u64, p99: u64) -> Line<'static> {
    Line::from(format!(
        "{}  Latency  P50: {}μs  P95: {}μs  P99: {}μs",
        label, p50, p95, p99,
    ))
}

/// 虚拟滚动窗口计算 — 仅返回可见区域 + 上下缓冲行的事件索引范围
///
/// 给定总条目数、滚动偏移与可见行数,返回 `[start_index, end_index)` 的渲染范围。
/// 仅渲染可见区域 + 上下 `VIRTUAL_SCROLL_BUFFER` 行缓冲,
/// 将万级事件的渲染复杂度从 O(n) 降至 O(visible + 2×BUFFER)。
///
/// # 参数
/// - `total_items`: 列表总条目数
/// - `scroll_offset`: 当前滚动偏移(可见区域起始行,通常由 `list_state::adjust_scroll` 计算)
/// - `visible_rows`: 可见区域行数
///
/// # 返回
/// `(start_index, end_index)`,其中:
/// - `start_index` 已应用上缓冲(向后扩展 BUFFER 行,不超过 0)
/// - `end_index` 已应用下缓冲(向前扩展 BUFFER 行,不超过 total_items)
///
/// # 边界情况
/// - `total_items == 0`:返回 `(0, 0)`
/// - `visible_rows == 0`:返回 `(0, 0)`(无可见区域时不渲染)
/// - `scroll_offset` 超出范围:自动钳位到 `[0, total_items)`
///
/// # 设计决策(WHY)
/// - **基于 scroll_offset 而非 selected**:与 `list_state::adjust_scroll` 配合,
///   后者已确保 selected 位于 `[scroll_offset, scroll_offset + visible_rows)` 内,
///   本函数只需在 scroll_offset 基础上扩展缓冲,职责单一。
/// - **半开区间**:与 Rust 切片语法 `&items[start..end]` 自然契合,避免 +1 偏移错误。
/// - **缓冲行数 5**:见 `VIRTUAL_SCROLL_BUFFER` 常量文档。
pub fn virtual_scroll_window(
    total_items: usize,
    scroll_offset: usize,
    visible_rows: usize,
) -> (usize, usize) {
    if total_items == 0 || visible_rows == 0 {
        return (0, 0);
    }

    // 钳位 scroll_offset 到 [0, total_items - 1],避免溢出
    let clamped_offset = scroll_offset.min(total_items.saturating_sub(1));

    // 起始索引:滚动偏移向上扩展缓冲,但不超过 0
    let start = clamped_offset.saturating_sub(VIRTUAL_SCROLL_BUFFER);

    // 结束索引:滚动偏移 + 可见行数 + 下缓冲,但不超过总条目数
    let end = clamped_offset
        .saturating_add(visible_rows)
        .saturating_add(VIRTUAL_SCROLL_BUFFER)
        .min(total_items);

    (start, end)
}

/// 渲染条形热图 — 将标量值映射为带颜色编码的 Line
///
/// 用于 OsaSparse 面板(稀疏度热图)与 ClvVector 面板(分块均值热图)。
/// 颜色编码策略:
/// - 值 < min + 25% 范围: 蓝色(低值,使用 '░' 浅字符)
/// - 值在 25%-75% 范围: 灰色(中值,使用 '▒' 中字符)
/// - 值 > 75% 范围: 红色(高值,使用 '▓' 深字符)
///
/// WHY 字符渐进:使用 '░'(浅) → '▒'(中) → '▓'(深) 表示强度递增,
/// 比纯色块更直观,且在非彩色终端也能区分强度。
///
/// # 参数
/// - `value`: 要渲染的标量值
/// - `min`: 值域下界(用于归一化)
/// - `max`: 值域上界(用于归一化)
/// - `width`: 条形图字符宽度(建议 10)
///
/// # 返回
/// ratatui::text::Line<'static>,包含样式化的 Span
///
/// # 边界处理
/// - value < min: 钳位为 min
/// - value > max: 钳位为 max
/// - min == max: 取中值 0.5,返回灰色中字符约 50% 填充(避免除零)
pub fn heat_bar(value: f64, min: f64, max: f64, width: usize) -> Line<'static> {
    // 钳位到 [min, max] 范围,避免值域越界
    let clamped = value.clamp(min, max);

    // 计算归一化比例 [0.0, 1.0];min == max 时取中值 0.5 避免除零
    let ratio = if (max - min).abs() < f64::EPSILON {
        0.5
    } else {
        (clamped - min) / (max - min)
    };

    // 计算填充字符数(至少 1 个避免空条;最多 width 个)
    let filled = ((ratio * width as f64).round() as usize).clamp(1, width);
    let empty = width - filled;

    // 颜色与字符三档编码:低值蓝/浅字符,中值灰/中字符,高值红/深字符
    let (color, filled_char, empty_char) = if ratio < 0.25 {
        (Color::Blue, "░", "·")
    } else if ratio < 0.75 {
        (Color::DarkGray, "▒", "·")
    } else {
        (Color::Red, "▓", "·")
    };

    let filled_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let empty_style = Style::default().fg(Color::DarkGray);

    Line::from(vec![
        Span::styled(filled_char.repeat(filled), filled_style),
        Span::styled(empty_char.repeat(empty), empty_style),
    ])
}

/// 阈值着色阈值定义 — 用于 gauge_thresholded 的三档颜色分界
///
/// WHY 独立结构体:Health 评分等指标需要语义化颜色(绿好/黄警告/红危险),
/// 现有 gauge 需调用方手动算颜色,此结构体封装阈值配置,便于复用与测试。
#[derive(Debug, Clone, Copy)]
pub struct GaugeThreshold {
    /// 绿色阈值上限(0-100 百分比,低于此值显示绿色)
    pub green_max: f64,
    /// 黄色阈值上限(green_max 到此值显示黄色)
    pub yellow_max: f64,
    // 超过 yellow_max 显示红色
}

/// 构造双系列 Sparkline widget — 用于叠加显示两个相关趋势
///
/// 返回主系列与次系列两个 Sparkline 的元组,调用方分别渲染到上下两行。
///
/// # 参数
/// - `data1`: 主系列数据点
/// - `data2`: 次系列数据点
/// - `title`: 图表标题
/// - `color1`: 主系列颜色
/// - `color2`: 次系列颜色
///
/// # 设计决策(WHY)
/// Health 面板需同时展示事件速率与慢消费者数,单系列 sparkline 无法表达相关性。
/// ratatui 不支持单 widget 内多系列叠加,因此返回元组由调用方在相邻区域渲染,
/// 保持 widget 职责单一(一个 Sparkline = 一条数据线)。
pub fn sparkline_dual(
    data1: &[u64],
    data2: &[u64],
    title: &str,
    color1: Color,
    color2: Color,
) -> (Sparkline<'static>, Sparkline<'static>) {
    (
        sparkline(data1, title, color1),
        sparkline(data2, &format!("{} (secondary)", title), color2),
    )
}

/// 构造阈值着色 Gauge widget — 根据值区间自动选颜色
///
/// 颜色分档逻辑(基于 value/max 的百分比):
/// - 低于 `green_max`:绿色(健康)
/// - `green_max` 到 `yellow_max`:黄色(警告)
/// - 不低于 `yellow_max`:红色(危险)
///
/// # 参数
/// - `value`: 当前值
/// - `max`: 最大值(必须 > 0,否则按 0% 处理)
/// - `thresholds`: 阈值定义(green_max/yellow_max, 0-100 百分比)
/// - `label`: 中心标签
///
/// # 设计决策(WHY)
/// Health 评分等指标需要语义化颜色(绿好/黄警告/红危险),现有 gauge 需调用方
/// 手动算颜色,此函数封装阈值逻辑,消除重复的 if-else 颜色判断。
/// 边界语义:percent < green_max 为绿,percent < yellow_max 为黄(此处 green_max
/// 已落入黄色区间),其余为红 — 与 severity() 风格一致(左闭右开)。
pub fn gauge_thresholded(
    value: f64,
    max: f64,
    thresholds: GaugeThreshold,
    label: &str,
) -> Gauge<'static> {
    let ratio = if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let percent = ratio * 100.0;
    let color = if percent < thresholds.green_max {
        Color::Green
    } else if percent < thresholds.yellow_max {
        Color::Yellow
    } else {
        Color::Red
    };
    gauge(value, max, label, color)
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
    fn test_latency_line_contains_all_percentiles() {
        let line = latency_line("KVBSR", 120, 480, 950);
        let text = line.to_string();
        assert!(text.contains("KVBSR"), "label should be present");
        assert!(text.contains("P50"), "P50 label should be present");
        assert!(text.contains("P95"), "P95 label should be present");
        assert!(text.contains("P99"), "P99 label should be present");
        assert!(text.contains("120"), "P50 value should be present");
        assert!(text.contains("480"), "P95 value should be present");
        assert!(text.contains("950"), "P99 value should be present");
    }

    #[test]
    fn test_latency_line_zero_values() {
        let line = latency_line("FaaE", 0, 0, 0);
        let text = line.to_string();
        assert!(text.contains("P50: 0μs"));
        assert!(text.contains("P95: 0μs"));
        assert!(text.contains("P99: 0μs"));
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

    // ============================================================
    // 虚拟滚动辅助函数测试
    // ============================================================

    #[test]
    fn test_virtual_scroll_window_empty() {
        assert_eq!(virtual_scroll_window(0, 0, 10), (0, 0));
    }

    #[test]
    fn test_virtual_scroll_window_zero_visible() {
        assert_eq!(virtual_scroll_window(100, 50, 0), (0, 0));
    }

    #[test]
    fn test_virtual_scroll_window_small_list() {
        // 列表条数 < 可视窗口:应返回部分(从 0 开始)
        let (start, end) = virtual_scroll_window(10, 0, 20);
        assert_eq!(start, 0);
        assert_eq!(end, 10);
    }

    #[test]
    fn test_virtual_scroll_window_large_list_middle() {
        // 10000 条数据,scroll_offset=5000,可视 20 行,缓冲 5 行
        // 期望:start = 5000 - 5 = 4995
        //       end   = 5000 + 20 + 5 = 5025
        let (start, end) = virtual_scroll_window(10000, 5000, 20);
        assert_eq!(start, 4995, "start should be scroll_offset - BUFFER");
        assert_eq!(end, 5025, "end should be scroll_offset + visible + BUFFER");
        assert_eq!(end - start, 30, "window size = visible + 2*BUFFER");
    }

    #[test]
    fn test_virtual_scroll_window_near_start() {
        // scroll_offset 接近 0:上缓冲会被 saturating_sub 钳位到 0
        let (start, end) = virtual_scroll_window(1000, 2, 20);
        assert_eq!(
            start, 0,
            "start should be clamped to 0 when offset < BUFFER"
        );
        assert_eq!(end, 27, "end = 2 + 20 + 5 = 27");
    }

    #[test]
    fn test_virtual_scroll_window_near_end() {
        // 接近末尾时,end 应被钳位到 total_items
        let (start, end) = virtual_scroll_window(100, 95, 20);
        assert_eq!(end, 100, "end should be clamped to total_items");
        assert_eq!(start, 90, "start = 95 - 5 = 90");
    }

    #[test]
    fn test_virtual_scroll_window_offset_exceeds_total() {
        // scroll_offset 超出 total_items:应被钳位
        let (start, end) = virtual_scroll_window(50, 100, 20);
        assert_eq!(start, 44, "offset clamped to 49, start = 49 - 5");
        assert_eq!(end, 50, "end clamped to total_items");
    }

    // ============================================================
    // heat_bar 条形热图测试
    // ============================================================

    #[test]
    fn test_heat_bar_min_value() {
        // 极小值:应显示蓝色 + 浅字符
        let line = heat_bar(0.0, 0.0, 100.0, 10);
        // Line 应包含 2 个 Span(filled + empty)
        assert_eq!(line.spans.len(), 2);
        // 值为 0 时,filled 至少 1 个字符(避免空条)
        let filled = &line.spans[0];
        assert!(!filled.content.is_empty());
    }

    #[test]
    fn test_heat_bar_max_value() {
        // 极大值:应显示红色 + 深字符,全填充
        let line = heat_bar(100.0, 0.0, 100.0, 10);
        assert_eq!(line.spans.len(), 2);
        // 最大值时 filled 应为 width 个字符
        let filled = &line.spans[0];
        assert_eq!(filled.content.chars().count(), 10);
        // empty 应为 0 个字符
        let empty = &line.spans[1];
        assert!(empty.content.is_empty());
    }

    #[test]
    fn test_heat_bar_mid_value() {
        // 中值(50%):应显示灰色 + 中字符
        let line = heat_bar(50.0, 0.0, 100.0, 10);
        assert_eq!(line.spans.len(), 2);
        let filled = &line.spans[0];
        // 50% 应填充约 5 个字符
        assert_eq!(filled.content.chars().count(), 5);
    }

    #[test]
    fn test_heat_bar_clamp_below_min() {
        // 值低于 min:应钳位为 min(蓝色 + 最少填充)
        let line = heat_bar(-10.0, 0.0, 100.0, 10);
        assert_eq!(line.spans.len(), 2);
        let filled = &line.spans[0];
        // 钳位后 ratio=0,filled 至少 1 个字符
        assert!(!filled.content.is_empty());
    }

    #[test]
    fn test_heat_bar_clamp_above_max() {
        // 值高于 max:应钳位为 max(红色 + 全填充)
        let line = heat_bar(150.0, 0.0, 100.0, 10);
        assert_eq!(line.spans.len(), 2);
        let filled = &line.spans[0];
        assert_eq!(filled.content.chars().count(), 10);
    }

    #[test]
    fn test_heat_bar_min_equals_max() {
        // min == max:避免除零,返回中值(灰色 + 50% 填充)
        let line = heat_bar(5.0, 5.0, 5.0, 10);
        assert_eq!(line.spans.len(), 2);
        // 中值应填充约 5 个字符(ratio=0.5)
        let filled = &line.spans[0];
        assert_eq!(filled.content.chars().count(), 5);
    }

    #[test]
    fn test_heat_bar_negative_range() {
        // 负值范围:-1.0 到 1.0,值 0.0 应为中值(50%)
        let line = heat_bar(0.0, -1.0, 1.0, 10);
        assert_eq!(line.spans.len(), 2);
        let filled = &line.spans[0];
        // 0.0 在 [-1.0, 1.0] 范围中是中值,应填充约 5 个字符
        assert_eq!(filled.content.chars().count(), 5);
    }
}
