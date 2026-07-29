//! AEGIS Stage 1: TrajectoryDigester — 轨迹消化与失败模式统计聚类
//!
//! 对应 ADR:ADR-050 决策 2(Digester 降级为纯规则统计聚类)
//!
//! # R2 冻结声明(ADR-042)
//! 本阶段为纯统计实现:按 `error_kind + error_location` 分桶计数,
//! 无 LLM 摘要、无梯度更新(FormalVerifier 落地前无条件冻结)。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::TrajectoryOutcome;

/// 失败模式 — 同类失败的聚类桶
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailurePattern {
    /// 错误类别(聚类键第一维,如 "timeout")
    pub error_kind: String,
    /// 错误位置(聚类键第二维,如 "pvl-layer::verifier")
    pub error_location: String,
    /// 该模式在本批次的出现频次
    pub frequency: u32,
}

/// 消化后的轨迹摘要 — Stage 2 Planner 的输入
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DigestedTrajectories {
    /// 本批次轨迹总数
    pub total_count: usize,
    /// 成功轨迹数
    pub success_count: usize,
    /// 成功率 [0.0, 1.0]
    pub success_rate: f32,
    /// 失败模式聚类(按频次降序,最高频模式在前)
    pub failure_patterns: Vec<FailurePattern>,
    /// 成功轨迹平均时长(毫秒;无成功轨迹时为 0)
    pub avg_success_duration_ms: u64,
}

impl DigestedTrajectories {
    /// 返回最高频失败模式(无失败时 None)
    pub fn dominant_failure(&self) -> Option<&FailurePattern> {
        self.failure_patterns.first()
    }
}

/// 轨迹消化器 — Stage 1
#[derive(Debug, Default, Clone, Copy)]
pub struct TrajectoryDigester;

impl TrajectoryDigester {
    /// 创建消化器
    pub fn new() -> Self {
        Self
    }

    /// 消化一批轨迹:统计成功率 + 聚类失败模式
    ///
    /// # 聚类规则
    /// 失败轨迹按 `(error_kind, error_location)` 二元组分桶计数;
    /// 缺失字段(理论上失败轨迹必带,防御边界输入)归入 "unknown" 桶。
    /// 结果按频次降序排序,频次相同按 error_kind 字典序(保证确定性输出)。
    pub fn digest(&self, trajectories: &[TrajectoryOutcome]) -> DigestedTrajectories {
        let total_count = trajectories.len();
        let mut success_count = 0usize;
        let mut success_duration_sum: u64 = 0;
        let mut buckets: HashMap<(String, String), u32> = HashMap::new();

        for traj in trajectories {
            if traj.success {
                success_count += 1;
                success_duration_sum += traj.duration_ms;
            } else {
                let kind = traj.error_kind.clone().unwrap_or_else(|| "unknown".into());
                let location = traj
                    .error_location
                    .clone()
                    .unwrap_or_else(|| "unknown".into());
                *buckets.entry((kind, location)).or_insert(0) += 1;
            }
        }

        let mut failure_patterns: Vec<FailurePattern> = buckets
            .into_iter()
            .map(|((error_kind, error_location), frequency)| FailurePattern {
                error_kind,
                error_location,
                frequency,
            })
            .collect();
        // 频次降序 + error_kind 字典序次键:HashMap 迭代序不确定,双键排序保证输出确定性
        failure_patterns.sort_by(|a, b| {
            b.frequency
                .cmp(&a.frequency)
                .then_with(|| a.error_kind.cmp(&b.error_kind))
        });

        let success_rate = if total_count == 0 {
            0.0
        } else {
            success_count as f32 / total_count as f32
        };
        let avg_success_duration_ms = if success_count == 0 {
            0
        } else {
            success_duration_sum / success_count as u64
        };

        DigestedTrajectories {
            total_count,
            success_count,
            success_rate,
            failure_patterns,
            avg_success_duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_digest_clusters_failures_by_kind_and_location() {
        let digester = TrajectoryDigester::new();
        let trajectories = vec![
            TrajectoryOutcome::failed("t1", "timeout", "pvl-layer", 5000),
            TrajectoryOutcome::failed("t2", "timeout", "pvl-layer", 4000),
            TrajectoryOutcome::failed("t3", "verification_failed", "pvl-layer", 300),
            TrajectoryOutcome::succeeded("t4", 1000),
        ];
        let digested = digester.digest(&trajectories);

        assert_eq!(digested.total_count, 4);
        assert_eq!(digested.success_count, 1);
        assert!((digested.success_rate - 0.25).abs() < f32::EPSILON);
        // 聚类:timeout@pvl-layer 频次 2 应排首位
        let dominant = digested.dominant_failure().expect("应有失败模式");
        assert_eq!(dominant.error_kind, "timeout");
        assert_eq!(dominant.frequency, 2);
        assert_eq!(digested.failure_patterns.len(), 2);
    }

    #[test]
    fn test_digest_empty_batch() {
        let digested = TrajectoryDigester::new().digest(&[]);
        assert_eq!(digested.total_count, 0);
        assert_eq!(digested.success_rate, 0.0);
        assert!(digested.dominant_failure().is_none());
    }

    #[test]
    fn test_digest_deterministic_ordering_on_tie() {
        let digester = TrajectoryDigester::new();
        // 两个模式频次均为 1,应按 error_kind 字典序稳定输出
        let trajectories = vec![
            TrajectoryOutcome::failed("t1", "zeta_error", "loc", 100),
            TrajectoryOutcome::failed("t2", "alpha_error", "loc", 100),
        ];
        let digested = digester.digest(&trajectories);
        assert_eq!(digested.failure_patterns[0].error_kind, "alpha_error");
        assert_eq!(digested.failure_patterns[1].error_kind, "zeta_error");
    }

    #[test]
    fn test_digest_avg_success_duration() {
        let digester = TrajectoryDigester::new();
        let trajectories = vec![
            TrajectoryOutcome::succeeded("t1", 1000),
            TrajectoryOutcome::succeeded("t2", 3000),
        ];
        let digested = digester.digest(&trajectories);
        assert_eq!(digested.avg_success_duration_ms, 2000);
    }
}
