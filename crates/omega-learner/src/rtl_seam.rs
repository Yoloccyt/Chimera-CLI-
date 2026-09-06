//! RTL 接缝 — 影子先验消费 + 扩展上下文 + reward 稳定性监控（P3-T12，v4.0 WI-30 续）
//!
//! 对应架构层: **L6 Router**（omega-learner，ADR-151 裁决：D-P2——不新建 nexus-learn，
//! 扩展既有 crate）
//! 对应任务: **P3-T12**（手册 W18，WI-30 续：rtl_shadow 先验接缝 + LinUCB 特征扩展）
//!
//! # 设计（D-P2 裁决 + v4.0 WI-30 规格）
//! - [`ShadowPriorSource`]:trait 注入（组合根装配 gsoe rtl_shadow
//!   AsyncFeedbackCollector 适配;omega-learner 零新依赖,依赖铁律不破）;
//! - [`ExtendedContext`]:LinUCB 上下文特征扩展——**默认保留 d=6**
//!   （5 特征 + bias,regret 上界 O(√(T·d·ln(KT))) 随 d 恶化,诚实数据红线）;
//!   特征齐备（有 bench 证据）才升 ≤64,全部特征 Clamp [0,1] 保 `||x||` 有界;
//! - [`RewardStabilityMonitor`]:影子 reward 分布稳定性监控（EWMA 均值/方差 +
//!   变异系数 CV;CV > 阈值 → 不稳定告警;周度报告语义）。
//!
//! # R2 红线
//! 本模块只消费影子先验（读）与统计（Shadow 限定）——零 Python/零梯度/
//! 零在线权重写入（rtl_shadow 编译期锁定延续）。

/// 默认上下文维度（D-P2:保留 d=6 = 5 特征 + bias）
pub const DEFAULT_CONTEXT_DIM: usize = 6;
/// 扩展维度上限（特征齐备 + bench 证据才可升至该上限）
pub const MAX_CONTEXT_DIM: usize = 64;

/// 影子先验源 — RTL Shadow 先验消费接缝（WI-30）
///
/// 组合根装配 gsoe-evolution::rtl_shadow::AsyncFeedbackCollector 的适配器;
/// omega-learner 保持零 gsoe 依赖（trait 注入,禁 feature 标志）。
pub trait ShadowPriorSource: Send + Sync {
    /// 查询候选先验（None = 无历史,中性 0.5）
    fn prior(&self, task_type: &str, phase: &str, candidate: &str) -> Option<f64>;
}

/// 无先验源 — 默认（无历史 = 中性,不偏置）
#[derive(Debug, Default)]
pub struct NoopShadowPrior;

impl ShadowPriorSource for NoopShadowPrior {
    fn prior(&self, _task_type: &str, _phase: &str, _candidate: &str) -> Option<f64> {
        None
    }
}

/// 扩展上下文 — 特征向量（Clamp [0,1] 保 ||x|| 有界）
///
/// # 维度纪律（D-P2 诚实数据）
/// - `dim ≤ DEFAULT_CONTEXT_DIM` 为稳定基线（5 特征 + bias）;
/// - 升维须特征工程证据 + bench（regret 上界随 d 恶化,禁止为数字而数字）。
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedContext {
    /// 特征向量（已 Clamp [0,1],末位恒为 bias 1.0）
    features: Vec<f64>,
}

impl ExtendedContext {
    /// 从原始特征构造（Clamp [0,1];自动附加 bias）
    ///
    /// # 返回
    /// `Err` = 特征数超 [`MAX_CONTEXT_DIM`]（上限硬约束）
    pub fn new(features: Vec<f64>) -> Result<Self, String> {
        if features.len() + 1 > MAX_CONTEXT_DIM {
            return Err(format!(
                "context dim {} exceeds max {} (regret bound)",
                features.len() + 1,
                MAX_CONTEXT_DIM
            ));
        }
        let mut clamped: Vec<f64> = features.iter().map(|f| f.clamp(0.0, 1.0)).collect();
        clamped.push(1.0); // bias
        Ok(Self { features: clamped })
    }

    /// 特征向量（含 bias）
    #[must_use]
    pub fn as_slice(&self) -> &[f64] {
        &self.features
    }

    /// 维度（含 bias）
    #[must_use]
    pub fn dim(&self) -> usize {
        self.features.len()
    }

    /// 默认 6 维上下文 — 5 特征 + bias（与 S9Context 对齐）
    #[must_use]
    pub fn default_s9(
        task_complexity: f64,
        budget_water_level: f64,
        latency_sensitivity: f64,
        cache_hit_history: f64,
        risk_level: f64,
    ) -> Self {
        Self::new(vec![
            task_complexity,
            budget_water_level,
            latency_sensitivity,
            cache_hit_history,
            risk_level,
        ])
        .expect("5 特征 ≤ 上限")
    }
}

/// reward 稳定性监控 — 影子分布 EWMA 均值/方差 + 变异系数
///
/// # 稳定性判定（WI-30 门禁）
/// 变异系数 `CV = σ/μ` > [`STABILITY_CV_THRESHOLD`] → 不稳定（告警,
/// 周度报告语义——[`RewardStabilityMonitor::report`]）。
#[derive(Debug)]
pub struct RewardStabilityMonitor {
    /// EWMA 均值
    mean: f64,
    /// EWMA 方差（M2 递推）
    m2: f64,
    /// 样本数
    n: u64,
    /// EWMA 平滑系数（α）
    alpha: f64,
    /// 周度累计样本（周报窗口）
    week_samples: u64,
}

/// 稳定性阈值 — CV > 0.5 判不稳定（保守,防抖动误判）
pub const STABILITY_CV_THRESHOLD: f64 = 0.5;

impl Default for RewardStabilityMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl RewardStabilityMonitor {
    /// 新建监控器（α=0.1 默认平滑）
    #[must_use]
    pub fn new() -> Self {
        Self {
            mean: 0.0,
            m2: 0.0,
            n: 0,
            alpha: 0.1,
            week_samples: 0,
        }
    }

    /// 记录一次影子 reward
    pub fn record(&mut self, reward: f64) {
        self.n += 1;
        self.week_samples += 1;
        // EWMA 更新（Welford 风格:均值与 M2 递推）
        let diff = reward - self.mean;
        self.mean += self.alpha * diff;
        self.m2 = (1.0 - self.alpha) * (self.m2 + self.alpha * diff * diff);
    }

    /// 当前均值（EWMA）
    #[must_use]
    pub fn mean(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.mean
        }
    }

    /// 当前方差（EWMA）
    #[must_use]
    pub fn variance(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.m2
        }
    }

    /// 变异系数 CV = σ/μ（0 均值防御:返回 0）
    #[must_use]
    pub fn cv(&self) -> f64 {
        let m = self.mean();
        if m.abs() < 1e-12 {
            return 0.0;
        }
        self.variance().sqrt() / m.abs()
    }

    /// 稳定性判定 — CV ≤ 阈值 = 稳定
    #[must_use]
    pub fn is_stable(&self) -> bool {
        self.cv() <= STABILITY_CV_THRESHOLD
    }

    /// 周度报告 — 均值/方差/CV/样本数（周报语义）
    #[must_use]
    pub fn report(&self) -> RewardStabilityReport {
        RewardStabilityReport {
            mean: self.mean(),
            variance: self.variance(),
            cv: self.cv(),
            samples: self.n,
            week_samples: self.week_samples,
            stable: self.is_stable(),
        }
    }

    /// 周报重置（周度边界）
    pub fn reset_week(&mut self) {
        self.week_samples = 0;
    }
}

/// 周度稳定性报告
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardStabilityReport {
    /// EWMA 均值
    pub mean: f64,
    /// EWMA 方差
    pub variance: f64,
    /// 变异系数
    pub cv: f64,
    /// 累计样本
    pub samples: u64,
    /// 本周样本
    pub week_samples: u64,
    /// 稳定性判定
    pub stable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ExtendedContext — Clamp [0,1] + bias 附加 + 维度上限
    #[test]
    fn context_clamp_and_bias() {
        let ctx = ExtendedContext::new(vec![-0.5, 0.3, 2.0]).expect("合法");
        assert_eq!(ctx.dim(), 4, "3 特征 + bias");
        let f = ctx.as_slice();
        assert_eq!(f[0], 0.0, "负值 clamp 到 0");
        assert_eq!(f[2], 1.0, "超限 clamp 到 1");
        assert_eq!(f[3], 1.0, "bias 恒 1");
        // 超上限拒绝
        let too_big = vec![0.0; MAX_CONTEXT_DIM]; // 64 特征 + bias = 65 > 64
        assert!(ExtendedContext::new(too_big).is_err(), "超上限必须拒绝");
        // 默认 6 维
        let s9 = ExtendedContext::default_s9(0.5, 0.5, 0.5, 0.5, 0.5);
        assert_eq!(s9.dim(), DEFAULT_CONTEXT_DIM, "默认 d=6");
    }

    /// NoopShadowPrior — 中性（无历史不偏置）
    #[test]
    fn noop_prior_neutral() {
        let p = NoopShadowPrior;
        assert_eq!(p.prior("code", "execute", "tool-a"), None);
    }

    /// RewardStabilityMonitor — 稳定分布 CV 低,抖动分布 CV 高
    #[test]
    fn stability_detection() {
        // 稳定:恒定 reward
        let mut stable = RewardStabilityMonitor::new();
        for _ in 0..100 {
            stable.record(0.8);
        }
        assert!(stable.is_stable(), "恒定 reward 必须稳定");
        assert!((stable.mean() - 0.8).abs() < 0.05, "均值收敛 0.8");
        // 抖动:交替 ±1
        let mut jitter = RewardStabilityMonitor::new();
        for i in 0..100 {
            jitter.record(if i % 2 == 0 { 1.0 } else { -1.0 });
        }
        assert!(!jitter.is_stable(), "交替 reward 必须判不稳定");
        // 空监控器 — 稳定（无风险即满分）
        let empty = RewardStabilityMonitor::new();
        assert!(empty.is_stable());
        assert_eq!(empty.cv(), 0.0);
    }

    /// 周度报告 — 字段完整 + reset_week
    #[test]
    fn weekly_report() {
        let mut m = RewardStabilityMonitor::new();
        for _ in 0..50 {
            m.record(0.7);
        }
        let r = m.report();
        assert_eq!(r.samples, 50);
        assert_eq!(r.week_samples, 50);
        assert!(r.stable);
        m.reset_week();
        m.record(0.7);
        assert_eq!(m.report().week_samples, 1, "周报重置后本周计数");
        assert_eq!(m.report().samples, 51, "累计不清零");
    }

    /// 自定义先验源 — 有历史时非中性
    #[test]
    fn custom_prior_source() {
        struct FixedPrior;
        impl ShadowPriorSource for FixedPrior {
            fn prior(&self, _t: &str, _p: &str, _c: &str) -> Option<f64> {
                Some(0.9)
            }
        }
        let p = FixedPrior;
        assert_eq!(p.prior("a", "b", "c"), Some(0.9));
    }
}
