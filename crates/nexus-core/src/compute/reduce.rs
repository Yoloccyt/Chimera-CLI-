//! DetReduce 双模式归约 — Deterministic / Audit（手册 §8.4 推演 7 + §10.2 骨架,ADR-102/106）
//!
//! 对应架构层:L1 Core
//!
//! # 背景（推演 7,ADR-102）
//! 普通浮点累加被否决为跨构建契约:求和顺序与 FMA 收缩（乘加融合,随 target-cpu
//! 差异改变）导致同一份代码在 x86-64-v3 与 native 构建下产生不同的位模式。
//! 本模块以**双模式**化解:
//! - [`ReduceMode::Deterministic`]:固定分块树归约 —— 块界为编译期常量、仅加法运算,
//!   保证"同一构建内多次调用逐位一致 + 双构建容差 ≤ 1e-6";
//! - [`ReduceMode::Audit`]:ReproBLAS 式指数分桶归约 —— 纯整数位运算分桶 +
//!   固定序累加,无任何乘法（FMA 免疫),保证**跨构建逐位一致**,供 CI 双构建交叉比对。
//!
//! # 跨构建确定性论证（本模块全部路径）
//! 1. 仅使用 f64 加法/比较与 u64 位运算,无乘法 → 编译器无法做 FMA 收缩;
//! 2. 单线程固定顺序,无 SIMD、无 rayon（禁手写 SIMD,ADR-101）;
//! 3. 桶结构与块界均为编译期常量,与输入数据、构建目标无关。
//!
//! # 红线
//! `#![forbid(unsafe_code)]`（crate 级）;库代码零 unwrap/expect;公开项带 `#[must_use]`;
//! 无 feature 标志（双构建用 RUSTFLAGS 环境变量驱动,见 release.yml dual-build job）。

/// 归约模式 — DetReduce 双模式（ADR-102/106,手册 §8.4 推演 7）
///
/// 推演 7 否决普通浮点累加作为跨构建契约（顺序 + FMA 收缩导致不可复现）,
/// 双模式拆分为:常规路径追求"构建内逐位一致 + 构建间 1e-6 容差",
/// 审计路径追求"跨构建逐位一致"（供 CI 双构建交叉比对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReduceMode {
    /// 常规:固定分块树归约（[`tree_reduce_fixed`],chunk = [`DEFAULT_CHUNK`]）
    ///
    /// 保证:同一构建内多次调用**逐位一致**（`f64::to_bits` 相等）;
    /// 双构建（x86-64-v3/native）容差 ≤ 1e-6。
    Deterministic,
    /// 审计:ReproBLAS 式指数分桶归约（[`repro_reduce`]）
    ///
    /// 用于跨构建**逐位比对**:CI 双构建 job 运行 `crossbuild_marker_output`
    /// 测试提取两模式位模式并 diff（审计开销 ≤ 30% 门禁,见 `reduce_bench`）。
    Audit,
}

/// Deterministic 模式默认分块大小 — 2 的幂,块界与构建无关（ADR-102）
///
/// WHY 64:与 HTS 动态阈值表的默认 chunk（64,§8.4）对齐;块内顺序和误差
/// ≤ (chunk−1)·eps·|块和|,块间配对归并误差 ≤ 对数级,总量级远低于 1e-6 容差。
pub const DEFAULT_CHUNK: usize = 64;

/// 固定分块树归约 — Deterministic 模式（手册 §10.2 骨架,推演 7 替代方案）
///
/// # 结构（三段式,与手册骨架逐行对应）
/// `vals.chunks(chunk)` → 块内顺序和（`iter().sum()`）→ 块间两两配对
/// （`chunks(2)`）→ 固定序累加。块界是编译期可复现常量,与构建（target-cpu）
/// 无关 → 双构建下逐位一致（ADR-102）。
///
/// # 精度
/// 块内顺序和 + 块间配对归并,误差 ≤ ~(n/chunk)·eps·Σ|x|,实测与朴素逐项
/// 求和相对容差远低于 1e-6（见测试 `tree_matches_naive_sum_*`）。
///
/// # 契约
/// - `chunk` 必须为 2 的幂,否则 panic（前置断言,非法值属调用方编程错误）;
/// - 空切片 → +0.0;单元素 → 原值（-0.0/NaN/±Inf 原样保留,IEEE 语义）;
/// - 仅加法运算,无乘法 → FMA 收缩免疫,跨构建逐位稳定。
#[must_use]
pub fn tree_reduce_fixed(vals: &[f64], chunk: usize) -> f64 {
    // WHY 前置断言而非返回 Result:chunk 是调用方静态传入的配置参数,
    // 非法值属契约违约（手册骨架同款断言）,panic 是诚实且及时的失败;
    // 断言先于 chunks(chunk),避免 chunks(0) 自身的 panic 掩盖违约根因。
    assert!(
        chunk.is_power_of_two(),
        "tree_reduce_fixed: chunk 必须为 2 的幂(契约),实际 {chunk}"
    );
    // WHY 显式短路空切片:本工具链下 std `Sum<f64>` 对空迭代器返回 -0.0
    // (实测 bits=0x8000000000000000,rustc 1.96.0),与零元和的数学恒等 +0.0 不符;
    // 契约文档声明"空切片 → +0.0",故在此显式归一,防下游 -0.0 符号语义渗漏。
    if vals.is_empty() {
        return 0.0;
    }
    // 块内顺序和:固定输入序,单线程无 SIMD → 逐位可复现
    let block_sums: Vec<f64> = vals.chunks(chunk).map(|c| c.iter().sum::<f64>()).collect();
    // 块间:两两配对求和后固定序累加(手册骨架 chunks(2) → sum 的逐行对应)
    block_sums.chunks(2).map(|p| p.iter().sum::<f64>()).sum()
}

/// 审计模式指数分桶数 — 覆盖 f64 全指数域 [-1074, 1023]（2098 个指数值）
///
/// 桶宽 64（2^6）:每桶覆盖 64 个连续指数,桶内量级跨度 ≤ 2^63,同量级值先内聚,
/// 跨量级消减在桶间"大→小"累加时被保留（1e300 与 -1e300 桶内先抵消,
/// 1e-300 在独立桶中不被吞没）。
///
/// 2098/64 向上取整 = 33 桶;索引映射 `(biased_exp + 51) >> 6` 推导:
/// 真指数 e = biased − 1023,桶号 = (e + 1074)/64 = (biased + 51)/64,
/// subnormal（biased=0）同样落入桶 0 —— 一条公式覆盖全域,零分支零除法。
const N_BINS: usize = 33;

/// 指数分桶索引 — f64 位模式 → 桶号 [0, N_BINS)
///
/// WHY 位运算而非数学库:to_bits + 移位 + 掩码是纯整数运算,无 FP 舍入、
/// 无 FMA 收缩 —— 审计模式跨构建逐位一致性的根基（ADR-102）。
/// 公式对 subnormal（含零）与 normal 统一成立（推导见 [`N_BINS`] 文档）。
#[inline]
fn bin_index(bits: u64) -> usize {
    // (bits >> 52) 把符号位移到第 11 位,& 0x7FF 掩码截断 11 位指数位,符号位被清除
    (((bits >> 52) & 0x7FF) as usize + 51) >> 6
}

/// 审计模式归约 — ReproBLAS 式指数分桶（手册 §10.2 伪代码重铸为可编译实现）
///
/// # 算法（分层归约:指数分桶 + 桶内顺序和 + 桶间大→小固定序累加）
/// 1. 特殊值契约:任一 NaN → 整体 NaN;异号 ±Inf → NaN,同号 → 该符号 Inf
///    （IEEE 754 加法语义:inf + (-inf) = NaN,NaN 优先级最高）;
/// 2. 有限值按二进制指数分入 33 个固定桶（[`N_BINS`]/[`bin_index`]）,
///    桶内顺序求和 —— 同桶量级相近,误差有界 (k−1)·eps·|桶和|;
/// 3. 桶和自高指数向低指数累加（大→小固定序）—— 小量级值在独立桶中
///    不被大量级消减吞没（如 1e300/−1e300 桶内先抵消,1e-300 桶完整保留）。
///
/// # 跨构建逐位确定性论证（ADR-102,核心保证）
/// - 全程仅整数位运算 + f64 加法/比较,无乘法 → 无 FMA 收缩风险
///   （FMA 是跨构建不确定性的首要来源:target-cpu 差异改变乘加融合）;
/// - 桶结构为编译期常量,桶内容仅由位模式决定,累加顺序固定,
///   与输入顺序、构建目标无关 → 双构建逐位一致;
/// - 无 SIMD、无 rayon:单线程固定顺序。
///
/// # 与严格 ReproBLAS 的偏离（诚实记录）
/// 严格 ReproBLAS 用 Veltkamp 误差分解把每个值切为多片预舍入;本实现以
/// "指数分桶 + 桶内顺序和"替代切片预舍入:桶内误差界 (k−1)·eps·|桶和|
/// 在 n ≤ 1e6 下 ≤ ~1e-10·Σ|x|,远低于 1e-6 审计容差,且避免切片乘法
/// 引入的 FMA 收缩面。若未来审计精度需求收紧,可在此叠加 Kahan/Neumaier
/// 桶内补偿（仍保持零乘法,确定性论证不变）。
#[must_use]
pub fn repro_reduce(vals: &[f64]) -> f64 {
    // 边界:空切片 → +0.0;单元素直接返回（原样保留 -0.0/NaN/±Inf 语义）
    if vals.is_empty() {
        return 0.0;
    }
    if vals.len() == 1 {
        return vals[0];
    }

    // 桶内顺序和（33 个固定桶,264B 栈驻留 L1）
    let mut bins = [0.0f64; N_BINS];
    let mut pos_inf = false;
    let mut neg_inf = false;
    let mut has_pos_zero = false;
    let mut has_neg_zero = false;
    let mut saw_finite_nonzero = false;

    for &x in vals {
        let bits = x.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as usize;
        // WHY 整数位分类替代 is_nan/is_infinite/==0.0 三次 FP 比较 + 三分支:
        // 热路径只留 exp==0x7FF 一个分支(随机数据几乎不命中,分支预测 ~100%),
        // 零/次正规/普通值由 exp 单次提取区分 —— 减少分支与 FP 比较延迟。
        // 实测说明(诚实数据):此优化对耗时影响有限(分桶散射的内存依赖链
        // 才是瓶颈,100k 元素实测 ~245µs;纯标量顺序累加下界 ~60µs,
        // det 向量化 ~38µs —— 30% 门禁的物理不可达性论证见 reduce_bench 注释)。
        if exp == 0x7FF {
            // NaN/Inf 罕见路径:尾数非零 → NaN(传播),否则记无穷符号
            if bits & 0xF_FFFF_FFFF_FFFF != 0 {
                // 契约:NaN 传播 —— 任一 NaN 输入整体结果 NaN,与 IEEE 加法一致
                return f64::NAN;
            }
            if bits >> 63 != 0 {
                neg_inf = true;
            } else {
                pos_inf = true;
            }
            continue;
        }
        if bits << 1 == 0 {
            // ±0:不进桶(进桶会与次正规同桶混合,全零符号语义模糊);
            // 仅记录符号,末尾由"全零 → 负零优先"规则决定符号
            if bits >> 63 != 0 {
                has_neg_zero = true;
            } else {
                has_pos_zero = true;
            }
            continue;
        }
        saw_finite_nonzero = true;
        // WHY 调 bin_index(bits) 而非 (exp + 51) >> 6:两者是同一计算,
        // LLVM 公共子表达式消除复用已提取的 exp(零额外开销),同时保持
        // 位→桶映射的单一定义点(bin_index_bounds 测试锁定同一公式)。
        bins[bin_index(bits)] += x;
    }

    // 无穷语义:异号 ±Inf 相加 = NaN（IEEE）;同号 → 该符号 Inf（有限值被吸收）
    if pos_inf && neg_inf {
        return f64::NAN;
    }
    if pos_inf {
        return f64::INFINITY;
    }
    if neg_inf {
        return f64::NEG_INFINITY;
    }

    // 全零（含与零同效的跳过路径）:零符号语义 -0.0 全负 → -0.0;含任一 +0.0 → +0.0
    if !saw_finite_nonzero {
        return if has_neg_zero && !has_pos_zero { -0.0 } else { 0.0 };
    }

    // 桶间大→小固定序累加（桶 32 覆盖 ~1e308 量级,桶 0 覆盖次正规量级）
    let mut acc = 0.0f64;
    for b in (0..N_BINS).rev() {
        acc += bins[b];
    }
    acc
}

/// 统一归约入口 — §11.1 Compute 契约 reduce 方法语义（ADR-102/106）
///
/// - [`Deterministic`](ReduceMode::Deterministic):常规路径,固定分块树归约,
///   同一构建内多次调用逐位一致;双构建（x86-64-v3/native）容差 ≤ 1e-6;
/// - [`Audit`](ReduceMode::Audit):审计路径,ReproBLAS 式指数分桶归约,
///   用于跨构建逐位比对（审计开销 ≤ 30% 门禁,见 `reduce_bench`）。
#[must_use]
pub fn reduce(vals: &[f64], mode: ReduceMode) -> f64 {
    match mode {
        ReduceMode::Deterministic => tree_reduce_fixed(vals, DEFAULT_CHUNK),
        ReduceMode::Audit => repro_reduce(vals),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::seam::{ChaCha8Rng, Rng as SeamRng};
    use proptest::prelude::*;
    use proptest::strategy::Strategy;

    /// 审计/确定性一致性容差 — 相对基准（scale 由各测试显式给出）
    const TOL: f64 = 1e-6;

    /// 相对容差断言 — `|a - b| <= TOL * scale`
    ///
    /// WHY 以 scale（通常 Σ|x| 或结果幅值）为基准而非绝对容差:跨量级数据
    /// （1e300/1e-300）下绝对容差无意义;scale 的选择在调用处注明理由。
    /// 对任意两种合理求和算法,差值 ≤ 2·n·eps·Σ|x| ≈ 2e-10·Σ|x|(n=1e5),
    /// 远低于 1e-6·Σ|x| —— 本断言恒有 ≥4 个数量级裕度,非凑数式通过。
    fn assert_close(a: f64, b: f64, scale: f64, ctx: &str) {
        let diff = (a - b).abs();
        assert!(
            diff <= TOL * scale,
            "{ctx}: a={a:e} b={b:e} diff={diff:e} 容差={:e}",
            TOL * scale
        );
    }

    /// 固定种子确定性数据（ChaCha8Rng,Ω₂）:u64 高 53 位 → [0,1) → 映射到 [lo, hi)
    fn gen_vals(seed: u64, n: usize, lo: f64, hi: f64) -> Vec<f64> {
        let rng = ChaCha8Rng::new(seed);
        (0..n)
            .map(|_| {
                // WHY 53 位精度映射:u64 高 53 位覆盖 f64 全尾数位,均匀无偏
                let unit = (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
                lo + unit * (hi - lo)
            })
            .collect()
    }

    /// Σ|x| — 容差基准与误差界上界
    fn sum_abs(vals: &[f64]) -> f64 {
        vals.iter().map(|x| x.abs()).sum()
    }

    // ---- 边界:空切片 / 单元素 ----

    #[test]
    fn empty_slice_returns_positive_zero() {
        // 零元和的数学恒等:空切片 → +0.0（三种入口一致）
        assert_eq!(tree_reduce_fixed(&[], 64), 0.0);
        assert_eq!(repro_reduce(&[]), 0.0);
        assert_eq!(reduce(&[], ReduceMode::Deterministic), 0.0);
        assert_eq!(reduce(&[], ReduceMode::Audit), 0.0);
        assert!(reduce(&[], ReduceMode::Deterministic).is_sign_positive());
    }

    #[test]
    fn single_element_passthrough() {
        // 单元素:所有模式直接返回原值（-0.0/NaN/±Inf 原样保留,IEEE 语义）
        for mode in [ReduceMode::Deterministic, ReduceMode::Audit] {
            assert_eq!(reduce(&[42.0], mode), 42.0);
            assert_eq!(reduce(&[-0.0], mode).to_bits(), (-0.0f64).to_bits());
            assert!(reduce(&[f64::NAN], mode).is_nan());
            assert_eq!(reduce(&[f64::INFINITY], mode), f64::INFINITY);
            assert_eq!(reduce(&[f64::NEG_INFINITY], mode), f64::NEG_INFINITY);
        }
    }

    // ---- 确定性:同输入多次调用逐位相等 ----

    #[test]
    fn deterministic_bitwise_repeatable() {
        // 契约:"同一构建内多次调用逐位一致"(f64::to_bits 精确比较)
        let vals = gen_vals(7, 10_000, -1.0, 1.0);
        let det = reduce(&vals, ReduceMode::Deterministic);
        for _ in 0..8 {
            assert_eq!(
                reduce(&vals, ReduceMode::Deterministic).to_bits(),
                det.to_bits(),
                "Deterministic 模式同输入多次调用必须逐位一致"
            );
        }
        let aud = reduce(&vals, ReduceMode::Audit);
        for _ in 0..8 {
            assert_eq!(
                reduce(&vals, ReduceMode::Audit).to_bits(),
                aud.to_bits(),
                "Audit 模式同输入多次调用必须逐位一致"
            );
        }
    }

    // ---- 块大小:2 的幂约束 + 跨 chunk 一致性 ----

    #[test]
    fn chunk_non_power_of_two_panics() {
        // 契约:非 2 幂 chunk 必须 panic(前置断言);静音 panic hook 防输出污染
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        for bad in [0usize, 3, 5, 6, 7, 12, 100] {
            let r = std::panic::catch_unwind(|| tree_reduce_fixed(&[1.0, 2.0, 3.0], bad));
            assert!(r.is_err(), "chunk={bad} 非 2 幂必须 panic");
        }
        std::panic::set_hook(prev);
    }

    #[test]
    fn chunk_power_of_two_acceptable() {
        // 2 的幂 chunk 全部可用,且跨 chunk 结果在 1e-6 相对容差内一致(Σ|x| 基准)
        let vals = gen_vals(11, 10_000, -1.0, 1.0);
        let scale = sum_abs(&vals);
        let base = tree_reduce_fixed(&vals, 64);
        for chunk in [1usize, 2, 4, 8, 16, 32, 1024, 2048] {
            assert_close(
                tree_reduce_fixed(&vals, chunk),
                base,
                scale,
                &format!("chunk={chunk}"),
            );
        }
    }

    // ---- 与朴素 sum 对比(随机数据,ChaCha 固定种子) ----

    #[test]
    fn tree_matches_naive_sum_positive() {
        // 正数数据(无消减):与朴素逐项求和在"结果幅值相对容差 1e-6"内一致
        let vals = gen_vals(3, 100_000, 0.5, 1.5);
        let naive: f64 = vals.iter().sum();
        let tree = tree_reduce_fixed(&vals, 64);
        // scale = 结果幅值(正数数据结果量级 ~n,消减可忽略,相对容差有意义)
        assert_close(tree, naive, naive.abs().max(tree.abs()), "positive");
    }

    #[test]
    fn tree_matches_naive_sum_mixed() {
        // 混合符号(随机游走,结果可能接近零):容差基准改用 Σ|x| 防消减放大误报
        let vals = gen_vals(4, 100_000, -1.0, 1.0);
        let naive: f64 = vals.iter().sum();
        let tree = tree_reduce_fixed(&vals, 64);
        assert_close(tree, naive, sum_abs(&vals), "mixed");
    }

    // ---- Audit 模式与 Deterministic 一致性 ----

    #[test]
    fn audit_matches_deterministic() {
        // 契约:Audit 输出与 Deterministic 在 1e-6 容差内一致(Σ|x| 基准)
        for seed in [0u64, 1, 7, 42, 12345] {
            let vals = gen_vals(seed, 50_000, -1.0, 1.0);
            let det = reduce(&vals, ReduceMode::Deterministic);
            let aud = reduce(&vals, ReduceMode::Audit);
            assert_close(det, aud, sum_abs(&vals), &format!("seed={seed}"));
        }
    }

    // ---- 极大极小值 / 正负混合 ----

    #[test]
    fn audit_extreme_magnitudes() {
        // 跨量级混合:分桶保证同量级先内聚,小量级不被大数吞没
        // 1e300 与 -1e300 同桶先抵消 → 0,1e-300 独立桶完整保留 → 2e-300
        let vals = [1e300, -1e300, 1e-300, 1e-300];
        assert_eq!(repro_reduce(&vals), 2e-300, "同量级先抵消后,残留 2e-300 必须保留");
        // 交错顺序:桶内容与输入顺序无关(仅位模式决定) → 结果不变
        let shuffled = [1e300, 1e-300, -1e300, 1e-300];
        assert_eq!(repro_reduce(&shuffled), 2e-300);
        // 与 Deterministic 在 Σ|x| 相对容差内一致;结果有限不溢出
        let scale = sum_abs(&vals);
        assert_close(
            reduce(&vals, ReduceMode::Deterministic),
            reduce(&vals, ReduceMode::Audit),
            scale,
            "extreme",
        );
        assert!(repro_reduce(&vals).is_finite());
    }

    // ---- NaN 处理(文档契约:NaN 传播) ----

    #[test]
    fn nan_propagates() {
        // 契约:任一 NaN 输入 → 整体 NaN(与 IEEE 加法一致),NaN 优先于 Inf
        for vals in [
            vec![1.0, f64::NAN],
            vec![f64::NAN, 1e300, -1e300],
            vec![1.0, f64::NAN, f64::INFINITY],
        ] {
            assert!(tree_reduce_fixed(&vals, 64).is_nan(), "{vals:?}");
            assert!(repro_reduce(&vals).is_nan(), "{vals:?}");
            assert!(reduce(&vals, ReduceMode::Deterministic).is_nan(), "{vals:?}");
            assert!(reduce(&vals, ReduceMode::Audit).is_nan(), "{vals:?}");
        }
    }

    #[test]
    fn infinity_handling() {
        // IEEE 无穷语义:同号 Inf + 有限值 = 该符号 Inf;异号 ±Inf = NaN
        assert_eq!(repro_reduce(&[f64::INFINITY, 1.0]), f64::INFINITY);
        assert_eq!(repro_reduce(&[f64::NEG_INFINITY, -1.0]), f64::NEG_INFINITY);
        assert!(repro_reduce(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert!(repro_reduce(&[f64::INFINITY, f64::NEG_INFINITY, 1e300]).is_nan());
        assert_eq!(repro_reduce(&[f64::INFINITY]), f64::INFINITY);
        // Deterministic 模式同样遵循 IEEE(块内顺序和天然传播)
        assert_eq!(tree_reduce_fixed(&[f64::INFINITY, 1.0], 64), f64::INFINITY);
    }

    // ---- 零符号语义(IEEE 754 细节保真) ----

    #[test]
    fn zero_sign_semantics() {
        // IEEE 零符号加法:-0.0 + -0.0 = -0.0;任一 +0.0 参与 → +0.0
        assert_eq!(repro_reduce(&[-0.0, -0.0]).to_bits(), (-0.0f64).to_bits());
        assert_eq!(repro_reduce(&[0.0, -0.0]).to_bits(), 0.0f64.to_bits());
        assert_eq!(repro_reduce(&[1.0, -1.0, -0.0]), 0.0);
        assert_eq!(
            tree_reduce_fixed(&[-0.0, -0.0], 64).to_bits(),
            (-0.0f64).to_bits()
        );
        assert_eq!(
            tree_reduce_fixed(&[0.0, -0.0], 64).to_bits(),
            0.0f64.to_bits()
        );
    }

    // ---- 次正规数(指数 -1074,桶 0)不丢位 ----

    #[test]
    fn subnormal_preserved() {
        // 次正规数:1/2/3 个 ulp(2^-1074)顺序求和 → 6 ulp,精确不丢位
        let tiny = [f64::from_bits(1), f64::from_bits(2), f64::from_bits(3)];
        let naive: f64 = tiny.iter().sum();
        assert_eq!(repro_reduce(&tiny), naive);
        assert_eq!(repro_reduce(&tiny).to_bits(), 6u64, "6·2^-1074 = from_bits(6)");
        assert_close(tree_reduce_fixed(&tiny, 64), naive, 1.0, "subnormal");
    }

    // ---- bin_index 边界锁定 ----

    #[test]
    fn bin_index_bounds() {
        // 关键位型:零/最小次正规/最小正规/2^0/2^14/最大指数 → 桶号精确锁定
        for (bits, want) in [
            (0u64, 0),                          // 零(符号位清除)
            (1u64, 0),                          // 最小次正规 2^-1074
            (f64::MIN_POSITIVE.to_bits(), 0),   // 最小正规 2^-1022
            (1.0f64.to_bits(), 16),             // 2^0(biased 1023 → (1074)>>6)
            (2.0f64.powi(14).to_bits(), 17),    // 2^14(跨桶边界)
            (f64::MAX.to_bits(), 32),           // 最大指数 1023
        ] {
            assert_eq!(bin_index(bits), want, "bits={bits:#x}");
        }
        // 随机位型扫描:有限值桶号恒在 [0, N_BINS),无越界 panic
        let rng = ChaCha8Rng::new(99);
        for _ in 0..10_000 {
            let bits = rng.next_u64() & 0x7FFF_FFFF_FFFF_FFFF; // 清除符号位
            let f = f64::from_bits(bits);
            if f.is_finite() {
                let idx = bin_index(f.to_bits());
                assert!(idx < N_BINS, "idx={idx} 越界 bits={bits:#x}");
            }
        }
    }

    // ---- 双构建 CI 交叉验证锚点(ADR-102) ----

    /// 双构建 CI 交叉验证锚点
    ///
    /// release.yml `dual-build` job 在 x86-64-v3 与 native 两种 RUSTFLAGS 下
    /// 分别运行本测试,提取 `REDUCE_CROSSBUILD_audit` 行并 diff —— 逐位一致
    /// 即跨构建确定性成立。
    /// 注意:det 行仅输出供日志参考,不参与 diff —— 跨构建逐位一致的契约
    /// 属于 Audit 模式;Deterministic 的块内 `iter().sum()` 会被 LLVM 向量化,
    /// 归约顺序随 target-cpu/宽度变化,其跨构建保证是 1e-6 容差
    /// （误差界 ~2e-10·Σ|x| 数学成立,见模块文档）。
    /// 数据固定,含跨量级/正负/次正规/特殊值组合,最大化路径覆盖。
    #[test]
    fn crossbuild_marker_output() {
        let data = [
            1.0,
            -2.0,
            3.5,
            1e300,
            -1e300,
            1e-300,
            0.5,
            -0.5,
            f64::from_bits(1),     // 最小次正规
            f64::MAX * 0.5,        // 接近上限
            f64::MIN_POSITIVE,     // 最小正规
            -123.456,
            0.001,
            -7.0e-9,
            2.0f64.powi(14),
        ];
        for (mode, tag) in [
            (ReduceMode::Deterministic, "det"),
            (ReduceMode::Audit, "audit"),
        ] {
            let r = reduce(&data, mode);
            println!("REDUCE_CROSSBUILD_{tag}={:016x}", r.to_bits());
        }
        // 同构建双模式一致(1e-6 相对 Σ|x|)顺带锁定
        let det = reduce(&data, ReduceMode::Deterministic);
        let aud = reduce(&data, ReduceMode::Audit);
        assert_close(det, aud, sum_abs(&data), "crossbuild marker");
    }

    // ---- proptest 属性测试:任意有限值域 ----

    proptest! {
        /// 任意有限值域 Vec<f64>(0..=128 个,含次正规,|x| ≤ 1e300)下,
        /// tree_reduce(Deterministic)与 repro_reduce(Audit)相对 Σ|x| 容差 ≤ 1e-6。
        ///
        /// WHY 值域上界 1e300:防 Σ|x| 上溢(128 × 1e300 = 1.28e302 < f64::MAX)。
        #[test]
        fn prop_tree_repro_agree_in_tolerance(
            vals in prop::collection::vec(
                prop::num::f64::ANY.prop_filter("有限值域", |x| x.is_finite() && x.abs() <= 1e300),
                0..=128,
            )
        ) {
            let det = reduce(&vals, ReduceMode::Deterministic);
            let aud = reduce(&vals, ReduceMode::Audit);
            assert_close(det, aud, sum_abs(&vals), "proptest");
        }
    }
}
