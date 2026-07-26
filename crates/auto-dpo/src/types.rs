//! AutoDPO 核心类型 — 偏好对、模型输出与样本质量
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块)
//!
//! # 设计决策(WHY)
//! - `SampleQuality` 为 enum:三档(High/Medium/Low)语义清晰,基于质量分数
//!   分级,匹配 §6 架构红线的"禁止功能标志"——质量分级是连续分数的离散投影
//! - `PreferencePair` 为值对象:携带 chosen/rejected 与质量分级,便于下游
//!   (GSOE 进化)按质量加权使用
//! - `ModelOutput` 为输入值对象:封装模型输出文本与质量分数

use serde::{Deserialize, Serialize};

// ============================================================
// 样本质量 — 三档枚举
// ============================================================

/// 样本质量 — 偏好对的质量分级
///
/// - `High`:高质量(分数 >= 0.8),优先用于训练
/// - `Medium`:中等质量(0.5 <= 分数 < 0.8),可用但权重较低
/// - `Low`:低质量(分数 < 0.5),默认过滤
///
/// WHY Copy + PartialEq:质量分级频繁参与比较与传递,Copy 避免克隆开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleQuality {
    /// 高质量(分数 >= 0.8)
    High,
    /// 中等质量(0.5 <= 分数 < 0.8)
    Medium,
    /// 低质量(分数 < 0.5)
    Low,
}

impl SampleQuality {
    /// 根据质量分数 [0.0, 1.0] 分级
    ///
    /// WHY:阈值 0.8/0.5 与 AutoDpoConfig 的默认质量阈值对齐,
    /// 0.8 以上为 High(优先训练),0.5 以上为 Medium(可用),以下为 Low(过滤)
    pub fn from_score(score: f32) -> Self {
        if score >= 0.8 {
            SampleQuality::High
        } else if score >= 0.5 {
            SampleQuality::Medium
        } else {
            SampleQuality::Low
        }
    }

    /// 返回质量分级的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            SampleQuality::High => "high",
            SampleQuality::Medium => "medium",
            SampleQuality::Low => "low",
        }
    }

    /// 是否通过质量门控(非 Low 即通过)
    pub fn is_acceptable(&self) -> bool {
        !matches!(self, SampleQuality::Low)
    }
}

impl std::fmt::Display for SampleQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 模型输出 — 输入候选
// ============================================================

/// 模型输出 — 偏好对生成的输入候选
///
/// WHY 值对象:封装模型输出文本与质量评分,便于生成器排序与选择。
/// `quality` 由 `score` 派生,构造时自动计算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelOutput {
    /// 输出文本
    pub text: String,
    /// 质量评分 [0.0, 1.0]
    pub score: f32,
    /// 质量分级(由 score 派生)
    pub quality: SampleQuality,
}

impl ModelOutput {
    /// 创建新的模型输出,自动计算质量分级
    ///
    /// WHY score clamp:外部传入的 score 可能因浮点误差越界,clamp 保证合法
    pub fn new(text: impl Into<String>, score: f32) -> Self {
        let clamped = if score.is_nan() {
            0.0
        } else {
            score.clamp(0.0, 1.0)
        };
        Self {
            text: text.into(),
            quality: SampleQuality::from_score(clamped),
            score: clamped,
        }
    }
}

// ============================================================
// 偏好对 — 生成结果
// ============================================================

/// 偏好对 — DPO 训练的 chosen/rejected 样本对
///
/// WHY 独立结构体:携带 pair_id 便于追踪与去重,quality 供下游加权使用。
/// `chosen` 是高分输出(偏好),`rejected` 是低分输出(不偏好)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreferencePair {
    /// 偏好对唯一标识(UUIDv7,由生成器分配)
    pub pair_id: String,
    /// 被选中的输出(高分,偏好)
    pub chosen: String,
    /// 被拒绝的输出(低分,不偏好)
    pub rejected: String,
    /// chosen 的质量评分
    pub chosen_score: f32,
    /// rejected 的质量评分
    pub rejected_score: f32,
    /// 偏好对整体质量分级(取 chosen 的分级)
    pub quality: SampleQuality,
}

impl PreferencePair {
    /// 创建新的偏好对
    pub fn new(
        pair_id: impl Into<String>,
        chosen: impl Into<String>,
        rejected: impl Into<String>,
        chosen_score: f32,
        rejected_score: f32,
    ) -> Self {
        let quality = SampleQuality::from_score(chosen_score);
        Self {
            pair_id: pair_id.into(),
            chosen: chosen.into(),
            rejected: rejected.into(),
            chosen_score,
            rejected_score,
            quality,
        }
    }

    /// 偏好对的质量差异(chosen_score - rejected_score)
    ///
    /// WHY:差异越大,偏好信号越强,适合作为训练样本的置信度
    pub fn score_gap(&self) -> f32 {
        self.chosen_score - self.rejected_score
    }

    /// P5.1.1: 从相邻 spec 版本与评判结果构造偏好对
    ///
    /// # RHI-CG 通道 A 复用机制（C2 决策 / ADR-032 决策 1）
    ///
    /// v5.0 设计文档 §7.4 规定 RHI-CG 通道 A 复用既有 PreferencePair 机制：
    /// - chosen = 胜出版本的 `HarnessSpec::canonical_merkle_input()` 规范化字符串
    /// - rejected = 失败版本的 `canonical_merkle_input()`
    /// - chosen_score = winner_score（评判器返回的胜出者质量分）
    /// - rejected_score = loser_score（评判器返回的失败者质量分）
    ///
    /// # 设计决策（WHY）
    ///
    /// - **pair_id 由调用方传入**：与 `PreferencePairGenerator::next_pair_id()` 解耦，
    ///   允许 RHI-CG 编排器使用自己的 ID 命名空间（如 "rhi-pair-{version_i}-{version_i_minus_1}"）
    /// - **canonical_merkle_input 作为 chosen/rejected 内容**：保证 Merkle 完整性
    ///   （ADR-031 防注入设计），spec 任何字段变化都会反映在 hash 输入中
    /// - **不修改 spec**：仅 `&self` 借用，符合 spec "无写路径" 红线
    ///
    /// # 参数
    /// - `pair_id`: 偏好对唯一标识（由调用方生成，如 "rhi-pair-47-46"）
    /// - `spec_v_i`: 当前版本 spec（v_i，被提议的新版本）
    /// - `spec_v_i_minus_1`: 上一版本 spec（v_{i-1}，基线版本）
    /// - `verdict`: 评判结果，决定哪个版本为 chosen
    ///
    /// # 返回
    /// 新构造的 PreferencePair，chosen 为胜出版本的 merkle input
    pub fn from_adjacent_specs(
        pair_id: impl Into<String>,
        spec_v_i: &nexus_contracts::HarnessSpec,
        spec_v_i_minus_1: &nexus_contracts::HarnessSpec,
        verdict: &crate::rhi_channel_a::JudgeVerdict,
    ) -> Self {
        use crate::rhi_channel_a::SpecVersion;

        // 根据评判结果选择 chosen/rejected
        let (chosen, rejected) = match verdict.winner {
            SpecVersion::Current => (
                spec_v_i.canonical_merkle_input(),
                spec_v_i_minus_1.canonical_merkle_input(),
            ),
            SpecVersion::Previous => (
                spec_v_i_minus_1.canonical_merkle_input(),
                spec_v_i.canonical_merkle_input(),
            ),
        };

        Self::new(
            pair_id,
            chosen,
            rejected,
            verdict.winner_score,
            verdict.loser_score,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // SampleQuality 测试
    // ============================================================

    #[test]
    fn test_quality_from_score_high() {
        assert_eq!(SampleQuality::from_score(0.9), SampleQuality::High);
        assert_eq!(SampleQuality::from_score(0.8), SampleQuality::High);
    }

    #[test]
    fn test_quality_from_score_medium() {
        assert_eq!(SampleQuality::from_score(0.7), SampleQuality::Medium);
        assert_eq!(SampleQuality::from_score(0.5), SampleQuality::Medium);
    }

    #[test]
    fn test_quality_from_score_low() {
        assert_eq!(SampleQuality::from_score(0.4), SampleQuality::Low);
        assert_eq!(SampleQuality::from_score(0.0), SampleQuality::Low);
    }

    #[test]
    fn test_quality_as_str() {
        assert_eq!(SampleQuality::High.as_str(), "high");
        assert_eq!(SampleQuality::Medium.as_str(), "medium");
        assert_eq!(SampleQuality::Low.as_str(), "low");
    }

    #[test]
    fn test_quality_is_acceptable() {
        assert!(SampleQuality::High.is_acceptable());
        assert!(SampleQuality::Medium.is_acceptable());
        assert!(!SampleQuality::Low.is_acceptable());
    }

    #[test]
    fn test_quality_display() {
        assert_eq!(SampleQuality::High.to_string(), "high");
    }

    #[test]
    fn test_quality_serde_roundtrip() {
        let q = SampleQuality::Medium;
        let json = serde_json::to_string(&q).unwrap();
        let restored: SampleQuality = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, q);
    }

    // ============================================================
    // ModelOutput 测试
    // ============================================================

    #[test]
    fn test_model_output_new() {
        let out = ModelOutput::new("hello", 0.9);
        assert_eq!(out.text, "hello");
        assert!((out.score - 0.9).abs() < 1e-6);
        assert_eq!(out.quality, SampleQuality::High);
    }

    #[test]
    fn test_model_output_score_clamp_high() {
        let out = ModelOutput::new("test", 1.5);
        assert!((out.score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_model_output_score_clamp_low() {
        let out = ModelOutput::new("test", -0.5);
        assert!((out.score - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_model_output_nan_becomes_zero() {
        // WHY NaN 映射为 0.0:保证质量分级为 Low,被过滤
        let out = ModelOutput::new("test", f32::NAN);
        assert!((out.score - 0.0).abs() < 1e-6);
        assert_eq!(out.quality, SampleQuality::Low);
    }

    #[test]
    fn test_model_output_serde_roundtrip() {
        let out = ModelOutput::new("text", 0.6);
        let json = serde_json::to_string(&out).unwrap();
        let restored: ModelOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, out);
    }

    // ============================================================
    // PreferencePair 测试
    // ============================================================

    #[test]
    fn test_preference_pair_new() {
        let pair = PreferencePair::new("pair-1", "good", "bad", 0.9, 0.2);
        assert_eq!(pair.pair_id, "pair-1");
        assert_eq!(pair.chosen, "good");
        assert_eq!(pair.rejected, "bad");
        assert!((pair.chosen_score - 0.9).abs() < 1e-6);
        assert!((pair.rejected_score - 0.2).abs() < 1e-6);
        assert_eq!(pair.quality, SampleQuality::High);
    }

    #[test]
    fn test_preference_pair_score_gap() {
        let pair = PreferencePair::new("pair-1", "good", "bad", 0.9, 0.2);
        assert!((pair.score_gap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_preference_pair_serde_roundtrip() {
        let pair = PreferencePair::new("pair-1", "good", "bad", 0.9, 0.2);
        let json = serde_json::to_string(&pair).unwrap();
        let restored: PreferencePair = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, pair);
    }

    // ============================================================
    // P5.1.1: from_adjacent_specs 测试
    // ============================================================

    /// 构造最小合法 HarnessSpec 用于 from_adjacent_specs 测试
    fn make_minimal_spec(version: u32, name_suffix: &str) -> nexus_contracts::HarnessSpec {
        use nexus_contracts::{ContractSpec, HarnessMeta, HopSpec, RetryPolicy};
        nexus_contracts::HarnessSpec {
            meta: HarnessMeta {
                name: format!("rhi-test-{name_suffix}"),
                version,
                immutable: false,
                parent: if version > 1 { Some(version - 1) } else { None },
                task_type: Some("code_refactor".to_string()),
            },
            contracts: vec![ContractSpec {
                name: "no_panic".to_string(),
                property: "must_not_panic".to_string(),
                description: None,
                from: None,
                to: None,
                fields: Vec::new(),
            }],
            hops: vec![HopSpec {
                name: "execute".to_string(),
                input_type: None,
                output_type: None,
                contracts: Vec::new(),
                description: None,
                order: Vec::new(),
                on_veto: None,
                fallback: None,
            }],
            retry: RetryPolicy::default(),
            auxiliary: None,
        }
    }

    #[test]
    fn test_from_adjacent_specs_current_wins() {
        // 评判器裁决当前版本 v_i 胜出
        let spec_v_i = make_minimal_spec(2, "v2");
        let spec_v_i_minus_1 = make_minimal_spec(1, "v1");
        let verdict = crate::rhi_channel_a::JudgeVerdict {
            winner: crate::rhi_channel_a::SpecVersion::Current,
            winner_score: 0.85,
            loser_score: 0.45,
            confidence: 0.9,
            rationale: "v2 wins".to_string(),
        };

        let pair = PreferencePair::from_adjacent_specs(
            "rhi-pair-2-1",
            &spec_v_i,
            &spec_v_i_minus_1,
            &verdict,
        );

        // 验证 chosen = v_i 的 merkle input
        assert_eq!(pair.pair_id, "rhi-pair-2-1");
        assert_eq!(pair.chosen, spec_v_i.canonical_merkle_input());
        assert_eq!(pair.rejected, spec_v_i_minus_1.canonical_merkle_input());
        assert!((pair.chosen_score - 0.85).abs() < 1e-6);
        assert!((pair.rejected_score - 0.45).abs() < 1e-6);
        // 验证 chosen 与 rejected 不同（不同版本 spec 产生不同 merkle input）
        assert_ne!(pair.chosen, pair.rejected);
    }

    #[test]
    fn test_from_adjacent_specs_previous_wins() {
        // 评判器裁决上一版本 v_{i-1} 胜出（通道 B 否决的典型场景）
        let spec_v_i = make_minimal_spec(2, "v2");
        let spec_v_i_minus_1 = make_minimal_spec(1, "v1");
        let verdict = crate::rhi_channel_a::JudgeVerdict {
            winner: crate::rhi_channel_a::SpecVersion::Previous,
            winner_score: 0.8,
            loser_score: 0.3,
            confidence: 0.85,
            rationale: "v1 wins".to_string(),
        };

        let pair = PreferencePair::from_adjacent_specs(
            "rhi-pair-2-1",
            &spec_v_i,
            &spec_v_i_minus_1,
            &verdict,
        );

        // 验证 chosen = v_{i-1} 的 merkle input（胜出版本）
        assert_eq!(pair.chosen, spec_v_i_minus_1.canonical_merkle_input());
        assert_eq!(pair.rejected, spec_v_i.canonical_merkle_input());
        assert!((pair.chosen_score - 0.8).abs() < 1e-6);
        assert!((pair.rejected_score - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_from_adjacent_specs_score_gap_reflects_verdict() {
        // 验证 score_gap 反映评判差异（用于下游加权训练）
        let spec_v_i = make_minimal_spec(2, "v2");
        let spec_v_i_minus_1 = make_minimal_spec(1, "v1");
        let verdict = crate::rhi_channel_a::JudgeVerdict {
            winner: crate::rhi_channel_a::SpecVersion::Current,
            winner_score: 0.9,
            loser_score: 0.2,
            confidence: 0.95,
            rationale: "strong win".to_string(),
        };

        let pair = PreferencePair::from_adjacent_specs(
            "rhi-pair-2-1",
            &spec_v_i,
            &spec_v_i_minus_1,
            &verdict,
        );

        // score_gap = 0.9 - 0.2 = 0.7（强偏好信号）
        assert!((pair.score_gap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_from_adjacent_specs_quality_derived_from_winner_score() {
        // 验证 quality 由 winner_score 派生（保持既有 PreferencePair 语义）
        let spec_v_i = make_minimal_spec(2, "v2");
        let spec_v_i_minus_1 = make_minimal_spec(1, "v1");
        let verdict = crate::rhi_channel_a::JudgeVerdict {
            winner: crate::rhi_channel_a::SpecVersion::Current,
            winner_score: 0.85, // >= 0.8 → High
            loser_score: 0.3,
            confidence: 0.9,
            rationale: "quality test".to_string(),
        };

        let pair = PreferencePair::from_adjacent_specs(
            "rhi-pair-2-1",
            &spec_v_i,
            &spec_v_i_minus_1,
            &verdict,
        );

        assert_eq!(pair.quality, SampleQuality::High);
    }
}
