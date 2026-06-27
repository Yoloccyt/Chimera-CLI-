//! 配置 — GSOE 进化引擎的可调参数
//!
//! 对应架构层:L5 Knowledge
//!
//! # 设计要点
//! - 所有默认值来自 ADR-025 与 GRPO 经验值
//! - `max_generation` 防止无限进化(架构红线:避免资源过度消耗)
//! - 配置可通过 Figment 多源合并(CLI > Env > File > Default)

use serde::{Deserialize, Serialize};

/// GSOE 进化引擎配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GsoeConfig {
    /// 默认变异率(典型 0.05-0.3,值越大探索越强)
    pub default_mutation_rate: f32,
    /// 默认选择压力(典型 1.0-2.0,放大优势差异)
    pub default_selection_pressure: f32,
    /// 默认精英比例 `[0.0, 1.0]`(直接传承到下一代)
    pub default_elite_ratio: f32,
    /// 默认每轮采样数(GRPO 要求 ≥ 2)
    pub default_rollout_count: u32,
    /// 最大进化世代(防止无限进化消耗资源)
    pub max_generation: u64,
}

impl Default for GsoeConfig {
    fn default() -> Self {
        Self {
            default_mutation_rate: 0.1,
            default_selection_pressure: 1.5,
            default_elite_ratio: 0.2,
            default_rollout_count: 8,
            max_generation: 1000,
        }
    }
}

impl GsoeConfig {
    /// 校验配置合法性,返回错误描述(空字符串表示合法)
    pub fn validate(&self) -> Result<(), crate::error::GsoeError> {
        if !(0.0..=1.0).contains(&self.default_mutation_rate) {
            return Err(crate::error::GsoeError::ConfigError {
                reason: format!(
                    "default_mutation_rate={} 超出 [0.0, 1.0]",
                    self.default_mutation_rate
                ),
            });
        }
        if self.default_selection_pressure < 0.0 {
            return Err(crate::error::GsoeError::ConfigError {
                reason: format!(
                    "default_selection_pressure={} 不能为负",
                    self.default_selection_pressure
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.default_elite_ratio) {
            return Err(crate::error::GsoeError::ConfigError {
                reason: format!(
                    "default_elite_ratio={} 超出 [0.0, 1.0]",
                    self.default_elite_ratio
                ),
            });
        }
        if self.default_rollout_count < 2 {
            return Err(crate::error::GsoeError::ConfigError {
                reason: format!(
                    "default_rollout_count={} 至少为 2",
                    self.default_rollout_count
                ),
            });
        }
        if self.max_generation == 0 {
            return Err(crate::error::GsoeError::ConfigError {
                reason: "max_generation 不能为 0".into(),
            });
        }
        Ok(())
    }

    /// 基于配置构造初始进化策略
    pub fn to_initial_policy(
        &self,
    ) -> Result<crate::types::EvolutionPolicy, crate::error::GsoeError> {
        crate::types::EvolutionPolicy::new(
            self.default_mutation_rate,
            self.default_selection_pressure,
            self.default_elite_ratio,
            self.default_rollout_count,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let cfg = GsoeConfig::default();
        assert_eq!(cfg.default_mutation_rate, 0.1);
        assert_eq!(cfg.default_selection_pressure, 1.5);
        assert_eq!(cfg.default_elite_ratio, 0.2);
        assert_eq!(cfg.default_rollout_count, 8);
        assert_eq!(cfg.max_generation, 1000);
    }

    #[test]
    fn test_default_config_valid() {
        assert!(GsoeConfig::default().validate().is_ok());
    }

    #[test]
    fn test_invalid_mutation_rate() {
        let cfg = GsoeConfig {
            default_mutation_rate: 2.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_invalid_rollout_count() {
        let cfg = GsoeConfig {
            default_rollout_count: 1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_to_initial_policy() {
        let cfg = GsoeConfig::default();
        let policy = cfg.to_initial_policy().unwrap();
        assert_eq!(policy.mutation_rate, 0.1);
        assert_eq!(policy.rollout_count, 8);
    }

    #[test]
    fn test_max_generation_zero_invalid() {
        let cfg = GsoeConfig {
            max_generation: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
}
