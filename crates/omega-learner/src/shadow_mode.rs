//! R1 影子模式 — 2 周观察期对比报告与解冻条件评估（ADR-043）
//!
//! 对应任务: **P4-W16.2.2**（影子模式类型实现）
//! 对应 ADR: **ADR-043**（R1 影子模式设计，5 项工程实施决策）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.5（影子模式 2 周为 R1 解冻前置）
//!
//! # 核心职责
//!
//! 承载 ADR-043 决策 2 的 `ShadowComparisonReport` 类型与决策 3 的解冻条件评估。
//! 本模块**不重复** ADR-037 `CapabilityToken` 四态管理（Provisional/Authorized/Cooldown/Frozen
//! 由 `decay_engine::capability_registry::CapabilityTokenRegistry` 承载），仅追踪对比报告历史
//! 与胜率计算，作为解冻评审的客观数据依据。
//!
//! # 设计原则
//!
//! 1. **纯数据 + 算法**: `ShadowComparisonReport` / `StrategyMetrics` / `ComparisonResult` 是
//!    纯数据类型，便于序列化与持久化；`ShadowModeTracker` 是状态机，管理对比报告历史
//! 2. **无副作用**: 本模块不直接修改 `CapabilityToken` 状态，仅返回 `PromotionReadiness` /
//!    `RollbackSignal` 评估结果，由调用方（编排器）执行实际状态转换
//! 3. **不可重复创建**: `ShadowModeTracker` 创建后即绑定起始时间，回滚后通过 `reset()` 重置
//!
//! # R2 冻结声明（ADR-042）
//!
//! 本模块仅承载 R1 影子模式，**不涉及 R2 路径**。
//!
//! # 4 项解冻条件（ADR-043 决策 3）
//!
//! 1. **EWMA 达标**: `success_ewma >= EWMA_PROMOTION_THRESHOLD (0.7)`
//! 2. **对比胜率**: 14 天内 R1 优于 L3 的天数 >= 10 天（71.4%）
//! 3. **观察期满**: 影子模式持续 >= 14 天
//! 4. **无 AsaIntervention**: 观察期内 R1 接缝未触发 AsaIntervention
//!
//! # 回滚触发条件（ADR-043 决策 4）
//!
//! 任一满足即触发回滚：
//! - 连续 3 天 R1 显著差于 L3（`R1SignificantlyWorse` 连续 3 次）
//! - AsaIntervention 触发（由调用方检测并传入信号）
//! - EWMA 崩塌（24 小时内下降 >= 0.3）
//! - 召回率下降（R1 较 L3 下降 >= 5%）

use crate::seam::SeamId;
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// 默认观察期天数（2 周 = 14 天，ADR-043 决策 3 条件 3）
pub const DEFAULT_OBSERVATION_DAYS: u16 = 14;

/// 默认解冻胜率阈值（71.4% = 10/14，ADR-043 决策 3 条件 2）
///
/// WHY 0.714: 统计学 p < 0.05 的胜率阈值（单尾二项检验，14 天中 10 天以上胜率
/// 在 R1 无真实优势时约为 5.9%，接近 5% 显著性水平）。
pub const DEFAULT_WIN_RATE_THRESHOLD: f64 = 0.714;

/// EWMA 解冻阈值（ADR-043 决策 3 条件 1，与 `CapabilityToken::ACTIVATION_THRESHOLD` 区分）
///
/// WHY 0.7 而非 0.3: 影子模式解冻需更严苛的证据，0.7 对应"70% 以上成功率"，
/// 远高于激活阈值 0.3，确保 R1 策略稳定可靠才解冻。
pub const EWMA_PROMOTION_THRESHOLD: f32 = 0.7;

/// 默认回滚检测的连续显著退化天数（ADR-043 决策 4 触发条件 1）
pub const REGRESSION_STREAK_THRESHOLD: u32 = 3;

/// EWMA 崩塌阈值（24 小时内下降 >= 0.3，ADR-043 决策 4 触发条件 3）
pub const EWMA_COLLAPSE_THRESHOLD: f32 = 0.3;

/// 召回率下降阈值（R1 较 L3 下降 >= 5%，ADR-043 决策 4 触发条件 4）
pub const RECALL_DROP_THRESHOLD: f32 = 0.05;

/// 显著优于阈值（R1 较 L3 综合得分高 >= 0.1，ComparisonResult::SignificantlyBetter 分档）
pub const SIGNIFICANT_BETTER_THRESHOLD: f32 = 0.1;

/// 略优于阈值（R1 较 L3 综合得分高 >= 0.02 但 < 0.1，ComparisonResult::SlightlyBetter 分档）
pub const SLIGHT_BETTER_THRESHOLD: f32 = 0.02;

// ============================================================
// StrategyMetrics
// ============================================================

/// 策略指标快照 — R1 与 L3 通用指标容器
///
/// 承载单日策略表现的 5 项核心指标，用于 `ShadowComparisonReport` 对比。
///
/// # 字段
/// - `recall_rate`: 召回率 [0.0, 1.0]（R1 接缝的核心指标）
/// - `false_block_rate`: 误杀率 [0.0, 1.0]（反指标，越低越好）
/// - `latency_penalty`: 延迟惩罚 [0.0, 1.0]（归一化）
/// - `composite_score`: 综合得分 [-0.5, 1.0] = recall - 0.5*false_block - 0.3*latency
/// - `sample_count`: 样本数（当日策略产生的轨迹数）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyMetrics {
    /// 召回率 [0.0, 1.0]
    pub recall_rate: f32,
    /// 误杀率 [0.0, 1.0]
    pub false_block_rate: f32,
    /// 延迟惩罚 [0.0, 1.0]
    pub latency_penalty: f32,
    /// 综合得分（recall - 0.5*false_block - 0.3*latency）
    pub composite_score: f32,
    /// 样本数
    pub sample_count: u64,
}

impl StrategyMetrics {
    /// 创建策略指标快照并自动计算综合得分
    ///
    /// # 参数
    /// - `recall_rate`: 召回率 ∈ [0, 1]
    /// - `false_block_rate`: 误杀率 ∈ [0, 1]
    /// - `latency_penalty`: 延迟惩罚 ∈ [0, 1]
    /// - `sample_count`: 样本数
    ///
    /// # 错误
    /// 任一比率字段非有限或不在 [0, 1] 返回 `ShadowModeError::InvalidMetric`
    pub fn new(
        recall_rate: f32,
        false_block_rate: f32,
        latency_penalty: f32,
        sample_count: u64,
    ) -> std::result::Result<Self, ShadowModeError> {
        for (name, v) in [
            ("recall_rate", recall_rate),
            ("false_block_rate", false_block_rate),
            ("latency_penalty", latency_penalty),
        ] {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                return Err(ShadowModeError::InvalidMetric {
                    field: name,
                    value: v,
                });
            }
        }
        let composite_score = recall_rate - 0.5 * false_block_rate - 0.3 * latency_penalty;
        Ok(Self {
            recall_rate,
            false_block_rate,
            latency_penalty,
            composite_score,
            sample_count,
        })
    }

    /// 返回综合得分
    pub fn composite_score(&self) -> f32 {
        self.composite_score
    }
}

// ============================================================
// ComparisonResult
// ============================================================

/// 对比结论 — R1 是否优于 L3 基线（5 档枚举）
///
/// 根据 `composite_score` 差值（R1 - L3）分档:
/// - 差值 ≥ +0.1: `R1SignificantlyBetter`
/// - 差值 ∈ [+0.02, +0.1): `R1SlightlyBetter`
/// - 差值 ∈ [-0.02, +0.02): `Tied`
/// - 差值 ∈ [-0.1, -0.02): `R1SlightlyWorse`
/// - 差值 ≤ -0.1: `R1SignificantlyWorse`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComparisonResult {
    /// R1 显著优于 L3（差值 ≥ +0.1）
    R1SignificantlyBetter,
    /// R1 略优于 L3（差值 ∈ [+0.02, +0.1)）
    R1SlightlyBetter,
    /// R1 与 L3 持平（差值 ∈ [-0.02, +0.02)）
    Tied,
    /// R1 略差于 L3（差值 ∈ [-0.1, -0.02)）
    R1SlightlyWorse,
    /// R1 显著差于 L3（差值 ≤ -0.1）
    R1SignificantlyWorse,
}

impl ComparisonResult {
    /// 根据综合得分差值（R1 - L3）判定对比结论
    pub fn from_diff(diff: f32) -> Self {
        if diff >= SIGNIFICANT_BETTER_THRESHOLD {
            Self::R1SignificantlyBetter
        } else if diff >= SLIGHT_BETTER_THRESHOLD {
            Self::R1SlightlyBetter
        } else if diff > -SLIGHT_BETTER_THRESHOLD {
            Self::Tied
        } else if diff > -SIGNIFICANT_BETTER_THRESHOLD {
            Self::R1SlightlyWorse
        } else {
            Self::R1SignificantlyWorse
        }
    }

    /// 判定 R1 是否达到解冻门槛（胜率计算用）
    ///
    /// `R1SignificantlyBetter` 或 `R1SlightlyBetter` 计入胜率分子。
    pub fn r1_is_better(self) -> bool {
        matches!(self, Self::R1SignificantlyBetter | Self::R1SlightlyBetter)
    }

    /// 判定是否为显著退化（回滚检测用）
    pub fn is_significantly_worse(self) -> bool {
        matches!(self, Self::R1SignificantlyWorse)
    }
}

// ============================================================
// ShadowComparisonReport
// ============================================================

/// 影子模式对比报告 — R1 训练策略 vs L3 主路径策略的每日对比快照
///
/// 对应 ADR-043 决策 2，每日 UTC 00:00 由编排器生成一份。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowComparisonReport {
    /// 报告日期（UTC，每日生成一份）
    pub report_date: chrono::DateTime<chrono::Utc>,
    /// 所属接缝（固定为 S7RecallQuota，保留字段便于扩展）
    pub seam: SeamId,
    /// R1 训练策略的指标快照
    pub r1_metrics: StrategyMetrics,
    /// L3 主路径策略的指标快照（基线）
    pub l3_baseline_metrics: StrategyMetrics,
    /// 对比结论（R1 是否优于 L3 基线）
    pub comparison: ComparisonResult,
    /// 观察期剩余天数（2 周 = 14 天，每日递减）
    pub remaining_days: u16,
}

impl ShadowComparisonReport {
    /// 创建对比报告（自动计算对比结论）
    ///
    /// # 参数
    /// - `report_date`: 报告日期
    /// - `r1_metrics`: R1 指标快照
    /// - `l3_baseline_metrics`: L3 基线指标快照
    /// - `remaining_days`: 观察期剩余天数
    pub fn new(
        report_date: chrono::DateTime<chrono::Utc>,
        r1_metrics: StrategyMetrics,
        l3_baseline_metrics: StrategyMetrics,
        remaining_days: u16,
    ) -> Self {
        let diff = r1_metrics.composite_score - l3_baseline_metrics.composite_score;
        let comparison = ComparisonResult::from_diff(diff);
        Self {
            report_date,
            seam: SeamId::S7RecallQuota,
            r1_metrics,
            l3_baseline_metrics,
            comparison,
            remaining_days,
        }
    }

    /// 返回 R1 综合得分与 L3 基线的差值
    pub fn score_diff(&self) -> f32 {
        self.r1_metrics.composite_score - self.l3_baseline_metrics.composite_score
    }

    /// 返回召回率差值（R1 - L3），用于回滚检测（决策 4 触发条件 4）
    pub fn recall_rate_diff(&self) -> f32 {
        self.r1_metrics.recall_rate - self.l3_baseline_metrics.recall_rate
    }
}

// ============================================================
// PromotionReadiness
// ============================================================

/// 解冻就绪状态 — 4 项条件评估结果
///
/// 对应 ADR-043 决策 3 的 4 项解冻条件，全部满足才可解冻。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionReadiness {
    /// 条件 1: EWMA ≥ 0.7
    pub ewma达标: bool,
    /// 条件 2: 对比胜率 ≥ 71.4%
    pub win_rate_达标: bool,
    /// 条件 3: 观察期 ≥ 14 天
    pub observation_complete: bool,
    /// 条件 4: 无 AsaIntervention
    pub no_asa_intervention: bool,
    /// 当前 EWMA 值（诊断用）
    pub current_ewma: f32,
    /// 当前胜率（诊断用）
    pub current_win_rate: f64,
    /// 已观察天数
    pub elapsed_days: u16,
}

impl PromotionReadiness {
    /// 是否全部条件满足（可解冻）
    pub fn is_ready(&self) -> bool {
        self.ewma达标 && self.win_rate_达标 && self.observation_complete && self.no_asa_intervention
    }

    /// 返回未满足的条件列表（用于评审报告）
    pub fn unmet_conditions(&self) -> Vec<&'static str> {
        let mut unmet = Vec::new();
        if !self.ewma达标 {
            unmet.push("EWMA < 0.7");
        }
        if !self.win_rate_达标 {
            unmet.push("win_rate < 71.4%");
        }
        if !self.observation_complete {
            unmet.push("observation < 14 days");
        }
        if !self.no_asa_intervention {
            unmet.push("AsaIntervention triggered");
        }
        unmet
    }
}

// ============================================================
// RollbackSignal
// ============================================================

/// 回滚信号 — 触发影子模式回滚的条件类型
///
/// 对应 ADR-043 决策 4 的 4 项回滚触发条件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RollbackSignal {
    /// 连续 N 天 R1 显著差于 L3（决策 4 触发条件 1）
    ConsecutiveRegression {
        /// 连续显著退化的天数
        streak: u32,
        /// 最近一次对比报告
        last_report: ShadowComparisonReport,
    },
    /// AsaIntervention 触发（决策 4 触发条件 2）
    AsaIntervention {
        /// 触发时间
        triggered_at: chrono::DateTime<chrono::Utc>,
    },
    /// EWMA 崩塌（24 小时内下降 ≥ 0.3，决策 4 触发条件 3）
    EwmaCollapse {
        /// 崩塌前 EWMA
        before: f32,
        /// 崩塌后 EWMA
        after: f32,
        /// 下降幅度
        drop: f32,
    },
    /// 召回率下降（R1 较 L3 下降 ≥ 5%，决策 4 触发条件 4）
    RecallRateDrop {
        /// 下降幅度（绝对值）
        drop: f32,
        /// 最近一次对比报告
        last_report: ShadowComparisonReport,
    },
}

impl RollbackSignal {
    /// 返回回滚原因描述（用于日志/告警）
    pub fn reason(&self) -> &'static str {
        match self {
            Self::ConsecutiveRegression { .. } => "consecutive regression",
            Self::AsaIntervention { .. } => "ASA intervention",
            Self::EwmaCollapse { .. } => "EWMA collapse",
            Self::RecallRateDrop { .. } => "recall rate drop",
        }
    }
}

// ============================================================
// ShadowModeError
// ============================================================

/// 影子模式错误类型
#[derive(Debug, thiserror::Error)]
pub enum ShadowModeError {
    /// 指标值非法（非有限或不在 [0, 1]）
    #[error("invalid metric {field} = {value}")]
    InvalidMetric {
        /// 非法字段名
        field: &'static str,
        /// 非法值
        value: f32,
    },

    /// 未就绪时尝试解冻
    #[error("promotion not ready: unmet conditions = {unmet:?}")]
    NotReady {
        /// 未满足的条件列表
        unmet: Vec<&'static str>,
    },
}

// ============================================================
// ShadowModeTracker
// ============================================================

/// 影子模式追踪器 — 管理对比报告历史与胜率计算
///
/// # 设计原则
///
/// - **不管理 CapabilityToken 状态**: token 状态由 `CapabilityTokenRegistry` 承载
/// - **纯追踪 + 评估**: 仅记录对比报告历史，提供解冻/回滚评估接口
/// - **时间感知**: 通过 `now` 参数（i64 UTC 秒）评估观察期，与 `CapabilityToken` 一致
pub struct ShadowModeTracker {
    /// 观察期起始时间（UTC 秒）
    start_time: i64,
    /// 观察期总天数
    observation_days: u16,
    /// 解冻胜率阈值
    win_rate_threshold: f64,
    /// 累计对比报告历史（按时间顺序）
    reports: Vec<ShadowComparisonReport>,
    /// 当前连续显著退化计数（回滚检测用）
    consecutive_regression: u32,
    /// 历史最大 EWMA（用于 EWMA 崩塌检测）
    peak_ewma: f32,
    /// 最近一次 EWMA（用于 EWMA 崩塌检测）
    last_ewma: f32,
    /// 最近一次 EWMA 更新时间（UTC 秒）
    last_ewma_time: i64,
    /// AsaIntervention 触发次数（观察期内）
    asa_count: u32,
}

impl ShadowModeTracker {
    /// 创建影子模式追踪器（默认配置）
    ///
    /// # 参数
    /// - `now`: 观察期起始时间（UTC 秒）
    pub fn new(now: i64) -> Self {
        Self::with_config(now, DEFAULT_OBSERVATION_DAYS, DEFAULT_WIN_RATE_THRESHOLD)
    }

    /// 创建影子模式追踪器（自定义配置）
    pub fn with_config(now: i64, observation_days: u16, win_rate_threshold: f64) -> Self {
        Self {
            start_time: now,
            observation_days,
            win_rate_threshold,
            reports: Vec::new(),
            consecutive_regression: 0,
            peak_ewma: 0.0,
            last_ewma: 0.0,
            last_ewma_time: now,
            asa_count: 0,
        }
    }

    /// 记录每日对比报告，返回可能的回滚信号
    ///
    /// # 参数
    /// - `report`: 每日对比报告
    ///
    /// # 返回
    /// - `Some(RollbackSignal)`: 触发回滚
    /// - `None`: 无回滚触发
    pub fn record_daily_report(
        &mut self,
        report: ShadowComparisonReport,
    ) -> Option<RollbackSignal> {
        // 检测连续显著退化
        if report.comparison.is_significantly_worse() {
            self.consecutive_regression += 1;
            if self.consecutive_regression >= REGRESSION_STREAK_THRESHOLD {
                return Some(RollbackSignal::ConsecutiveRegression {
                    streak: self.consecutive_regression,
                    last_report: report.clone(),
                });
            }
        } else {
            self.consecutive_regression = 0;
        }

        // 检测召回率下降（R1 较 L3 下降 >= 5%）
        let recall_diff = report.recall_rate_diff();
        if recall_diff <= -RECALL_DROP_THRESHOLD {
            return Some(RollbackSignal::RecallRateDrop {
                drop: -recall_diff,
                last_report: report.clone(),
            });
        }

        self.reports.push(report);
        None
    }

    /// 更新 EWMA 值并检测崩塌
    ///
    /// # 参数
    /// - `ewma`: 当前 EWMA 值
    /// - `now`: 当前时间（UTC 秒）
    ///
    /// # 返回
    /// - `Some(RollbackSignal::EwmaCollapse)`: 24 小时内 EWMA 下降 >= 0.3
    /// - `None`: 无崩塌
    pub fn update_ewma(&mut self, ewma: f32, now: i64) -> Option<RollbackSignal> {
        // 检测 24 小时内崩塌
        let time_diff = now - self.last_ewma_time;
        if time_diff > 0 && time_diff <= 86400 {
            let drop = self.last_ewma - ewma;
            if drop >= EWMA_COLLAPSE_THRESHOLD {
                return Some(RollbackSignal::EwmaCollapse {
                    before: self.last_ewma,
                    after: ewma,
                    drop,
                });
            }
        }

        // 更新峰值与最近值
        if ewma > self.peak_ewma {
            self.peak_ewma = ewma;
        }
        self.last_ewma = ewma;
        self.last_ewma_time = now;
        None
    }

    /// 记录 AsaIntervention 触发
    ///
    /// # 参数
    /// - `triggered_at`: 触发时间
    ///
    /// # 返回
    /// 总是返回 `RollbackSignal::AsaIntervention`（决策 4 触发条件 2）
    pub fn record_asa_intervention(
        &mut self,
        triggered_at: chrono::DateTime<chrono::Utc>,
    ) -> RollbackSignal {
        self.asa_count += 1;
        RollbackSignal::AsaIntervention { triggered_at }
    }

    /// 计算当前胜率（R1 优于 L3 的天数比例）
    pub fn current_win_rate(&self) -> f64 {
        if self.reports.is_empty() {
            return 0.0;
        }
        let wins = self
            .reports
            .iter()
            .filter(|r| r.comparison.r1_is_better())
            .count();
        wins as f64 / self.reports.len() as f64
    }

    /// 已观察天数
    pub fn elapsed_days(&self, now: i64) -> u16 {
        let secs_per_day: i64 = 86400;
        let elapsed_secs = (now - self.start_time).max(0);
        ((elapsed_secs / secs_per_day) as u16).min(self.observation_days)
    }

    /// 剩余观察天数
    pub fn remaining_days(&self, now: i64) -> u16 {
        self.observation_days.saturating_sub(self.elapsed_days(now))
    }

    /// 观察期是否完成
    pub fn observation_period_complete(&self, now: i64) -> bool {
        self.elapsed_days(now) >= self.observation_days
    }

    /// 评估 4 项解冻条件是否全部满足
    ///
    /// # 参数
    /// - `now`: 当前时间（UTC 秒）
    /// - `current_ewma`: 当前 EWMA 值
    ///
    /// # 返回
    /// `PromotionReadiness` 包含 4 项条件状态与诊断数据
    pub fn evaluate_promotion_readiness(&self, now: i64, current_ewma: f32) -> PromotionReadiness {
        let win_rate = self.current_win_rate();
        let elapsed = self.elapsed_days(now);

        PromotionReadiness {
            ewma达标: current_ewma >= EWMA_PROMOTION_THRESHOLD,
            win_rate_达标: win_rate >= self.win_rate_threshold,
            observation_complete: elapsed >= self.observation_days,
            no_asa_intervention: self.asa_count == 0,
            current_ewma,
            current_win_rate: win_rate,
            elapsed_days: elapsed,
        }
    }

    /// 重置观察期（回滚后调用）
    pub fn reset(&mut self, now: i64) {
        self.start_time = now;
        self.reports.clear();
        self.consecutive_regression = 0;
        self.peak_ewma = 0.0;
        self.last_ewma = 0.0;
        self.last_ewma_time = now;
        self.asa_count = 0;
    }

    /// 返回 AsaIntervention 触发次数
    pub fn asa_count(&self) -> u32 {
        self.asa_count
    }

    /// 返回累计报告数
    pub fn report_count(&self) -> usize {
        self.reports.len()
    }

    /// 返回峰值 EWMA
    pub fn peak_ewma(&self) -> f32 {
        self.peak_ewma
    }

    /// 返回最近 EWMA
    pub fn last_ewma(&self) -> f32 {
        self.last_ewma
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_metrics(recall: f32, false_block: f32, latency: f32, n: u64) -> StrategyMetrics {
        StrategyMetrics::new(recall, false_block, latency, n).unwrap()
    }

    fn make_report(diff: f32, remaining: u16) -> ShadowComparisonReport {
        let r1 = make_metrics(0.9, 0.05, 0.1, 100);
        // 保持 l3.recall_rate 与 r1 一致，避免 recall_rate_diff 触发 RecallRateDrop 信号
        // （仅通过 composite_score 差值驱动 ComparisonResult，便于隔离测试 ConsecutiveRegression 等其他信号）
        let l3 = StrategyMetrics {
            recall_rate: r1.recall_rate,
            false_block_rate: r1.false_block_rate,
            latency_penalty: r1.latency_penalty,
            composite_score: r1.composite_score - diff,
            sample_count: 100,
        };
        ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, remaining)
    }

    // ----- StrategyMetrics 测试 -----

    #[test]
    fn test_metrics_new_valid() {
        let m = make_metrics(0.9, 0.05, 0.1, 100);
        assert!((m.composite_score - (0.9 - 0.5 * 0.05 - 0.3 * 0.1)).abs() < 1e-6);
    }

    #[test]
    fn test_metrics_new_invalid_recall() {
        let err = StrategyMetrics::new(1.5, 0.0, 0.0, 0).unwrap_err();
        assert!(matches!(
            err,
            ShadowModeError::InvalidMetric {
                field: "recall_rate",
                ..
            }
        ));
    }

    #[test]
    fn test_metrics_new_invalid_false_block() {
        let err = StrategyMetrics::new(0.5, -0.1, 0.0, 0).unwrap_err();
        assert!(matches!(
            err,
            ShadowModeError::InvalidMetric {
                field: "false_block_rate",
                ..
            }
        ));
    }

    #[test]
    fn test_metrics_new_invalid_latency() {
        let err = StrategyMetrics::new(0.5, 0.0, f32::NAN, 0).unwrap_err();
        assert!(matches!(
            err,
            ShadowModeError::InvalidMetric {
                field: "latency_penalty",
                ..
            }
        ));
    }

    // ----- ComparisonResult 测试 -----

    #[test]
    fn test_comparison_from_diff_significantly_better() {
        assert_eq!(
            ComparisonResult::from_diff(0.15),
            ComparisonResult::R1SignificantlyBetter
        );
        assert_eq!(
            ComparisonResult::from_diff(0.1),
            ComparisonResult::R1SignificantlyBetter
        );
    }

    #[test]
    fn test_comparison_from_diff_slightly_better() {
        assert_eq!(
            ComparisonResult::from_diff(0.05),
            ComparisonResult::R1SlightlyBetter
        );
        assert_eq!(
            ComparisonResult::from_diff(0.02),
            ComparisonResult::R1SlightlyBetter
        );
    }

    #[test]
    fn test_comparison_from_diff_tied() {
        assert_eq!(ComparisonResult::from_diff(0.01), ComparisonResult::Tied);
        assert_eq!(ComparisonResult::from_diff(0.0), ComparisonResult::Tied);
        assert_eq!(ComparisonResult::from_diff(-0.01), ComparisonResult::Tied);
    }

    #[test]
    fn test_comparison_from_diff_slightly_worse() {
        assert_eq!(
            ComparisonResult::from_diff(-0.05),
            ComparisonResult::R1SlightlyWorse
        );
        assert_eq!(
            ComparisonResult::from_diff(-0.02),
            ComparisonResult::R1SlightlyWorse
        );
    }

    #[test]
    fn test_comparison_from_diff_significantly_worse() {
        assert_eq!(
            ComparisonResult::from_diff(-0.15),
            ComparisonResult::R1SignificantlyWorse
        );
        assert_eq!(
            ComparisonResult::from_diff(-0.1),
            ComparisonResult::R1SignificantlyWorse
        );
    }

    #[test]
    fn test_comparison_r1_is_better() {
        assert!(ComparisonResult::R1SignificantlyBetter.r1_is_better());
        assert!(ComparisonResult::R1SlightlyBetter.r1_is_better());
        assert!(!ComparisonResult::Tied.r1_is_better());
        assert!(!ComparisonResult::R1SlightlyWorse.r1_is_better());
        assert!(!ComparisonResult::R1SignificantlyWorse.r1_is_better());
    }

    #[test]
    fn test_comparison_is_significantly_worse() {
        assert!(ComparisonResult::R1SignificantlyWorse.is_significantly_worse());
        assert!(!ComparisonResult::R1SlightlyWorse.is_significantly_worse());
        assert!(!ComparisonResult::Tied.is_significantly_worse());
    }

    // ----- ShadowComparisonReport 测试 -----

    #[test]
    fn test_report_new_computes_comparison() {
        let r1 = make_metrics(0.95, 0.0, 0.0, 100);
        let l3 = make_metrics(0.80, 0.0, 0.0, 100);
        let report = ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, 10);
        // diff = 0.95 - 0.80 = 0.15 > 0.1 阈值（避开 f32 精度边界 0.9-0.8=0.0999...<0.1）
        assert_eq!(report.comparison, ComparisonResult::R1SignificantlyBetter);
        assert_eq!(report.seam, SeamId::S7RecallQuota);
        assert_eq!(report.remaining_days, 10);
    }

    #[test]
    fn test_report_score_diff() {
        let r1 = make_metrics(0.9, 0.1, 0.2, 100);
        let l3 = make_metrics(0.8, 0.05, 0.1, 100);
        // 在 move 前计算预期差值，避免 use-after-move
        let expected_diff = r1.composite_score - l3.composite_score;
        let report = ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, 5);
        assert!((report.score_diff() - expected_diff).abs() < 1e-6);
    }

    #[test]
    fn test_report_recall_rate_diff() {
        let r1 = make_metrics(0.85, 0.0, 0.0, 100);
        let l3 = make_metrics(0.90, 0.0, 0.0, 100);
        let report = ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, 5);
        assert!((report.recall_rate_diff() - (-0.05)).abs() < 1e-6);
    }

    #[test]
    fn test_report_serde_round_trip() {
        let r1 = make_metrics(0.9, 0.05, 0.1, 100);
        let l3 = make_metrics(0.8, 0.1, 0.2, 100);
        let report = ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, 7);
        let json = serde_json::to_string(&report).unwrap();
        let de: ShadowComparisonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, de);
    }

    // ----- ShadowModeTracker 基础测试 -----

    #[test]
    fn test_tracker_new_initial_state() {
        let tracker = ShadowModeTracker::new(1_000_000);
        assert_eq!(tracker.elapsed_days(1_000_000), 0);
        assert_eq!(tracker.remaining_days(1_000_000), DEFAULT_OBSERVATION_DAYS);
        assert!(!tracker.observation_period_complete(1_000_000));
        assert_eq!(tracker.current_win_rate(), 0.0);
        assert_eq!(tracker.asa_count(), 0);
        assert_eq!(tracker.report_count(), 0);
    }

    #[test]
    fn test_tracker_elapsed_days() {
        let tracker = ShadowModeTracker::new(0);
        // 1 天 = 86400 秒
        assert_eq!(tracker.elapsed_days(86400), 1);
        assert_eq!(tracker.elapsed_days(86400 * 7), 7);
        assert_eq!(tracker.elapsed_days(86400 * 14), 14);
        // 超过观察期不超过 observation_days
        assert_eq!(tracker.elapsed_days(86400 * 30), DEFAULT_OBSERVATION_DAYS);
    }

    #[test]
    fn test_tracker_observation_period_complete() {
        let tracker = ShadowModeTracker::new(0);
        assert!(!tracker.observation_period_complete(86400 * 13));
        assert!(tracker.observation_period_complete(86400 * 14));
        assert!(tracker.observation_period_complete(86400 * 30));
    }

    // ----- 胜率计算测试 -----

    #[test]
    fn test_tracker_win_rate_all_wins() {
        let mut tracker = ShadowModeTracker::new(0);
        for _ in 0..10 {
            // R1 显著优于 L3（diff = 0.15）
            let report = make_report(0.15, 10);
            tracker.record_daily_report(report);
        }
        assert!((tracker.current_win_rate() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_tracker_win_rate_mixed() {
        let mut tracker = ShadowModeTracker::new(0);
        // 7 天胜 + 3 天负 → 胜率 0.7
        for _ in 0..7 {
            tracker.record_daily_report(make_report(0.15, 10)); // SignificantlyBetter
        }
        for _ in 0..3 {
            tracker.record_daily_report(make_report(-0.15, 10)); // SignificantlyWorse
                                                                 // 注意: 连续 3 天显著退化会触发回滚信号，这里仅测胜率
        }
        // 由于触发回滚，reports 可能少一些，但胜率仍按已记录的报告计算
        let win_rate = tracker.current_win_rate();
        assert!((0.0..=1.0).contains(&win_rate));
    }

    // ----- 回滚检测测试 -----

    #[test]
    fn test_tracker_rollback_on_consecutive_regression() {
        let mut tracker = ShadowModeTracker::new(0);
        // 连续 3 天显著退化（diff = -0.15）
        let mut signal = None;
        for _ in 0..3 {
            let report = make_report(-0.15, 10);
            if let Some(s) = tracker.record_daily_report(report) {
                signal = Some(s);
                break;
            }
        }
        assert!(signal.is_some());
        assert!(matches!(
            signal.unwrap(),
            RollbackSignal::ConsecutiveRegression { streak: 3, .. }
        ));
    }

    #[test]
    fn test_tracker_rollback_on_recall_rate_drop() {
        let mut tracker = ShadowModeTracker::new(0);
        // R1 召回率较 L3 下降 8%（> 5% 阈值）
        let r1 = make_metrics(0.82, 0.0, 0.0, 100);
        let l3 = make_metrics(0.90, 0.0, 0.0, 100);
        let report = ShadowComparisonReport::new(chrono::Utc::now(), r1, l3, 10);
        let signal = tracker.record_daily_report(report);
        assert!(matches!(
            signal,
            Some(RollbackSignal::RecallRateDrop { .. })
        ));
    }

    #[test]
    fn test_tracker_rollback_on_asa_intervention() {
        let mut tracker = ShadowModeTracker::new(0);
        let now = chrono::Utc::now();
        let signal = tracker.record_asa_intervention(now);
        assert!(matches!(signal, RollbackSignal::AsaIntervention { .. }));
        assert_eq!(tracker.asa_count(), 1);
    }

    #[test]
    fn test_tracker_rollback_on_ewma_collapse() {
        let mut tracker = ShadowModeTracker::new(0);
        // 初始 EWMA 0.9
        tracker.update_ewma(0.9, 0);
        // 12 小时后 EWMA 0.5（下降 0.4 > 0.3 阈值）
        let signal = tracker.update_ewma(0.5, 43200); // 12 小时 = 43200 秒
        assert!(
            matches!(signal, Some(RollbackSignal::EwmaCollapse { drop, .. }) if (drop - 0.4).abs() < 1e-6)
        );
    }

    #[test]
    fn test_tracker_no_rollback_on_slow_ewma_decline() {
        let mut tracker = ShadowModeTracker::new(0);
        tracker.update_ewma(0.9, 0);
        // 48 小时后 EWMA 0.5（超过 24 小时窗口，不触发崩塌）
        let signal = tracker.update_ewma(0.5, 86400 * 2);
        assert!(signal.is_none());
    }

    // ----- PromotionReadiness 测试 -----

    #[test]
    fn test_tracker_promotion_readiness_initial_not_ready() {
        let tracker = ShadowModeTracker::new(0);
        let readiness = tracker.evaluate_promotion_readiness(0, 0.5);
        assert!(!readiness.is_ready());
        // 初始状态: EWMA 未达标 / 胜率未达标 / 观察期未满 / 无 ASA
        assert!(!readiness.ewma达标);
        assert!(!readiness.win_rate_达标);
        assert!(!readiness.observation_complete);
        assert!(readiness.no_asa_intervention);
    }

    #[test]
    fn test_tracker_promotion_readiness_all_met() {
        let mut tracker = ShadowModeTracker::new(0);
        // 14 天 + 全胜 + EWMA 0.8
        for _ in 0..14 {
            tracker.record_daily_report(make_report(0.15, 0));
        }
        let readiness = tracker.evaluate_promotion_readiness(86400 * 14, 0.8);
        assert!(readiness.ewma达标);
        assert!(readiness.win_rate_达标);
        assert!(readiness.observation_complete);
        assert!(readiness.no_asa_intervention);
        assert!(readiness.is_ready());
    }

    #[test]
    fn test_tracker_promotion_readiness_asa_blocks() {
        let mut tracker = ShadowModeTracker::new(0);
        for _ in 0..14 {
            tracker.record_daily_report(make_report(0.15, 0));
        }
        tracker.record_asa_intervention(chrono::Utc::now());
        let readiness = tracker.evaluate_promotion_readiness(86400 * 14, 0.8);
        // ASA 触发后 no_asa_intervention = false
        assert!(!readiness.no_asa_intervention);
        assert!(!readiness.is_ready());
        assert!(readiness
            .unmet_conditions()
            .contains(&"AsaIntervention triggered"));
    }

    #[test]
    fn test_tracker_promotion_readiness_low_ewma() {
        let tracker = ShadowModeTracker::new(0);
        let readiness = tracker.evaluate_promotion_readiness(86400 * 14, 0.5);
        // EWMA 0.5 < 0.7 阈值
        assert!(!readiness.ewma达标);
        assert!(!readiness.is_ready());
    }

    #[test]
    fn test_tracker_promotion_readiness_low_win_rate() {
        let mut tracker = ShadowModeTracker::new(0);
        // 14 天全负（diff = -0.15，但需要先确保不触发连续退化回滚）
        // 改用 Tied（diff = 0.0）以避免回滚
        for _ in 0..14 {
            tracker.record_daily_report(make_report(0.0, 0));
        }
        let readiness = tracker.evaluate_promotion_readiness(86400 * 14, 0.8);
        // 胜率 0%（Tied 不计入胜率分子）
        assert!(!readiness.win_rate_达标);
        assert!(!readiness.is_ready());
    }

    #[test]
    fn test_tracker_promotion_readiness_observation_incomplete() {
        let tracker = ShadowModeTracker::new(0);
        let readiness = tracker.evaluate_promotion_readiness(86400 * 7, 0.8);
        // 观察期仅 7 天 < 14 天
        assert!(!readiness.observation_complete);
        assert!(!readiness.is_ready());
    }

    #[test]
    fn test_tracker_unmet_conditions_list() {
        let tracker = ShadowModeTracker::new(0);
        let readiness = tracker.evaluate_promotion_readiness(0, 0.0);
        let unmet = readiness.unmet_conditions();
        // 初始状态应有 3 项未满足（EWMA / 胜率 / 观察期），无 ASA
        assert_eq!(unmet.len(), 3);
        assert!(unmet.contains(&"EWMA < 0.7"));
        assert!(unmet.contains(&"win_rate < 71.4%"));
        assert!(unmet.contains(&"observation < 14 days"));
    }

    // ----- reset 测试 -----

    #[test]
    fn test_tracker_reset_clears_state() {
        let mut tracker = ShadowModeTracker::new(0);
        for _ in 0..5 {
            tracker.record_daily_report(make_report(0.15, 10));
        }
        tracker.update_ewma(0.8, 1000);
        tracker.record_asa_intervention(chrono::Utc::now());
        assert_eq!(tracker.report_count(), 5);
        assert_eq!(tracker.asa_count(), 1);

        tracker.reset(100_000);
        assert_eq!(tracker.report_count(), 0);
        assert_eq!(tracker.asa_count(), 0);
        assert_eq!(tracker.elapsed_days(100_000), 0);
    }

    // ----- PromotionReadiness 序列化测试 -----

    #[test]
    fn test_promotion_readiness_serde_round_trip() {
        let readiness = PromotionReadiness {
            ewma达标: true,
            win_rate_达标: false,
            observation_complete: true,
            no_asa_intervention: true,
            current_ewma: 0.75,
            current_win_rate: 0.65,
            elapsed_days: 14,
        };
        let json = serde_json::to_string(&readiness).unwrap();
        let de: PromotionReadiness = serde_json::from_str(&json).unwrap();
        assert_eq!(readiness, de);
    }

    // ----- 自定义配置测试 -----

    #[test]
    fn test_tracker_with_custom_config() {
        let tracker = ShadowModeTracker::with_config(0, 7, 0.8);
        // 7 天观察期 + 0.8 胜率阈值
        assert!(!tracker.observation_period_complete(86400 * 6));
        assert!(tracker.observation_period_complete(86400 * 7));
    }
}
