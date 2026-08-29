//! Loop 终止记分卡 — 三维打分终止判定（P3-T13c，v4.0 WI-32）
//!
//! 对应架构层: **L9 Quest**（quest-engine，ADR-151 裁决：挂既有 crate 增强）
//! 对应任务: **P3-T13c**（手册 W19，WI-32：收敛分×边际收益×配额余量）
//!
//! # 设计（v4.0 WI-32 规格）
//! 终止决策显式化:三维打分——收敛分（Gate 满足度）× 边际收益（近 N 轮
//! Evidence 增量）× 配额余量;低于阈值 → `AlreadyConverged` / `NoActionableWork`;
//! 作为 ThreeFactorAdjudicator 第四因子输入（StopRulingIssued 已消费于
//! control.rs,接入点已存在）。
//!
//! # 门禁
//! 记分卡误停率 <2%;人工"继续"覆盖键（保守阈值）。

use serde::{Deserialize, Serialize};

/// 三维权重 — 收敛 0.5 / 边际收益 0.3 / 配额余量 0.2
pub const W_CONVERGENCE: f64 = 0.5;
/// 边际收益权重
pub const W_MARGINAL_GAIN: f64 = 0.3;
/// 配额余量权重
pub const W_QUOTA: f64 = 0.2;
/// 终止阈值 — score < 该值 → 建议停止（保守:误停率 <2%）
pub const STOP_THRESHOLD: f64 = 0.35;
/// 收敛分阈值 — Gate 满足度 ≥ 该值才可能判 AlreadyConverged
pub const CONVERGED_GATE: f64 = 0.8;

/// 记分卡输入 — 三维证据
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoreInput {
    /// 收敛分（Gate 满足度,0-1）
    pub convergence: f64,
    /// 边际收益（近 N 轮 Evidence 增量,0-1 归一化）
    pub marginal_gain: f64,
    /// 配额余量（剩余/总额,0-1）
    pub quota_remaining: f64,
}

impl ScoreInput {
    /// 新建输入（各维 Clamp [0,1]）
    #[must_use]
    pub fn new(convergence: f64, marginal_gain: f64, quota_remaining: f64) -> Self {
        Self {
            convergence: convergence.clamp(0.0, 1.0),
            marginal_gain: marginal_gain.clamp(0.0, 1.0),
            quota_remaining: quota_remaining.clamp(0.0, 1.0),
        }
    }
}

/// 记分卡裁决 — 与 mas-sched ShouldRunVerdict 对齐
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreVerdict {
    /// 应继续运行
    Run,
    /// 已收敛（Gate 满足度达标,无需再跑）
    AlreadyConverged,
    /// 无可行工作（边际收益 < 阈值）
    NoActionableWork,
    /// 延后（配额不足,等下一周期）
    Defer,
}

impl ScoreVerdict {
    /// 是否停止建议（AlreadyConverged / NoActionableWork）
    #[must_use]
    pub const fn suggests_stop(self) -> bool {
        matches!(self, Self::AlreadyConverged | Self::NoActionableWork)
    }
}

/// Loop 记分卡 — 三维打分终止判定
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoopScorecard {
    /// 终止阈值（保守:误停率 <2%）
    pub stop_threshold: f64,
    /// 收敛判定门槛
    pub converged_gate: f64,
    /// 边际收益阈值（低于 → NoActionableWork）
    pub marginal_gate: f64,
}

impl Default for LoopScorecard {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopScorecard {
    /// 新建记分卡（默认保守阈值）
    #[must_use]
    pub fn new() -> Self {
        Self {
            stop_threshold: STOP_THRESHOLD,
            converged_gate: CONVERGED_GATE,
            marginal_gate: 0.1,
        }
    }

    /// 三维打分 — 收敛 0.5 + 边际 0.3 + 配额 0.2
    #[must_use]
    pub fn score(&self, input: &ScoreInput) -> f64 {
        W_CONVERGENCE * input.convergence
            + W_MARGINAL_GAIN * input.marginal_gain
            + W_QUOTA * input.quota_remaining
    }

    /// 裁决 — 显式终止决策
    ///
    /// # 判定顺序（保守优先）
    /// 1. 已收敛:Gate 满足度达标 **且** 边际收益 < 阈值（收敛完成,无进一步增量）;
    /// 2. 无可行工作:未收敛但边际收益 < 阈值（无增量且未收敛）;
    /// 3. 延后:配额余量 < 0.2（下一周期再评）;
    /// 4. 综合分 < 阈值:保守兜底停止;
    /// 5. 其余:Run。
    #[must_use]
    pub fn adjudicate(&self, input: &ScoreInput) -> ScoreVerdict {
        let s = self.score(input);
        if input.convergence >= self.converged_gate && input.marginal_gain < self.marginal_gate {
            return ScoreVerdict::AlreadyConverged;
        }
        if input.marginal_gain < self.marginal_gate {
            return ScoreVerdict::NoActionableWork;
        }
        if input.quota_remaining < 0.2 {
            return ScoreVerdict::Defer;
        }
        if s < self.stop_threshold {
            return ScoreVerdict::AlreadyConverged;
        }
        ScoreVerdict::Run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三维打分 — 权重正确
    #[test]
    fn score_weights() {
        let c = LoopScorecard::new();
        let input = ScoreInput::new(1.0, 1.0, 1.0);
        assert!((c.score(&input) - 1.0).abs() < 1e-9);
        let half = ScoreInput::new(0.5, 0.5, 0.5);
        assert!((c.score(&half) - 0.5).abs() < 1e-9);
        // 输入 Clamp
        let over = ScoreInput::new(2.0, -1.0, 0.5);
        assert!((c.score(&over) - (0.5 + 0.0 + 0.1)).abs() < 1e-9);
    }

    /// 运行 — 高收敛 + 高边际 + 配额充足 → Run
    #[test]
    fn run_verdict() {
        let c = LoopScorecard::new();
        let input = ScoreInput::new(0.9, 0.8, 0.8);
        assert_eq!(c.adjudicate(&input), ScoreVerdict::Run);
    }

    /// 已收敛 — Gate 满足度达标 + 综合分低 → AlreadyConverged
    #[test]
    fn already_converged() {
        let c = LoopScorecard::new();
        // 收敛 0.9 ≥ gate,但边际 0.05 < 0.1 → 但边际 < gate 且收敛 ≥ gate
        // 第一分支要求收敛 < gate → 不触发 NoActionableWork;第二分支触发
        let input = ScoreInput::new(0.9, 0.05, 0.5);
        assert_eq!(c.adjudicate(&input), ScoreVerdict::AlreadyConverged);
        assert!(ScoreVerdict::AlreadyConverged.suggests_stop());
    }

    /// 无可行工作 — 边际低且未收敛 → NoActionableWork
    #[test]
    fn no_actionable_work() {
        let c = LoopScorecard::new();
        let input = ScoreInput::new(0.3, 0.02, 0.5);
        assert_eq!(c.adjudicate(&input), ScoreVerdict::NoActionableWork);
        assert!(ScoreVerdict::NoActionableWork.suggests_stop());
    }

    /// 延后 — 配额不足 → Defer
    #[test]
    fn defer_on_low_quota() {
        let c = LoopScorecard::new();
        let input = ScoreInput::new(0.5, 0.5, 0.05);
        assert_eq!(c.adjudicate(&input), ScoreVerdict::Defer);
    }

    /// 误停率 — 随机健康输入下建议停止比例 <2%（门禁口径）
    #[test]
    fn low_false_stop_rate() {
        let c = LoopScorecard::new();
        let mut stops = 0usize;
        let n = 1000;
        for i in 0..n {
            // 健康负载:收敛/边际/配额均匀偏高（0.6-1.0）
            let input = ScoreInput::new(
                0.6 + (i % 40) as f64 / 100.0,
                0.6 + (i % 35) as f64 / 100.0,
                0.6 + (i % 30) as f64 / 100.0,
            );
            if c.adjudicate(&input).suggests_stop() {
                stops += 1;
            }
        }
        let rate = stops as f64 / n as f64;
        assert!(rate < 0.02, "健康输入误停率必须 <2%,实测 {rate:.3}");
    }

    /// 序列化往返 — 记分卡配置可持久化
    #[test]
    fn serde_roundtrip() {
        let c = LoopScorecard::new();
        let json = serde_json::to_string(&c).expect("编码成功");
        let back: LoopScorecard = serde_json::from_str(&json).expect("解码成功");
        assert_eq!(back, c);
    }
}
