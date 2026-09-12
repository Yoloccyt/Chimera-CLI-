//! TUI 渲染辅助函数 — 统一 Sparkline/Gauge/进度条等可视化组件
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - 将常用可视化组件抽取为纯函数,避免各面板重复构造 ratatui widget。
//! - 辅助函数接收原始数值与主题色,返回可直接 `render` 的 widget,
//!   保持面板代码聚焦于业务布局。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
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
    Line::from(vec![
        Span::from("["),
        Span::styled("=".repeat(used), Style::default().fg(Color::Cyan)),
        Span::styled("-".repeat(remaining), Style::default().fg(Color::Gray)),
        Span::from(format!("] {}", percent_detail(ratio))),
    ])
}

/// 延迟单位自适应格式化(输入微秒)
///
/// WHY 自适应:P99 达秒级时 "1500000μs" 七位数字可读性差;按量级切换
/// μs(<1_000)→ ms(<1_000_000,1 位小数)→ s(2 位小数)三档,
/// 保持三列 P50/P95/P99 输出宽度稳定且可快速扫读。
pub fn format_latency(us: u64) -> String {
    if us < 1_000 {
        format!("{us}μs")
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

// ============================================================
// 百分比格式化统一(PS-2 U-1/U-2,评估报告)
// ============================================================

/// 摘要视图的百分比精度(整数位)
///
/// WHY 用常量而非各写 `{:.0}`/`{:.1}`:评估报告 U-2 实锤同类字段在不同视图
/// 采用不同精度且无据可依(`security` 主视图 0 位、详情弹窗 1 位;
/// 各面板各写各的)。约定:**摘要=整数位、详情=一位小数**,两者由常量承载,
/// 新增视图按层级取用即可,无需再"拍脑袋选精度"。
pub const PERCENT_PRECISION_SUMMARY: usize = 0;

/// 详情视图的百分比精度(一位小数)
pub const PERCENT_PRECISION_DETAIL: usize = 1;

/// 可参与百分比格式化的数值类型(f32 / f64)
///
/// WHY 用 trait 而非 `percent_x` / `percent_x_f32` 两套函数:
/// 面板持有的字段类型不一(`MemMetrics.hit_rate_percent` 是 f32、
/// `BudgetMetrics.utilization_rate` 是 f64),若为每种类型各开一套函数,
/// API 会成对膨胀且易误用。此处以 trait 收敛为**单一 API**,
/// 类型提升在 `as_f64` 内**显式**完成(非调用点隐式转换,故不违反
/// §4.4 #6"避免隐式 f64 转换"的意图;且 f32→f64 加宽对有限值为精确变换)。
pub trait PercentValue: Copy {
    /// 显式提升为 f64(格式化用;不参与业务计算)
    fn as_f64(self) -> f64;
}

impl PercentValue for f32 {
    fn as_f64(self) -> f64 {
        f64::from(self)
    }
}

impl PercentValue for f64 {
    fn as_f64(self) -> f64 {
        self
    }
}

/// 由 **0-1 比值** 格式化百分比(如 0.72 → "72.0%")
///
/// WHY 函数名里带基数:`0-1 比值` 与 `0-100 数值` 是两种常见约定,
/// 评估报告 U-1 实锤 `memory.rs` 同一面板内两种基数混用且格式串完全相同
/// (`hit_rate_percent` 是 0-100,`compressed_ratio` 是 0-1),
/// 读代码无法判断该不该乘 100。把基数写进函数名后,调用点自解释、歧义消失。
pub fn percent_from_ratio<V: PercentValue>(ratio: V, precision: usize) -> String {
    format!("{:.*}%", precision, ratio.as_f64() * 100.0)
}

/// 由 **0-100 数值** 格式化百分比(如 87.5 → "87.5%")
///
/// 用于数据源本身已是百分数的字段(如 `MemMetrics.hit_rate_percent`、
/// `MemMetrics.usage_percent`),避免在调用点重复 `/100.0` 再 `*100.0` 的往返。
pub fn percent_from_value<V: PercentValue>(value: V, precision: usize) -> String {
    format!("{:.*}%", precision, value.as_f64())
}

/// 摘要视图快捷式(0-1 比值 → 整数百分比)
pub fn percent_summary<V: PercentValue>(ratio: V) -> String {
    percent_from_ratio(ratio, PERCENT_PRECISION_SUMMARY)
}

/// 详情视图快捷式(0-1 比值 → 一位小数百分比)
pub fn percent_detail<V: PercentValue>(ratio: V) -> String {
    percent_from_ratio(ratio, PERCENT_PRECISION_DETAIL)
}

/// 时延格式化(输入**毫秒**,内部折算微秒后交给 [`format_latency`])
///
/// # WHY 需要本适配器
/// 指标字段常见的单位是**毫秒**(如 `HealthMetrics.average_latency_ms: f64`),
/// 而 [`format_latency`] 的输入是**微秒**(采样侧原值)。若无本函数,调用点会
/// 各自写 `format!("{:.1} ms", ms)` —— 这正是评估报告 U-3:单位被写死,
/// 极值下可读性差(0.0003ms 显示为 "0.0 ms")。
///
/// # 非有限值处理(诚实优先)
/// `NaN` / `±Inf` / 负值**不折算为 "0μs"** —— 那会把"数据异常或缺失"
/// 伪装成"零延迟(完美)"。此处如实返回 `n/a`
/// (与 health 面板既有的 `n/a` 标注惯例一致)。
///
/// # 参数
/// - `ms`:时延(毫秒);`0.0` 是合法值(未采样)且保持输出 `0μs`
///
/// # 返回值
/// 自适应单位字符串(`μs` / `ms` / `s`)
pub fn format_latency_ms(ms: f64) -> String {
    if !ms.is_finite() || ms < 0.0 {
        return "n/a".to_string();
    }
    // round 后转 u64:避免 0.5μs 级抖动在整数截断下丢失(采样精度为 μs)
    format_latency((ms * 1_000.0).round() as u64)
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
/// 单位经 `format_latency` 自适应(μs/ms/s),调用方无需感知量级。
pub fn latency_line(label: &str, p50: u64, p95: u64, p99: u64) -> Line<'static> {
    Line::from(format!(
        "{}  {}  P50: {}  P95: {}  P99: {}",
        label,
        crate::t!("panel.router.latency"),
        format_latency(p50),
        format_latency(p95),
        format_latency(p99),
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

/// 渲染带阈值着色的 sparkline
///
/// 数据点超过 `warn_threshold` 时用黄色渲染,超过 `crit_threshold` 时用红色渲染,
/// 正常范围用前景色渲染。
///
/// WHY:ResourceMonitor 面板需要一眼识别 CPU/内存是否进入危险区域,
/// 单纯单色 sparkline 无法传达告警信息。
///
/// # 参数
/// - `data`:数据点数组(u64,最大值决定每列高度)
/// - `area`:渲染区域
/// - `buf`:ratatui Buffer
/// - `fg`:正常数据点颜色
/// - `warn_threshold`:警告阈值(>=)
/// - `crit_threshold`:危险阈值(>=)
/// - `max_value`:数据上限(用于归一化条高度)
pub fn sparkline_thresholded(
    data: &[u64],
    area: Rect,
    buf: &mut Buffer,
    fg: Color,
    warn_threshold: u64,
    crit_threshold: u64,
    max_value: Option<u64>,
) {
    if data.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let max = max_value.unwrap_or_else(|| data.iter().copied().max().unwrap_or(1).max(1));

    let warn_color = Color::Yellow;
    let crit_color = Color::Red;

    // 7 级 sparkline 字符,从低到高表达趋势幅度,与传统 Sparkline widget 一致。
    const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    // 数据点多于可用列时,每个列取子窗口最大值进行下采样;
    // 数据点少于可用列时,每个数据点占据一列,剩余列留白。
    let cols = area.width as usize;
    let len = data.len();
    for col in 0..cols {
        let value = if len <= cols {
            // 一列一个数据点,超出数据范围的列不渲染
            let idx = col;
            if idx >= len {
                continue;
            }
            data[idx]
        } else {
            // 下采样:当前列对应数据子窗口的最大值
            let start = (col * len) / cols;
            let end = ((col + 1) * len) / cols;
            let end = end.max(start + 1).min(len);
            data[start..end].iter().copied().max().unwrap_or(0)
        };

        let bar_color = if value >= crit_threshold {
            crit_color
        } else if value >= warn_threshold {
            warn_color
        } else {
            fg
        };

        // 将值映射到 8 级 sparkline 字符之一
        let spark_idx = if max > 0 {
            ((value as f64 / max as f64) * (SPARKS.len() - 1) as f64)
                .round()
                .clamp(0.0, (SPARKS.len() - 1) as f64) as usize
        } else {
            0
        };

        let y = area.y + area.height.saturating_sub(1);
        if let Some(cell) = buf.cell_mut((area.x + col as u16, y)) {
            cell.set_char(SPARKS[spark_idx]);
            cell.set_fg(bar_color);
        }
    }
}

/// 渲染双色 sparkline(两条数据线在同一区域,不同颜色)
///
/// 与 `sparkline_dual` 的区别:用 `fg1`/`fg2` 参数显式控制两条线的颜色,
/// 而非复用 `fg` + 硬编码 `Color::Cyan`,使调用方可为双线指定独立颜色方案。
///
/// WHY:ResourceMonitor 中磁盘 R/W 和网络 RX/TX 需要明确的颜色区分,
/// 绿色=读/收,红色=写/发,避免混淆。
pub fn sparkline_dual_colored(
    data1: &[u64],
    data2: &[u64],
    area: Rect,
    buf: &mut Buffer,
    fg1: Color,
    fg2: Color,
    max_value: Option<u64>,
) {
    let mut combined_max = max_value.unwrap_or(1);
    if max_value.is_none() {
        combined_max = combined_max
            .max(data1.iter().copied().max().unwrap_or(1))
            .max(data2.iter().copied().max().unwrap_or(1))
            .max(1);
    }

    let len = data1.len().min(data2.len());
    if len == 0 || area.width == 0 || area.height == 0 {
        return;
    }

    let step = (len as f32 / area.width as f32).max(1.0) as usize;
    for col in 0..area.width as usize {
        let idx = (col * step).min(len - 1);
        let v1 = data1[idx];
        let v2 = data2[idx];
        let h1 = ((v1 as f64 / combined_max as f64) * area.height as f64) as usize;
        let h2 = ((v2 as f64 / combined_max as f64) * area.height as f64) as usize;

        for row in 0..area.height as usize {
            let y = area.bottom().saturating_sub(1 + row as u16);
            let in_1 = row < h1;
            let in_2 = row < h2;
            if !in_1 && !in_2 {
                continue;
            }
            if let Some(cell) = buf.cell_mut((area.x + col as u16, y)) {
                if in_1 && in_2 {
                    cell.set_char('█');
                    cell.set_fg(fg1);
                } else if in_1 {
                    cell.set_char('▄');
                    cell.set_fg(fg1);
                } else if in_2 {
                    cell.set_char('▀');
                    cell.set_fg(fg2);
                }
            }
        }
    }
}

/// 渲染水平柱状图 — 用于 CPU 每核使用率等指标
///
/// 在单行区域内渲染一组水平柱状条。每个值映射为不同长度的色块。
///
/// WHY 自定义而非 ratatui::BarChart:ratatui BarChart 占多行且需要 Label,
/// ResourceMonitor 的每核 CPU 展示需要紧凑水平排列的单行柱状图,
/// 自定义渲染更灵活可控。
///
/// # 参数
/// - `values`:各柱的值(0.0-100.0 百分比)
/// - `labels`:各柱的标签
/// - `area`:渲染区域
/// - `buf`:ratatui Buffer
/// - `bar_fg`:正常颜色
/// - `warn_fg`:警告颜色(>= warn_threshold)
/// - `crit_fg`:危险颜色(>= crit_threshold)
/// - `warn_threshold`:警告阈值百分比(默认 60.0)
/// - `crit_threshold`:危险阈值百分比(默认 80.0)
// WHY allow: 此渲染函数参数本质上是领域配置（值/标签/区域/颜色/阈值），
// 引入额外配置 struct 不会降低整体复杂度且徒增间接层
#[allow(clippy::too_many_arguments)]
pub fn horizontal_bar_chart(
    values: &[f32],
    labels: &[String],
    area: Rect,
    buf: &mut Buffer,
    bar_fg: Color,
    warn_fg: Color,
    crit_fg: Color,
    warn_threshold: f32,
    crit_threshold: f32,
) {
    if values.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let bar_count = values.len().min(labels.len());
    let label_width = 4;
    let available_width = area.width as usize;

    for i in 0..bar_count {
        let row = area.y + i as u16;
        if row >= area.bottom() {
            break;
        }

        let label = &labels[i];
        for (j, ch) in label.chars().enumerate() {
            if j >= label_width {
                break;
            }
            let x = area.x + j as u16;
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, row)) {
                    cell.set_char(ch);
                    cell.set_fg(bar_fg);
                }
            }
        }

        let value = values[i].clamp(0.0, 100.0);
        let bar_color = if value >= crit_threshold {
            crit_fg
        } else if value >= warn_threshold {
            warn_fg
        } else {
            bar_fg
        };

        let max_bar_width = available_width.saturating_sub(label_width);
        let bar_width = if max_bar_width > 0 {
            ((value / 100.0) * max_bar_width as f32) as usize
        } else {
            0
        };

        for j in 0..bar_width {
            let x = area.x + (label_width + j) as u16;
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, row)) {
                    cell.set_char('█');
                    cell.set_fg(bar_color);
                }
            }
        }
    }
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
    fn test_format_latency_micros() {
        // < 1_000 μs:保持微秒档,既有输出不变
        assert_eq!(format_latency(0), "0μs");
        assert_eq!(format_latency(120), "120μs");
        assert_eq!(format_latency(999), "999μs");
    }

    #[test]
    fn test_format_latency_millis() {
        // 1_000 ≤ us < 1_000_000:毫秒档,1 位小数
        assert_eq!(format_latency(1_000), "1.0ms");
        assert_eq!(format_latency(1_500), "1.5ms");
        assert_eq!(format_latency(150_000), "150.0ms");
    }

    #[test]
    fn test_format_latency_seconds() {
        // ≥ 1_000_000 μs:秒档,2 位小数
        assert_eq!(format_latency(1_000_000), "1.00s");
        assert_eq!(format_latency(2_500_000), "2.50s");
    }

    #[test]
    fn test_latency_line_adapts_units() {
        // 既有 μs 档(KVBSR 120/480/950)输出保持不变(回归保护)
        let line = latency_line("KVBSR", 120, 480, 950).to_string();
        assert!(line.contains("P50: 120μs"), "μs 档输出不应变化: {line}");
        assert!(line.contains("P95: 480μs"));
        assert!(line.contains("P99: 950μs"));

        // 跨档:P99 达秒级时自适应为 s,不再打印 7 位数字 μs
        let line = latency_line("SESA", 1_500, 1_000, 2_500_000).to_string();
        assert!(line.contains("P50: 1.5ms"), "ms 档应自适应: {line}");
        assert!(line.contains("P95: 1.0ms"));
        assert!(line.contains("P99: 2.50s"), "s 档应自适应: {line}");
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

    #[test]
    fn test_format_latency_ms_adapts_units() {
        // 常规:毫秒档(与旧 "{:.1} ms" 数值一致,仅单位写法统一为无空格)
        assert_eq!(format_latency_ms(15.5), "15.5ms");
        // 极小值:进入微秒档(旧实现显示 "0.0 ms",现如实显示亚毫秒)
        assert_eq!(format_latency_ms(0.3), "300μs");
        // 极大值:进入秒档(旧实现显示 "15000.0 ms",可读性差)
        assert_eq!(format_latency_ms(15_000.0), "15.00s");
        // 零值合法(未采样),不经 n/a 分支
        assert_eq!(format_latency_ms(0.0), "0μs");
    }

    #[test]
    fn test_format_latency_ms_marks_non_finite_as_na() {
        // WHY NaN 不得折算为 0μs:那会把"数据异常"伪装成"零延迟(完美)"
        assert_eq!(format_latency_ms(f64::NAN), "n/a");
        assert_eq!(format_latency_ms(f64::INFINITY), "n/a");
        assert_eq!(format_latency_ms(f64::NEG_INFINITY), "n/a");
        assert_eq!(format_latency_ms(-1.0), "n/a");
    }
}
