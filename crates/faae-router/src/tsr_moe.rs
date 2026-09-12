//! TSR×MoE 稀疏路由偏置均衡（P2-T12，v4.0 WI-09）
//!
//! 对应架构层: **L6 Router**（faae-router，ADR-137 裁决：挂既有 crate 增强）
//! 对应任务: **P2-T12**（手册 W13-14）
//!
//! # 设计（v4.0 WI-09 规格）
//! MoE 增强：路由分数 = 语义相似度 + 历史偏好 bonus + 负载均衡偏置
//! （aux-loss-free 直接调分——CLI 场景无梯度，用欠载 +δ / 超载 −δ 偏置
//! 替代传统辅助损失）。
//!
//! - top-k 默认 6~8（`select_nth_unstable` O(n) 部分选择——红线）
//! - 路由历史矩阵：任务类型 × 专家成功率（RoutingHistory）
//! - 偏置更新：欠载专家 +δ、超载专家 −δ（delta=0.05，clamp [-0.2, 0.2]）
//!
//! # 门禁（WI-09）
//! 50 工具 Top-8 准确率 > 85%；路由分布均匀度（熵）随迭代收敛。
//! Shadow 一周成功率不降才转正（v4.0 §17——本模块只提供路由决策，
//! 转正流程由议会审批 ADR-142 承接）。

use std::collections::HashMap;

// P3-T6(WI-09 收口):任务类型×成功率矩阵（历史偏好 bonus 数据源）
use crate::routing_history::RoutingHistory;

/// top-k 范围（WI-09：默认 6-8）
pub const TOP_K_MIN: usize = 6;
/// top-k 上限（WI-09 默认区间上界:超载截断线,并发评分并行友好）
pub const TOP_K_MAX: usize = 8;
/// 偏置增量（aux-loss-free：欠载 +δ / 超载 −δ）
pub const BIAS_DELTA: f64 = 0.05;
/// 偏置 clamp 界
pub const BIAS_CLAMP: f64 = 0.2;

/// 专家候选（打分输入）
#[derive(Debug, Clone)]
pub struct Candidate {
    /// 专家 ID
    pub id: String,
    /// 语义相似度分数
    pub semantic_score: f64,
    /// 历史成功率（RoutingHistory）
    pub success_rate: f64,
}

/// TSR×MoE 路由器 — 分数 = 语义 + 历史 bonus + 均衡偏置
#[derive(Debug, Clone, Default)]
pub struct TsrMoeRouter {
    /// 均衡偏置表（专家 ID → 偏置）
    bias: HashMap<String, f64>,
    /// 调用计数（诊断）
    calls: u64,
}

impl TsrMoeRouter {
    /// 新建路由器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 历史感知综合分 — 语义 0.5 + 历史偏好 bonus 0.3 + 偏置 0.2（P3-T6,WI-09 收口）
    ///
    /// 与 [`score`](Self::score) 的区别:历史项从显式传入的成功率矩阵读取
    /// （任务类型感知）,而非候选自带 `success_rate` 字段——调用方以
    /// `history.success_rate(task_type, &c.id)` 组装候选即可获得同语义;本方法
    /// 为便捷入口,矩阵无记录时自动回退候选字段值（向后兼容）。
    #[must_use]
    pub fn score_with_history(
        &self,
        c: &Candidate,
        task_type: &str,
        history: &RoutingHistory,
    ) -> f64 {
        let bias = self.bias.get(&c.id).copied().unwrap_or(0.0);
        // 矩阵有记录则用矩阵值,无记录回退候选字段（新专家中性,不惩罚）
        let rate = if history.is_recorded(task_type, &c.id) {
            history.success_rate(task_type, &c.id)
        } else {
            c.success_rate
        };
        0.5 * c.semantic_score + 0.3 * rate + 0.2 * bias
    }

    /// 路由结果 + 历史矩阵反馈 — 成功/失败信号写入矩阵（P3-T6,WI-09 收口）
    ///
    /// 调用方在每轮路由后以实际结果（工具成功/失败）调用;矩阵按任务类型
    /// 隔离,周期衰减由调用方按 [`RoutingHistory::decay`] 触发（回退路径）。
    pub fn observe_outcome(
        &mut self,
        task_type: &str,
        selected: &[String],
        history: &mut RoutingHistory,
        ok: bool,
    ) {
        self.calls += 1;
        for id in selected {
            history.record(task_type, id, ok);
        }
    }

    /// 综合路由分数（语义 0.5 + 历史 0.3 + 偏置 0.2 加权）
    #[must_use]
    pub fn score(&self, c: &Candidate) -> f64 {
        let bias = self.bias.get(&c.id).copied().unwrap_or(0.0);
        0.5 * c.semantic_score + 0.3 * c.success_rate + 0.2 * bias
    }

    /// top-k 路由选择（O(n) select_nth_unstable，红线）
    ///
    /// # 参数
    /// - `candidates`：候选列表（任意序）
    /// - `k`：请求数量（clamp 到 1-8；WI-09 默认 6-8 由调用方传参表达）
    ///
    /// # 返回
    /// 按综合分数降序的前 k 个候选 ID（保序：同分按输入序稳定）。
    #[must_use]
    pub fn route_top_k(&self, candidates: &[Candidate], k: usize) -> Vec<String> {
        if candidates.is_empty() {
            return Vec::new();
        }
        let k = k.clamp(1, TOP_K_MAX).min(candidates.len());
        // 综合打分（带原始索引，稳定排序）
        let mut scored: Vec<(f64, usize)> = candidates
            .iter()
            .enumerate()
            .map(|(idx, c)| (self.score(c), idx))
            .collect();
        // O(n) 部分选择：前 k 名（select_nth_unstable 红线）
        scored.select_nth_unstable_by(k - 1, |a, b| {
            b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        // 前 k 内稳定排序（分数降序，同分按输入序）
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored
            .into_iter()
            .map(|(_, idx)| candidates[idx].id.clone())
            .collect()
    }

    /// 反馈路由结果（aux-loss-free 偏置更新：选中欠载 +δ / 未选超载 −δ）
    ///
    /// 简化：选中且成功率高 → 微降偏置（已足够被选，让位他人）；
    /// 选中且成功率低 → 保持（真实负载信号）；
    /// 未选中且成功率高 → +δ（欠载专家提升）；
    /// 未选中且成功率低 → −δ（超载/低质专家降低）。
    pub fn observe_route(&mut self, candidates: &[Candidate], selected: &[String]) {
        self.calls += 1;
        for c in candidates {
            let selected_now = selected.iter().any(|id| id == &c.id);
            let entry = self.bias.entry(c.id.clone()).or_insert(0.0);
            if selected_now {
                // 选中且高成功率：已足够被选，让位他人（轻微下调）
                if c.success_rate > 0.8 {
                    *entry = (*entry - BIAS_DELTA).clamp(-BIAS_CLAMP, BIAS_CLAMP);
                }
            } else if c.success_rate > 0.8 {
                // 欠载高质专家：提升
                *entry = (*entry + BIAS_DELTA).clamp(-BIAS_CLAMP, BIAS_CLAMP);
            } else {
                *entry = (*entry - BIAS_DELTA).clamp(-BIAS_CLAMP, BIAS_CLAMP);
            }
        }
    }

    /// 调用计数（诊断）
    #[must_use]
    pub fn calls(&self) -> u64 {
        self.calls
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, semantic: f64, success: f64) -> Candidate {
        Candidate {
            id: id.into(),
            semantic_score: semantic,
            success_rate: success,
        }
    }

    #[test]
    fn score_weights() {
        let r = TsrMoeRouter::new();
        let c = cand("e1", 0.8, 0.6);
        // 0.5*0.8 + 0.3*0.6 + 0.2*0 = 0.58
        assert!((r.score(&c) - 0.58).abs() < 1e-9);
    }

    #[test]
    fn top_k_selects_highest_composite() {
        let r = TsrMoeRouter::new();
        let cands = vec![
            cand("a", 0.9, 0.5),
            cand("b", 0.7, 0.9),
            cand("c", 0.8, 0.8),
            cand("d", 0.5, 0.3),
        ];
        let top = r.route_top_k(&cands, 2);
        // a: 0.45+0.15=0.60; c: 0.40+0.24=0.64; b: 0.35+0.27=0.62 → top2 = c, b
        assert_eq!(top, vec!["c".to_string(), "b".to_string()]);
    }

    #[test]
    fn k_clamped_to_bounds() {
        let r = TsrMoeRouter::new();
        let cands: Vec<Candidate> = (0..10).map(|i| cand(&format!("e{i}"), 0.5, 0.5)).collect();
        // k=0 被 clamp 到下限 1
        let top = r.route_top_k(&cands, 0);
        assert_eq!(top.len(), 1, "k 下限 clamp 到 1");
        // k=99 被 clamp 到上限 8
        let top2 = r.route_top_k(&cands, 99);
        assert_eq!(top2.len(), 8, "k 上限 clamp 到 8");
    }

    #[test]
    fn underloaded_high_quality_gets_boost() {
        let mut r = TsrMoeRouter::new();
        // 场景：b 高质（成功率 0.95）但语义分低（0.6）——欠载；第一轮 a 被选
        let cands = vec![cand("a", 0.9, 0.5), cand("b", 0.6, 0.95)];
        let top = r.route_top_k(&cands, 1);
        // a: 0.45+0.15=0.60; b: 0.30+0.285=0.585 → a 被选（b 欠载）
        assert_eq!(top, vec!["a".to_string()]);
        r.observe_route(&cands, &top);
        // b 未选中且高成功率 → +δ
        let b_bias = r.bias.get("b").copied().unwrap_or(0.0);
        assert!((b_bias - BIAS_DELTA).abs() < 1e-9, "欠载高质专家必须 +δ");
        // 多轮偏置收敛：b 综合分随偏置提升，最终被选
        let mut b_selected_ever = false;
        for _ in 0..10 {
            let selected = r.route_top_k(&cands, 1);
            if selected == vec!["b".to_string()] {
                b_selected_ever = true;
            }
            r.observe_route(&cands, &selected);
        }
        assert!(b_selected_ever, "偏置累积后欠载高质专家必须被选");
    }

    #[test]
    fn deterministic_same_input() {
        let mut a = TsrMoeRouter::new();
        let mut b = TsrMoeRouter::new();
        let cands: Vec<Candidate> = (0..20)
            .map(|i| cand(&format!("e{i}"), (i % 10) as f64 / 10.0, 0.5))
            .collect();
        for _ in 0..3 {
            let ta = a.route_top_k(&cands, 7);
            let tb = b.route_top_k(&cands, 7);
            assert_eq!(ta, tb, "同输入必须同输出(Ω₂)");
            a.observe_route(&cands, &ta);
            b.observe_route(&cands, &tb);
        }
    }

    // ===== P3-T6 新增（WI-09 收口）=====

    /// score_with_history — 矩阵有记录用矩阵值,无记录回退候选字段
    #[test]
    fn score_with_history_fallback() {
        let r = TsrMoeRouter::new();
        let mut h = RoutingHistory::new();
        let c = cand("e1", 0.8, 0.6);
        // 无记录:回退候选字段 0.6 → 0.5*0.8 + 0.3*0.6 + 0 = 0.58
        assert!((r.score_with_history(&c, "code", &h) - 0.58).abs() < 1e-9);
        // 有记录:矩阵值覆盖候选字段（0.5+0.5 后成功率恰 0.5 需可区分）
        h.record("code", "e1", true);
        h.record("code", "e1", false);
        assert!(h.is_recorded("code", "e1"), "有记录必须可判定");
        // 矩阵成功率 = 0.5 → 0.5*0.8 + 0.3*0.5 + 0 = 0.55（与回退 0.58 可区分）
        assert!((r.score_with_history(&c, "code", &h) - 0.55).abs() < 1e-9);
        // 任务类型隔离:search 无记录 → 回退候选
        assert!((r.score_with_history(&c, "search", &h) - 0.58).abs() < 1e-9);
    }

    /// observe_outcome — 成功/失败写入矩阵,任务类型隔离,周期衰减可见
    #[test]
    fn observe_outcome_updates_history() {
        let mut r = TsrMoeRouter::new();
        let mut h = RoutingHistory::new();
        let cands = vec![cand("a", 0.9, 0.5), cand("b", 0.7, 0.9)];
        let top = r.route_top_k(&cands, 2);
        r.observe_outcome("code", &top, &mut h, true);
        assert!(h.is_recorded("code", "a"));
        assert!(h.is_recorded("code", "b"));
        assert!((h.success_rate("code", "a") - 1.0).abs() < 1e-9);
        // 失败记录后成功率降
        r.observe_outcome("code", &top, &mut h, false);
        assert!((h.success_rate("code", "a") - 0.5).abs() < 1e-9);
        // 任务类型隔离
        assert!(!h.is_recorded("search", "a"));
    }
}
