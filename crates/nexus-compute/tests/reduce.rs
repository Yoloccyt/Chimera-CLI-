//! DetReduce 双模式归约的集成契约 — 确定性、块大小无关性与边界（ADR-102/106）
//!
//! 与 `src/reduce.rs` 内联单测的分工:内联单测锁实现细节（桶号映射、次正规位型）,
//! 本文件只从**公开入口**验证对外契约——同输入逐位可复现、不同 chunk 结果一致、
//! 空/单元素边界,以及三种入口（`tree_reduce_fixed` / `repro_reduce` / `reduce`）的语义一致性。
//!
//! 确定性纪律:数据一律由内联 SplitMix64 生成（固定种子,零外部随机源）,
//! 不依赖时间、线程调度或平台。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use nexus_compute::reduce::{reduce, repro_reduce, tree_reduce_fixed, ReduceMode, DEFAULT_CHUNK};

/// 相对容差 — 与 `src/reduce.rs` 单测同一口径（1e-6 × Σ|x|）
const TOL: f64 = 1e-6;

/// SplitMix64 派生 [lo, hi) 区间内的确定性样本（与 bench 数据源同族,无 rand 依赖）
fn gen_vals(seed: u64, n: usize, lo: f64, hi: f64) -> Vec<f64> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // 高 53 位 → [0,1)（覆盖 f64 全尾数位,均匀无偏）
            let unit = (z >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
            lo + unit * (hi - lo)
        })
        .collect()
}

/// Σ|x| — 跨量级数据的相对容差基准
fn sum_abs(vals: &[f64]) -> f64 {
    vals.iter().map(|x| x.abs()).sum()
}

#[test]
fn tree_reduce_is_bitwise_repeatable_for_same_input() {
    // 契约:Deterministic 模式"同一构建内多次调用逐位一致"
    let vals = gen_vals(7, 10_000, -1.0, 1.0);
    let first = tree_reduce_fixed(&vals, DEFAULT_CHUNK).to_bits();
    for run in 1..8 {
        assert_eq!(
            tree_reduce_fixed(&vals, DEFAULT_CHUNK).to_bits(),
            first,
            "第 {run} 次重算与首次结果不逐位一致"
        );
    }
}

#[test]
fn repro_reduce_is_bitwise_repeatable_for_same_input() {
    // 契约:Audit 模式（指数分桶）同输入同结果 —— CI 双构建逐位比对的同构前提前置
    let vals = gen_vals(11, 10_000, -1e3, 1e3);
    let first = repro_reduce(&vals).to_bits();
    for run in 1..8 {
        assert_eq!(
            repro_reduce(&vals).to_bits(),
            first,
            "第 {run} 次重算与首次结果不逐位一致"
        );
    }
}

#[test]
fn tree_reduce_is_block_size_invariant() {
    // 契约:块界只影响舍入路径,不影响量级 —— 全部 2 的幂 chunk 在 1e-6·Σ|x| 内一致
    let vals = gen_vals(13, 10_000, -1.0, 1.0);
    let base = tree_reduce_fixed(&vals, DEFAULT_CHUNK);
    let tol = TOL * sum_abs(&vals);
    for chunk in [1usize, 2, 4, 8, 16, 32, 256, 1024, 4096] {
        let got = tree_reduce_fixed(&vals, chunk);
        assert!(
            (got - base).abs() <= tol,
            "chunk={chunk} 结果偏离基准: got={got:e} base={base:e} tol={tol:e}"
        );
    }
}

#[test]
fn reduce_modes_agree_on_mixed_sign_data() {
    // 契约:两模式互不替代但必须相容（Audit 供 CI 比对,Deterministic 供生产路径）
    for seed in [0u64, 1, 42, 12_345] {
        let vals = gen_vals(seed, 5_000, -1.0, 1.0);
        let det = reduce(&vals, ReduceMode::Deterministic);
        let aud = reduce(&vals, ReduceMode::Audit);
        let tol = TOL * sum_abs(&vals);
        assert!(
            (det - aud).abs() <= tol,
            "seed={seed} 两模式偏差超限: det={det:e} aud={aud:e} tol={tol:e}"
        );
    }
}

#[test]
fn reduce_empty_slice_is_positive_zero_for_both_modes() {
    // 契约:零元和的数学恒等 +0.0（含符号位,防 -0.0 渗漏）
    for mode in [ReduceMode::Deterministic, ReduceMode::Audit] {
        let r = reduce(&[], mode);
        assert_eq!(r, 0.0, "{mode:?}: 空切片必须为 +0.0");
        assert!(r.is_sign_positive(), "{mode:?}: 空切片符号位必须为正");
    }
    assert_eq!(tree_reduce_fixed(&[], DEFAULT_CHUNK), 0.0);
    assert_eq!(repro_reduce(&[]), 0.0);
}

#[test]
fn reduce_single_element_is_identity_preserving_ieee_edge_values() {
    // 契约:单元素原样返回 —— -0.0 / NaN / ±Inf 的 IEEE 语义不得被归约改写
    for mode in [ReduceMode::Deterministic, ReduceMode::Audit] {
        assert_eq!(reduce(&[42.5], mode), 42.5);
        assert_eq!(
            reduce(&[-0.0], mode).to_bits(),
            (-0.0f64).to_bits(),
            "{mode:?}: -0.0 符号位必须保留"
        );
        assert!(reduce(&[f64::NAN], mode).is_nan());
        assert_eq!(reduce(&[f64::INFINITY], mode), f64::INFINITY);
        assert_eq!(reduce(&[f64::NEG_INFINITY], mode), f64::NEG_INFINITY);
    }
}

#[test]
fn repro_reduce_keeps_tiny_values_across_huge_cancellation() {
    // 契约:指数分桶让同量级先抵消,次小量级不被大数吞没（Audit 模式的存在理由）
    let vals = [1e300, -1e300, 1e-300, 1e-300];
    assert_eq!(
        repro_reduce(&vals),
        2e-300,
        "1e300/−1e300 抵消后 2e-300 必须保留"
    );
    assert!(repro_reduce(&vals).is_finite(), "分桶不得溢出为 Inf");
}

#[test]
fn tree_reduce_rejects_non_power_of_two_chunk() {
    // 契约:非 2 幂 chunk 是调用方编程错误 → panic（失言即失败,不静默降级）
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // 静音默认 hook,避免测试输出污染
    for bad in [0usize, 3, 6, 7, 100] {
        let r = std::panic::catch_unwind(|| tree_reduce_fixed(&[1.0, 2.0], bad));
        assert!(r.is_err(), "chunk={bad} 非 2 的幂必须 panic");
    }
    std::panic::set_hook(prev);
}
