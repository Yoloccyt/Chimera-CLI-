//! L1 共享衰减公式的集成契约 — `exponential_decay_factor` 的边界与单调性
//!
//! 对应 cmt-tiering / decay-engine 共用的数学基座（P1-4）。
//! 覆盖:Δt=0 / τ≤0 / τ=∞ / Δt=∞ 四类边界、负 Δt 的时钟漂移容忍、
//! 以及"因子恒落 [0,1]"与"对 Δt 单调不增"两条不变量。
//!
//! 注:`apply_exponential_decay` 不在本文件覆盖范围（按 L1-C-2 收敛口径只测保留的
//! `exponential_decay_factor`）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use nexus_core::decay::exponential_decay_factor;

/// 浮点近等断言 — 1e-12 绝对容差（本函数只做一次 exp,误差远小于此）
fn assert_close(actual: f64, expected: f64, ctx: &str) {
    assert!(
        (actual - expected).abs() <= 1e-12,
        "{ctx}: actual={actual:?} expected={expected:?}"
    );
}

#[test]
fn decay_factor_is_one_when_delta_is_zero() {
    // 契约:刚发生的事件不衰减（Δt = 0 → e^0 = 1）
    for tau in [1.0_f64, 86_400.0, 1e-6, 1e12] {
        assert_close(
            exponential_decay_factor(0.0, tau),
            1.0,
            &format!("tau={tau}"),
        );
    }
}

#[test]
fn decay_factor_at_one_tau_is_inverse_e() {
    // 契约:Δt = τ 时衰减到 1/e ≈ 0.36787944117144233（时间常数的定义式）
    let expected = std::f64::consts::E.recip();
    for tau in [1.0_f64, 60.0, 86_400.0] {
        assert_close(
            exponential_decay_factor(tau, tau),
            expected,
            &format!("tau={tau}"),
        );
    }
}

#[test]
fn decay_factor_clamps_negative_delta_as_no_decay() {
    // 契约:负 Δt 视为时钟漂移 → clamp 到 0,因子为 1（不产生 >1 的"增值"）
    for delta in [-1e-9_f64, -1.0, -86_400.0, f64::MIN] {
        assert_close(
            exponential_decay_factor(delta, 3_600.0),
            1.0,
            &format!("delta={delta:e}"),
        );
    }
}

#[test]
fn decay_factor_degrades_to_one_for_non_positive_tau() {
    // 契约:τ ≤ 0 是非法参数 → 降级为无衰减（绝不 panic、绝不除零）
    for tau in [0.0_f64, -0.0, -1.0, -86_400.0, f64::NEG_INFINITY] {
        assert_close(
            exponential_decay_factor(1_000.0, tau),
            1.0,
            &format!("tau={tau:?}"),
        );
    }
}

#[test]
fn decay_factor_extremes_converge_to_zero_or_one() {
    // 契约:Δt → ∞ 时因子 → 0;τ → ∞ 时因子 → 1（两端极限均不得产生负值/NaN）
    let vanishing = exponential_decay_factor(1e12, 1.0);
    assert_eq!(vanishing, 0.0, "超大 Δt/τ 比必须下溢到 0.0");

    let infinite_delta = exponential_decay_factor(f64::INFINITY, 60.0);
    assert_eq!(infinite_delta, 0.0, "Δt = +Inf 必须衰减到 0.0");

    let infinite_tau = exponential_decay_factor(1e6, f64::INFINITY);
    assert_close(infinite_tau, 1.0, "τ = +Inf 视为永不衰减");
}

#[test]
fn decay_factor_is_monotonically_non_increasing_in_delta() {
    // 契约:同一 τ 下 Δt 越大因子越小（衰减曲线不得反弹）
    let tau = 3_600.0_f64;
    let deltas = [0.0_f64, 1.0, 60.0, 600.0, 3_600.0, 36_000.0, 1e7];
    let mut prev = f64::INFINITY;
    for delta in deltas {
        let factor = exponential_decay_factor(delta, tau);
        assert!(
            factor <= prev + 1e-15,
            "Δt={delta} 处因子反弹: {factor} > {prev}"
        );
        prev = factor;
    }
}

#[test]
fn decay_factor_stays_within_unit_interval_for_finite_inputs() {
    // 契约:有限输入下输出恒落 [0,1]（含 -0.0 边界;不得出现 >1 或负值）
    for delta in [0.0_f64, 1e-9, 1.0, 1e9] {
        for tau in [1e-9_f64, 1.0, 1e9] {
            let factor = exponential_decay_factor(delta, tau);
            assert!(
                (0.0..=1.0).contains(&factor),
                "delta={delta:e} tau={tau:e} → factor={factor} 越界"
            );
        }
    }
}
