//! HTS-CPU 序贯检验（第二重阈值来源）— Inline vs Rayon 双桶 A/B 对照
//!
//! 序贯检验框架 — PostHog 式 Inline vs Rayon 双桶 A/B 对照（手册 §8.4 三重来源②）
//!
//! # 方法来源
//! - Wald, A. (1945). *Sequential Tests of Statistical Hypotheses*,
//!   Annals of Mathematical Statistics 16(2): 117-186 —— 序贯概率比检验（SPRT）;
//! - 实现采用 SPRT 正态近似:差值样本 \(d_i = inline_i - ray on_i\) 视为 iid 正态,
//!   \(\sigma^2\) 用样本方差估计（近似处理;PostHog 实际用 delta-method / bootstrap,
//!   其复杂度超出本波次框架,文档如实标注）。
//! - Alpha spending:本实现为 **一次性预算墙**（整段检验消耗 alpha,终局即耗尽,
//!   耗尽后强制 Reject 不 Promote）——PostHog 式"预先承诺边界"防 p-hacking 的简化;
//!   正式的 alpha-spending 函数（如 O'Brien-Fleming 多次中间分析分摊）留待 T14。
//!
//! # 判定规则（契约,勿调整）
//! 1. `n < min_samples` → [`Continue`](TestDecision::Continue)（小样本不判定）;
//! 2. SPRT 对数似然比跨上界 `ln((1-β)/α)` **且**效果量达标（rayon 相对 inline
//!    快 ≥ `effect_size`）→ [`Promote`](TestDecision::Promote);
//! 3. SPRT 跨上界但效果量未达标 → [`Reject`](TestDecision::Reject)（**Promote 只在
//!    效果量达到时发出**,否则一律 Reject）;
//! 4. SPRT 跨下界 `ln(β/(1-α))` → [`Reject`](TestDecision::Reject);
//! 5. `n >= max_samples` → 硬停止,强制给出终局（达标 Promote,否则 Reject）;
//! 6. 终局判定后实例关闭（alpha 预算已耗尽）:后续 `record` **强制 [`Reject`](TestDecision::Reject)**,
//!    永不产生新的 Promote 判定——PostHog 式预算墙,防 p-hacking 门禁核心
//!    （本次检验的结论经 [`decision`](SequentialTest::decision) 查询,不再经 `record` 回传）。
//!
//! # 确定性（Ω₂）
//! 本框架无随机源,同输入序列必同决策序列（单测锁定）。
//!
// WHY 独立成文件（ADR-314 W7 / BC-04）：序贯检验的统计判定语义与阈值表的
// 调度语义是正交的两件事，混在一个文件里时读者无法一眼分辨；拆出后
// `hts::sequential_test::*` 的公开路径逐字不变（子模块仍为 public）。

use thiserror::Error;

/// 序贯检验错误 — 配置校验失败（thiserror,库层错误标准 §4.1）
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum HtsError {
    /// alpha 不在 (0,1) 开区间（含 NaN）
    #[error("alpha {0} 必须在 (0,1) 开区间内")]
    AlphaOutOfRange(f64),
    /// power 不在 (0,1) 开区间（含 NaN）
    #[error("power {0} 必须在 (0,1) 开区间内")]
    PowerOutOfRange(f64),
    /// min_samples 大于 max_samples（样本窗口倒挂）
    #[error("min_samples {0} 必须不大于 max_samples {1}")]
    SampleRange(usize, usize),
    /// effect_size 非正（效果量无意义）
    #[error("effect_size {0} 必须为正")]
    EffectSizeNonPositive(f64),
}

/// 序贯检验配置 — 手册 §8.4 / PostHog 式双桶对照参数
///
/// # 默认值
/// - `alpha = 0.05`（Type I error,误 Promote 概率上限,预算墙）;
/// - `power = 0.8`（检验功效,β = 0.2 为漏检概率）;
/// - `min_samples = 30`（达到此样本量才允许判定,小样本强制 Continue）;
/// - `max_samples = 500`（硬停止上限,防止无限收集）;
/// - `effect_size = 0.2`（效果量判据:rayon 相对 inline 快 ≥20%,§4.1 推演 15
///   的工程初值;手册未给默认,此值 T1 复测后可校准）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SequentialTestConfig {
    /// 显著性水平（Type I error 预算）
    pub alpha: f64,
    /// 检验功效（1 − β）
    pub power: f64,
    /// 最小判定样本量（小样本不判定）
    pub min_samples: usize,
    /// 最大样本量（硬停止上限）
    pub max_samples: usize,
    /// 效果量判据:rayon 相对 inline 快 ≥ 此比例才允许 Promote
    pub effect_size: f64,
}

impl Default for SequentialTestConfig {
    /// 手册 §8.4 默认参数（effect_size 为工程初值 0.2,标注 T1 可校准）
    fn default() -> Self {
        Self {
            alpha: 0.05,
            power: 0.8,
            min_samples: 30,
            max_samples: 500,
            effect_size: 0.2,
        }
    }
}

impl SequentialTestConfig {
    /// 校验配置合法性 — 非法配置返回 [`HtsError`]（编程错误显式化）
    pub fn validate(&self) -> Result<(), HtsError> {
        if !(0.0 < self.alpha && self.alpha < 1.0) {
            return Err(HtsError::AlphaOutOfRange(self.alpha));
        }
        if !(0.0 < self.power && self.power < 1.0) {
            return Err(HtsError::PowerOutOfRange(self.power));
        }
        if self.min_samples > self.max_samples {
            return Err(HtsError::SampleRange(self.min_samples, self.max_samples));
        }
        if self.effect_size <= 0.0 || !self.effect_size.is_finite() {
            return Err(HtsError::EffectSizeNonPositive(self.effect_size));
        }
        Ok(())
    }
}

/// 检验决策 — `record` 每次调用返回三态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestDecision {
    /// 证据不足,继续采样
    Continue,
    /// 序贯检验通过 + 效果量达标 — 建议更新阈值
    ///
    /// `threshold` 为候选阈值建议:由调用方经 [`SequentialTest::set_candidate_threshold`]
    /// 注入（T14 接线时设为测定/校准的目标阈值）;未注入时为 0,
    /// 表示"仅标记效果量达标,具体阈值待接线层决定"。
    Promote {
        /// 建议的新阈值（候选值,由调用方注入）
        threshold: usize,
    },
    /// 检验拒绝 — 不更新阈值（含:效果量未达标 / SPRT 跨下界 / 预算耗尽）
    Reject,
}

/// 序贯检验状态机 — Inline vs Rayon 双桶样本积累 + Wald SPRT 判定
///
/// # 线程模型
/// 单实例单线程使用（T14 接线时每个 [`TaskKind`] 一个实例,串行喂样本）;
/// 不实现 `Sync` 共享——检验状态是私有证据,不跨线程竞争。
pub struct SequentialTest {
    /// 检验参数（构造后不可变）
    config: SequentialTestConfig,
    /// Inline 桶样本（µs 计时）
    inline_samples: Vec<f64>,
    /// Rayon 桶样本（µs 计时）
    rayon_samples: Vec<f64>,
    /// 候选阈值建议（Promote 时回传;0 = 未注入）
    candidate_threshold: usize,
    /// 已消耗的 alpha 预算（0 或 alpha:一次性预算墙,终局即耗尽）
    alpha_spent: f64,
    /// 终局判定（None = 检验进行中）
    decision: Option<TestDecision>,
}

impl SequentialTest {
    /// 构造状态机 — 便捷入口
    ///
    /// # Panics
    /// 配置非法时 panic（编程错误,构造前提;调用方可用
    /// [`try_new`](Self::try_new) 获得 [`Result`] 语义）。
    /// 与 [`crate::bridge::ComputeBridge::new`] 同模式:前提失败即进程级缺陷,
    /// 无运行期降级路径。
    #[must_use]
    pub fn new(config: SequentialTestConfig) -> Self {
        match config.validate() {
            Ok(()) => Self {
                config,
                inline_samples: Vec::new(),
                rayon_samples: Vec::new(),
                candidate_threshold: 0,
                alpha_spent: 0.0,
                decision: None,
            },
            Err(e) => panic!("SequentialTest 配置非法（构造前应 validate）: {e}"),
        }
    }

    /// 构造状态机 — 校验失败返回 [`HtsError`]（非 panic 路径）
    pub fn try_new(config: SequentialTestConfig) -> Result<Self, HtsError> {
        config.validate()?;
        Ok(Self {
            config,
            inline_samples: Vec::new(),
            rayon_samples: Vec::new(),
            candidate_threshold: 0,
            alpha_spent: 0.0,
            decision: None,
        })
    }

    /// 注入候选阈值建议 — Promote 时随决策回传（T14 接线用）
    pub fn set_candidate_threshold(&mut self, threshold: usize) {
        self.candidate_threshold = threshold;
    }

    /// 记录一组双桶计时并返回当前决策
    ///
    /// # 前置条件
    /// `inline_us` / `rayon_us` 须为有限正计时值（µs,计时器输出天然满足）。
    ///
    /// # 决策语义（契约）
    /// 见模块文档"判定规则"1-6;终局后实例关闭,`record` 一律返回
    /// [`Reject`](TestDecision::Reject)（预算耗尽强制 Reject,永不二次 Promote）。
    #[must_use]
    pub fn record(&mut self, inline_us: f64, rayon_us: f64) -> TestDecision {
        // 终局后（alpha 预算已耗尽,实例关闭）:强制 Reject——不再接受样本,
        // 永不产生新的 Promote 判定（PostHog 式预算墙,防 p-hacking 的预算语义）
        if self.decision.is_some() {
            return TestDecision::Reject;
        }
        self.inline_samples.push(inline_us);
        self.rayon_samples.push(rayon_us);
        let n = self.inline_samples.len();
        // 小样本不判定（契约 1）
        if n < self.config.min_samples {
            return TestDecision::Continue;
        }
        let decision = self.decide_now();
        if decision != TestDecision::Continue {
            // alpha spending:终局判定一次性耗尽本实例的 alpha 预算（PostHog 式预算墙）
            self.alpha_spent = self.config.alpha;
            self.decision = Some(decision);
        }
        decision
    }

    /// 已消耗的 alpha 预算 — 0（检验进行中）或 `config.alpha`（终局已判定）
    ///
    /// WHY 一次性预算墙:PostHog 式"预先承诺边界"要求每次检验只允许一次终局判定,
    /// 防止反复检验直到偶然显著（p-hacking）;正式 alpha-spending 函数留待 T14。
    #[must_use]
    pub fn spent_alpha(&self) -> f64 {
        self.alpha_spent
    }

    /// 已积累的样本数（双桶各 n 条）
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.inline_samples.len()
    }

    /// 当前终局决策（None = 检验进行中）
    #[must_use]
    pub fn decision(&self) -> Option<TestDecision> {
        self.decision
    }

    /// 判定核心 — 硬停止 / SPRT 边界 / 效果量三重规则
    fn decide_now(&self) -> TestDecision {
        let n = self.inline_samples.len();
        // 契约 5:硬停止 — 达到 max_samples 必须给出终局
        if n >= self.config.max_samples {
            return if self.effect_met() && self.lr_crossed_upper() {
                TestDecision::Promote {
                    threshold: self.candidate_threshold,
                }
            } else {
                TestDecision::Reject
            };
        }
        let lr = self.log_likelihood_ratio();
        // Wald 1945 SPRT 边界:A = ln((1-β)/α),B = ln(β/(1-α))
        let upper = ((1.0 - self.config.power) / self.config.alpha).ln();
        let lower = (self.config.power / (1.0 - self.config.alpha)).ln();
        if lr >= upper {
            // 契约 3:跨上界但效果量未达标 → Reject（Promote 仅在效果量达到时发出）
            return if self.effect_met() {
                TestDecision::Promote {
                    threshold: self.candidate_threshold,
                }
            } else {
                TestDecision::Reject
            };
        }
        if lr <= lower {
            // 契约 4:跨下界 → Reject（接受 H0:无证据表明 rayon 更快）
            return TestDecision::Reject;
        }
        TestDecision::Continue
    }

    /// 效果量判据 — rayon 相对 inline 快 ≥ effect_size（如快 ≥20%）
    fn effect_met(&self) -> bool {
        let mean_inline = mean(&self.inline_samples);
        // 防御:NaN 或非正均值无比较意义（计时输入不应出现）
        if !mean_inline.is_finite() || mean_inline <= 0.0 {
            return false;
        }
        let mean_rayon = mean(&self.rayon_samples);
        let d = mean_inline - mean_rayon;
        d >= self.config.effect_size * mean_inline
    }

    /// SPRT 对数似然比（Wald 1945,正态近似）— 仅上界跨越用,下界由数值比较
    ///
    /// H0:μ_d = 0;H1:μ_d = δ = effect_size × mean_inline。
    /// Λ_n = (δ/σ²)·Σd_i − n·δ²/(2σ²)。
    /// 零方差（所有差值相同）时检验退化为确定性比较:达标 → +∞,未达标 → −∞。
    fn log_likelihood_ratio(&self) -> f64 {
        let n = self.inline_samples.len();
        debug_assert!(n >= 1, "record 已保证至少 1 个样本");
        let mean_inline = mean(&self.inline_samples);
        let delta = self.config.effect_size * mean_inline;
        // 防御:mean_inline 非正或 NaN 时差值检验无意义（计时输入不应出现）
        if !delta.is_finite() || delta <= 0.0 {
            return f64::NEG_INFINITY;
        }
        let diffs: Vec<f64> = self
            .inline_samples
            .iter()
            .zip(&self.rayon_samples)
            .map(|(a, b)| a - b)
            .collect();
        let mean_d = mean(&diffs);
        if !mean_d.is_finite() {
            return f64::NEG_INFINITY;
        }
        let var = sample_variance(&diffs, mean_d);
        if var <= 0.0 {
            // 零方差:全部差值相同 → 无随机性,退化为确定性比较
            return if mean_d >= delta {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            };
        }
        let sum_d: f64 = diffs.iter().sum();
        (delta / var) * sum_d - (n as f64 * delta * delta) / (2.0 * var)
    }

    /// SPRT 上界跨越判定（供硬停止分支复用）
    fn lr_crossed_upper(&self) -> bool {
        let upper = ((1.0 - self.config.power) / self.config.alpha).ln();
        self.log_likelihood_ratio() >= upper
    }
}

/// 算术均值 — n ≥ 1 保证（调用方保证非空切片）
fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// 样本方差（ddof = 1）;少于 2 个样本返回 0（由调用方退化处理）
fn sample_variance(v: &[f64], m: f64) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let ss = v.iter().map(|x| {
        let d = x - m;
        d * d
    });
    ss.sum::<f64>() / (v.len() - 1) as f64
}
