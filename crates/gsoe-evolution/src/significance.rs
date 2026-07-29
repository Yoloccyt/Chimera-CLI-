//! RHI-CG 通道 B 显著性检测 — 单尾二项检验实现(P5.2.2)
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution)
//! 对应 ADR: ADR-044 决策 6(显著性检测算法选型)+ ADR-045 决策 8(否决证据检查独立化)
//! 对应任务: P5.2.2(SignificanceDetector 显著性检测算法)
//!
//! # 核心职责
//!
//! 通道 B 的"连续 3 次统计显著回归才否决"判定需要两层逻辑:
//!
//! 1. **单尾二项检验**(本模块):给定 N 次运行中观察到 K 次回归,
//!    计算 P(X >= K | n=N, p=0.5) 单尾 p-value。p < 0.05 表示回归
//!    在统计上显著(非随机抖动)。
//!
//! 2. **否决证据检查**(`check_veto_evidence`):独立函数,组合
//!    `regression_streak >= 3` + `significance < 0.05` 两个条件,
//!    满足则允许通道 B 否决,否则返回 `VetoEvidenceInsufficient`。
//!
//! # 设计决策(WHY)
//!
//! ## 1. 手写二项检验而非引入 statrs 依赖
//!
//! workspace Cargo.toml 未引入 `statrs` 依赖(核实见 ADR-044 决策 6
//! 描述偏差)。为遵循"依赖最小化"原则(§4.1 通用约定),本模块手写
//! `binomial_sf`(survival function)实现。N <= 10 时数值稳定
//! (VETO_STREAK_THRESHOLD = 3,典型 N = 3-5),无需对数空间。
//!
//! ## 2. 单尾而非双尾
//!
//! 设计 §7.4 明确"连续 3 次显著回归"是单方向(回归方向)的统计显著性,
//! 单尾检验比双尾检验更符合语义,且与 ADR-043 决策 3 的 71.4% 胜率
//! 阈值统计方法一致(单尾 P(X >= 10 | n=14, p=0.5) ≈ 0.059)。
//!
//! ## 3. 否决证据检查独立函数(ADR-045 决策 8)
//!
//! `check_veto_evidence(regression_streak, significance)` 是独立函数,
//! 不占用 INV-9 命名空间。INV-9 是 MAS 子系统委托图无环不变量(L9),
//! 否决证据检查是通道 B 进化回路逻辑(L5),两者架构层归属不同。
//!
//! ## 4. SignificanceDetector 有状态累积器
//!
//! `SignificanceDetector` 持有 `regression_streak` 状态,通过 `record_regression()`
//! 累积回归次数,通过 `reset()` 在 CI 通过时清零。这样调用方无需
//! 自己维护 streak 计数器,降低误用风险。
//!
//! # 学习不在关键路径(ADR-031 决策 4)
//!
//! `SignificanceDetector::is_significant()` 与 `check_veto_evidence()` 是
//! 纯计算函数,无 IO 与网络调用,延迟 < 1µs,不阻塞推理路径。

use crate::error::GsoeError;

// ============================================================
// 常量(P5.2.2)
// ============================================================

/// 否决连续回归次数阈值 — 连续 3 次回归才允许否决(ADR-045 决策 8)
///
/// WHY 3:平衡误杀率与漏杀率。N=3 时:
/// - 单尾二项检验 P(X >= 3 | n=3, p=0.5) = 0.125(随机抖动概率)
/// - 加上显著性阈值 p < 0.05 双重过滤,误杀率 < 5%(ADR-032 决策 2 KPI)
pub const VETO_STREAK_THRESHOLD: u32 = 3;

/// 显著性阈值 — p < 0.05 表示回归在统计上显著(ADR-045 决策 8)
///
/// WHY 0.05:经典统计学显著性水平(Fisher, 1925),与 ADR-043 决策 3
/// 的 71.4% 胜率阈值(P(X >= 10 | n=14, p=0.5) ≈ 0.059)一致。
pub const SIGNIFICANCE_THRESHOLD: f64 = 0.05;

/// 二项检验的零假设概率 — H0: p = 0.5(回归与不回归等概率)
///
/// WHY 0.5:零假设下回归与不回归等概率(无回归效应),若观察到的
/// 回归次数显著偏离 n/2,则拒绝 H0(存在回归效应)。
pub const NULL_HYPOTHESIS_P: f64 = 0.5;

// ============================================================
// 单尾二项检验 — 核心统计算法
// ============================================================

/// 计算二项系数 C(n, k) — 从 n 次试验中选 k 次的组合数
///
/// 使用迭代乘法避免阶乘溢出:C(n, k) = prod_{i=0}^{k-1} (n-i) / (i+1)
///
/// # 算法选择
///
/// - **迭代乘法**:O(k) 时间复杂度,数值稳定(k <= n <= 10 时无溢出)
/// - **对数空间**:O(k) 时间,适合大 n,但 N <= 10 时无必要
///
/// # 边界
/// - k > n:返回 0(不可能)
/// - k = 0 或 k = n:返回 1
#[inline]
fn binomial_coefficient(n: u32, k: u32) -> u64 {
    if k > n {
        return 0;
    }
    // 取 min(k, n-k) 减少迭代次数
    let k = k.min(n - k);
    let mut result: u64 = 1;
    for i in 0..k {
        // result = result * (n - i) / (i + 1)
        // 先乘后除避免精度损失(整数除法)
        result = result
            .checked_mul((n - i) as u64)
            .expect("binomial_coefficient: 乘法溢出(N <= 32 时不应发生)")
            / (i + 1) as u64;
    }
    result
}

/// 计算二项分布概率质量函数 PMF: P(X = k | n, p)
///
/// 公式: C(n, k) * p^k * (1-p)^(n-k)
///
/// # 参数
/// - `k`: 成功次数
/// - `n`: 总试验次数
/// - `p`: 单次成功概率
///
/// # 返回
/// P(X = k | n, p),范围 [0.0, 1.0]
#[inline]
fn binomial_pmf(k: u32, n: u32, p: f64) -> f64 {
    if k > n {
        return 0.0;
    }
    let c = binomial_coefficient(n, k) as f64;
    let p_k = p.powi(k as i32);
    let q_nk = (1.0 - p).powi((n - k) as i32);
    c * p_k * q_nk
}

/// 计算二项分布生存函数 SF: P(X >= k | n, p)
///
/// 单尾右尾概率 — 用于单尾二项检验(观察值 K 越大越显著)。
///
/// 公式: SF(k, n, p) = sum_{i=k}^{n} PMF(i, n, p)
///
/// # 参数
/// - `k`: 观察到的回归次数(检验统计量)
/// - `n`: 总运行次数
/// - `p`: 零假设下的单次回归概率(典型 0.5)
///
/// # 返回
/// P(X >= k | n, p),范围 [0.0, 1.0]
///
/// # 边界
/// - k = 0:返回 1.0(必然事件)
/// - k > n:返回 0.0(不可能事件)
/// - n = 0:返回 1.0(k = 0 时)或 0.0(k > 0 时)
///
/// # 数值稳定性
///
/// N <= 10 时直接求和,PMF 各项和 = 1.0,无累积误差问题。
/// N > 10 时建议改用对数空间计算(本模块不需要,VETO_STREAK_THRESHOLD = 3)。
fn binomial_sf(k: u32, n: u32, p: f64) -> f64 {
    if k == 0 {
        return 1.0;
    }
    if k > n {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in k..=n {
        sum += binomial_pmf(i, n, p);
    }
    // 钳制到 [0.0, 1.0] 避免浮点误差导致的微小越界
    sum.clamp(0.0, 1.0)
}

// ============================================================
// check_veto_evidence — 独立否决证据检查函数(ADR-045 决策 8)
// ============================================================

/// 通道 B 否决证据检查 — 独立于 INV-9 的 CiGate 内部逻辑(ADR-045 决策 8)
///
/// 判定逻辑:连续回归次数 >= `VETO_STREAK_THRESHOLD`(3)+ 显著性 p-value
/// < `SIGNIFICANCE_THRESHOLD`(0.05)两个条件同时满足时,允许通道 B 否决。
///
/// # 参数
/// - `regression_streak`: 当前连续回归次数(由调用方累积)
/// - `significance`: 当前显著性 p-value(由 `SignificanceDetector::is_significant` 计算)
///
/// # 返回
/// - `Ok(())`: 证据充分,允许通道 B 否决
/// - `Err(VetoEvidenceInsufficient)`: 证据不足,不应否决(继续观察或放行)
///
/// # 设计依据
///
/// ADR-045 决策 8 明确将"否决证据充分性"从 INV-9 中拆分,作为独立函数。
/// INV-9 的语义是 MAS 子系统委托图无环检查(L9),与本函数的"连续回归
/// 统计显著性"是两个独立概念,不应混淆。
///
/// # 示例
///
/// ```
/// use gsoe_evolution::significance::check_veto_evidence;
///
/// // 证据充分:3 次回归 + p < 0.05
/// assert!(check_veto_evidence(3, 0.04).is_ok());
///
/// // 证据不足:回归次数不够
/// assert!(check_veto_evidence(2, 0.04).is_err());
///
/// // 证据不足:显著性不够
/// assert!(check_veto_evidence(3, 0.06).is_err());
/// ```
pub fn check_veto_evidence(regression_streak: u32, significance: f64) -> Result<(), GsoeError> {
    if regression_streak >= VETO_STREAK_THRESHOLD && significance < SIGNIFICANCE_THRESHOLD {
        Ok(()) // 证据充分,允许否决
    } else {
        Err(GsoeError::VetoEvidenceInsufficient {
            regression_streak,
            significance,
        })
    }
}

// ============================================================
// SignificanceDetector — 有状态显著性检测器
// ============================================================

/// 通道 B 显著性检测器 — 累积回归次数并计算单尾二项检验 p-value
///
/// # 设计
///
/// - **有状态累积器**:持有 `regression_streak` 状态,调用方通过
///   `record_regression()` / `reset()` 维护
/// - **p-value 计算**:`is_significant()` 调用 `binomial_sf(streak, n, 0.5)`
///   计算单尾 p-value,与 `SIGNIFICANCE_THRESHOLD` 比较返回显著性
/// - **观察窗口**:`observed_runs` 记录总运行次数(N),用于二项检验的 n 参数
///
/// # 使用模式
///
/// ```
/// use gsoe_evolution::significance::SignificanceDetector;
///
/// let mut detector = SignificanceDetector::new();
///
/// // 模拟 3 次 CI 执行,全部回归
/// detector.record_regression(); // streak=1, n=1
/// detector.record_regression(); // streak=2, n=2
/// detector.record_regression(); // streak=3, n=3
///
/// // 检查是否达到否决阈值
/// if detector.is_significant() {
///     let p_value = detector.p_value();
///     // 通道 B 可否决
/// }
/// ```
///
/// # 边界
///
/// - `streak = 0`:`is_significant()` 返回 false(p-value = 1.0)
/// - `streak > observed_runs`:不应发生(调用方应保证一致性),
///   若发生则按 `binomial_sf` 边界规则处理(返回 0.0,视为显著)
#[derive(Debug, Clone)]
pub struct SignificanceDetector {
    /// 当前连续回归次数(streak)
    regression_streak: u32,
    /// 总观察运行次数(N,用于二项检验的 n 参数)
    observed_runs: u32,
}

impl SignificanceDetector {
    /// 创建新的显著性检测器(streak=0, n=0)
    pub fn new() -> Self {
        Self {
            regression_streak: 0,
            observed_runs: 0,
        }
    }

    /// 记录一次回归(streak +1, observed_runs +1)
    ///
    /// 调用方在 `CiGate::execute()` 返回 `passed=false` 时调用此方法。
    pub fn record_regression(&mut self) {
        self.regression_streak += 1;
        self.observed_runs += 1;
    }

    /// 记录一次非回归(streak 重置为 0, observed_runs +1)
    ///
    /// 调用方在 `CiGate::execute()` 返回 `passed=true` 时调用此方法。
    /// WHY 重置 streak:连续性是"连续 3 次"的语义,任意一次通过即打断连续性。
    pub fn record_pass(&mut self) {
        self.regression_streak = 0;
        self.observed_runs += 1;
    }

    /// 重置检测器状态(streak=0, observed_runs=0)
    ///
    /// 用于通道 B 否决后或测试场景,清零所有累积状态。
    pub fn reset(&mut self) {
        self.regression_streak = 0;
        self.observed_runs = 0;
    }

    /// 返回当前连续回归次数
    pub fn regression_streak(&self) -> u32 {
        self.regression_streak
    }

    /// 返回当前总观察运行次数
    pub fn observed_runs(&self) -> u32 {
        self.observed_runs
    }

    /// 计算当前单尾二项检验的 p-value
    ///
    /// 公式: P(X >= streak | n=observed_runs, p=0.5)
    ///
    /// # 返回
    /// - p-value ∈ [0.0, 1.0]
    /// - streak = 0 时返回 1.0(必然事件,不显著)
    /// - streak = observed_runs = 3 时返回 0.125(随机抖动概率)
    pub fn p_value(&self) -> f64 {
        binomial_sf(
            self.regression_streak,
            self.observed_runs,
            NULL_HYPOTHESIS_P,
        )
    }

    /// 判断当前回归是否统计显著(p < `SIGNIFICANCE_THRESHOLD`)
    ///
    /// 注意:此方法仅检查统计显著性,不检查连续回归次数。
    /// 完整否决判定应使用 `check_veto_evidence()` 组合两个条件。
    pub fn is_significant(&self) -> bool {
        self.p_value() < SIGNIFICANCE_THRESHOLD
    }

    /// 判断是否达到否决证据充分性(组合两个条件)
    ///
    /// 等价于 `check_veto_evidence(self.regression_streak(), self.p_value()).is_ok()`
    ///
    /// # 返回
    /// - `true`: 连续回归次数 >= 3 且 p < 0.05,允许通道 B 否决
    /// - `false`: 证据不足,不应否决
    pub fn is_veto_justified(&self) -> bool {
        check_veto_evidence(self.regression_streak, self.p_value()).is_ok()
    }

    /// 返回当前显著性水平(`is_significant` 的逆向接口,便于审计)
    ///
    /// 返回 (p_value, is_significant) 元组,便于日志记录与审计。
    pub fn significance_report(&self) -> (f64, bool) {
        let p = self.p_value();
        (p, p < SIGNIFICANCE_THRESHOLD)
    }
}

impl Default for SignificanceDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试(P5.2.2)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // 常量测试
    // ============================================================

    #[test]
    fn test_constants_match_adr_045_decision_8() {
        // ADR-045 决策 8 第 328-329 行明确
        assert_eq!(VETO_STREAK_THRESHOLD, 3);
        assert_eq!(SIGNIFICANCE_THRESHOLD, 0.05);
        assert_eq!(NULL_HYPOTHESIS_P, 0.5);
    }

    // ============================================================
    // binomial_coefficient 测试
    // ============================================================

    #[test]
    fn test_binomial_coefficient_basic() {
        // C(n, k) 经典值
        assert_eq!(binomial_coefficient(0, 0), 1);
        assert_eq!(binomial_coefficient(1, 0), 1);
        assert_eq!(binomial_coefficient(1, 1), 1);
        assert_eq!(binomial_coefficient(2, 1), 2);
        assert_eq!(binomial_coefficient(3, 2), 3);
        assert_eq!(binomial_coefficient(4, 2), 6);
        assert_eq!(binomial_coefficient(5, 2), 10);
        assert_eq!(binomial_coefficient(10, 3), 120);
    }

    #[test]
    fn test_binomial_coefficient_symmetry() {
        // C(n, k) = C(n, n-k)
        assert_eq!(binomial_coefficient(10, 3), binomial_coefficient(10, 7));
        assert_eq!(binomial_coefficient(5, 2), binomial_coefficient(5, 3));
    }

    #[test]
    fn test_binomial_coefficient_out_of_range() {
        // k > n 返回 0
        assert_eq!(binomial_coefficient(3, 5), 0);
        assert_eq!(binomial_coefficient(0, 1), 0);
    }

    #[test]
    fn test_binomial_coefficient_boundaries() {
        assert_eq!(binomial_coefficient(5, 0), 1);
        assert_eq!(binomial_coefficient(5, 5), 1);
    }

    // ============================================================
    // binomial_pmf 测试
    // ============================================================

    #[test]
    fn test_binomial_pmf_p_equal_0_5() {
        // p = 0.5 时,PMF 对称
        let p = 0.5;
        // P(X=0 | n=2, p=0.5) = 0.25
        assert!((binomial_pmf(0, 2, p) - 0.25).abs() < 1e-10);
        // P(X=1 | n=2, p=0.5) = 0.5
        assert!((binomial_pmf(1, 2, p) - 0.5).abs() < 1e-10);
        // P(X=2 | n=2, p=0.5) = 0.25
        assert!((binomial_pmf(2, 2, p) - 0.25).abs() < 1e-10);
    }

    #[test]
    fn test_binomial_pmf_sums_to_one() {
        // PMF 总和 = 1.0
        for n in 0..=10 {
            let sum: f64 = (0..=n).map(|k| binomial_pmf(k, n, 0.5)).sum();
            assert!(
                (sum - 1.0).abs() < 1e-10,
                "n={n} 时 PMF 总和 = {sum},应为 1.0"
            );
        }
    }

    #[test]
    fn test_binomial_pmf_out_of_range() {
        assert_eq!(binomial_pmf(5, 3, 0.5), 0.0);
    }

    // ============================================================
    // binomial_sf 测试 — 核心单尾二项检验
    // ============================================================

    #[test]
    fn test_binomial_sf_k_zero_returns_one() {
        // P(X >= 0) = 1.0(必然事件)
        assert!((binomial_sf(0, 5, 0.5) - 1.0).abs() < 1e-10);
        assert!((binomial_sf(0, 0, 0.5) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_binomial_sf_k_greater_than_n_returns_zero() {
        // P(X >= k | n, p) 当 k > n 时 = 0.0
        assert_eq!(binomial_sf(5, 3, 0.5), 0.0);
        assert_eq!(binomial_sf(1, 0, 0.5), 0.0);
    }

    #[test]
    fn test_binomial_sf_n_three_k_three() {
        // P(X >= 3 | n=3, p=0.5) = 0.5^3 = 0.125
        // 这是 ADR-044 决策 6 提到的关键值
        let p = binomial_sf(3, 3, 0.5);
        assert!(
            (p - 0.125).abs() < 1e-10,
            "P(X >= 3 | n=3, p=0.5) = {p}, 应为 0.125"
        );
    }

    #[test]
    fn test_binomial_sf_n_three_k_two() {
        // P(X >= 2 | n=3, p=0.5) = C(3,2)*0.5^3 + C(3,3)*0.5^3 = 3/8 + 1/8 = 4/8 = 0.5
        let p = binomial_sf(2, 3, 0.5);
        assert!((p - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_binomial_sf_n_three_k_one() {
        // P(X >= 1 | n=3, p=0.5) = 1 - P(X=0) = 1 - 0.125 = 0.875
        let p = binomial_sf(1, 3, 0.5);
        assert!((p - 0.875).abs() < 1e-10);
    }

    #[test]
    fn test_binomial_sf_monotone_decreasing_in_k() {
        // 固定 n, SF(k) 随 k 单调递减
        let n = 5;
        let mut prev = 1.0;
        let mut prev_k: i32 = -1;
        for k in 0..=n {
            let sf = binomial_sf(k, n, 0.5);
            assert!(sf <= prev, "SF({k}) = {sf} 应 <= SF({prev_k}) = {prev}");
            prev = sf;
            prev_k = k as i32;
        }
    }

    #[test]
    fn test_binomial_sf_monotone_increasing_in_n() {
        // 固定 k, SF(k, n) 随 n 单调递增(更多试验,达到 k 次回归概率更高)
        let k = 2;
        let mut prev = 0.0;
        for n in k..=10 {
            let sf = binomial_sf(k, n, 0.5);
            assert!(sf >= prev, "SF({k}, {n}) = {sf} 应 >= 前一项 {prev}");
            prev = sf;
        }
    }

    #[test]
    fn test_binomial_sf_clamps_to_unit_interval() {
        // 边界值不应越界
        for n in 0..=10 {
            for k in 0..=n {
                let sf = binomial_sf(k, n, 0.5);
                assert!((0.0..=1.0).contains(&sf), "SF({k}, {n}) = {sf} 越界");
            }
        }
    }

    // ============================================================
    // check_veto_evidence 测试 — 否决证据检查
    // ============================================================

    #[test]
    fn test_check_veto_evidence_streak_below_threshold() {
        // streak < 3,无论 significance 多低都不否决
        assert!(check_veto_evidence(0, 0.001).is_err());
        assert!(check_veto_evidence(1, 0.001).is_err());
        assert!(check_veto_evidence(2, 0.001).is_err());
    }

    #[test]
    fn test_check_veto_evidence_significance_above_threshold() {
        // streak >= 3 但 significance >= 0.05,不否决
        assert!(check_veto_evidence(3, 0.05).is_err());
        assert!(check_veto_evidence(3, 0.06).is_err());
        assert!(check_veto_evidence(5, 0.10).is_err());
    }

    #[test]
    fn test_check_veto_evidence_both_conditions_met() {
        // streak >= 3 且 significance < 0.05,允许否决
        assert!(check_veto_evidence(3, 0.04).is_ok());
        assert!(check_veto_evidence(4, 0.049).is_ok());
        assert!(check_veto_evidence(10, 0.001).is_ok());
    }

    #[test]
    fn test_check_veto_evidence_boundary_streak_equal_3() {
        // 边界:streak 恰好 = 3,significance 恰好 = 0.049
        assert!(check_veto_evidence(3, 0.049).is_ok());
        // streak = 3, significance = 0.05(等于阈值,不满足严格小于)
        assert!(check_veto_evidence(3, 0.05).is_err());
    }

    #[test]
    fn test_check_veto_evidence_error_carries_context() {
        let err = check_veto_evidence(2, 0.10).unwrap_err();
        match err {
            GsoeError::VetoEvidenceInsufficient {
                regression_streak,
                significance,
            } => {
                assert_eq!(regression_streak, 2);
                assert!((significance - 0.10).abs() < 1e-10);
            }
            other => panic!("期望 VetoEvidenceInsufficient, 收到: {other:?}"),
        }
    }

    // ============================================================
    // SignificanceDetector 测试 — 有状态累积器
    // ============================================================

    #[test]
    fn test_significance_detector_new_initializes_to_zero() {
        let detector = SignificanceDetector::new();
        assert_eq!(detector.regression_streak(), 0);
        assert_eq!(detector.observed_runs(), 0);
    }

    #[test]
    fn test_significance_detector_default_is_new() {
        let detector = SignificanceDetector::default();
        assert_eq!(detector.regression_streak(), 0);
        assert_eq!(detector.observed_runs(), 0);
    }

    #[test]
    fn test_record_regression_increments_streak_and_runs() {
        let mut detector = SignificanceDetector::new();
        detector.record_regression();
        assert_eq!(detector.regression_streak(), 1);
        assert_eq!(detector.observed_runs(), 1);

        detector.record_regression();
        assert_eq!(detector.regression_streak(), 2);
        assert_eq!(detector.observed_runs(), 2);
    }

    #[test]
    fn test_record_pass_resets_streak_increments_runs() {
        let mut detector = SignificanceDetector::new();
        detector.record_regression();
        detector.record_regression();
        // streak=2, runs=2

        detector.record_pass();
        // streak 重置, runs +1
        assert_eq!(detector.regression_streak(), 0);
        assert_eq!(detector.observed_runs(), 3);
    }

    #[test]
    fn test_reset_clears_all_state() {
        let mut detector = SignificanceDetector::new();
        detector.record_regression();
        detector.record_regression();
        detector.reset();
        assert_eq!(detector.regression_streak(), 0);
        assert_eq!(detector.observed_runs(), 0);
    }

    #[test]
    fn test_p_value_zero_streak_returns_one() {
        let detector = SignificanceDetector::new();
        // streak=0, n=0 → p-value = 1.0(必然事件)
        let p = detector.p_value();
        assert!((p - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_p_value_three_regressions_in_three_runs() {
        // 关键测试:3 次回归 / 3 次运行 → p-value = 0.125
        // 这是 ADR-044 决策 6 提到的关键值,不应 < 0.05
        let mut detector = SignificanceDetector::new();
        detector.record_regression();
        detector.record_regression();
        detector.record_regression();

        let p = detector.p_value();
        assert!(
            (p - 0.125).abs() < 1e-10,
            "3 次回归 / 3 次运行 p-value = {p}, 应为 0.125"
        );
        // p = 0.125 不显著(>= 0.05)
        assert!(!detector.is_significant());
    }

    #[test]
    fn test_p_value_five_regressions_in_five_runs() {
        // 5 次回归 / 5 次运行 → P(X >= 5 | n=5, p=0.5) = 0.5^5 = 0.03125
        // 这是显著性阈值以下的第一个 N=5 场景
        let mut detector = SignificanceDetector::new();
        for _ in 0..5 {
            detector.record_regression();
        }

        let p = detector.p_value();
        assert!(
            (p - 0.03125).abs() < 1e-10,
            "5 次回归 / 5 次运行 p-value = {p}, 应为 0.03125"
        );
        // p = 0.03125 < 0.05,显著
        assert!(detector.is_significant());
    }

    #[test]
    fn test_is_significant_false_for_zero_streak() {
        let detector = SignificanceDetector::new();
        assert!(!detector.is_significant());
    }

    #[test]
    fn test_is_veto_justified_requires_both_conditions() {
        // streak < 3,即便显著也不应否决
        let mut d = SignificanceDetector::new();
        // 模拟 1 次回归 / 1 次运行 → p = 0.5(不显著)
        d.record_regression();
        assert!(!d.is_veto_justified());

        // streak = 3 但 p = 0.125(不显著)
        let mut d = SignificanceDetector::new();
        for _ in 0..3 {
            d.record_regression();
        }
        assert!(!d.is_veto_justified());

        // streak = 5 且 p = 0.03125(显著)→ 允许否决
        let mut d = SignificanceDetector::new();
        for _ in 0..5 {
            d.record_regression();
        }
        assert!(d.is_veto_justified());
    }

    #[test]
    fn test_significance_report_returns_p_value_and_flag() {
        let mut detector = SignificanceDetector::new();
        for _ in 0..5 {
            detector.record_regression();
        }

        let (p, is_sig) = detector.significance_report();
        assert!((p - 0.03125).abs() < 1e-10);
        assert!(is_sig);
    }

    #[test]
    fn test_streak_interrupted_by_pass() {
        // 2 次回归 → 通过 → 3 次回归:连续性被打断
        let mut detector = SignificanceDetector::new();
        detector.record_regression();
        detector.record_regression();
        detector.record_pass(); // streak 重置
        detector.record_regression();
        detector.record_regression();
        detector.record_regression();

        // streak = 3,但 observed_runs = 6
        assert_eq!(detector.regression_streak(), 3);
        assert_eq!(detector.observed_runs(), 6);
        // p-value = P(X >= 3 | n=6, p=0.5)
        // = C(6,3)*0.5^6 + C(6,4)*0.5^6 + C(6,5)*0.5^6 + C(6,6)*0.5^6
        // = (20 + 15 + 6 + 1) / 64 = 42/64 = 0.65625
        let p = detector.p_value();
        assert!(
            (p - 42.0 / 64.0).abs() < 1e-10,
            "streak=3, n=6 时 p-value = {p}, 应为 0.65625"
        );
        // p = 0.65625 不显著
        assert!(!detector.is_significant());
        // 不应否决(streak >= 3 但 p 不显著)
        assert!(!detector.is_veto_justified());
    }

    // ============================================================
    // proptest — 验证 binomial_sf 的统计性质(P5.2.2)
    // ============================================================

    use proptest::prelude::*;

    proptest! {
        /// 验证 SF(k, n, 0.5) ∈ [0, 1] 且随 k 单调递减
        #[test]
        fn prop_binomial_sf_in_unit_interval_and_monotone(
            n in 0u32..20
        ) {
            let mut prev = 1.0f64;
            for k in 0..=n {
                let sf = binomial_sf(k, n, 0.5);
                prop_assert!((0.0..=1.0).contains(&sf), "SF({k}, {n}) = {sf} 越界");
                prop_assert!(sf <= prev, "SF({k}, {n}) = {sf} 应 <= 前一项 {prev}");
                prev = sf;
            }
        }

        /// 验证 SF(0, n, p) = 1.0(必然事件)
        #[test]
        fn prop_sf_k_zero_is_one(
            n in 0u32..20
        ) {
            let sf = binomial_sf(0, n, 0.5);
            prop_assert!((sf - 1.0).abs() < 1e-10, "SF(0, {n}) = {sf}, 应为 1.0");
        }

        /// 验证 SF(n, n, 0.5) = 0.5^n
        #[test]
        fn prop_sf_n_n_is_half_pow_n(
            n in 1u32..20
        ) {
            let sf = binomial_sf(n, n, 0.5);
            let expected = 0.5f64.powi(n as i32);
            prop_assert!((sf - expected).abs() < 1e-10, "SF({n}, {n}) = {sf}, 应为 {expected}");
        }

        /// 验证 SignificanceDetector 在任意回归序列下状态一致性
        #[test]
        fn prop_detector_streak_never_exceeds_runs(
            regressions in proptest::collection::vec(0u8..=1, 0..20)
        ) {
            let mut detector = SignificanceDetector::new();
            for &r in &regressions {
                if r == 1 {
                    detector.record_regression();
                } else {
                    detector.record_pass();
                }
            }
            prop_assert!(detector.regression_streak() <= detector.observed_runs(),
                "streak {} 不应超过 runs {}", detector.regression_streak(), detector.observed_runs());
            // p-value 必在 [0, 1]
            let p = detector.p_value();
            prop_assert!((0.0..=1.0).contains(&p), "p-value = {p} 越界");
        }
    }
}
