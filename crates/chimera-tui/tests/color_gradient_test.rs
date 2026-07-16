//! ResourceMonitorPanel 阈值颜色渐变 — RED 阶段测试
//!
//! 对应 spec:enterprise-tui-monitoring-task-viz §二·P4.1(体验优化)
//! 对应 Task 4.1:在三档阈值(Normal/Warning/Critical)基础上实现平滑颜色渐变。
//!
//! # 设计目标
//! - 避免 70% 突然从绿变黄的视觉突变(RGB 距离突变)
//! - 相邻 1% 的 RGB 距离 < 25(防止跳跃式颜色变化)
//! - 边界外钳制(< 0 → Green, > 100 → Red)
//!
//! # 算法
//! RGB 线性插值三段:
//! - 0-70%: Green(#2ECC40) → Yellow(#FFDC00)
//! - 70-90%: Yellow(#FFDC00) → OrangeRed(#FF851B)
//! - 90-100%: OrangeRed(#FF851B) → Red(#FF4136)

#![forbid(unsafe_code)]

use chimera_tui::data::resource_history::gradient_color;
use ratatui::style::Color;

/// 提取 Color::Rgb 的 (r, g, b) 三元组
///
/// WHY helper:ratatui 的 `Color` 是 enum,Rgb 与 Green/Yellow/Red 等命名色
/// 不直接可比较。`gradient_color` 返回 Rgb,但测试需要数值化比较,
/// 故用辅助函数解构 Rgb 变体。
fn rgb_components(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        other => panic!("expected Color::Rgb, got {other:?}"),
    }
}

/// 计算两个 RGB 颜色之间的欧氏距离(0-441.67 范围)
///
/// WHY 欧氏距离:RGB 三维空间中两个颜色的"接近度"用欧氏距离
/// 度量是工业标准(参考 sRGB 色差公式的简化版)。对终端色,
/// 平方和开方比曼哈顿距离更接近人眼感知。
fn rgb_distance(c1: Color, c2: Color) -> f64 {
    let (r1, g1, b1) = rgb_components(c1);
    let (r2, g2, b2) = rgb_components(c2);
    let dr = (r1 as f64 - r2 as f64).powi(2);
    let dg = (g1 as f64 - g2 as f64).powi(2);
    let db = (b1 as f64 - b2 as f64).powi(2);
    (dr + dg + db).sqrt()
}

// ============================================================
// 1. 颜色段端点测试
// ============================================================

#[test]
fn test_gradient_normal_low_30_returns_green() {
    // 30% 应该是 Green 系(G 通道高,R/B 低)
    let c = gradient_color(30.0);
    let (r, g, b) = rgb_components(c);
    assert!(g > r, "30% 应该是绿色系,g({g}) > r({r})");
    assert!(g > b, "30% 应该是绿色系,g({g}) > b({b})");
}

#[test]
fn test_gradient_warning_mid_75_returns_yellow_tinted() {
    // 75% 应该是黄/橙系(R+G 高,B 低)
    let c = gradient_color(75.0);
    let (r, g, b) = rgb_components(c);
    assert!(r > b, "75% 应该偏黄/橙,r({r}) > b({b})");
    assert!(g > b, "75% 应该偏黄/橙,g({g}) > b({b})");
}

#[test]
fn test_gradient_critical_95_returns_red() {
    // 95% 应该是红系(R 高,G/B 低)
    let c = gradient_color(95.0);
    let (r, g, b) = rgb_components(c);
    assert!(r > g, "95% 应该是红色系,r({r}) > g({g})");
    assert!(r > b, "95% 应该是红色系,r({r}) > b({b})");
}

// ============================================================
// 2. 平滑过渡测试 — 核心防突变断言
// ============================================================

#[test]
fn test_gradient_smooth_transition_no_jump() {
    // 相邻 1% 变化的 RGB 距离 < 25(避免视觉突变)
    // 70%/90% 边界处的分段切换也应平滑(段内插值)
    let mut max_step = 0.0f64;
    let mut prev = gradient_color(0.0);
    for v in 1..=100 {
        let curr = gradient_color(v as f32);
        let d = rgb_distance(prev, curr);
        if d > max_step {
            max_step = d;
        }
        prev = curr;
    }
    // WHY 25:三段平均每段 23 个 step,RGB 端点最大距离约 √(3*255²) ≈ 441,
    // 段内 23 步线性插值最大步长 ≈ 441/23 ≈ 19,留 25 缓冲以容许边界处
    // 段间色彩距离稍大(理论段间应连续)。
    assert!(
        max_step < 25.0,
        "相邻 1% 变化的最大 RGB 距离应 < 25,实际 {max_step:.2}"
    );
}

// ============================================================
// 3. 边界值测试
// ============================================================

#[test]
fn test_gradient_at_boundary_70_is_yellow() {
    // 70% 整点应该是 Yellow 端点(R=255, G=220, B=0)
    let c = gradient_color(70.0);
    let (r, g, b) = rgb_components(c);
    assert_eq!((r, g, b), (255, 220, 0), "70% 应该是 Yellow 端点 #FFDC00");
}

#[test]
fn test_gradient_at_boundary_90_is_orange_red() {
    // 90% 整点应该是 OrangeRed 端点(R=255, G=133, B=27)
    let c = gradient_color(90.0);
    let (r, g, b) = rgb_components(c);
    assert_eq!(
        (r, g, b),
        (255, 133, 27),
        "90% 应该是 OrangeRed 端点 #FF851B"
    );
}

#[test]
fn test_gradient_zero_is_green() {
    // 0% 应该是 Green 端点(R=46, G=204, B=64)
    let c = gradient_color(0.0);
    let (r, g, b) = rgb_components(c);
    assert_eq!((r, g, b), (46, 204, 64), "0% 应该是 Green 端点 #2ECC40");
}

#[test]
fn test_gradient_hundred_is_red() {
    // 100% 应该是 Red 端点(R=255, G=65, B=54)
    let c = gradient_color(100.0);
    let (r, g, b) = rgb_components(c);
    assert_eq!((r, g, b), (255, 65, 54), "100% 应该是 Red 端点 #FF4136");
}

// ============================================================
// 4. 边界外钳制测试
// ============================================================

#[test]
fn test_gradient_clamps_below_0_and_above_100() {
    // 负值应钳制为 Green
    let c_neg = gradient_color(-50.0);
    let c_zero = gradient_color(0.0);
    assert_eq!(c_neg, c_zero, "负值应钳制为 0% Green");

    // 超过 100 应钳制为 Red
    let c_over = gradient_color(150.0);
    let c_max = gradient_color(100.0);
    assert_eq!(c_over, c_max, "> 100 应钳制为 100% Red");
}

// ============================================================
// 5. 单调性测试 — 颜色随百分比单调变化(R 通道应单调非降)
// ============================================================

#[test]
fn test_gradient_red_channel_monotonic() {
    // R 通道应从 Green(46) → Yellow(255) → OrangeRed(255) → Red(255)
    // 中间段保持 255 端点,故 R 通道单调非降。
    let mut prev_r = 0u8;
    for v in 0..=100 {
        let (r, _, _) = rgb_components(gradient_color(v as f32));
        assert!(
            r >= prev_r,
            "R 通道在 {v}% 应单调非降,前值={prev_r},当前={r}"
        );
        prev_r = r;
    }
}

#[test]
fn test_gradient_green_channel_monotonic_decreasing_in_late_segment() {
    // G 通道从 Green(204) → Yellow(220,略升) → OrangeRed(133) → Red(65)
    // 在 70%-100% 段单调非升(220 → 65)
    let mut prev_g = 255u8;
    for v in 70..=100 {
        let (_, g, _) = rgb_components(gradient_color(v as f32));
        assert!(
            g <= prev_g,
            "G 通道在 {v}% 应单调非升(70-100 段),前值={prev_g},当前={g}"
        );
        prev_g = g;
    }
}
