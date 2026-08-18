//! 动态验证深度 + 熵加权 — OpenMLE + 快手融合（设计文档 §12.3）
//!
//! 对应架构层: **L7 Execution**（pvl-layer 子模块，ADR-049 决策 1 内嵌）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §12.3
//! 对应论文: OpenMLE（动态验证深度）+ 快手 KAT（熵加权融合）
//!
//! # 核心职责
//!
//! - [`DynamicVerifier`]: 按任务风险与算子类型门控验证深度（五档），
//!   EMA 追踪各深度历史有效性，支持铁律6 轨迹导出预留（D-7）
//! - [`EntropyWeightedScorer`]: 熵加权评分——低熵（确定性高）候选获得
//!   加成，高熵（探索性）候选经 (1 + H×0.5) 系数放大潜力分
//!
//! # 设计约束（铁律）
//!
//! - **铁律4**: select_depth 与 score 均为纯函数（EMA 更新除外，显式 &mut）
//! - **铁律6**: `export_depth_history` 导出 [`RLTrajectory`]（v4.0 RL 数据流预留）
//! - 全程 f32 不隐式升 f64

use std::collections::HashMap;

use nexus_contracts::experience_card::AtomicOperator;
use nexus_contracts::rl_hooks::{RLActionVector, RLStateVector, RLTrajectory};
use nexus_contracts::ExperienceCard;

/// 验证深度 — 五档（规范 §12.3）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerificationDepth {
    /// 全量验证（最高成本，最高保障）
    FullVerify,
    /// 标准验证（默认档）
    StandardVerify,
    /// 增量验证（仅变更路径）
    IncrementalVerify,
    /// 仅语法检查
    SyntaxOnly,
    /// 跳过验证（低风险 + 历史高有效性）
    SkipVerify,
}

impl VerificationDepth {
    /// 动作编码（RLTrajectory 导出用，铁律6）
    pub fn as_code(self) -> u32 {
        match self {
            Self::FullVerify => 0,
            Self::StandardVerify => 1,
            Self::IncrementalVerify => 2,
            Self::SyntaxOnly => 3,
            Self::SkipVerify => 4,
        }
    }
}

/// 任务风险画像（规范 §12.3 TaskRisk）
#[derive(Clone, Debug)]
pub struct TaskRisk {
    /// 风险等级（0-100，>80 强制全量验证）
    pub level: u8,
    /// 风险因子列表（可观测性）
    pub factors: Vec<String>,
}

/// 深度有效性历史记录（铁律6 轨迹导出输入）
#[derive(Clone, Debug)]
struct DepthRecord {
    depth: VerificationDepth,
    success: bool,
    timestamp_ms: u64,
}

/// 动态验证器 — 风险门控 + EMA 有效性追踪（规范 §12.3）
#[derive(Clone, Debug)]
pub struct DynamicVerifier {
    /// 各深度历史有效性（EMA 0.9/0.1）
    depth_effectiveness: HashMap<VerificationDepth, f32>,
    /// 默认深度
    default_depth: VerificationDepth,
    /// 有效性更新历史（铁律6 导出输入）
    history: Vec<DepthRecord>,
}

impl Default for DynamicVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicVerifier {
    /// 创建动态验证器（规范初始有效性表）
    pub fn new() -> Self {
        let mut de = HashMap::new();
        de.insert(VerificationDepth::FullVerify, 0.95);
        de.insert(VerificationDepth::StandardVerify, 0.90);
        de.insert(VerificationDepth::IncrementalVerify, 0.85);
        de.insert(VerificationDepth::SyntaxOnly, 0.70);
        de.insert(VerificationDepth::SkipVerify, 0.50);
        Self {
            depth_effectiveness: de,
            default_depth: VerificationDepth::StandardVerify,
            history: Vec::new(),
        }
    }

    /// 选择验证深度 — 风险门控纯函数（铁律4，规范 §12.3 select_depth）
    ///
    /// 门控优先级：
    /// 1. 风险 > 80 → 全量验证
    /// 2. Crossover 且风险 > 50 → 全量验证（融合操作不确定性高）
    /// 3. Debug → 标准验证（修复需验证闭环）
    /// 4. 风险 > 50 → 标准验证
    /// 5. 其余 → 历史最佳深度
    pub fn select_depth(
        &self,
        task_risk: &TaskRisk,
        operator: &AtomicOperator,
    ) -> VerificationDepth {
        if task_risk.level > 80 {
            return VerificationDepth::FullVerify;
        }
        if matches!(operator, AtomicOperator::Crossover) && task_risk.level > 50 {
            return VerificationDepth::FullVerify;
        }
        if matches!(operator, AtomicOperator::Debug) {
            return VerificationDepth::StandardVerify;
        }
        if task_risk.level > 50 {
            return VerificationDepth::StandardVerify;
        }
        // 历史最佳深度（EMA 最高者）
        self.depth_effectiveness
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(d, _)| *d)
            .unwrap_or(self.default_depth)
    }

    /// 更新深度有效性 — EMA 0.9/0.1 + 历史记录（铁律6 导出输入）
    pub fn update_effectiveness(&mut self, depth: VerificationDepth, success: bool) {
        let current = self.depth_effectiveness.entry(depth).or_insert(0.5);
        let reward = if success { 1.0 } else { 0.0 };
        *current = *current * 0.9 + reward * 0.1;
        self.history.push(DepthRecord {
            depth,
            success,
            timestamp_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
        });
    }

    /// 导出深度决策历史为 RLTrajectory（铁律6，D-7 v4.0 RL 预留）
    ///
    /// 状态用全零向量（L7 深度决策无 CLV 语义），动作编码深度档位，
    /// 奖励为验证成功标志，供 L1 rl-client 投影消费。
    pub fn export_depth_history(&self, episode_id: &str) -> RLTrajectory {
        let states: Vec<RLStateVector> = self
            .history
            .iter()
            .map(|_| RLStateVector::zeros())
            .collect();
        let actions: Vec<RLActionVector> = self
            .history
            .iter()
            .map(|r| {
                RLActionVector::new(
                    "L7",
                    r.depth.as_code(),
                    vec![self
                        .depth_effectiveness
                        .get(&r.depth)
                        .copied()
                        .unwrap_or(0.5)],
                )
            })
            .collect();
        let rewards: Vec<f32> = self
            .history
            .iter()
            .map(|r| if r.success { 1.0 } else { 0.0 })
            .collect();
        let timestamps: Vec<u64> = self.history.iter().map(|r| r.timestamp_ms).collect();
        RLTrajectory::new(episode_id, states, actions, rewards, timestamps)
    }

    /// 深度有效性只读访问（可观测性）
    pub fn effectiveness(&self, depth: VerificationDepth) -> Option<f32> {
        self.depth_effectiveness.get(&depth).copied()
    }

    /// 历史记录数只读访问（可观测性）
    pub fn history_len(&self) -> usize {
        self.history.len()
    }
}

/// 熵加权评分器 — 低熵确定性加成（规范 §12.3）
///
/// `final = base × (1 + H × 0.5)`，其中 base 为三因子 selection_utility，
/// H 为候选在 softmax(quality) 分布下的二元熵。消费 L0
/// `ThreeFactorScore::selection_utility`（不依赖 L5 选择器，语义边界清晰）。
pub struct EntropyWeightedScorer;

impl EntropyWeightedScorer {
    /// 熵加权评分 — 纯函数（铁律4）
    ///
    /// - `card`: 被评分卡片
    /// - `candidates`: 候选集（含 card 自身，softmax 归一化基准）
    /// - 空候选集返回 0.0（除零保护）
    pub fn score(card: &ExperienceCard, candidates: &[ExperienceCard]) -> f32 {
        if candidates.is_empty() {
            return 0.0;
        }
        let base_score = card.three_factor.selection_utility();
        // softmax(quality) 归一化概率
        let scores: Vec<f32> = candidates
            .iter()
            .map(|c| c.three_factor.quality.exp())
            .collect();
        let sum_scores: f32 = scores.iter().sum();
        let p = if sum_scores > 0.0 {
            card.three_factor.quality.exp() / sum_scores
        } else {
            1.0 / candidates.len() as f32
        };
        // 二元熵（p∈{0,1} 边界时 H=0，防 ln(0)）
        let entropy = if p > 0.0 && p < 1.0 {
            -p * p.ln() - (1.0 - p) * (1.0 - p).ln()
        } else {
            0.0
        };
        base_score * (1.0 + entropy * 0.5)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nexus_contracts::experience_card::{CardMetadata, ExecutionStatus, ThreeFactorScore};

    fn risk(level: u8) -> TaskRisk {
        TaskRisk {
            level,
            factors: Vec::new(),
        }
    }

    fn card(quality: f32) -> ExperienceCard {
        ExperienceCard {
            card_id: "c1".into(),
            task_id: "t1".into(),
            node_id: "n1".into(),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score: quality,
            delta_vs_parent: 0.0,
            method_family: "test".into(),
            error_signature: None,
            three_factor: ThreeFactorScore {
                quality,
                progress: 0.1,
                novelty: 0.2,
            },
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        }
    }

    #[test]
    fn high_risk_forces_full_verify() {
        let verifier = DynamicVerifier::new();
        assert_eq!(
            verifier.select_depth(&risk(90), &AtomicOperator::Draft),
            VerificationDepth::FullVerify
        );
    }

    #[test]
    fn crossover_medium_risk_forces_full_verify() {
        let verifier = DynamicVerifier::new();
        assert_eq!(
            verifier.select_depth(&risk(60), &AtomicOperator::Crossover),
            VerificationDepth::FullVerify
        );
    }

    #[test]
    fn debug_forces_standard_verify() {
        let verifier = DynamicVerifier::new();
        assert_eq!(
            verifier.select_depth(&risk(10), &AtomicOperator::Debug),
            VerificationDepth::StandardVerify
        );
    }

    #[test]
    fn medium_risk_standard_verify() {
        let verifier = DynamicVerifier::new();
        assert_eq!(
            verifier.select_depth(&risk(60), &AtomicOperator::Draft),
            VerificationDepth::StandardVerify
        );
    }

    #[test]
    fn low_risk_selects_historical_best() {
        let verifier = DynamicVerifier::new();
        // 初始有效性 FullVerify 0.95 最高
        assert_eq!(
            verifier.select_depth(&risk(10), &AtomicOperator::Draft),
            VerificationDepth::FullVerify
        );
    }

    #[test]
    fn ema_converges_to_success() {
        let mut verifier = DynamicVerifier::new();
        // SkipVerify 初始 0.5，连续成功 → 收敛至 1.0
        for _ in 0..50 {
            verifier.update_effectiveness(VerificationDepth::SkipVerify, true);
        }
        let eff = verifier
            .effectiveness(VerificationDepth::SkipVerify)
            .expect("已注册深度");
        assert!(eff > 0.99, "EMA 应收敛至 1.0（实际 {eff}）");
    }

    #[test]
    fn ema_converges_to_failure() {
        let mut verifier = DynamicVerifier::new();
        for _ in 0..50 {
            verifier.update_effectiveness(VerificationDepth::FullVerify, false);
        }
        let eff = verifier
            .effectiveness(VerificationDepth::FullVerify)
            .expect("已注册深度");
        assert!(eff < 0.01, "EMA 应收敛至 0.0（实际 {eff}）");
    }

    #[test]
    fn export_depth_history_rl_trajectory() {
        // 铁律6: 深度决策历史可导出 RLTrajectory
        let mut verifier = DynamicVerifier::new();
        verifier.update_effectiveness(VerificationDepth::FullVerify, true);
        verifier.update_effectiveness(VerificationDepth::SyntaxOnly, false);
        let traj = verifier.export_depth_history("ep-1");
        assert_eq!(traj.episode_id.as_ref(), "ep-1");
        assert_eq!(traj.states.len(), 2);
        assert_eq!(traj.actions.len(), 2);
        assert_eq!(traj.rewards.len(), 2);
        assert_eq!(traj.timestamps.len(), 2);
        assert_eq!(traj.actions[0].action_code, 0, "FullVerify 编码 0");
        assert_eq!(traj.actions[1].action_code, 3, "SyntaxOnly 编码 3");
        assert_eq!(traj.rewards[0], 1.0);
        assert_eq!(traj.rewards[1], 0.0);
    }

    #[test]
    fn entropy_score_single_candidate() {
        // 单候选 p=1 → H=0 → final = base
        let c = card(0.8);
        let score = EntropyWeightedScorer::score(&c, std::slice::from_ref(&c));
        let base = c.three_factor.selection_utility();
        assert!((score - base).abs() < 1e-6, "单候选无熵加成");
    }

    #[test]
    fn entropy_score_empty_candidates() {
        let c = card(0.8);
        assert_eq!(EntropyWeightedScorer::score(&c, &[]), 0.0, "空候选除零保护");
    }

    #[test]
    fn entropy_score_amplifies_uncertain_candidates() {
        // 双候选均匀分布 p=0.5 → H=ln(2)≈0.693 → 加成 ×(1+0.347)
        let c1 = card(0.5);
        let c2 = card(0.5);
        let score = EntropyWeightedScorer::score(&c1, &[c1.clone(), c2]);
        let base = c1.three_factor.selection_utility();
        let expected = base * (1.0 + (2.0f32.ln()) * 0.5);
        assert!(
            (score - expected).abs() < 1e-5,
            "熵加成应放大（实际 {score}，期望 {expected}）"
        );
    }

    #[test]
    fn depth_codes_distinct() {
        let codes: Vec<u32> = [
            VerificationDepth::FullVerify,
            VerificationDepth::StandardVerify,
            VerificationDepth::IncrementalVerify,
            VerificationDepth::SyntaxOnly,
            VerificationDepth::SkipVerify,
        ]
        .iter()
        .map(|d| d.as_code())
        .collect();
        // 五档编码互异（RLTrajectory 动作空间完备）
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(codes[i], codes[j]);
            }
        }
    }
}
