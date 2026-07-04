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
}
