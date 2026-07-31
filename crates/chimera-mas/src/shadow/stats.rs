//! 影子模式统计核 — Wilson 下界 / 游程哨兵 / moving block bootstrap(纯函数)
//!
//! 对应架构层: L9 Quest(chimera-mas shadow 子模块)
//! 对应 ADR: ADR-053-rev4 决策 3A′-P2(游程哨兵单侧化 + 下界兜底)
//!   + ADR-053-rev3 决策 3A′-P(Wilson/bootstrap 判定规则)
//!
//! # 核心职责:晋级门的统计判定原语(零 IO 纯函数)
//!
//! 独立批次配对胜负序列 → 单侧 95% 置信下界。三个原语:
//!
//! 1. [`wilson_lower_bound`] — 独立序列的主判定(解析式,无 MC 噪声,
//!    小样本覆盖率有 Brown-Cai-DasGupta 2001 背书)
//! 2. [`runs_sentinel_one_sided`] — Wald–Wolfowitz 游程哨兵,**仅对
//!    "游程过少"(正自相关/聚集)单侧拒绝**(α=0.10)——负自相关只会使
//!    Wilson 更保守,不触发 bootstrap 分支,消除 fail-open 暗道(rev4 定死)
//! 3. [`moving_block_bootstrap_lower`] — 哨兵拒绝时的保守分支
//!    (L=3,B=10000,percentile 单侧;排除 BCa——二值小样本 jackknife
//!    伪值退化,加速常数不稳,rev3 裁决)
//!
//! # fail-closed 单调化(rev4 决策 3A′-P2 的两重保险)
//!
//! [`effective_lower_bound`] 保证:**无论哨兵是否拒绝,最终下界恒
//! ≤ Wilson 下界**——哨兵拒绝时取 `min(Wilson, bootstrap)`,切换到
//! bootstrap 永远不会比 Wilson 更宽松。哨兵低功效不再是隐患(即便漏判,
//! min 兜底也不会放松)。该单调不变量由 proptest 守护(见模块测试)。
//!
//! # 数值稳定边界(诚实声明,吸取 R3-E06-4 教训)
//!
//! - Wilson:解析式全程 f64,n ∈ [1, u32::MAX] 无溢出;n=0 返回 0.0(fail-closed)
//! - 游程检验:正态近似在 n1/n2 均 ≥ 约 10 时精度良好;R2 语境 n=14/25,
//!   胜负比例极端(如 13/1)时近似退化,但退化方向是哨兵**更难拒绝**,
//!   由 min 兜底覆盖,不构成 fail-open
//! - bootstrap:均值统计量无数值风险;B=10000 下 5% 分位的 MC 标准误
//!   约 0.002,对 0.5 阈值判定足够

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ============================================================
// 常量(预注册参数,ADR-053-rev3/rev4 定死,禁止运行时调整)
// ============================================================

/// Wilson 单侧 95% 置信下界的 z 分位数(P(Z ≤ z) = 0.95)
///
/// WHY 单侧:晋级门只关心"真实胜率是否 > 0.5"的下界方向,
/// 与 rev3 决策 3A′-P"Wilson 单侧 95% 下界 > 0.5 为唯一主判定"一致。
pub const WILSON_Z_ONE_SIDED_95: f64 = 1.644_853_626_951_472_2;

/// 游程哨兵单侧显著性水平 α=0.10 对应的 z 分位数(P(Z ≤ z) = 0.90)
///
/// WHY α=0.10 故意取宽:使"疑似正自相关"更易触发保守分支,
/// 方向对 R2 不利 = fail-closed(rev3 决策 3A′-P)。
pub const RUNS_SENTINEL_Z_ALPHA_010: f64 = 1.281_551_565_544_600_4;

/// moving block bootstrap 块长 L=3(rev3 决策 3A′-P 预注册)
pub const BOOTSTRAP_BLOCK_LEN: usize = 3;

/// moving block bootstrap 重采样次数 B=10000(rev3 决策 3A′-P 预注册)
pub const BOOTSTRAP_RESAMPLES: usize = 10_000;

/// bootstrap 单侧 95% 下界对应的分位(取 5% 分位)
const BOOTSTRAP_LOWER_QUANTILE: f64 = 0.05;

// ============================================================
// 哨兵裁决类型
// ============================================================

/// 游程哨兵裁决 — 单侧化(rev4 决策 3A′-P2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentinelVerdict {
    /// 未检出正自相关 → 唯一判定 = Wilson 单侧下界
    Independent,
    /// 游程过少(疑似正自相关/聚集)→ 转 moving block bootstrap,
    /// 最终取 min(Wilson, bootstrap) 兜底
    PositiveAutocorrelation,
}

impl SentinelVerdict {
    /// 是否触发 bootstrap 保守分支
    #[must_use]
    pub fn triggers_bootstrap(&self) -> bool {
        matches!(self, Self::PositiveAutocorrelation)
    }
}

/// 有效下界计算结果 — 携带审计分解(哨兵裁决 + 各分支下界)
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveLowerBound {
    /// 最终生效的单侧 95% 下界(恒 ≤ `wilson`,单调 fail-closed)
    pub value: f64,
    /// Wilson 单侧 95% 下界(主判定)
    pub wilson: f64,
    /// bootstrap 单侧 95% 下界(仅哨兵拒绝时计算,否则 None)
    pub bootstrap: Option<f64>,
    /// 游程哨兵裁决(审计追溯)
    pub sentinel: SentinelVerdict,
}

// ============================================================
// 原语 1:Wilson 单侧 95% 下界
// ============================================================

/// 计算胜率的 Wilson 单侧 95% 置信下界
///
/// 公式(z = 1.645):
/// `LB = (p̂ + z²/2n − z·√(p̂(1−p̂)/n + z²/4n²)) / (1 + z²/n)`
///
/// # 边界
/// - `n == 0`:返回 0.0(无样本 = 无证据,fail-closed 拒绝)
/// - `wins > n`:按 `wins = n` 截断(防御调用方计数错误,截断方向保守性
///   无影响——全胜是下界最高的合法输入)
///
/// # rev3 临界值交叉验证(见单元测试)
/// - 11/14 → 0.568 > 0.5 ✓(晋级);10/14 → 0.494 < 0.5 ✗
/// - 17/25 → 0.5156 > 0.5 ✓(扩展批临界)
#[must_use]
pub fn wilson_lower_bound(wins: u32, n: u32) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let n_f = f64::from(n);
    let wins_f = f64::from(wins.min(n));
    let p_hat = wins_f / n_f;
    let z = WILSON_Z_ONE_SIDED_95;
    let z2 = z * z;

    let center = p_hat + z2 / (2.0 * n_f);
    let margin = z * (p_hat * (1.0 - p_hat) / n_f + z2 / (4.0 * n_f * n_f)).sqrt();
    let denom = 1.0 + z2 / n_f;
    // 下界理论上 ∈ [0, 1],浮点误差 clamp 兜底
    ((center - margin) / denom).clamp(0.0, 1.0)
}

// ============================================================
// 原语 2:游程哨兵(单侧化)
// ============================================================

/// Wald–Wolfowitz 游程检验哨兵 — 仅对"游程过少"单侧拒绝(α=0.10)
///
/// 统计量:z = (R − E[R]) / √Var(R),其中
/// `E[R] = 1 + 2·n1·n2/n`,`Var(R) = 2·n1·n2·(2·n1·n2 − n) / (n²·(n−1))`。
/// 仅当 `z < −z_α`(游程显著偏少 = 正自相关/聚集)时判
/// [`SentinelVerdict::PositiveAutocorrelation`]。
///
/// # WHY 单侧(rev4 决策 3A′-P2 根因)
/// 双侧检验下"游程过多"(负自相关)也会触发 bootstrap,而负自相关时
/// moving block bootstrap 方差常小于二项,下界反而抬高、更易晋级
/// (fail-open 暗道)。负自相关只会使 Wilson 更保守,无需切换。
///
/// # 退化边界(fail-closed 方向分析)
/// - 序列全胜/全负(n1 或 n2 = 0)或 n < 2:Var 无定义,返回
///   `Independent`(不触发 bootstrap)。此时 Wilson 是标准判定,
///   且 min 兜底只在能收紧时生效,跳过 bootstrap 不构成放松。
#[must_use]
pub fn runs_sentinel_one_sided(outcomes: &[bool]) -> SentinelVerdict {
    let n = outcomes.len();
    let n1 = outcomes.iter().filter(|&&w| w).count();
    let n2 = n - n1;
    if n < 2 || n1 == 0 || n2 == 0 {
        return SentinelVerdict::Independent;
    }

    // 游程数 R:相邻元素不同则开启新游程
    let runs = 1 + outcomes.windows(2).filter(|w| w[0] != w[1]).count();

    let n_f = n as f64;
    let n1_f = n1 as f64;
    let n2_f = n2 as f64;
    let two_n1n2 = 2.0 * n1_f * n2_f;
    let expected = 1.0 + two_n1n2 / n_f;
    let variance = two_n1n2 * (two_n1n2 - n_f) / (n_f * n_f * (n_f - 1.0));
    if variance <= 0.0 {
        // 数值退化(如 n1=n2=1):无法判定,不触发保守分支(见模块头诚实声明)
        return SentinelVerdict::Independent;
    }

    let z = (runs as f64 - expected) / variance.sqrt();
    if z < -RUNS_SENTINEL_Z_ALPHA_010 {
        SentinelVerdict::PositiveAutocorrelation
    } else {
        SentinelVerdict::Independent
    }
}

// ============================================================
// 原语 3:moving block bootstrap 单侧下界
// ============================================================

/// moving block bootstrap 胜率单侧 95% 下界(L=3,B=10000,percentile)
///
/// 算法:从长度 n 的胜负序列构造 n−L+1 个重叠块(L=3),每轮重采样
/// ⌈n/L⌉ 个块拼接后截断到 n,计算均值;重复 B 次后取 5% 分位。
///
/// # 参数
/// - `seed`:注入式随机种子——测试固定种子可复现,生产由调用方提供
///   (预注册审计要求:同一批次序列 + 同一种子 → 同一下界,杜绝重跑挑选)
///
/// # 边界
/// - `n == 0`:返回 0.0(fail-closed)
/// - `n < L`:块长退化为 n(整段重采样,等价于无自相关修正的普通
///   bootstrap;此场景仅理论存在——晋级门要求 n ≥ 14 > 3)
#[must_use]
pub fn moving_block_bootstrap_lower(outcomes: &[bool], seed: u64) -> f64 {
    let n = outcomes.len();
    if n == 0 {
        return 0.0;
    }
    let block_len = BOOTSTRAP_BLOCK_LEN.min(n);
    let n_blocks_available = n - block_len + 1;
    let n_blocks_per_resample = n.div_ceil(block_len);

    let mut rng = StdRng::seed_from_u64(seed);
    let mut means: Vec<f64> = Vec::with_capacity(BOOTSTRAP_RESAMPLES);

    for _ in 0..BOOTSTRAP_RESAMPLES {
        // 拼接 ⌈n/L⌉ 个随机起点块,截断到 n 后统计胜数
        let mut wins = 0usize;
        let mut taken = 0usize;
        'blocks: for _ in 0..n_blocks_per_resample {
            let start = rng.gen_range(0..n_blocks_available);
            for &w in &outcomes[start..start + block_len] {
                if taken == n {
                    break 'blocks;
                }
                wins += usize::from(w);
                taken += 1;
            }
        }
        means.push(wins as f64 / n as f64);
    }

    // 5% 分位(单侧 95% 下界):select_nth_unstable O(B) 替代全排序(§6.2 红线)
    let idx = ((BOOTSTRAP_RESAMPLES as f64) * BOOTSTRAP_LOWER_QUANTILE) as usize;
    let idx = idx.min(means.len() - 1);
    let (_, lower, _) = means.select_nth_unstable_by(idx, |a, b| {
        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
    });
    *lower
}

// ============================================================
// 组合:有效下界(min 兜底,单调 fail-closed)
// ============================================================

/// 计算晋级门的有效单侧 95% 下界(rev4 决策 3A′-P2 两重 fail-closed)
///
/// 判定流程:
/// 1. 游程哨兵单侧检验(仅"游程过少"拒绝)
/// 2. 哨兵不拒绝 → 唯一判定 = Wilson 下界
/// 3. 哨兵拒绝 → 转 moving block bootstrap,最终取
///    **`min(Wilson, bootstrap)`**——切换永远不会比 Wilson 更宽松
///
/// # 不变量(proptest 守护)
/// 任意胜负序列与种子下,`result.value ≤ result.wilson` 恒成立。
#[must_use]
pub fn effective_lower_bound(outcomes: &[bool], bootstrap_seed: u64) -> EffectiveLowerBound {
    let wins = outcomes.iter().filter(|&&w| w).count() as u32;
    let n = outcomes.len() as u32;
    let wilson = wilson_lower_bound(wins, n);
    let sentinel = runs_sentinel_one_sided(outcomes);

    match sentinel {
        SentinelVerdict::Independent => EffectiveLowerBound {
            value: wilson,
            wilson,
            bootstrap: None,
            sentinel,
        },
        SentinelVerdict::PositiveAutocorrelation => {
            let boot = moving_block_bootstrap_lower(outcomes, bootstrap_seed);
            EffectiveLowerBound {
                value: wilson.min(boot),
                wilson,
                bootstrap: Some(boot),
                sentinel,
            }
        }
    }
}

// ============================================================
// 单元测试 + proptest 不变量
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// rev3 临界值交叉验证:11/14 晋级、10/14 不晋级
    #[test]
    fn test_wilson_rev3_critical_n14() {
        let lb_11 = wilson_lower_bound(11, 14);
        assert!(
            (lb_11 - 0.568).abs() < 0.001,
            "11/14 下界应 ≈0.568,实得 {lb_11}"
        );
        assert!(lb_11 > 0.5, "11/14 应过晋级门");

        let lb_10 = wilson_lower_bound(10, 14);
        assert!(
            (lb_10 - 0.494).abs() < 0.001,
            "10/14 下界应 ≈0.494,实得 {lb_10}"
        );
        assert!(lb_10 < 0.5, "10/14 不应过晋级门");
    }

    /// rev3 复评 H1 修正:n=25 临界 ≥17/25(Wilson 下界 0.5156 > 0.5)
    #[test]
    fn test_wilson_rev3_critical_n25() {
        let lb_17 = wilson_lower_bound(17, 25);
        assert!(
            (lb_17 - 0.5156).abs() < 0.001,
            "17/25 下界应 ≈0.5156,实得 {lb_17}"
        );
        assert!(lb_17 > 0.5, "17/25 应过晋级门");
        assert!(wilson_lower_bound(16, 25) < 0.5, "16/25 不应过晋级门");
    }

    /// 零样本 fail-closed:n=0 下界为 0
    #[test]
    fn test_wilson_zero_samples_fail_closed() {
        assert_eq!(wilson_lower_bound(0, 0), 0.0);
    }

    /// wins > n 防御性截断
    #[test]
    fn test_wilson_wins_clamped_to_n() {
        assert!((wilson_lower_bound(20, 14) - wilson_lower_bound(14, 14)).abs() < f64::EPSILON);
    }

    /// 交替序列(负自相关,游程最多)不触发 bootstrap(rev4 单侧化核心)
    #[test]
    fn test_sentinel_alternating_does_not_trigger() {
        let alternating: Vec<bool> = (0..14).map(|i| i % 2 == 0).collect();
        assert_eq!(
            runs_sentinel_one_sided(&alternating),
            SentinelVerdict::Independent,
            "游程过多(负自相关)不应触发 bootstrap 分支(fail-open 暗道已封)"
        );
    }

    /// 强聚集序列(正自相关,游程最少)触发保守分支
    #[test]
    fn test_sentinel_clustered_triggers() {
        // 7 连胜 + 7 连负:R=2,显著低于 E[R]=8
        let clustered: Vec<bool> = (0..14).map(|i| i < 7).collect();
        assert_eq!(
            runs_sentinel_one_sided(&clustered),
            SentinelVerdict::PositiveAutocorrelation
        );
    }

    /// 退化序列(全胜/过短)不触发 bootstrap
    #[test]
    fn test_sentinel_degenerate_independent() {
        assert_eq!(
            runs_sentinel_one_sided(&[true; 14]),
            SentinelVerdict::Independent
        );
        assert_eq!(
            runs_sentinel_one_sided(&[true]),
            SentinelVerdict::Independent
        );
        assert_eq!(runs_sentinel_one_sided(&[]), SentinelVerdict::Independent);
    }

    /// bootstrap 固定种子可复现
    #[test]
    fn test_bootstrap_reproducible_with_seed() {
        let outcomes: Vec<bool> = (0..14).map(|i| i % 3 != 0).collect();
        let a = moving_block_bootstrap_lower(&outcomes, 42);
        let b = moving_block_bootstrap_lower(&outcomes, 42);
        assert_eq!(a, b, "同序列同种子必须产出同一下界(预注册审计要求)");
    }

    /// bootstrap 空序列 fail-closed
    #[test]
    fn test_bootstrap_empty_fail_closed() {
        assert_eq!(moving_block_bootstrap_lower(&[], 42), 0.0);
    }

    /// 哨兵拒绝路径:value = min(wilson, bootstrap) 且分解字段一致
    #[test]
    fn test_effective_lower_bound_clustered_takes_min() {
        let clustered: Vec<bool> = (0..14).map(|i| i < 10).collect(); // 10 连胜 + 4 连负
        let result = effective_lower_bound(&clustered, 42);
        assert!(result.sentinel.triggers_bootstrap());
        let boot = result.bootstrap.expect("哨兵拒绝时必有 bootstrap 下界");
        assert_eq!(result.value, result.wilson.min(boot));
        assert!(
            result.value <= result.wilson,
            "单调 fail-closed:恒 ≤ Wilson"
        );
    }

    proptest! {
        /// 单调不变量(rev4 决策 3A′-P2):任意序列与种子下,
        /// 有效下界恒 ≤ Wilson 下界(晋级门单调 fail-closed)
        #[test]
        fn prop_effective_lower_bound_monotone_le_wilson(
            outcomes in proptest::collection::vec(any::<bool>(), 0..40),
            seed in any::<u64>(),
        ) {
            let result = effective_lower_bound(&outcomes, seed);
            prop_assert!(
                result.value <= result.wilson + f64::EPSILON,
                "有效下界 {} 超过 Wilson 下界 {}(违反单调 fail-closed)",
                result.value,
                result.wilson
            );
        }

        /// Wilson 下界恒 ∈ [0, 1] 且不超过点估计 p̂
        #[test]
        fn prop_wilson_bounds(wins in 0u32..100, n in 1u32..100) {
            let wins = wins.min(n);
            let lb = wilson_lower_bound(wins, n);
            prop_assert!((0.0..=1.0).contains(&lb));
            prop_assert!(lb <= f64::from(wins) / f64::from(n) + f64::EPSILON);
        }
    }
}
