//! render.rs 可视化组件测试 — sparkline_dual 与 gauge_thresholded 行为验证
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **构造验证**:验证 widget 构造不 panic,与现有 render.rs 内联测试风格一致。
//! - **渲染验证**:对 gauge_thresholded 渲染到 Buffer 并检查前景色,
//!   确保阈值着色逻辑实际生效(而非仅构造成功)。
//! - **边界用例**:覆盖 max=0、value=0、value=max 等边界,与现有 gauge 测试互补。

use chimera_tui::render::{gauge, gauge_thresholded, sparkline, sparkline_dual, GaugeThreshold};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

/// 构造一个最小终端 Buffer 与对应的 Rect 用于渲染验证。
/// ratatui 0.29 的 Widget::render 签名为 render(self, area: Rect, buf: &mut Buffer)。
fn make_buffer(width: u16, height: u16) -> (Buffer, Rect) {
    let area = Rect::new(0, 0, width, height);
    (Buffer::empty(area), area)
}

/// 从 Gauge widget 的 gauge_style 提取前景色。
///
/// ratatui Gauge 内部字段为私有,但渲染后填充区域的 cell 会继承 gauge_style 的 fg。
/// 本函数渲染 Gauge 到 Buffer 后扫描非空白 cell 的 fg 颜色。
fn gauge_rendered_fg(g: ratatui::widgets::Gauge<'_>, width: u16, height: u16) -> Option<Color> {
    let (mut buf, area) = make_buffer(width, height);
    ratatui::widgets::Widget::render(g, area, &mut buf);
    // 扫描 Buffer 中第一个非空白、非默认样式的 cell,返回其 fg
    for y in 0..height {
        for x in 0..width {
            // buf.cell 返回 Option<&Cell>,需解引用
            if let Some(cell) = buf.cell(Position::new(x, y)) {
                let style = cell.style();
                if let Some(fg) = style.fg {
                    // 跳过 Block 边框的默认色(White),只关心填充色
                    if fg != Color::White && fg != Color::Reset {
                        return Some(fg);
                    }
                }
            }
        }
    }
    None
}

// ============================================================
// sparkline_dual 双系列 Sparkline 测试
// ============================================================

#[test]
fn sparkline_dual_returns_two_widgets() {
    let (s1, s2) = sparkline_dual(&[1, 2, 3], &[4, 5, 6], "Test", Color::Cyan, Color::Magenta);
    // 验证返回两个非空 sparkline(widget 构造不 panic 即成功)
    // ratatui Sparkline 没有直接的数据访问 API,用渲染测试验证
    let _ = s1; // 编译期验证类型
    let _ = s2;
}

#[test]
fn sparkline_dual_renders_both_without_panic() {
    // 渲染两个 sparkline 到 Buffer,确保都能正常渲染
    let (s1, s2) = sparkline_dual(
        &[10, 20, 30],
        &[40, 50, 60],
        "Dual",
        Color::Cyan,
        Color::Magenta,
    );
    let (mut buf1, area1) = make_buffer(20, 5);
    let (mut buf2, area2) = make_buffer(20, 5);
    ratatui::widgets::Widget::render(s1, area1, &mut buf1);
    ratatui::widgets::Widget::render(s2, area2, &mut buf2);
    // 渲染不 panic 即通过
}

#[test]
fn sparkline_dual_handles_empty_data() {
    // 空数据应能正常构造(与现有 sparkline 行为一致)
    let (s1, s2) = sparkline_dual(&[], &[], "Empty", Color::Green, Color::Yellow);
    let _ = s1;
    let _ = s2;
}

// ============================================================
// gauge_thresholded 阈值着色测试
// ============================================================

#[test]
fn gauge_thresholded_green_when_below_green_max() {
    let g = gauge_thresholded(
        30.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "30%",
    );
    // 渲染并验证颜色为绿色
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Green),
        "30% should render green (< green_max 70%)"
    );
}

#[test]
fn gauge_thresholded_yellow_when_between_thresholds() {
    let g = gauge_thresholded(
        75.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "75%",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Yellow),
        "75% should render yellow (between 70% and 90%)"
    );
}

#[test]
fn gauge_thresholded_red_when_above_yellow_max() {
    let g = gauge_thresholded(
        95.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "95%",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Red),
        "95% should render red (> yellow_max 90%)"
    );
}

#[test]
fn gauge_thresholded_boundary_at_green_max() {
    // 边界:percent == green_max 应为黄色(因为条件是 percent < green_max)
    let g = gauge_thresholded(
        70.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "70%",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Yellow),
        "70% == green_max should render yellow (boundary)"
    );
}

#[test]
fn gauge_thresholded_boundary_at_yellow_max() {
    // 边界:percent == yellow_max 应为红色(因为条件是 percent < yellow_max)
    let g = gauge_thresholded(
        90.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "90%",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Red),
        "90% == yellow_max should render red (boundary)"
    );
}

#[test]
fn gauge_thresholded_zero_max_returns_green() {
    // max=0 时 ratio=0,percent=0,应返回绿色(< green_max)
    let g = gauge_thresholded(
        50.0,
        0.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "n/a",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Green),
        "max=0 should clamp to 0% and render green"
    );
}

#[test]
fn gauge_thresholded_value_exceeds_max_clamps_to_red() {
    // value > max 时 ratio 被 clamp 到 1.0,percent=100,应返回红色
    let g = gauge_thresholded(
        150.0,
        100.0,
        GaugeThreshold {
            green_max: 70.0,
            yellow_max: 90.0,
        },
        "150%",
    );
    let fg = gauge_rendered_fg(g, 30, 5);
    assert_eq!(
        fg,
        Some(Color::Red),
        "value > max should clamp to 100% and render red"
    );
}

// ============================================================
// 回归测试:现有函数签名不变
// ============================================================

#[test]
fn existing_sparkline_still_works() {
    let s = sparkline(&[1, 2, 3], "Existing", Color::Cyan);
    let _ = s;
}

#[test]
fn existing_gauge_still_works() {
    let g = gauge(50.0, 100.0, "50%", Color::Green);
    let _ = g;
}
