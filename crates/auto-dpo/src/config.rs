//! AutoDPO 配置类型 — 样本数与质量阈值
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块)
//!
//! # 设计决策(WHY)
//! - `min_samples` 默认 2:DPO 至少需要 2 个候选(chosen + rejected)
//! - `quality_threshold` 默认 0.5:过滤 Low 质量样本,避免污染训练集
//! - `max_pairs_per_batch` 默认 100:限制单批生成数量,防止内存爆炸
//!   (§6 架构红线:内存爆炸防护)

use serde::{Deserialize, Serialize};

use crate::error::AutoDpoError;

/// AutoDPO 生成器配置
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `PreferencePairGenerator` 时会调用 `validate()` 校验配置合法性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDpoConfig {
    /// 最少输入候选数(DPO 至少需要 2 个:chosen + rejected)
    pub min_samples: usize,
    /// 质量阈值 [0.0, 1.0]:候选质量分数低于此值被过滤
    pub quality_threshold: f32,
    /// 单批最大生成偏好对数(防止内存爆炸)
    pub max_pairs_per_batch: usize,
    /// 是否启用 EventBus 事件发布(测试时可关闭)
    pub enable_event_publish: bool,

    // ============================================================
    // P1-3: FormalVerifier M1 偏好对一致性验证配置(ADR-047)
    // ============================================================
    /// 是否启用 FormalVerifier M1 偏好对一致性验证
    ///
    /// WHY 默认 true:作为 R2 解冻前置条件,所有生产路径必须通过 M1 验证。
    /// 测试场景可关闭以验证旧路径行为。
    pub enable_formal_verification: bool,

    /// 最小偏好差值 — 低于此值的偏好对视为噪声(无训练价值)
    ///
    /// WHY:过小的 margin 表明 chosen 与 rejected 几乎无差异,
    /// 这类偏好对会引入噪声到 DPO 训练中,浪费计算资源。
    #[doc = "默认值: 0.01"]
    pub min_margin: f32,

    /// 最大偏好差值 — 超过此值的偏好对视为疑似评分操纵
    ///
    /// WHY:过大的 margin(如 0.99)表明评分可能被操纵,
    /// 是奖励黑客(Reward Hacking)的典型信号。
    /// 正值 RHI-CG 双通道评估的抗奖励黑客设计。
    #[doc = "默认值: 0.95"]
    pub max_margin: f32,
}

impl Default for AutoDpoConfig {
    fn default() -> Self {
        Self {
            // WHY 2:DPO 最小需求,一个 chosen 一个 rejected
            min_samples: 2,
            // WHY 0.5:过滤 Low 质量(< 0.5),保留 Medium/High
            quality_threshold: 0.5,
            // WHY 100:单批上限,防止大批量生成导致内存爆炸
            max_pairs_per_batch: 100,
            enable_event_publish: true,
            // P1-3: M1 形式化验证默认启用(R2 解冻前置条件)
            enable_formal_verification: true,
            // WHY 0.01:低于 1% 的 margin 在实用上不可区分,视为噪声
            min_margin: 0.01,
            // WHY 0.95:score 域 [0,1],0.95 对应 chosen ≈ 1.0,rejected ≈ 0.05,
            // 超过此限的 margin 极不可能由真实质量差异产生
            max_margin: 0.95,
        }
    }
}

impl AutoDpoConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造生成器时调用,提前暴露配置错误。
    ///
    /// # 校验规则
    /// - `min_samples` >= 2(DPO 最小需求)
    /// - `quality_threshold` ∈ [0.0, 1.0]
    /// - `max_pairs_per_batch` > 0
    pub fn validate(&self) -> Result<(), AutoDpoError> {
        if self.min_samples < 2 {
            return Err(AutoDpoError::ConfigError {
                detail: format!(
                    "min_samples must be >= 2 (DPO requires chosen + rejected), got {}",
                    self.min_samples
                ),
            });
        }
        if self.quality_threshold.is_nan() || !(0.0..=1.0).contains(&self.quality_threshold) {
            return Err(AutoDpoError::ConfigError {
                detail: format!(
                    "quality_threshold must be in [0.0, 1.0], got {}",
                    self.quality_threshold
                ),
            });
        }
        if self.max_pairs_per_batch == 0 {
            return Err(AutoDpoError::ConfigError {
                detail: "max_pairs_per_batch must be > 0".into(),
            });
        }

        // P1-3: M1 formali verifier margin 参数校验
        if self.min_margin.is_nan() || self.min_margin <= 0.0 {
            return Err(AutoDpoError::ConfigError {
                detail: format!("min_margin must be > 0.0, got {}", self.min_margin),
            });
        }
        if self.max_margin.is_nan() || self.max_margin <= self.min_margin {
            return Err(AutoDpoError::ConfigError {
                detail: format!(
                    "max_margin ({}) must be > min_margin ({})",
                    self.max_margin, self.min_margin
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = AutoDpoConfig::default();
        assert_eq!(cfg.min_samples, 2);
        assert!((cfg.quality_threshold - 0.5).abs() < 1e-6);
        assert_eq!(cfg.max_pairs_per_batch, 100);
        assert!(cfg.enable_event_publish);
        // P1-3: M1 形式化验证默认配置
        assert!(cfg.enable_formal_verification);
        assert!((cfg.min_margin - 0.01).abs() < 1e-6);
        assert!((cfg.max_margin - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = AutoDpoConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_min_samples_too_small() {
        let cfg = AutoDpoConfig {
            min_samples: 1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_quality_threshold_out_of_range() {
        let cfg = AutoDpoConfig {
            quality_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_quality_threshold_negative() {
        let cfg = AutoDpoConfig {
            quality_threshold: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_max_pairs() {
        let cfg = AutoDpoConfig {
            max_pairs_per_batch: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = AutoDpoConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: AutoDpoConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.min_samples, cfg.min_samples);
        assert_eq!(restored.max_pairs_per_batch, cfg.max_pairs_per_batch);
    }

    // ============================================================
    // P1-3: M1 形式化验证配置测试
    // ============================================================

    #[test]
    fn test_validate_min_margin_negative() {
        let cfg = AutoDpoConfig {
            min_margin: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_min_margin_zero() {
        let cfg = AutoDpoConfig {
            min_margin: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_max_margin_less_than_min() {
        let cfg = AutoDpoConfig {
            min_margin: 0.5,
            max_margin: 0.3,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_margin_equal() {
        let cfg = AutoDpoConfig {
            min_margin: 0.5,
            max_margin: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
}
