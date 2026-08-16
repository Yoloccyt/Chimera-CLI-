//! 专家反馈注册表 — 专家级能力画像与 PDCA 调优闭环(专家 Agent 优化 2026-08-11)
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 按专家 ID 记录激活结果反馈(成功/失败/延迟),为 PDCA Act 阶段
//!           提供专家级调优依据(WSJF 权重微调 + 优先级调整建议)。
//!
//! ## 与 gea-activator 反馈的关系
//!
//! - `gea_activator::ExpertProfile` 携带**单个专家实例**的运行时统计,门控
//!   confidence 直接消费;本注册表是 **MAS 层聚合视图**(E01-E08 静态编制 +
//!   自定义专家),供 PDCA 按专家粒度生成调整建议。
//! - 两者互补:gea 侧管"激活倾向",mas 侧管"调度/分配倾向"。
//!
//! ## 设计要点
//!
//! - `DashMap` 并发安全:专家反馈上报与 PDCA 查询可并行
//! - 延迟 EMA 平滑(alpha=0.2):单次抖动不影响建议稳定性
//! - `top_performers(k)` 按成功率降序返回 Top-K,供 PDCA 生成优先分配建议
//! - 与 `ExpertRegistry`(E01-E08 静态编制)解耦:反馈按专家 ID 键控,
//!   静态编制专家与动态专家均可上报

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// 单专家反馈条目 — 聚合统计(线程安全,位于 DashMap 值中)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpertFeedbackEntry {
    /// 专家 ID(对应 ExpertRegistry 的 E01..E08 或自定义专家)
    pub expert_id: String,
    /// 历史成功次数
    pub success_count: u64,
    /// 历史总激活次数(含失败)
    pub total_count: u64,
    /// 平均激活延迟(ms,EMA 平滑,alpha=0.2)
    pub avg_latency_ms: f32,
}

impl ExpertFeedbackEntry {
    /// 创建空反馈条目
    pub fn new(expert_id: impl Into<String>) -> Self {
        Self {
            expert_id: expert_id.into(),
            success_count: 0,
            total_count: 0,
            avg_latency_ms: 0.0,
        }
    }

    /// 记录一次结果反馈
    ///
    /// 延迟 EMA:`new = 0.2×latest + 0.8×old`(与 gea-activator 一致)。
    pub fn record(&mut self, success: bool, latency_ms: f32) {
        self.total_count = self.total_count.saturating_add(1);
        if success {
            self.success_count = self.success_count.saturating_add(1);
        }
        let latency = latency_ms.max(0.0);
        if self.total_count == 1 {
            self.avg_latency_ms = latency;
        } else {
            self.avg_latency_ms = 0.2 * latency + 0.8 * self.avg_latency_ms;
        }
    }

    /// 历史成功率 [0.0, 1.0];无数据时返回 0.5 中性值
    pub fn success_rate(&self) -> f64 {
        if self.total_count == 0 {
            return 0.5;
        }
        self.success_count as f64 / self.total_count as f64
    }
}

/// 专家级优先级调整建议 — PDCA Act 阶段输出
///
/// 由 `ExpertFeedbackRegistry` 驱动:高成功率专家建议提高调度优先级,
/// 低成功率专家建议降低(配合 WSJF 权重实现按专家粒度的资源分配调优)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpertPriorityAdjustment {
    /// 目标专家 ID
    pub expert_id: String,
    /// 建议的优先级增量(正 = 提高,负 = 降低;由调用方 clamp 到合法区间)
    pub priority_delta: f64,
    /// 建议理由(如 "success_rate=0.9 over 20 activations")
    pub reason: String,
}

/// 专家反馈注册表 — 专家级能力画像的聚合视图
///
/// ## 线程安全
/// `DashMap` 分片锁:反馈上报(写)与 PDCA 查询(读)可并发,无全局锁竞争。
#[derive(Debug, Clone, Default)]
pub struct ExpertFeedbackRegistry {
    /// 专家 ID → 反馈条目
    entries: DashMap<String, ExpertFeedbackEntry>,
}

impl ExpertFeedbackRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// 上报一次专家激活结果反馈(专家不存在时自动创建条目)
    pub fn record_outcome(&self, expert_id: &str, success: bool, latency_ms: f32) {
        let mut entry = self
            .entries
            .entry(expert_id.to_string())
            .or_insert_with(|| ExpertFeedbackEntry::new(expert_id));
        entry.record(success, latency_ms);
    }

    /// 查询专家成功率;未上报过返回 None
    pub fn success_rate(&self, expert_id: &str) -> Option<f64> {
        self.entries.get(expert_id).map(|e| e.success_rate())
    }

    /// 查询专家平均延迟(ms);未上报过返回 None
    pub fn avg_latency_ms(&self, expert_id: &str) -> Option<f32> {
        self.entries.get(expert_id).map(|e| e.avg_latency_ms)
    }

    /// 已登记反馈的专家数
    pub fn expert_count(&self) -> usize {
        self.entries.len()
    }

    /// 按成功率降序返回 Top-K 专家反馈(样本数 ≥ min_samples 的才参与排名)
    ///
    /// WHY min_samples 过滤:仅 1-2 次样本的成功率无统计意义,
    /// 需达到最小样本量才进入性能排名(与 gea confidence 的小样本收缩一致)。
    pub fn top_performers(&self, k: usize, min_samples: u64) -> Vec<ExpertFeedbackEntry> {
        let mut ranked: Vec<ExpertFeedbackEntry> = self
            .entries
            .iter()
            .filter(|e| e.total_count >= min_samples)
            .map(|e| e.clone())
            .collect();
        // Top-K 选择用 select_nth_unstable(§4.1 红线:O(n) 替代 sort O(n log n))
        if ranked.len() > k {
            ranked.select_nth_unstable_by(k, |a, b| {
                b.success_rate()
                    .partial_cmp(&a.success_rate())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            ranked.truncate(k);
        }
        ranked.sort_by(|a, b| {
            b.success_rate()
                .partial_cmp(&a.success_rate())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
    }

    /// 生成专家级优先级调整建议(PDCA Act 阶段消费)
    ///
    /// 规则(min_samples = 10,与 confidence 全量信任阈值一致):
    /// - 成功率 ≥ 0.8 → priority_delta = +0.1(高绩效专家优先分配)
    /// - 成功率 ≤ 0.4 → priority_delta = -0.1(低绩效专家降权,交回培训/重配)
    /// - 其余 → 不调整(保持默认)
    pub fn priority_adjustments(&self, min_samples: u64) -> Vec<ExpertPriorityAdjustment> {
        let mut adjustments = Vec::new();
        for entry in self.entries.iter() {
            if entry.total_count < min_samples {
                continue;
            }
            let rate = entry.success_rate();
            if rate >= 0.8 {
                adjustments.push(ExpertPriorityAdjustment {
                    expert_id: entry.expert_id.clone(),
                    priority_delta: 0.1,
                    reason: format!(
                        "high success_rate={rate:.2} over {} activations",
                        entry.total_count
                    ),
                });
            } else if rate <= 0.4 {
                adjustments.push(ExpertPriorityAdjustment {
                    expert_id: entry.expert_id.clone(),
                    priority_delta: -0.1,
                    reason: format!(
                        "low success_rate={rate:.2} over {} activations",
                        entry.total_count
                    ),
                });
            }
        }
        // 确定性排序:按专家 ID 稳定输出(便于测试与审计)
        adjustments.sort_by(|a, b| a.expert_id.cmp(&b.expert_id));
        adjustments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_record_and_stats() {
        let mut entry = ExpertFeedbackEntry::new("E01");
        assert_eq!(entry.success_rate(), 0.5); // 无数据中性
        entry.record(true, 10.0);
        entry.record(false, 20.0);
        assert_eq!(entry.total_count, 2);
        assert_eq!(entry.success_count, 1);
        assert!((entry.success_rate() - 0.5).abs() < 1e-9);
        // EMA:0.2×20 + 0.8×10 = 12.0
        assert!((entry.avg_latency_ms - 12.0).abs() < 1e-5);
    }

    #[test]
    fn test_registry_record_and_query() {
        let reg = ExpertFeedbackRegistry::new();
        assert_eq!(reg.success_rate("E01"), None);
        reg.record_outcome("E01", true, 5.0);
        reg.record_outcome("E01", true, 7.0);
        assert_eq!(reg.expert_count(), 1);
        let rate = reg.success_rate("E01").expect("应存在");
        assert!((rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_top_performers_ranks_by_success_rate() {
        let reg = ExpertFeedbackRegistry::new();
        // E01:10 次全成功;E02:10 次全失败;E03:10 次 50%;E04:仅 2 次(样本不足)
        for _ in 0..10 {
            reg.record_outcome("E01", true, 1.0);
        }
        for _ in 0..10 {
            reg.record_outcome("E02", false, 1.0);
        }
        for i in 0..10 {
            reg.record_outcome("E03", i % 2 == 0, 1.0);
        }
        reg.record_outcome("E04", true, 1.0);
        reg.record_outcome("E04", true, 1.0);

        let top = reg.top_performers(3, 10);
        assert_eq!(top.len(), 3);
        // 按成功率降序:E01(1.0) > E03(0.5) > E02(0.0);E04 样本不足被过滤
        assert_eq!(top[0].expert_id, "E01");
        assert_eq!(top[1].expert_id, "E03");
        assert_eq!(top[2].expert_id, "E02");
    }

    #[test]
    fn test_priority_adjustments_rules() {
        let reg = ExpertFeedbackRegistry::new();
        for _ in 0..10 {
            reg.record_outcome("E01", true, 1.0); // 1.0 → +0.1
        }
        for _ in 0..10 {
            reg.record_outcome("E02", false, 1.0); // 0.0 → -0.1
        }
        for i in 0..10 {
            reg.record_outcome("E03", i % 2 == 0, 1.0); // 0.5 → 不调整
        }
        for _ in 0..5 {
            reg.record_outcome("E04", true, 1.0); // 样本不足 → 不调整
        }

        let adjustments = reg.priority_adjustments(10);
        assert_eq!(adjustments.len(), 2);
        // 按 ID 排序:E01 在前,E02 在后
        assert_eq!(adjustments[0].expert_id, "E01");
        assert!((adjustments[0].priority_delta - 0.1).abs() < 1e-9);
        assert_eq!(adjustments[1].expert_id, "E02");
        assert!((adjustments[1].priority_delta + 0.1).abs() < 1e-9);
    }
}
