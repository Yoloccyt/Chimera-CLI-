//! LSCT 配置 — 升降温阈值与扫描周期
//!
//! 对应架构层:L3 Storage

use crate::error::LsctError;
use serde::{Deserialize, Serialize};

/// LSCT 配置 — 控制升降温触发条件与扫描频率
///
/// # 字段语义
/// - `promotion_threshold`:任务强度超过此值触发升温(默认 0.7)
/// - `demotion_threshold`:任务强度低于此值触发降温(默认 0.3)
/// - `scan_interval_ms`:tick 扫描周期(毫秒,默认 1000)
///
/// # 不变量
/// `promotion_threshold` 必须严格大于 `demotion_threshold`,否则中强度区间为空,
/// 无法区分升降温。`validate()` 在构造后校验此约束。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LsctConfig {
    /// 升温阈值:任务强度超过此值触发升温
    pub promotion_threshold: f32,
    /// 降温阈值:任务强度低于此值触发降温
    pub demotion_threshold: f32,
    /// 扫描周期(毫秒),tick 之间的间隔
    pub scan_interval_ms: u64,
}

impl Default for LsctConfig {
    fn default() -> Self {
        Self {
            promotion_threshold: 0.7,
            demotion_threshold: 0.3,
            scan_interval_ms: 1000,
        }
    }
}

impl LsctConfig {
    /// 校验配置合法性
    ///
    /// 规则:
    /// 1. `promotion_threshold` 必须严格大于 `demotion_threshold`
    /// 2. 两个阈值必须在 [0.0, 1.0] 范围内
    ///
    /// # 错误
    /// 返回 `LsctError::ConfigError` 描述具体违规。
    pub fn validate(&self) -> Result<(), LsctError> {
        if !(0.0..=1.0).contains(&self.promotion_threshold) {
            return Err(LsctError::ConfigError {
                reason: format!(
                    "promotion_threshold ({}) 必须在 [0.0, 1.0] 范围内",
                    self.promotion_threshold
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.demotion_threshold) {
            return Err(LsctError::ConfigError {
                reason: format!(
                    "demotion_threshold ({}) 必须在 [0.0, 1.0] 范围内",
                    self.demotion_threshold
                ),
            });
        }
        if self.promotion_threshold <= self.demotion_threshold {
            return Err(LsctError::ConfigError {
                reason: format!(
                    "promotion_threshold ({}) 必须严格大于 demotion_threshold ({})",
                    self.promotion_threshold, self.demotion_threshold
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
    fn test_default_config_valid() {
        let config = LsctConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.promotion_threshold, 0.7);
        assert_eq!(config.demotion_threshold, 0.3);
        assert_eq!(config.scan_interval_ms, 1000);
    }

    #[test]
    fn test_invalid_promotion_leq_demotion() {
        let config = LsctConfig {
            promotion_threshold: 0.3,
            demotion_threshold: 0.3,
            scan_interval_ms: 1000,
        };
        assert!(config.validate().is_err());

        let config = LsctConfig {
            promotion_threshold: 0.2,
            demotion_threshold: 0.3,
            scan_interval_ms: 1000,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_threshold_out_of_range() {
        let config = LsctConfig {
            promotion_threshold: 1.5,
            demotion_threshold: 0.3,
            scan_interval_ms: 1000,
        };
        assert!(config.validate().is_err());

        let config = LsctConfig {
            promotion_threshold: 0.7,
            demotion_threshold: -0.1,
            scan_interval_ms: 1000,
        };
        assert!(config.validate().is_err());
    }
}
