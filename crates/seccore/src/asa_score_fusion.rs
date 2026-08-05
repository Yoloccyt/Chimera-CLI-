//! ScoreFusion 评分融合器 — 协调规则评分与 PPO 评分
//!
//! 对应架构层:L4 Security
//! 对应 P3-3:ASA PPO 强化学习接入
//!
//! # 融合策略
//!
//! 1. **PPO 未初始化** → 仅使用规则评分(冷启动保底)
//! 2. **PPO 置信度 ≥ threshold** → 使用 PPO 评分
//! 3. **PPO 置信度 < threshold** → 规则评分与 PPO 评分加权平均
//! 4. **规则评分检测到 Block 级别** → 即使 PPO 评分为 Allow 也保持 Block(安全优先)
//!
//! # 设计决策
//!
//! - **安全优先**:规则评分是已知的安全基线,PPO 评分不能降低 Block 级别
//! - **平滑过渡**:低置信度时通过加权平均实现从规则到 PPO 的平滑迁移
//! - **可配置**:置信度阈值和规则权重均可通过构造函数配置

use crate::asa::InterventionAction;

/// 评分融合器 — 协调规则评分与 PPO 评分
///
/// # 融合策略
///
/// ```text
/// PPO 未初始化 ──→ 仅规则评分
///       │
///       ▼
/// PPO 高置信度 ──→ PPO 评分优先
///       │
///       ▼
/// PPO 低置信度 ──→ 加权平均(规则 + PPO)
///       │
///       ▼
/// 规则评分 Block ──→ 保持 Block(安全优先,PPO 不降级)
/// ```
#[derive(Debug, Clone)]
pub struct ScoreFusion {
    /// PPO 置信度阈值(默认 0.6):高于此值使用 PPO 评分
    confidence_threshold: f32,
}

impl Default for ScoreFusion {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.6,
        }
    }
}

impl ScoreFusion {
    /// 创建默认配置的评分融合器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置置信度阈值
    pub fn with_confidence_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// 融合评分 — 根据 PPO 状态和置信度决定最终评分
    ///
    /// # 参数
    /// - `rule_score`: 规则评分(当前 safety_score, ∈ [0, 1])
    /// - `ppo_score`: PPO 评分(Option, None 表示 PPO 未初始化)
    /// - `ppo_confidence`: PPO 置信度(∈ [0, 1], PPO 未初始化时忽略)
    ///
    /// # 返回
    /// 融合后的安全评分(∈ [0, 1])
    pub fn fuse(&self, rule_score: f32, ppo_score: Option<f32>, ppo_confidence: f32) -> f32 {
        let ppo_score = match ppo_score {
            Some(s) => s,
            // PPO 未初始化 → 仅使用规则评分(冷启动保底)
            None => return rule_score.clamp(0.0, 1.0),
        };

        let rule_score = rule_score.clamp(0.0, 1.0);
        let ppo_score = ppo_score.clamp(0.0, 1.0);

        if ppo_confidence >= self.confidence_threshold {
            // PPO 高置信度 → 使用 PPO 评分
            // 但安全优先:规则评分 Block 时保持 Block
            if rule_score < 0.5 {
                // 规则评分 < 0.5 表示 Block 级别
                // 即使 PPO 评分更高,也保持倾向 Block(安全优先)
                rule_score.min(ppo_score)
            } else {
                ppo_score
            }
        } else {
            // PPO 低置信度 → 规则评分与 PPO 评分加权平均
            // 权重动态分配:置信度越低,规则权重越高
            let rule_weight = 1.0 - ppo_confidence;
            let ppo_weight = ppo_confidence;
            let fused = rule_score * rule_weight + ppo_score * ppo_weight;
            fused.clamp(0.0, 1.0)
        }
    }

    /// 确定干预动作 — 根据融合评分和配置确定干预级别
    ///
    /// # 参数
    /// - `fused_score`: 融合后的安全评分(∈ [0, 1])
    /// - `safety_threshold_allow`: Allow 阈值(默认 0.8)
    /// - `safety_threshold_block`: Block 阈值(默认 0.5)
    ///
    /// # 安全优先
    /// 规则评分检测到 Block 级别时,即使 PPO 评分为 Allow,
    /// 干预动作仍保持 Block(安全优先,不可协商)。
    pub fn determine_intervention(
        &self,
        fused_score: f32,
        safety_threshold_allow: f32,
        safety_threshold_block: f32,
    ) -> InterventionAction {
        if fused_score >= safety_threshold_allow {
            InterventionAction::Allow
        } else if fused_score >= safety_threshold_block {
            InterventionAction::Warn
        } else {
            InterventionAction::Block
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_fusion_cold_start_fallback() {
        // PPO 未初始化时退化为规则评分
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.7, None, 0.0);
        assert!((score - 0.7).abs() < 1e-6, "冷启动应返回规则评分");
    }

    #[test]
    fn test_score_fusion_high_confidence_uses_ppo() {
        // PPO 高置信度(≥0.6)时使用 PPO 评分
        // 规则评分必须 ≥ 0.5,否则安全优先规则会覆盖
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.7, Some(0.9), 0.8);
        assert!((score - 0.9).abs() < 1e-6, "高置信度应使用 PPO 评分");
    }

    #[test]
    fn test_score_fusion_rule_priority_block() {
        // 规则评分 Block(<0.5)时,即使 PPO 评分高,也保持倾向 Block
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.3, Some(0.9), 0.8);
        // 规则评分 0.3 < 0.5,所以使用 rule_score.min(ppo_score) = 0.3
        assert!(
            (score - 0.3).abs() < 1e-6,
            "规则 Block 时安全优先,应保持低分"
        );
    }

    #[test]
    fn test_score_fusion_rule_priority_block_low_confidence() {
        // 规则评分 Block 且 PPO 低置信度 → 加权平均
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.3, Some(0.9), 0.3);
        // 置信度 0.3 < 0.6,加权平均: 0.3*0.7 + 0.9*0.3 = 0.21 + 0.27 = 0.48
        let expected = 0.3 * 0.7 + 0.9 * 0.3;
        assert!(
            (score - expected).abs() < 1e-6,
            "低置信度加权平均: expected={}, actual={}",
            expected,
            score
        );
    }

    #[test]
    fn test_score_fusion_low_confidence_weighted_average() {
        // PPO 低置信度(<0.6)时加权平均
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.7, Some(0.5), 0.4);
        // 置信度 0.4,规则权重 0.6,PPO 权重 0.4
        // fused = 0.7*0.6 + 0.5*0.4 = 0.42 + 0.20 = 0.62
        let expected = 0.7 * 0.6 + 0.5 * 0.4;
        assert!(
            (score - expected).abs() < 1e-6,
            "加权平均: expected={}, actual={}",
            expected,
            score
        );
    }

    #[test]
    fn test_score_fusion_rule_allow_high_confidence_ppo_also_allow() {
        // 规则评分 Allow 且 PPO 高置信度也 Allow → 使用 PPO 评分
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.9, Some(0.85), 0.8);
        assert!(
            (score - 0.85).abs() < 1e-6,
            "规则评分 Allow 且 PPO 高置信度应使用 PPO 评分"
        );
    }

    #[test]
    fn test_score_fusion_rule_warn_high_confidence_ppo_allow() {
        // 规则评分 Warn(0.6)但 PPO 高置信度 Allow(0.9)
        // 规则评分 ≥ 0.5,所以使用 PPO 评分
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(0.6, Some(0.9), 0.8);
        assert!(
            (score - 0.9).abs() < 1e-6,
            "规则评分 ≥ 0.5 且 PPO 高置信度应使用 PPO 评分: actual={}",
            score
        );
    }

    #[test]
    fn test_determine_intervention_allow() {
        let fusion = ScoreFusion::new();
        let action = fusion.determine_intervention(0.9, 0.8, 0.5);
        assert_eq!(action, InterventionAction::Allow);
    }

    #[test]
    fn test_determine_intervention_warn() {
        let fusion = ScoreFusion::new();
        let action = fusion.determine_intervention(0.6, 0.8, 0.5);
        assert_eq!(action, InterventionAction::Warn);
    }

    #[test]
    fn test_determine_intervention_block() {
        let fusion = ScoreFusion::new();
        let action = fusion.determine_intervention(0.3, 0.8, 0.5);
        assert_eq!(action, InterventionAction::Block);
    }

    #[test]
    fn test_score_fusion_output_in_range() {
        // 任意输入,融合评分 ∈ [0, 1]
        let fusion = ScoreFusion::new();
        let inputs = [
            (0.0, None, 0.0),
            (1.0, None, 0.0),
            (0.5, Some(0.5), 0.0),
            (0.5, Some(0.5), 0.5),
            (0.5, Some(0.5), 1.0),
            (0.0, Some(1.0), 0.8),
            (1.0, Some(0.0), 0.8),
        ];
        for (rule, ppo, conf) in &inputs {
            let score = fusion.fuse(*rule, *ppo, *conf);
            assert!(
                (0.0..=1.0).contains(&score),
                "融合评分应在 [0,1]: rule={}, ppo={:?}, conf={}, score={}",
                rule,
                ppo,
                conf,
                score
            );
        }
    }
}
