//! chimera-tui 属性测试 — TUI 渲染不变量
//!
//! 对应架构层:L10 Interface
//! 对应 SubTask 13.1:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. PanelKind::next().prev() 往返恒等(对任意面板成立)
//! 2. 连续 5 次 next() 回到原面板(5 面板循环)
//! 3. tick_frame 单调递增 frame_count
//! 4. TuiConfig::validate 接受所有合法配置(ratio ∈ (0,1) 开区间、height ≥ 3、rate ≥ 1)
//! 5. TuiConfig::validate 拒绝 ratio 边界值(0.0 与 1.0)
//!
//! # 设计要点
//! - 使用整数策略生成 f32 值(避免 proptest 浮点策略引入 NaN/Inf)
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)
//!
//! # WHY 整数策略生成 f32
//! proptest 的 `proptest::num::f32::ANY` 会生成 NaN/Inf,导致 clamp 后行为不可预测。
//! 用 `0u32..=1000` 除以 1000.0 得到精确的 [0.0, 1.0] 区间,语义清晰可控。

#![forbid(unsafe_code)]

use chimera_tui::{PanelKind, TuiConfig, TuiState};
use proptest::prelude::*;

/// 将 5 个面板变体按索引访问,便于 proptest 参数化
fn panel_at(idx: u32) -> PanelKind {
    match idx % 5 {
        0 => PanelKind::Quest,
        1 => PanelKind::Parliament,
        2 => PanelKind::Budget,
        3 => PanelKind::Log,
        _ => PanelKind::Help,
    }
}

proptest! {
    /// 不变量 1:next().prev() 往返恒等
    ///
    /// 对任意面板 P,P.next().prev() == P 且 P.prev().next() == P。
    /// 验证 next/prev 的循环导航在两个方向上都可逆。
    #[test]
    fn prop_panel_next_prev_roundtrip(idx in 0u32..5) {
        let panel = panel_at(idx);
        let roundtrip_forward = panel.next().prev();
        let roundtrip_backward = panel.prev().next();
        prop_assert_eq!(roundtrip_forward, panel, "next().prev() 应回到原面板");
        prop_assert_eq!(roundtrip_backward, panel, "prev().next() 应回到原面板");
    }

    /// 不变量 2:连续 5 次 next() 回到原面板(5 面板循环)
    ///
    /// 5 个面板构成一个长度为 5 的循环,因此 5 次 next() 应回到起点。
    /// 1-4 次 next() 应到达不同面板(非恒等)。
    #[test]
    fn prop_panel_cycle_length_five(idx in 0u32..5) {
        let panel = panel_at(idx);

        // 5 次 next 应回到原面板
        let mut current = panel;
        for _ in 0..5 {
            current = current.next();
        }
        prop_assert_eq!(current, panel, "5 次 next() 应回到原面板");

        // 1-4 次 next 应到达不同面板(非起点)
        for steps in 1..5 {
            let mut p = panel;
            for _ in 0..steps {
                p = p.next();
            }
            prop_assert_ne!(p, panel, "{} 次 next() 不应回到原面板", steps);
        }
    }

    /// 不变量 3:tick_frame 单调递增 frame_count
    ///
    /// 对任意 N ≥ 1 次调用 tick_frame,frame_count 应精确增加 N。
    /// 验证计数器无溢出、无跳跃。
    #[test]
    fn prop_tick_frame_monotonic_increment(n in 1u64..=1000) {
        let mut state = TuiState::new();
        let initial = state.frame_count;
        for _ in 0..n {
            state.tick_frame();
        }
        // u64 加法在 n ≤ 1000 范围内不会溢出
        prop_assert_eq!(
            state.frame_count,
            initial + n,
            "frame_count 应精确增加 {}",
            n
        );
    }

    /// 不变量 4:TuiConfig::validate 接受所有合法配置
    ///
    /// 合法配置:ratio ∈ (0.0, 1.0) 开区间、log_panel_height ≥ 3、frame_rate ≥ 1。
    /// 使用整数策略生成精确的合法值,避免浮点边界误差。
    #[test]
    fn prop_config_validate_accepts_valid(
        ratio_milli in 1u32..=999,       // (0.001, 0.999) → 开区间 (0, 1)
        log_height in 3u16..=200,         // ≥ 3
        frame_rate in 1u16..=240,         // ≥ 1
    ) {
        let ratio = ratio_milli as f32 / 1000.0;
        let cfg = TuiConfig {
            theme: chimera_tui::config::Theme::Dark,
            main_panel_ratio: ratio,
            log_panel_height: log_height,
            enable_mouse: true,
            frame_rate,
        };
        prop_assert!(
            cfg.validate().is_ok(),
            "合法配置应通过校验: ratio={}, height={}, rate={}",
            ratio,
            log_height,
            frame_rate
        );
    }

    /// 不变量 5:TuiConfig::validate 拒绝 ratio 边界值(0.0 与 1.0)
    ///
    /// WHY 单独测试边界:validate 的规则是开区间 (0, 1),
    /// 即 ratio == 0.0(无主面板空间)和 ratio == 1.0(无侧边栏空间)均应拒绝。
    /// 这是 TUI 布局正确性的硬约束。
    #[test]
    fn prop_config_validate_rejects_ratio_boundaries(
        log_height in 3u16..=200,
        frame_rate in 1u16..=240,
    ) {
        // ratio == 0.0 应拒绝
        let cfg_zero = TuiConfig {
            main_panel_ratio: 0.0,
            log_panel_height: log_height,
            frame_rate,
            ..Default::default()
        };
        prop_assert!(
            cfg_zero.validate().is_err(),
            "ratio == 0.0 应被拒绝(无主面板空间)"
        );

        // ratio == 1.0 应拒绝
        let cfg_one = TuiConfig {
            main_panel_ratio: 1.0,
            log_panel_height: log_height,
            frame_rate,
            ..Default::default()
        };
        prop_assert!(
            cfg_one.validate().is_err(),
            "ratio == 1.0 应被拒绝(无侧边栏空间)"
        );
    }
}
