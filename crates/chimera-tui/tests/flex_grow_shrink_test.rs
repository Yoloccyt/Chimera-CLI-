//! Task 2.6:布局引擎 flex-grow/shrink 集成测试 + 响应式折叠测试
//!
//! 验证 `engine::layout::constraint::solve` 对新增 `FlexBasis` / `Grow` / `Shrink`
//! 变体的处理,以及 `PaneManager::should_collapse_companion` 的响应式折叠行为。
//!
//! 学术参考:W3C CSS Flexible Box Layout Module Level 1 §9.7
//! (W3C CR-flexbox-1-20181119) "Resolving Flexible Lengths"。
//!
//! 对应架构层:L10 Interface(`chimera-tui`)

#![forbid(unsafe_code)]

use chimera_tui::engine::layout::{solve, Constraint};
use chimera_tui::{TuiApp, TuiConfig};

/// flex-grow:剩余空间按 grow 因子比例分配
///
/// 总宽 100,Fixed(20) 占 20,剩 80 按 Grow(1):Grow(2) = 1:2 分配。
/// 期望:[20, 26, 54](整数近似,和 == 100)。
#[test]
fn test_flex_grow_distributes_remaining_space() {
    let sizes = solve(
        100,
        &[
            Constraint::Fixed(20),
            Constraint::Grow(1),
            Constraint::Grow(2),
        ],
    );
    assert_eq!(sizes.len(), 3);
    assert_eq!(sizes.iter().sum::<u16>(), 100, "和必须等于 total");
    assert_eq!(sizes[0], 20, "Fixed 段保持 20");
    // 剩余 80 按 1:2 分配:26.67 : 53.33 → 整数 26 : 54(和 80)
    assert_eq!(sizes[1] + sizes[2], 80, "两个 Grow 段应瓜分剩余 80 空间");
    // Grow(2) 应大于 Grow(1)(2 倍权重)
    assert!(
        sizes[2] > sizes[1],
        "Grow(2) 应大于 Grow(1):{} > {}",
        sizes[2],
        sizes[1]
    );
}

/// flex-shrink:空间不足时按 shrink × base 加权收缩
///
/// 总宽 50,FlexBasis(30) + FlexBasis(30) base_sum=60 > 50,按 shrink 收缩。
/// 期望:两段等权收缩,各 25(和 == 50)。
#[test]
fn test_flex_shrink_contracts_when_overflow() {
    let sizes = solve(50, &[Constraint::FlexBasis(30), Constraint::FlexBasis(30)]);
    assert_eq!(sizes.iter().sum::<u16>(), 50, "收缩后和必须等于 total");
    // 两段等权收缩:各 25
    assert_eq!(sizes[0], 25, "FlexBasis(30) 应收缩到 25");
    assert_eq!(sizes[1], 25, "FlexBasis(30) 应收缩到 25");
}

/// flex-basis 被 grow 覆盖:剩余空间时 FlexBasis 从 base 增长
///
/// FlexBasis(20) + Grow(1) 总宽 100:FlexBasis base=20 可增长,Grow base=0 可增长。
/// 剩余 80 按 weight 1:1 分配 → [60, 40]。
#[test]
fn test_flex_basis_overridden_by_grow() {
    let sizes = solve(100, &[Constraint::FlexBasis(20), Constraint::Grow(1)]);
    assert_eq!(sizes.iter().sum::<u16>(), 100);
    // FlexBasis 应从 20 增长(吸收部分剩余)
    assert!(
        sizes[0] >= 20,
        "FlexBasis 应至少保持 base 20,实际 {}",
        sizes[0]
    );
    // Grow(1) 应分得一半剩余 40
    assert_eq!(sizes[1], 40, "Grow(1) 应分得一半剩余 40");
}

/// Max 边界约束:Grow 段受 Max 封顶,剩余归 Flex
#[test]
fn test_min_max_bounds_enforced() {
    // Max(15) 限制段最多 15,剩余归 Flex(1)
    let sizes = solve(100, &[Constraint::Max(15), Constraint::Flex(1)]);
    assert_eq!(sizes.iter().sum::<u16>(), 100);
    assert_eq!(sizes[0], 15, "Max(15) 封顶");
    assert_eq!(sizes[1], 85, "Flex 吸收剩余 85");
}

/// 响应式折叠:终端宽度 < 阈值时返回 true(隐藏伴随面板)
#[test]
fn test_responsive_collapse_hides_companion_when_narrow() {
    let app = {
        let mut __app = TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .expect("TuiApp 构造失败");
        __app.state_mut().view_mode = chimera_tui::ViewMode::Dashboard;
        __app
    };
    // 默认阈值 100,宽度 99 < 100 → 应折叠
    assert!(
        app.should_collapse_companion(99),
        "宽度 99 < 阈值 100,应折叠伴随面板"
    );
    assert!(
        app.should_collapse_companion(80),
        "宽度 80 < 阈值 100,应折叠伴随面板"
    );
}

/// 响应式折叠:终端宽度 >= 阈值时返回 false(保持伴随面板)
#[test]
fn test_responsive_collapse_keeps_companion_when_wide() {
    let app = {
        let mut __app = TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .expect("TuiApp 构造失败");
        __app.state_mut().view_mode = chimera_tui::ViewMode::Dashboard;
        __app
    };
    // 宽度 100 == 阈值 → 不折叠(只在 < 阈值时折叠)
    assert!(
        !app.should_collapse_companion(100),
        "宽度 100 == 阈值 100,不应折叠"
    );
    // 宽度 120 > 阈值 → 不折叠
    assert!(
        !app.should_collapse_companion(120),
        "宽度 120 > 阈值 100,不应折叠"
    );
}

/// 响应式折叠:阈值 0 禁用折叠(始终返回 false)
#[test]
fn test_responsive_collapse_disabled_when_threshold_zero() {
    // 用 struct update 语法避免 clippy::field_reassign_with_default
    let config = TuiConfig {
        responsive_collapse_threshold: 0,
        ..Default::default()
    };
    let app = TuiApp::new(config).expect("TuiApp 构造失败");
    // 阈值 0 禁用:即使宽度很小也不折叠
    assert!(
        !app.should_collapse_companion(40),
        "阈值 0 禁用折叠,宽度 40 也不应折叠"
    );
}

/// 核心不变量:flex-grow/shrink 组合下和恒等于 total
#[test]
fn test_flex_combination_sum_invariant() {
    // 混合 Fixed + FlexBasis + Grow + Shrink,验证和 == total
    let sizes = solve(
        200,
        &[
            Constraint::Fixed(30),
            Constraint::FlexBasis(50),
            Constraint::Grow(2),
            Constraint::Shrink(1),
        ],
    );
    assert_eq!(sizes.len(), 4);
    assert_eq!(
        sizes.iter().sum::<u16>(),
        200,
        "任意 flex 组合,和必须等于 total"
    );
}
