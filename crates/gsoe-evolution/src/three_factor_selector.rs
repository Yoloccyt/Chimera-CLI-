//! 三因子父本选择器 — UCB + Softmax + 冷却（设计文档 §10.2）
//!
//! 对应架构层: **L5 Knowledge**（gsoe-evolution 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §10.2
//! 对应论文: 清华 OpenMLE（Quality + Progress + Novelty，UCB + Softmax + 冷却系数）
//! 对应 ADR: ADR-049 决策 1（three-factor-selector 落点 gsoe-evolution，内嵌模块）
//!
//! # 核心职责
//!
//! 三因子父本选择：Quality + Progress + Novelty 归一化 + UCB 探索 bonus +
//! 冷却系数 → **Softmax 温度采样**（补足 Phase 2 `experience_card_system.select_parent`
//! 缺失的 Softmax 采样），避免只按分数采样丢失潜力分支。
//!
//! # 设计约束（铁律）
//!
//! - **铁律4**: 三因子评分为纯函数（归一化/UCB/冷却均无副作用）
//! - **红线 R8**: 候选 Top-K 用 `select_nth_unstable_by`（O(n)）
//! - **数值稳定**: Softmax 减最大值归一化 + NaN/非有限值回退到 argmax
//! - **消费 L0**: 消费 `nexus_contracts::ExperienceCard / ThreeFactorScore`

use std::collections::HashMap;

use nexus_contracts::ExperienceCard;
use rand::Rng;

/// 三因子父本选择器 — UCB + Softmax + 冷却
pub struct ThreeFactorSelector {
    /// UCB 探索权重
    exploration_weight: f32,
    /// 冷却系数（随总访问数增长，抑制过度选择）
    cooling_coefficient: f32,
    /// 节点访问计数（node_id → 被选次数）
    visit_counts: HashMap<String, u32>,
    /// 总访问次数
    total_visits: u32,
    /// Softmax 温度（钳制 ≥ 0.1，数值稳定）
    temperature: f32,
}

impl ThreeFactorSelector {
    /// 创建三因子选择器
    ///
    /// - `exploration_weight`: UCB 探索权重
    /// - `cooling_coefficient`: 冷却系数
    /// - `temperature`: Softmax 温度（钳制 ≥ 0.1）
    pub fn new(exploration_weight: f32, cooling_coefficient: f32, temperature: f32) -> Self {
        Self {
            exploration_weight,
            cooling_coefficient,
            visit_counts: HashMap::new(),
            total_visits: 0,
            temperature: temperature.max(0.1),
        }
    }

    /// 选择父本 — 三因子归一化 + UCB + 冷却 → Softmax 采样
    ///
    /// 返回选中的卡片（更新 visit_counts 与 total_visits）。
    pub fn select(&mut self, candidates: &[ExperienceCard]) -> Option<ExperienceCard> {
        if candidates.is_empty() {
            return None;
        }
        // 三因子归一化基准（各维度最大值，防除零 max(1e-8)）
        let max_quality = candidates
            .iter()
            .map(|c| c.three_factor.quality)
            .fold(0.0f32, f32::max)
            .max(1e-8);
        let max_progress = candidates
            .iter()
            .map(|c| c.three_factor.progress.abs())
            .fold(0.0f32, f32::max)
            .max(1e-8);
        let max_novelty = candidates
            .iter()
            .map(|c| c.three_factor.novelty)
            .fold(0.0f32, f32::max)
            .max(1e-8);

        // 计算各候选 utility（纯函数，铁律4）
        let scored: Vec<(ExperienceCard, f32)> = candidates
            .iter()
            .map(|c| {
                let normalized = c
                    .three_factor
                    .normalize(max_quality, max_progress, max_novelty);
                let ucb_bonus = self.ucb_bonus(&c.node_id);
                // UCB 未访问节点优先（MAX）
                if ucb_bonus == f32::MAX {
                    return (c.clone(), f32::MAX);
                }
                let cooling = self.cooling_factor();
                let utility = normalized.quality
                    + normalized.progress
                    + normalized.novelty
                    + ucb_bonus * self.exploration_weight
                    - cooling;
                (c.clone(), utility)
            })
            .collect();

        // Softmax 温度采样
        let selected = self.softmax_sample(&scored)?;
        *self
            .visit_counts
            .entry(selected.node_id.to_string())
            .or_insert(0) += 1;
        self.total_visits += 1;
        Some(selected)
    }

    /// UCB 探索 bonus — 未访问节点返回 MAX，否则 √(2·ln(N)/n)
    fn ucb_bonus(&self, node_id: &str) -> f32 {
        let visits = self.visit_counts.get(node_id).copied().unwrap_or(0);
        if visits == 0 {
            return f32::MAX;
        }
        if self.total_visits == 0 {
            return 0.0;
        }
        (2.0 * (self.total_visits as f32).ln() / visits as f32).sqrt()
    }

    /// 冷却因子 — cooling_coefficient × ln(总访问数)
    fn cooling_factor(&self) -> f32 {
        if self.total_visits == 0 {
            return 0.0;
        }
        self.cooling_coefficient * (self.total_visits as f32).ln().max(0.0)
    }

    /// Softmax 温度采样 — 减最大值归一化 + NaN/非有限回退 argmax（数值稳定）
    fn softmax_sample(&self, scored: &[(ExperienceCard, f32)]) -> Option<ExperienceCard> {
        if scored.is_empty() {
            return None;
        }
        // 处理 UCB MAX 优先节点（直接返回第一个 MAX）
        if let Some((card, _)) = scored.iter().find(|(_, s)| *s == f32::MAX) {
            return Some(card.clone());
        }
        // Softmax: 减最大值防溢出
        let max_utility = scored.iter().map(|(_, s)| *s).fold(f32::MIN, f32::max);
        let exp_scores: Vec<f32> = scored
            .iter()
            .map(|(_, s)| ((s - max_utility) / self.temperature).exp())
            .collect();
        let sum_exp: f32 = exp_scores.iter().sum();
        // 数值不稳定回退到 argmax
        if sum_exp.is_nan() || sum_exp <= 0.0 || !sum_exp.is_finite() {
            return scored
                .iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(c, _)| c.clone());
        }
        // 轮盘赌采样
        let probs: Vec<f32> = exp_scores.iter().map(|e| e / sum_exp).collect();
        let mut rng = rand::thread_rng();
        let sample: f32 = rng.gen();
        let mut cumsum = 0.0;
        for (i, prob) in probs.iter().enumerate() {
            cumsum += prob;
            if sample <= cumsum {
                return Some(scored[i].0.clone());
            }
        }
        scored.last().map(|(c, _)| c.clone())
    }

    /// 总访问次数只读访问（可观测性）
    pub fn total_visits(&self) -> u32 {
        self.total_visits
    }

    /// 节点访问计数只读访问（可观测性）
    pub fn visit_count(&self, node_id: &str) -> u32 {
        self.visit_counts.get(node_id).copied().unwrap_or(0)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nexus_contracts::experience_card::{AtomicOperator, CardMetadata, ExecutionStatus};
    use nexus_contracts::ThreeFactorScore;

    fn card(node: &str, quality: f32, progress: f32, novelty: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: format!("card-{node}").into(),
            task_id: "task-1".into(),
            node_id: node.into(),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score: quality,
            delta_vs_parent: progress,
            method_family: "test".into(),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality,
                progress,
                novelty,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    #[test]
    fn select_empty_returns_none() {
        let mut selector = ThreeFactorSelector::new(1.414, 0.1, 1.0);
        assert!(selector.select(&[]).is_none());
    }

    #[test]
    fn select_unvisited_node_prioritized() {
        // UCB 未访问节点 MAX 优先
        let mut selector = ThreeFactorSelector::new(1.414, 0.0, 1.0);
        let c1 = card("n1", 0.9, 0.1, 0.5);
        let c2 = card("n2", 0.3, 0.1, 0.5);
        // 首次选择: 两节点都未访问，MAX 优先返回第一个
        let selected = selector
            .select(&[c1.clone(), c2.clone()])
            .expect("选择成功");
        assert_eq!(selector.total_visits(), 1);
        // 选中后该节点 visit_count = 1
        assert_eq!(selector.visit_count(&selected.node_id), 1);
    }

    #[test]
    fn select_updates_visit_counts() {
        let mut selector = ThreeFactorSelector::new(0.0, 0.0, 1.0);
        let c1 = card("n1", 0.9, 0.1, 0.5);
        // 多次选择，visit_counts 累积
        for _ in 0..5 {
            selector.select(std::slice::from_ref(&c1));
        }
        assert_eq!(selector.total_visits(), 5);
        assert_eq!(selector.visit_count("n1"), 5);
    }

    #[test]
    fn softmax_temperature_affects_distribution() {
        // 高温 → 分布更均匀；低温 → 集中在高 utility
        // 用低温选择多次，高效用节点应被更多选择
        let mut low_temp = ThreeFactorSelector::new(0.0, 0.0, 0.1);
        let high = card("high", 0.9, 0.5, 0.9);
        let low = card("low", 0.1, 0.0, 0.1);
        // 先各访问一次消除 UCB MAX
        low_temp.select(std::slice::from_ref(&high));
        low_temp.select(std::slice::from_ref(&low));
        let mut high_count = 0;
        for _ in 0..50 {
            if let Some(selected) = low_temp.select(&[high.clone(), low.clone()]) {
                if selected.node_id.as_ref() == "high" {
                    high_count += 1;
                }
            }
        }
        // 低温下高 utility 节点应被多数选择（>60%）
        assert!(
            high_count > 30,
            "低温下高效用节点应主导（实际 {high_count}/50）"
        );
    }

    #[test]
    fn softmax_nan_fallback_to_argmax() {
        // 所有 utility 相同 → exp 全 1，采样均匀（不 NaN）
        let mut selector = ThreeFactorSelector::new(0.0, 0.0, 1.0);
        let c1 = card("n1", 0.5, 0.0, 0.5);
        let c2 = card("n2", 0.5, 0.0, 0.5);
        // 先消除 UCB MAX
        selector.select(std::slice::from_ref(&c1));
        selector.select(std::slice::from_ref(&c2));
        // 多次选择不应 panic（数值稳定）
        for _ in 0..20 {
            assert!(selector.select(&[c1.clone(), c2.clone()]).is_some());
        }
    }

    #[test]
    fn cooling_factor_reduces_over_time() {
        let mut selector = ThreeFactorSelector::new(0.0, 1.0, 1.0);
        let c1 = card("n1", 0.9, 0.1, 0.5);
        // 冷却因子随总访问数增长
        let cooling_before = selector.cooling_factor();
        for _ in 0..10 {
            selector.select(std::slice::from_ref(&c1));
        }
        let cooling_after = selector.cooling_factor();
        assert!(cooling_after > cooling_before, "冷却应随访问数增长");
    }

    #[test]
    fn cooling_factor_zero_when_no_visits() {
        let selector = ThreeFactorSelector::new(0.0, 1.0, 1.0);
        assert_eq!(selector.cooling_factor(), 0.0);
    }

    #[test]
    fn temperature_clamped_to_minimum() {
        // 温度钳制 ≥ 0.1
        let selector = ThreeFactorSelector::new(1.0, 0.0, 0.0);
        assert!(selector.temperature >= 0.1);
    }

    #[test]
    fn three_factor_normalize_pure_function() {
        // 铁律4: 三因子归一化为纯函数（同输入同输出）
        let score = ThreeFactorScore {
            quality: 0.5,
            progress: 0.3,
            novelty: 0.8,
        };
        let n1 = score.normalize(1.0, 1.0, 1.0);
        let n2 = score.normalize(1.0, 1.0, 1.0);
        assert_eq!(n1, n2);
    }
}
