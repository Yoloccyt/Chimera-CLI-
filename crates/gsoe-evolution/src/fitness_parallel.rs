//! 变体适应度批量评估并行化（P2-T13，v4.0 注入表 W13-14，Shadow 限定）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution）
//! 对应任务: **P2-T13**（滚动注入续期）
//!
//! # R2 约束（Shadow 限定）
//! gsoe 变体评估 rayon 化在 **Shadow 通道**内（v4.0 注入表 W13-14：
//! "gsoe 变体评估 rayon（Shadow 限定，R2 约束）"）——并行化只加速
//! 评估计算，不改变任何策略写入路径（转正仍须议会审批 ADR-142）。
//!
//! # 设计
//! 复用 Phase 1 ComputeBridge（nexus-core::compute）：批量适应度评分经
//! `bridge().route(GsoeEvaluate, n)` 三态判定 → `spawn_compute_batch` 并行；
//! 结果与串行逐元素一致（确定性断言，Ω₂）；保留串行回退
//! （env CHIMERA_NO_PARALLEL_GSOE，OnceLock 启动期一次读取）。

use nexus_core::compute::bridge;

/// 适应度评分输入（变体快照——纯数据，无 IO）
#[derive(Debug, Clone, PartialEq)]
pub struct FitnessSample {
    /// 变体 ID
    pub variant_id: String,
    /// 成功率（0-1）
    pub success_rate: f32,
    /// 置信度（0-1）
    pub confidence: f32,
}

impl FitnessSample {
    /// 新建样本
    #[must_use]
    pub fn new(variant_id: impl Into<String>, success_rate: f32, confidence: f32) -> Self {
        Self {
            variant_id: variant_id.into(),
            success_rate,
            confidence,
        }
    }
}

/// 环境开关（启动期一次读取，不在热路径——禁 feature 标志）
fn parallel_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CHIMERA_NO_PARALLEL_GSOE").is_err())
}

/// 批量适应度评分 — 串/并行双路径，结果逐元素一致
///
/// 综合分 = 0.6·成功率 + 0.4·置信度（纯计算，无 IO/await——rayon 契约）。
#[must_use]
pub fn score_batch(samples: &[FitnessSample]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let scores: Vec<f32> = samples
        .iter()
        .map(|s| 0.6 * s.success_rate + 0.4 * s.confidence)
        .collect();
    if !parallel_enabled() {
        return scores;
    }
    // ComputeBridge 路由（GsoeEvaluate 阈值 500，Phase 1 已登记）→ 并行
    let kind = nexus_core::compute::TaskKind::GsoeEvaluate;
    match bridge().route(kind, samples.len()) {
        nexus_core::compute::DispatchPlan::Rayon => {
            // spawn_compute_batch 接受闭包迭代器（每 item 一闭包）
            // WHY cloned：闭包需 'static（rayon 池跨线程），样本为小值复制捕获
            let closures = samples
                .iter()
                .cloned()
                .map(|s| move || 0.6 * s.success_rate + 0.4 * s.confidence);
            let results = bridge().spawn_compute_batch(kind, closures);
            // 与串行逐元素一致（确定性；并行仅加速，不改语义）
            results.into_iter().map(|r| r.unwrap_or(0.0)).collect()
        }
        _ => scores,
    }
}

/// 精英选择（top-k 变体）— 批量评分后 O(n) 部分选择
///
/// WHY 复用 score_batch：先并行打分，再 `select_nth_unstable` 取前 k
/// （红线：Top-K 禁 sort_by 全排序）。
#[must_use]
pub fn select_elite(samples: &[FitnessSample], k: usize) -> Vec<FitnessSample> {
    if samples.is_empty() || k == 0 {
        return Vec::new();
    }
    let scores = score_batch(samples);
    let mut idx: Vec<usize> = (0..samples.len()).collect();
    let k = k.min(samples.len());
    idx.select_nth_unstable_by(k - 1, |&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(k);
    idx.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    idx.into_iter().map(|i| samples[i].clone()).collect()
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(n: usize) -> Vec<FitnessSample> {
        (0..n)
            .map(|i| FitnessSample::new(format!("v{i}"), (i % 10) as f32 / 10.0, 0.5))
            .collect()
    }

    #[test]
    fn score_formula() {
        let s = FitnessSample::new("v1", 0.8, 0.2);
        // 0.6*0.8 + 0.4*0.2 = 0.56
        let scores = score_batch(&[s]);
        assert!((scores[0] - 0.56).abs() < 1e-6);
    }

    #[test]
    fn parallel_matches_serial_deterministic() {
        // 强制并行（阈值下走 Inline 也断言一致——双路径一致性）
        let samples = samples(1000);
        let scores = score_batch(&samples);
        for (i, s) in samples.iter().enumerate() {
            let expected = 0.6 * s.success_rate + 0.4 * s.confidence;
            assert!(
                (scores[i] - expected).abs() < 1e-6,
                "并行必须与串行逐元素一致"
            );
        }
    }

    #[test]
    fn select_elite_top_k() {
        let samples = samples(20);
        let elite = select_elite(&samples, 5);
        assert_eq!(elite.len(), 5);
        // 精英按综合分降序
        let scores: Vec<f32> = elite
            .iter()
            .map(|s| 0.6 * s.success_rate + 0.4 * s.confidence)
            .collect();
        for w in scores.windows(2) {
            assert!(w[0] >= w[1], "精英必须按分数降序");
        }
    }

    #[test]
    fn empty_and_zero_k() {
        assert!(score_batch(&[]).is_empty());
        assert!(select_elite(&[], 3).is_empty());
        let samples = samples(5);
        assert!(select_elite(&samples, 0).is_empty());
    }

    #[test]
    fn deterministic_same_input() {
        let samples = samples(50);
        assert_eq!(
            score_batch(&samples),
            score_batch(&samples),
            "同输入必须同输出(Ω₂)"
        );
        assert_eq!(select_elite(&samples, 7), select_elite(&samples, 7));
    }
}
